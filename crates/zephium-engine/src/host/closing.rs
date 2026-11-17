//! Closed tabs whose native release waits for their web process to answer.
//! WebKit asks a closed page's process to exit by message; a hung one never
//! reads it and would outlive its tab, so a process that does not answer in
//! time is ended. The tab itself is gone from every map and window at once.

use std::collections::HashMap;
use std::time::Duration;

use zephium_core::ids::ProfileId;

use super::EngineHost;

/// How long a closed page's web process has to answer before it is ended.
const ANSWER_DEADLINE: Duration = Duration::from_secs(10);

/// Held by a token of their own, never by tab, so a reused tab id cannot
/// reach one.
pub(super) struct ClosingViews<V> {
    next: u64,
    held: HashMap<u64, (ProfileId, V)>,
}

impl<V> Default for ClosingViews<V> {
    fn default() -> Self {
        Self {
            next: 0,
            held: HashMap::new(),
        }
    }
}

impl<V> ClosingViews<V> {
    fn reserve(&mut self) -> u64 {
        self.next = self.next.wrapping_add(1);
        self.next
    }

    fn hold(&mut self, token: u64, profile: ProfileId, view: V) {
        self.held.insert(token, (profile, view));
    }

    /// The view held under `token`, once: its answer and its deadline race.
    fn take(&mut self, token: u64) -> Option<V> {
        self.held.remove(&token).map(|(_, view)| view)
    }

    /// Every view of `profile`, whose store WebKit removes only once nothing
    /// holds it.
    fn take_profile(&mut self, profile: ProfileId) -> Vec<V> {
        let tokens: Vec<u64> = self
            .held
            .iter()
            .filter_map(|(token, (owner, _))| (*owner == profile).then_some(*token))
            .collect();
        tokens
            .into_iter()
            .filter_map(|token| self.take(token))
            .collect()
    }

    pub(super) fn clear(&mut self) {
        self.held.clear();
    }
}

pub(super) struct ClosingView {
    view: super::ObservedView,
    _deadline: crate::platform::imp::ContentPolicyTimeout,
}

impl EngineHost {
    /// Releases a closed tab's view once its web process answers, or ends the
    /// process first when it has not answered by the deadline. Without the
    /// calls this needs, or while shutting down, the view is released now.
    pub(super) fn release_closed_view(
        &mut self,
        profile: Option<ProfileId>,
        view: super::ObservedView,
    ) {
        let Some(profile) = profile.filter(|_| self.shutdown_completion.is_none()) else {
            return;
        };
        view.event_permit.revoke();
        view.navigation.revoke();
        let token = self.closing.reserve();
        let answered = move || {
            let _ = super::dispatch::try_with(move |host| drop(host.closing.take(token)));
        };
        if !crate::platform::imp::ask_web_process(&view.view, answered) {
            return;
        }
        let Some(deadline) =
            crate::platform::imp::schedule_presentation_timeout(ANSWER_DEADLINE, move || {
                let _ = super::dispatch::try_with(move |host| {
                    if let Some(closing) = host.closing.take(token) {
                        eprintln!("engine: ended a closed page's web process that did not answer");
                        crate::platform::imp::end_web_process(&closing.view.view);
                    }
                });
            })
        else {
            return;
        };
        self.closing.hold(
            token,
            profile,
            ClosingView {
                view,
                _deadline: deadline,
            },
        );
    }

    pub(super) fn release_closing_views(&mut self, profile: ProfileId) {
        drop(self.closing.take_profile(profile));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_closed_view_is_released_once_whichever_of_answer_and_deadline_comes_first() {
        let profile = ProfileId::from(1);
        let mut closing = ClosingViews::default();

        // A process that answers is released then; its deadline finds nothing
        // to end.
        let answers = closing.reserve();
        closing.hold(answers, profile, "answers");
        assert_eq!(closing.take(answers), Some("answers"));
        assert_eq!(closing.take(answers), None);

        // One that never answers is there for the deadline to end, and a late
        // answer, as WebKit sends when the process exits, finds nothing.
        let hung = closing.reserve();
        closing.hold(hung, profile, "hung");
        assert_eq!(closing.take(hung), Some("hung"));
        assert_eq!(closing.take(hung), None);
    }

    #[test]
    fn held_views_answer_to_their_own_token_and_leave_with_their_profile() {
        let (one, two) = (ProfileId::from(1), ProfileId::from(2));
        let mut closing = ClosingViews::default();
        let first = closing.reserve();
        let second = closing.reserve();
        let third = closing.reserve();
        assert!(first != second && second != third);
        closing.hold(first, one, "first");
        closing.hold(second, two, "second");
        closing.hold(third, one, "third");

        let mut erased = closing.take_profile(one);
        erased.sort_unstable();
        assert_eq!(erased, ["first", "third"]);
        assert_eq!(closing.take(first), None, "released with its profile");
        assert_eq!(closing.take(second), Some("second"));

        closing.hold(closing.next, one, "late");
        closing.clear();
        assert_eq!(closing.take(closing.next), None);
    }

    #[test]
    fn without_the_calls_or_while_shutting_down_a_closed_view_is_released_at_once() {
        let body = |source: &'static str, start: &str, end: &str| {
            source
                .split_once(start)
                .and_then(|(_, rest)| rest.split_once(end))
                .map(|(body, _)| body)
                .expect(start)
        };
        let release = body(
            include_str!("closing.rs"),
            "fn release_closed_view(",
            "\n    }\n",
        );
        let at = |needle: &str| release.find(needle).expect(needle);
        assert!(at("self.shutdown_completion.is_none()") < at("ask_web_process("));
        assert!(at("ask_web_process(") < at("self.closing.hold("));
        assert!(at("schedule_presentation_timeout(") < at("self.closing.hold("));

        let ask = body(
            include_str!("../platform/macos/native.rs"),
            "pub(crate) fn ask_web_process(",
            "\n}\n",
        );
        let at = |needle: &str| ask.find(needle).expect(needle);
        assert!(at("sel!(_isJITEnabled:)") < at("if !supported {\n        return false;"));
        assert!(
            at("sel!(_killWebContentProcessAndResetState)")
                < at("if !supported {\n        return false;")
        );
        assert!(at("if !supported {\n        return false;") < at("_isJITEnabled: &*reply"));
    }
}
