//! Closed tabs whose native release waits for their web process to answer.
//! WebKit asks a closed page's process to exit by message; a hung one never
//! reads it and would outlive its tab, so a process that does not answer in
//! time is ended. The tab itself is gone from every map and window at once.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use zephium_core::ids::ProfileId;

use super::EngineHost;

/// How long a closed page's web process has to answer before it is ended.
const ANSWER_DEADLINE: Duration = Duration::from_secs(10);

/// A closed page's answer, recorded before its host task is queued: a reply
/// whose task a full queue refuses still spares the process its deadline.
#[derive(Clone)]
struct Answer {
    token: u64,
    given: Arc<AtomicBool>,
}

impl Answer {
    fn record(&self) {
        self.given.store(true, Ordering::Release);
    }
}

/// Held by a token of their own, never by tab, so a reused tab id cannot
/// reach one. Each view keeps its deadline `D`, which ends with it.
pub(super) struct ClosingViews<V, D> {
    next: u64,
    held: HashMap<u64, (ProfileId, V, D, Arc<AtomicBool>)>,
}

impl<V, D> Default for ClosingViews<V, D> {
    fn default() -> Self {
        Self {
            next: 0,
            held: HashMap::new(),
        }
    }
}

impl<V, D> ClosingViews<V, D> {
    /// Holds `view` until its process answers or its deadline comes. Hands it
    /// back, to be released now, without a profile, when its process cannot
    /// be asked, or when no deadline can be set.
    fn defer(
        &mut self,
        profile: Option<ProfileId>,
        view: V,
        ask: impl FnOnce(&V, Answer) -> bool,
        deadline: impl FnOnce(u64) -> Option<D>,
    ) -> Option<V> {
        let Some(profile) = profile else {
            return Some(view);
        };
        self.next = self.next.wrapping_add(1);
        let token = self.next;
        let given = Arc::new(AtomicBool::new(false));
        let answer = Answer {
            token,
            given: given.clone(),
        };
        if !ask(&view, answer) {
            return Some(view);
        }
        let Some(deadline) = deadline(token) else {
            return Some(view);
        };
        self.held.insert(token, (profile, view, deadline, given));
        None
    }

    /// The view held under `token`, once: its answer and its deadline race.
    fn take(&mut self, token: u64) -> Option<V> {
        self.held.remove(&token).map(|(_, view, _, _)| view)
    }

    /// At the deadline: the view whose process never answered, to be ended.
    /// One that answered is released here instead, its process left alone.
    fn expire(&mut self, token: u64) -> Option<V> {
        let (_, view, _, given) = self.held.remove(&token)?;
        (!given.load(Ordering::Acquire)).then_some(view)
    }

    /// Every view of `profile`, whose store WebKit removes only once nothing
    /// holds it.
    fn take_profile(&mut self, profile: ProfileId) -> Vec<V> {
        let tokens: Vec<u64> = self
            .held
            .iter()
            .filter_map(|(token, (owner, ..))| (*owner == profile).then_some(*token))
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

impl EngineHost {
    /// Releases a closed tab's view once its web process answers, or ends the
    /// process first when it has not answered by the deadline. Without the
    /// calls this needs, or while shutting down, the view is released now.
    pub(super) fn release_closed_view(
        &mut self,
        profile: Option<ProfileId>,
        view: super::ObservedView,
    ) {
        let profile = profile.filter(|_| self.shutdown_completion.is_none());
        let ask = |view: &super::ObservedView, answer: Answer| {
            view.event_permit.revoke();
            view.navigation.revoke();
            crate::platform::imp::ask_web_process(&view.view, move || {
                answer.record();
                let token = answer.token;
                let _ = super::dispatch::try_with(move |host| drop(host.closing.take(token)));
            })
        };
        let deadline = |token: u64| {
            crate::platform::imp::schedule_presentation_timeout(ANSWER_DEADLINE, move || {
                let _ = super::dispatch::with_closing_deadline(token, move |host| {
                    if let Some(view) = host.closing.expire(token) {
                        eprintln!("engine: ended a closed page's web process that did not answer");
                        crate::platform::imp::end_web_process(&view.view);
                    }
                });
            })
        };
        drop(self.closing.defer(profile, view, ask, deadline));
    }

    pub(super) fn release_closing_views(&mut self, profile: ProfileId) {
        drop(self.closing.take_profile(profile));
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use super::*;

    /// Counts its drops, as a native release would be counted.
    struct Counted(&'static str, Rc<Cell<usize>>);

    impl Drop for Counted {
        fn drop(&mut self) {
            self.1.set(self.1.get() + 1);
        }
    }

    fn counter() -> (Rc<Cell<usize>>, impl Fn(&'static str) -> Counted) {
        let drops = Rc::new(Cell::new(0));
        let made = drops.clone();
        (drops, move |name| Counted(name, made.clone()))
    }

    fn hold(
        closing: &mut ClosingViews<Counted, Counted>,
        profile: ProfileId,
        view: Counted,
        deadline: Counted,
    ) -> Answer {
        let mut asked = None;
        let left = closing.defer(
            Some(profile),
            view,
            |_, answer| {
                asked = Some(answer);
                true
            },
            |_| Some(deadline),
        );
        assert!(left.is_none(), "a supported close is held");
        asked.unwrap()
    }

    #[test]
    fn a_closed_view_is_released_at_once_when_it_cannot_wait_for_an_answer() {
        let (drops, made) = counter();
        let mut closing = ClosingViews::<Counted, Counted>::default();
        let profile = ProfileId::from(1);

        // No profile, as while shutting down: the process is not even asked.
        let left = closing.defer(
            None,
            made("shutdown"),
            |_, _| unreachable!("asked while shutting down"),
            |_| -> Option<Counted> { unreachable!() },
        );
        assert_eq!(left.map(|view| view.0), Some("shutdown"));

        // WebKit without the calls: no deadline is set.
        let left = closing.defer(
            Some(profile),
            made("unsupported"),
            |_, _| false,
            |_| -> Option<Counted> { unreachable!("a deadline without a question") },
        );
        assert_eq!(left.map(|view| view.0), Some("unsupported"));

        // A deadline that cannot be scheduled: an answer that still comes finds
        // nothing.
        let mut asked = None;
        let left = closing.defer(
            Some(profile),
            made("no timer"),
            |_, answer: Answer| {
                asked = Some(answer.token);
                true
            },
            |_| None,
        );
        assert_eq!(left.map(|view| view.0), Some("no timer"));
        assert!(closing.take(asked.unwrap()).is_none());

        assert_eq!(drops.get(), 3, "each was released at once");
        assert!(closing.held.is_empty());
    }

    #[test]
    fn a_closed_view_is_released_once_whichever_of_answer_and_deadline_comes_first() {
        let (drops, made) = counter();
        let mut closing = ClosingViews::default();
        let profile = ProfileId::from(1);

        // A process that answers is released then, and its deadline with it;
        // the deadline, if it still fires, finds nothing to end.
        let answers = hold(&mut closing, profile, made("answers"), made("timer"));
        assert_eq!(drops.get(), 0, "held until the answer");
        answers.record();
        assert_eq!(
            closing.take(answers.token).map(|view| view.0),
            Some("answers")
        );
        assert_eq!(drops.get(), 2);
        assert!(closing.expire(answers.token).is_none());

        // One that never answers is there for the deadline to end, and a late
        // answer, as WebKit sends when the process exits, finds nothing.
        let hung = hold(&mut closing, profile, made("hung"), made("timer"));
        assert_eq!(drops.get(), 2);
        assert_eq!(closing.expire(hung.token).map(|view| view.0), Some("hung"));
        hung.record();
        assert!(closing.take(hung.token).is_none());
        assert_eq!(drops.get(), 4);
    }

    #[test]
    fn an_answer_whose_task_is_refused_still_spares_its_process() {
        let (drops, made) = counter();
        let mut closing = ClosingViews::default();
        // The reply records its answer, then its host task is refused by a
        // full queue, so nothing takes the view before the deadline.
        let answered = hold(
            &mut closing,
            ProfileId::from(1),
            made("shared"),
            made("timer"),
        );
        answered.record();

        assert!(
            closing.expire(answered.token).is_none(),
            "an answered process is not ended"
        );
        assert_eq!(drops.get(), 2, "the deadline still releases the view");
        assert!(closing.held.is_empty());
    }

    #[test]
    fn held_views_answer_to_their_own_token_and_leave_with_their_profile() {
        let (drops, made) = counter();
        let (one, two) = (ProfileId::from(1), ProfileId::from(2));
        let mut closing = ClosingViews::default();
        let first = hold(&mut closing, one, made("first"), made("timer")).token;
        let second = hold(&mut closing, two, made("second"), made("timer")).token;
        let third = hold(&mut closing, one, made("third"), made("timer")).token;
        assert!(first != second && second != third);

        let mut erased: Vec<_> = closing
            .take_profile(one)
            .into_iter()
            .map(|view| view.0)
            .collect();
        erased.sort_unstable();
        assert_eq!(erased, ["first", "third"]);
        assert_eq!(drops.get(), 4, "both views and their deadlines");
        assert!(closing.take(first).is_none(), "released with its profile");
        assert_eq!(closing.take(second).map(|view| view.0), Some("second"));
        assert_eq!(drops.get(), 6);

        let late = hold(&mut closing, one, made("late"), made("timer")).token;
        closing.clear();
        assert_eq!(drops.get(), 8, "clear releases what is left");
        assert!(closing.take(late).is_none());
    }

    #[test]
    fn the_web_process_is_asked_only_when_webkit_can_also_end_it() {
        let ask = include_str!("../platform/macos/native.rs")
            .split_once("pub(crate) fn ask_web_process(")
            .and_then(|(_, rest)| rest.split_once("\n}\n"))
            .map(|(body, _)| body)
            .expect("ask_web_process");
        let at = |needle: &str| ask.find(needle).expect(needle);
        let unsupported = at("if !supported {\n        return false;");
        assert!(at("sel!(_isJITEnabled:)") < unsupported);
        assert!(at("sel!(_killWebContentProcessAndResetState)") < unsupported);
        assert!(unsupported < at("_isJITEnabled: &*reply"));
    }
}
