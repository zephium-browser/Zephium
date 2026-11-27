//! Automatic video PiP is initiated only by the browser's tab-selection port.
use super::EngineHost;
use crate::fullscreen::NativeFullscreen;
use crate::navigation_epoch::NavigationEpochTracker;
use std::cell::Cell;
use std::rc::Rc;
use zephium_core::ids::ItemId;

const SCRIPT: &str = include_str!("../platform/automatic_pip.js");

fn request_guard(
    permit: super::permits::EventPermit,
    navigation: NavigationEpochTracker,
    current_sequence: Rc<Cell<u64>>,
) -> Option<Rc<dyn Fn() -> bool>> {
    let epoch = navigation.current_committed()?;
    let activity = navigation.activity_snapshot()?;
    let sequence = current_sequence.get();
    Some(Rc::new(move || {
        permit.active_token().is_some()
            && navigation.current_committed() == Some(epoch)
            && navigation.matches_activity(activity)
            && current_sequence.get() == sequence
    }))
}

impl EngineHost {
    pub(crate) fn automatic_pip(&self, id: ItemId, background: bool) {
        let Some(view) = self.views.get(&id) else {
            return;
        };
        let _ = view.automatic_pip_after_fullscreen.borrow_mut().take();
        let sequence = view
            .automatic_pip_sequence
            .get()
            .checked_add(1)
            .filter(|sequence| *sequence <= 9_007_199_254_740_991);
        let Some(sequence) = sequence else {
            return;
        };
        view.automatic_pip_sequence.set(sequence);
        let Some((_, url)) = view.navigation.committed_snapshot() else {
            return;
        };
        if background
            && (!view.presentable || !url.starts_with("https://") && !url.starts_with("http://"))
        {
            return;
        }
        let Some(current) = request_guard(
            view.event_permit.clone(),
            view.navigation.clone(),
            view.automatic_pip_sequence.clone(),
        ) else {
            return;
        };
        // Capture the document and physical view lifetime before deferring.
        // Even a failed navigation attempt cancels this request permanently.
        if background
            && crate::platform::imp::fullscreen_state(&view.view) != NativeFullscreen::Inactive
        {
            *view.automatic_pip_after_fullscreen.borrow_mut() = Some(current);
            return;
        }
        let action = if background { "leave" } else { "return" };
        let url = serde_json::to_string(&url).expect("a string is serializable");
        let script =
            format!("location.href === {url} ? ({SCRIPT})('{action}', {sequence}) : false");
        crate::platform::imp::automatic_pip(&view.view, script, current);
    }

    /// `id` has left element fullscreen: offer its video now if its tab was
    /// left while it was still fullscreen and has not been shown since.
    pub(super) fn automatic_pip_after_fullscreen(&self, id: ItemId) {
        let Some(view) = self.views.get(&id) else {
            return;
        };
        let Some(current) = view.automatic_pip_after_fullscreen.borrow_mut().take() else {
            return;
        };
        if !current() {
            return;
        }
        #[cfg(target_os = "macos")]
        {
            // WebKit reports the view out of fullscreen before its document
            // hears of it, and the page declines while it is still fullscreen.
            let permit = view.event_permit.clone();
            crate::platform::imp::after_document_fullscreen(&view.view, move || {
                let _ = super::dispatch::try_with(move |host| {
                    if current()
                        && host
                            .views
                            .get(&id)
                            .is_some_and(|view| view.event_permit.same_generation(&permit))
                    {
                        host.automatic_pip(id, true);
                    }
                });
            });
        }
        #[cfg(target_os = "windows")]
        {
            let _ = view;
            self.automatic_pip(id, true);
        }
    }

    pub(crate) fn cancel_automatic_pip(&self) {
        for (&id, view) in &self.views {
            if view.automatic_pip_sequence.get() != 0 {
                self.automatic_pip(id, false);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;
    use wry::{NavigationEvent, NavigationEventPhase, NavigationId};

    fn commit(navigation: &NavigationEpochTracker, id: u64, url: &str) {
        navigation.begin(url).unwrap();
        for phase in [
            NavigationEventPhase::Started,
            NavigationEventPhase::Committed,
        ] {
            navigation.observe_navigation(&NavigationEvent {
                id: NavigationId::from_raw(id),
                phase,
                url: url.into(),
            });
        }
    }

    #[test]
    fn deferred_fullscreen_request_cannot_follow_navigation_or_a_new_selection() {
        let token = Arc::new(AtomicBool::new(true));
        let permit = super::super::permits::EventPermit::bound(&token);
        let navigation = NavigationEpochTracker::new();
        let sequence = Rc::new(Cell::new(1));
        commit(&navigation, 1, "https://video.example/a");
        let pending = request_guard(permit.clone(), navigation.clone(), sequence.clone()).unwrap();
        assert!(pending());
        // Navigation between the native fullscreen exit and its asynchronous
        // document probe must not let the retry acquire document B's snapshot.
        commit(&navigation, 2, "https://video.example/b");
        assert!(!pending());
        let pending = request_guard(permit.clone(), navigation.clone(), sequence.clone()).unwrap();
        navigation.begin("https://video.example/c").unwrap();
        assert!(!pending());
        navigation.observe_navigation(&NavigationEvent {
            id: NavigationId::from_raw(3),
            phase: NavigationEventPhase::Started,
            url: "https://video.example/c".into(),
        });
        navigation.observe_navigation(&NavigationEvent {
            id: NavigationId::from_raw(3),
            phase: NavigationEventPhase::Failed,
            url: "https://video.example/c".into(),
        });
        assert!(navigation.current_committed().is_some());
        assert!(
            !pending(),
            "failed navigation must not revive a deferred request"
        );
        let pending = request_guard(permit.clone(), navigation.clone(), sequence.clone()).unwrap();
        sequence.set(2);
        assert!(!pending());
        let pending = request_guard(permit.clone(), navigation, sequence).unwrap();
        permit.revoke();
        assert!(!pending(), "retirement must cancel the deferred request");
    }
}
