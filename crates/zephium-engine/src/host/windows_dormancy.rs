//! Content-tab dormancy native ops on Windows WebView2.
//!
//! Microsoft advises not mixing `MemoryUsageTargetLevel` with
//! `TrySuspend`/`Resume`: TrySuspend already drives Low. Low is reserved for
//! the final refuse path; Normal restores only for those refuse-path Lows.

#![cfg(any(target_os = "windows", test))]

/// Native effect the host must apply for one dormancy drive.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DormancyNativeOp {
    TrySuspend,
    MemoryLow,
    MemoryNormal,
}

/// Whether a view still owes a Normal restore after a refuse-path Low.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct RefuseMemoryLow(bool);

impl RefuseMemoryLow {
    pub(crate) const fn from_recorded(recorded: bool) -> Self {
        Self(recorded)
    }

    pub(crate) const fn is_set(self) -> bool {
        self.0
    }
}

/// Hide / desired-dormant admission: TrySuspend only — never a Low poke.
pub(crate) fn ops_on_hide() -> &'static [DormancyNativeOp] {
    &[DormancyNativeOp::TrySuspend]
}

/// Exact negative settle after the one retry: Low, and record refuse-path Low.
pub(crate) fn ops_on_final_refuse() -> &'static [DormancyNativeOp] {
    &[DormancyNativeOp::MemoryLow]
}

/// Become visible: Normal only when a refuse-path Low is still owed.
pub(crate) fn ops_on_show(refuse_low: RefuseMemoryLow) -> &'static [DormancyNativeOp] {
    if refuse_low.is_set() {
        &[DormancyNativeOp::MemoryNormal]
    } else {
        &[]
    }
}

/// Pump must not admit TrySuspend while a refuse-path Low is set.
pub(crate) fn may_admit_try_suspend(refuse_low: RefuseMemoryLow) -> bool {
    !refuse_low.is_set()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drive the hide → final-refuse → show sequence and assert the native ops
    /// the host must emit (TrySuspend only on hide, Low only after refuse,
    /// Normal on show only after that Low).
    #[test]
    fn hide_refuse_show_emits_try_suspend_then_low_then_normal() {
        let mut log = Vec::new();
        let mut refuse_low = RefuseMemoryLow::default();

        // Hide: TrySuspend owns the transition; no MemoryUsageTargetLevel poke.
        assert!(
            !ops_on_hide().contains(&DormancyNativeOp::MemoryLow),
            "hide must not set Low; TrySuspend owns that transition"
        );
        log.extend_from_slice(ops_on_hide());
        assert!(
            may_admit_try_suspend(refuse_low),
            "pump may admit TrySuspend before a refuse-path Low"
        );

        // Final refuse after retry: Low, and block further TrySuspend.
        log.extend_from_slice(ops_on_final_refuse());
        refuse_low = RefuseMemoryLow::from_recorded(true);
        assert!(
            !may_admit_try_suspend(refuse_low),
            "pump must not admit TrySuspend while refuse-path Low is set"
        );

        // Show after refuse Low: Normal restore, then TrySuspend may run again.
        let show = ops_on_show(refuse_low);
        assert_eq!(show, &[DormancyNativeOp::MemoryNormal]);
        log.extend_from_slice(show);
        refuse_low = RefuseMemoryLow::from_recorded(false);
        assert!(may_admit_try_suspend(refuse_low));

        assert_eq!(
            log,
            [
                DormancyNativeOp::TrySuspend,
                DormancyNativeOp::MemoryLow,
                DormancyNativeOp::MemoryNormal,
            ]
        );
    }

    #[test]
    fn show_without_refuse_low_does_not_poke_memory_target() {
        assert!(ops_on_show(RefuseMemoryLow::default()).is_empty());
    }
}
