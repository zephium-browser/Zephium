use crate::navigation_epoch::{NavigationEpoch, NavigationEpochTracker};
use zephium_core::ids::ItemId;
use zephium_core::ports::engine::{DiscardProbeId, EngineEvent};

use super::dispatch::with_discard_observation;
#[cfg(target_os = "windows")]
use super::dispatch::with_suspend_result;
use super::permits::EventPermit;
use super::scripts::DISCARD_SAFETY_QUERY_JS;
use super::EngineHost;

#[cfg(target_os = "windows")]
const MAX_CONCURRENT_SUSPENDS: usize = 8;

fn renderer_report_allows_discard(result: &str) -> bool {
    // Safe means the primitive mask contains only the ready bit. Reject every
    // alternate number/string/object representation without parsing.
    result == "1"
}

fn discard_probe_identity_matches(
    current_permit: &EventPermit,
    current_navigation: &NavigationEpochTracker,
    requested_permit: &EventPermit,
    requested_navigation: &NavigationEpochTracker,
    requested_epoch: NavigationEpoch,
) -> bool {
    current_permit.same_generation(requested_permit)
        && current_navigation.same_generation(requested_navigation)
        && requested_navigation.is_current(requested_epoch)
}

impl EngineHost {
    pub(crate) fn probe_discard_safety(&self, id: ItemId, probe: DiscardProbeId) {
        let Some(view) = self.views.get(&id) else {
            return;
        };
        let Some(epoch) = view.navigation.current_committed() else {
            return;
        };
        let permit = view.event_permit.clone();
        let navigation = view.navigation.clone();
        let queued_permit = permit.clone();
        let queued_navigation = navigation.clone();
        let _ = view.evaluate_script_with_callback(DISCARD_SAFETY_QUERY_JS, move |result| {
            // Callback completion can race focus-driven navigation or close.
            // Do not reinterpret a report from the old document under a new
            // same-id view/navigation; the shell timeout is fail-closed.
            if !queued_navigation.is_current(epoch) {
                return;
            }
            let renderer_safe = renderer_report_allows_discard(&result);
            let completion_permit = queued_permit.clone();
            let completion_navigation = queued_navigation.clone();
            with_discard_observation(id, move |host| {
                host.complete_discard_probe(
                    id,
                    probe,
                    &completion_permit,
                    &completion_navigation,
                    epoch,
                    renderer_safe,
                )
            });
        });
    }

    fn complete_discard_probe(
        &self,
        id: ItemId,
        probe: DiscardProbeId,
        permit: &EventPermit,
        navigation: &NavigationEpochTracker,
        epoch: NavigationEpoch,
        renderer_safe: bool,
    ) {
        let Some(view) = self.views.get(&id) else {
            return;
        };
        if !discard_probe_identity_matches(
            &view.event_permit,
            &view.navigation,
            permit,
            navigation,
            epoch,
        ) {
            return;
        }

        if !renderer_safe {
            permit.emit(
                &self.sink,
                EngineEvent::DiscardSafety {
                    id,
                    probe,
                    can_discard: false,
                },
            );
            return;
        }

        // WebView2 and WebKitGTK expose native audio activity; WKWebView has
        // public asynchronous playback plus synchronous camera/microphone
        // capture state. Page JavaScript cannot spoof these cross-checks.
        let activity_permit = permit.clone();
        let activity_navigation = navigation.clone();
        let started = crate::platform::imp::query_document_activity(view, move |native_allows| {
            with_discard_observation(id, move |host| {
                host.finish_discard_probe(
                    id,
                    probe,
                    &activity_permit,
                    &activity_navigation,
                    epoch,
                    native_allows,
                )
            });
        });
        if !started {
            // Missing native API/admission is uncertainty, never silence.
            permit.emit(
                &self.sink,
                EngineEvent::DiscardSafety {
                    id,
                    probe,
                    can_discard: false,
                },
            );
        }
    }

    fn finish_discard_probe(
        &self,
        id: ItemId,
        probe: DiscardProbeId,
        permit: &EventPermit,
        navigation: &NavigationEpochTracker,
        epoch: NavigationEpoch,
        native_allows: bool,
    ) {
        let Some(view) = self.views.get(&id) else {
            return;
        };
        if !discard_probe_identity_matches(
            &view.event_permit,
            &view.navigation,
            permit,
            navigation,
            epoch,
        ) {
            return;
        }
        // Admitted macOS downloads belong to the profile coordinator and
        // retain their WKDownload/delegate independently of this view. Pending
        // destination decisions remain view-bound and are revoked on teardown.
        // Windows/Linux continue denying downloads at their native boundary.
        permit.emit(
            &self.sink,
            EngineEvent::DiscardSafety {
                id,
                probe,
                can_discard: native_allows,
            },
        );
    }

    /// Suspends the given hidden views (shell idle policy). Only Windows has
    /// an explicit primitive; WebKit suspends hidden/unmapped processes on
    /// its own. Resume is implicit: WebView2 wakes a view on SetIsVisible.
    pub(crate) fn set_dormant(&mut self, ids: Vec<ItemId>) {
        #[cfg(target_os = "windows")]
        {
            let next: std::collections::HashSet<ItemId> = ids
                .into_iter()
                .filter(|id| self.hidden.contains(id))
                .collect();
            let wake: Vec<ItemId> = self.dormant.difference(&next).copied().collect();
            for id in wake {
                if let Some(view) = self.views.get(&id) {
                    crate::platform::imp::resume(view);
                }
                self.dormant.remove(&id);
            }
            self.suspend_failed.retain(|id| next.contains(id));
            self.desired_dormant = next;
            self.pump_suspends();
        }
        #[cfg(not(target_os = "windows"))]
        let _ = ids;
    }

    #[cfg(target_os = "windows")]
    fn on_suspend_result(&mut self, id: ItemId, suspended: bool) {
        self.suspending.remove(&id);
        if suspended && self.desired_dormant.contains(&id) && self.hidden.contains(&id) {
            self.dormant.insert(id);
        } else if suspended {
            // The desired state changed while the async request was in flight.
            if let Some(view) = self.views.get(&id) {
                crate::platform::imp::resume(view);
            }
        } else if self.desired_dormant.contains(&id) {
            // Do not spin on runtimes that cannot suspend. Becoming visible
            // clears this marker, so a later hide cycle can retry.
            self.suspend_failed.insert(id);
        }
        self.pump_suspends();
    }

    #[cfg(target_os = "windows")]
    fn pump_suspends(&mut self) {
        while self.suspending.len() < MAX_CONCURRENT_SUSPENDS {
            let candidate = self
                .desired_dormant
                .iter()
                .filter(|id| {
                    !self.dormant.contains(id)
                        && !self.suspending.contains(id)
                        && !self.suspend_failed.contains(id)
                })
                .copied()
                .min();
            // Preserve deterministic admission without allocating and sorting
            // every remaining tab for each slot (or synchronous rejection).
            let Some(id) = candidate else {
                break;
            };
            self.suspending.insert(id);
            let started = self.views.get(&id).is_some_and(|view| {
                crate::platform::imp::try_suspend(view, move |suspended| {
                    with_suspend_result(id, move |host| host.on_suspend_result(id, suspended));
                })
            });
            if !started {
                // Keep synchronous admission failure iterative. Recursively
                // pumping a runtime that rejects every request would consume
                // one stack frame per dormant tab.
                self.suspending.remove(&id);
                if self.desired_dormant.contains(&id) {
                    self.suspend_failed.insert(id);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;

    use crate::navigation_epoch::NavigationTransition;

    use super::super::scripts::DISCARD_SAFETY_BOOTSTRAP_JS;
    use super::*;

    #[test]
    fn discard_report_accepts_only_the_exact_safe_primitive_mask() {
        assert!(renderer_report_allows_discard("1"));
        for protected in [
            "0", "2", "3", "255", "256", "511", "null", "\"1\"", "{}", " 1",
        ] {
            assert!(
                !renderer_report_allows_discard(protected),
                "alternate renderer value must veto discard: {protected:?}"
            );
        }
        assert!(DISCARD_SAFETY_BOOTSTRAP_JS.contains("localUncertain ? 256 : 0"));
        assert!(DISCARD_SAFETY_QUERY_JS.contains("return 256"));
        assert!(!DISCARD_SAFETY_QUERY_JS.contains("return {"));
    }

    #[test]
    fn discard_probe_requires_exact_generation_and_current_navigation_epoch() {
        let first_token = Arc::new(AtomicBool::new(true));
        let first_permit = EventPermit::bound(&first_token);
        let first_navigation = NavigationEpochTracker::new();
        let first_epoch = first_navigation.begin("https://first.example/").unwrap();
        assert_eq!(
            first_navigation.observe_navigation(&wry::NavigationEvent {
                id: wry::NavigationId::from_raw(1),
                phase: wry::NavigationEventPhase::Started,
                url: "https://first.example/".into(),
            }),
            Some(NavigationTransition::Started(first_epoch))
        );
        assert_eq!(
            first_navigation.observe_navigation(&wry::NavigationEvent {
                id: wry::NavigationId::from_raw(1),
                phase: wry::NavigationEventPhase::Committed,
                url: "https://first.example/".into(),
            }),
            Some(NavigationTransition::Committed(first_epoch))
        );
        assert!(discard_probe_identity_matches(
            &first_permit,
            &first_navigation,
            &first_permit,
            &first_navigation,
            first_epoch,
        ));

        let second_epoch = first_navigation.begin("https://second.example/").unwrap();
        assert_ne!(first_epoch, second_epoch);
        assert!(!discard_probe_identity_matches(
            &first_permit,
            &first_navigation,
            &first_permit,
            &first_navigation,
            first_epoch,
        ));

        let replacement_token = Arc::new(AtomicBool::new(true));
        let replacement_permit = EventPermit::bound(&replacement_token);
        let replacement_navigation = NavigationEpochTracker::new();
        let replacement_epoch = replacement_navigation
            .begin("https://second.example/")
            .unwrap();
        assert!(!discard_probe_identity_matches(
            &replacement_permit,
            &replacement_navigation,
            &first_permit,
            &first_navigation,
            second_epoch,
        ));
        assert!(!discard_probe_identity_matches(
            &first_permit,
            &first_navigation,
            &replacement_permit,
            &replacement_navigation,
            replacement_epoch,
        ));
    }
}
