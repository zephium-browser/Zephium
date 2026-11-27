//! Refusal and memory-target state for Windows content tabs.

use std::collections::HashSet;
use zephium_core::ids::ItemId;

pub(super) enum SuspendCompletion {
    Dormant,
    Refused { native_refusal: bool },
    Stale,
}

pub(super) fn settle_suspend_completion(
    native_result: Option<bool>,
    current_cycle: bool,
    current_document: bool,
    within_deadline: bool,
    resume: impl FnOnce(),
) -> SuspendCompletion {
    if native_result == Some(true) {
        if current_cycle && current_document && within_deadline {
            return SuspendCompletion::Dormant;
        }
        resume();
    }
    if current_cycle {
        SuspendCompletion::Refused {
            native_refusal: current_document && native_result == Some(false),
        }
    } else {
        SuspendCompletion::Stale
    }
}

pub(super) enum SuspendFailureAction {
    Wait,
    ScheduleRetry,
    Report,
}

#[derive(Default)]
pub(super) struct SuspendFailures {
    failed: HashSet<ItemId>,
    // A successful Low request must be restored even if the shell stops asking
    // the hidden view to suspend.
    memory_low: HashSet<ItemId>,
}

impl SuspendFailures {
    pub(super) fn record(
        &mut self,
        id: ItemId,
        retry_pending: Option<bool>,
        native_refusal: bool,
        set_low: impl FnOnce() -> bool,
    ) -> SuspendFailureAction {
        let newly_failed = self.failed.insert(id);
        match retry_pending {
            None => SuspendFailureAction::ScheduleRetry,
            Some(true) => SuspendFailureAction::Wait,
            Some(false) => {
                // A timeout reports the refusal while the native call may still
                // be outstanding. Only its current, explicit refusal permits Low.
                if native_refusal && !self.memory_low.contains(&id) && set_low() {
                    self.memory_low.insert(id);
                }
                if newly_failed {
                    SuspendFailureAction::Report
                } else {
                    SuspendFailureAction::Wait
                }
            }
        }
    }

    pub(super) fn allows_suspend(&self, id: ItemId) -> bool {
        !self.failed.contains(&id) && !self.memory_low.contains(&id)
    }

    pub(super) fn restore_visible(
        &mut self,
        hidden: &HashSet<ItemId>,
        mut set_normal: impl FnMut(ItemId) -> bool,
    ) {
        self.memory_low
            .retain(|id| hidden.contains(id) || !set_normal(*id));
    }

    pub(super) fn retain_desired(&mut self, desired: &HashSet<ItemId>) {
        self.failed.retain(|id| desired.contains(id));
    }

    pub(super) fn clear_failure(&mut self, id: ItemId) {
        self.failed.remove(&id);
    }

    pub(super) fn remove(&mut self, id: ItemId) {
        self.failed.remove(&id);
        self.memory_low.remove(&id);
    }

    pub(super) fn clear(&mut self) {
        self.failed.clear();
        self.memory_low.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use SuspendFailureAction::{Report, ScheduleRetry, Wait};

    #[test]
    fn final_timeout_reports_once_and_only_a_late_native_refusal_sets_low() {
        let id = ItemId::from(1);
        for result in [None, Some(true), Some(false)] {
            let mut failures = SuspendFailures::default();
            assert!(failures.allows_suspend(id));
            assert!(matches!(
                failures.record(id, None, true, || panic!("first refusal set Low")),
                ScheduleRetry
            ));
            assert!(matches!(
                failures.record(id, Some(true), true, || panic!("retry still pending")),
                Wait
            ));
            assert!(!failures.allows_suspend(id));
            failures.clear_failure(id);
            assert!(failures.allows_suspend(id));
            assert!(matches!(
                failures.record(id, Some(false), false, || panic!("timeout set Low")),
                Report
            ));
            assert!(!failures.allows_suspend(id));

            let mut resumed = false;
            let SuspendCompletion::Refused { native_refusal } =
                settle_suspend_completion(result, true, true, false, || resumed = true)
            else {
                panic!("late completion made the view dormant");
            };
            assert_eq!(resumed, result == Some(true));

            let mut low_requests = 0;
            assert!(matches!(
                failures.record(id, Some(false), native_refusal, || {
                    low_requests += 1;
                    true
                }),
                Wait
            ));
            let expected_low_requests = if result == Some(false) { 1 } else { 0 };
            assert_eq!(low_requests, expected_low_requests);

            let mut normal_requests = 0;
            failures.restore_visible(&HashSet::new(), |restored| {
                assert_eq!(restored, id);
                normal_requests += 1;
                true
            });
            assert_eq!(normal_requests, low_requests);
        }
    }

    #[test]
    fn refusal_from_an_old_cycle_or_document_cannot_authorize_low() {
        let no_resume = || panic!("a refused suspension cannot need resuming");
        assert!(matches!(
            settle_suspend_completion(Some(false), false, true, true, no_resume),
            SuspendCompletion::Stale
        ));
        assert!(matches!(
            settle_suspend_completion(Some(false), true, false, true, no_resume),
            SuspendCompletion::Refused {
                native_refusal: false
            }
        ));
    }

    #[test]
    fn successful_suspend_needs_no_manual_memory_target() {
        assert!(matches!(
            settle_suspend_completion(Some(true), true, true, true, || {
                panic!("resumed an on-time suspension")
            }),
            SuspendCompletion::Dormant
        ));
        SuspendFailures::default().restore_visible(&HashSet::new(), |_| {
            panic!("TrySuspend owns the memory target")
        });
    }

    #[test]
    fn successful_low_blocks_readmission_until_normal_succeeds() {
        let id = ItemId::from(1);
        let mut failures = SuspendFailures::default();
        assert!(matches!(
            failures.record(id, Some(false), true, || true),
            Report
        ));
        failures.retain_desired(&HashSet::new());
        failures.retain_desired(&HashSet::from([id]));
        assert!(!failures.allows_suspend(id));
        failures.restore_visible(&HashSet::from([id]), |_| panic!("restored while hidden"));
        failures.restore_visible(&HashSet::new(), |restored| {
            assert_eq!(restored, id);
            false
        });
        assert!(!failures.allows_suspend(id));
        failures.restore_visible(&HashSet::new(), |restored| {
            assert_eq!(restored, id);
            true
        });
        assert!(failures.allows_suspend(id));
        failures.restore_visible(&HashSet::new(), |_| panic!("restored Normal twice"));
    }

    #[test]
    fn failed_low_reports_refusal_without_owing_normal() {
        let id = ItemId::from(1);
        let mut failures = SuspendFailures::default();
        assert!(matches!(
            failures.record(id, Some(false), true, || false),
            Report
        ));
        assert!(!failures.allows_suspend(id));
        failures.clear_failure(id);
        assert!(failures.allows_suspend(id));
        failures.restore_visible(&HashSet::new(), |_| panic!("Low never succeeded"));
    }

    #[test]
    fn retirement_and_shutdown_drop_failures_and_memory_targets() {
        let retired = ItemId::from(1);
        let live = ItemId::from(2);
        let mut failures = SuspendFailures::default();
        for id in [retired, live] {
            assert!(matches!(
                failures.record(id, Some(false), true, || true),
                Report
            ));
        }
        failures.remove(retired);
        assert!(failures.allows_suspend(retired));
        assert!(!failures.allows_suspend(live));
        failures.restore_visible(&HashSet::new(), |restored| {
            assert_eq!(restored, live);
            false
        });
        failures.clear();
        assert!(failures.allows_suspend(live));
        failures.restore_visible(&HashSet::new(), |_| panic!("restoration survived shutdown"));
    }
}
