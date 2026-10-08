//! Platform-neutral browser-runtime security assessment.
//!
//! Native adapters perform their own strict version, provenance, and
//! capability checks. This module carries only non-fatal, bounded advisory
//! state across the engine/application boundary; it never weakens an
//! admission failure.

/// Why an admitted native browser runtime still deserves user attention.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeSecurityAdvisoryKind {
    /// Zephium's embedded vendor review is older than its maintenance SLA.
    ReviewOverdue,
    /// The runtime satisfies the hard floor but is below the latest reviewed
    /// security release recommended by this Zephium build.
    UpdateRecommended,
    /// A newer stable runtime passed every mandatory capability/provenance
    /// check but its release line postdates this Zephium build's review.
    UnreviewedRuntime,
}

/// Fixed, non-page-derived destination of the recommended maintenance action.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeSecurityUpdateTarget {
    Zephium,
    OperatingSystem,
    BrowserRuntime,
}

/// One sanitized process-lifetime advisory for privileged chrome.
///
/// There are deliberately no native strings, URLs, versions, or vendor
/// messages here. Chrome maps this closed vocabulary to local copy, and future
/// signed policy updates can reuse the same boundary without making vendor
/// HTML or network responses authoritative.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RuntimeSecurityAdvisory {
    kind: RuntimeSecurityAdvisoryKind,
    update_target: RuntimeSecurityUpdateTarget,
}

impl RuntimeSecurityAdvisory {
    pub const fn review_overdue() -> Self {
        Self {
            kind: RuntimeSecurityAdvisoryKind::ReviewOverdue,
            update_target: RuntimeSecurityUpdateTarget::Zephium,
        }
    }

    pub const fn update_recommended(target: RuntimeSecurityUpdateTarget) -> Self {
        Self {
            kind: RuntimeSecurityAdvisoryKind::UpdateRecommended,
            update_target: target,
        }
    }

    pub const fn unreviewed_runtime() -> Self {
        Self {
            kind: RuntimeSecurityAdvisoryKind::UnreviewedRuntime,
            update_target: RuntimeSecurityUpdateTarget::Zephium,
        }
    }

    pub const fn kind(self) -> RuntimeSecurityAdvisoryKind {
        self.kind
    }

    pub const fn update_target(self) -> RuntimeSecurityUpdateTarget {
        self.update_target
    }
}

/// Canonical, allocation-free set of every process-lifetime advisory.
///
/// The closed vocabulary has five valid values: review age, an unreviewed
/// runtime, and an update recommendation for each fixed target. A bitset
/// prevents duplicates and makes capacity exhaustion impossible without
/// silently dropping a second independent security fact.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RuntimeSecurityAdvisories {
    bits: u8,
}

impl RuntimeSecurityAdvisories {
    const REVIEW_OVERDUE: u8 = 1 << 0;
    const UPDATE_ZEPHIUM: u8 = 1 << 1;
    const UPDATE_OPERATING_SYSTEM: u8 = 1 << 2;
    const UPDATE_BROWSER_RUNTIME: u8 = 1 << 3;
    const UNREVIEWED_RUNTIME: u8 = 1 << 4;
    const ORDERED: [RuntimeSecurityAdvisory; 5] = [
        RuntimeSecurityAdvisory::review_overdue(),
        RuntimeSecurityAdvisory::update_recommended(RuntimeSecurityUpdateTarget::Zephium),
        RuntimeSecurityAdvisory::update_recommended(RuntimeSecurityUpdateTarget::OperatingSystem),
        RuntimeSecurityAdvisory::update_recommended(RuntimeSecurityUpdateTarget::BrowserRuntime),
        RuntimeSecurityAdvisory::unreviewed_runtime(),
    ];

    pub const fn new() -> Self {
        Self { bits: 0 }
    }

    pub const fn from_advisory(advisory: RuntimeSecurityAdvisory) -> Self {
        let mut advisories = Self::new();
        advisories.insert(advisory);
        advisories
    }

    pub const fn insert(&mut self, advisory: RuntimeSecurityAdvisory) {
        self.bits |= match advisory.kind {
            RuntimeSecurityAdvisoryKind::ReviewOverdue => Self::REVIEW_OVERDUE,
            RuntimeSecurityAdvisoryKind::UnreviewedRuntime => Self::UNREVIEWED_RUNTIME,
            RuntimeSecurityAdvisoryKind::UpdateRecommended => match advisory.update_target {
                RuntimeSecurityUpdateTarget::Zephium => Self::UPDATE_ZEPHIUM,
                RuntimeSecurityUpdateTarget::OperatingSystem => Self::UPDATE_OPERATING_SYSTEM,
                RuntimeSecurityUpdateTarget::BrowserRuntime => Self::UPDATE_BROWSER_RUNTIME,
            },
        };
    }

    pub const fn with_optional(mut self, advisory: Option<RuntimeSecurityAdvisory>) -> Self {
        if let Some(advisory) = advisory {
            self.insert(advisory);
        }
        self
    }

    pub const fn contains(self, advisory: RuntimeSecurityAdvisory) -> bool {
        let candidate = Self::from_advisory(advisory);
        self.bits & candidate.bits != 0
    }

    pub const fn is_empty(self) -> bool {
        self.bits == 0
    }

    pub const fn len(self) -> usize {
        self.bits.count_ones() as usize
    }

    pub fn iter(self) -> impl Iterator<Item = RuntimeSecurityAdvisory> {
        Self::ORDERED
            .into_iter()
            .filter(move |advisory| self.contains(*advisory))
    }
}

/// How long after the release gate's review deadline an installed build
/// keeps quiet about it. The weekly review cadence is for releases; a person
/// on current software should not be told their browser is overdue days after
/// installing it. Two missed review cycles of that length is a real signal.
pub const RUNTIME_REVIEW_GRACE_SECONDS: u64 = 60 * 24 * 60 * 60;

pub const fn overdue_review_advisory(
    unix_seconds: u64,
    review_deadline_exclusive: u64,
) -> Option<RuntimeSecurityAdvisory> {
    if unix_seconds >= review_deadline_exclusive.saturating_add(RUNTIME_REVIEW_GRACE_SECONDS) {
        Some(RuntimeSecurityAdvisory::review_overdue())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn independent_advisories_are_canonical_bounded_and_never_displace_each_other() {
        let mut advisories =
            RuntimeSecurityAdvisories::new().with_optional(overdue_review_advisory(u64::MAX, 20));
        advisories.insert(RuntimeSecurityAdvisory::unreviewed_runtime());
        advisories.insert(RuntimeSecurityAdvisory::update_recommended(
            RuntimeSecurityUpdateTarget::OperatingSystem,
        ));
        advisories.insert(RuntimeSecurityAdvisory::review_overdue());

        assert_eq!(advisories.len(), 3);
        assert_eq!(
            advisories.iter().collect::<Vec<_>>(),
            vec![
                RuntimeSecurityAdvisory::review_overdue(),
                RuntimeSecurityAdvisory::update_recommended(
                    RuntimeSecurityUpdateTarget::OperatingSystem,
                ),
                RuntimeSecurityAdvisory::unreviewed_runtime(),
            ]
        );
    }

    #[test]
    fn closed_vocabulary_fits_without_capacity_failure() {
        let mut advisories = RuntimeSecurityAdvisories::new();
        for advisory in RuntimeSecurityAdvisories::ORDERED {
            advisories.insert(advisory);
        }
        assert_eq!(advisories.len(), RuntimeSecurityAdvisories::ORDERED.len());
    }

    #[test]
    fn review_deadline_is_advisory_after_its_grace() {
        let due = 20 + RUNTIME_REVIEW_GRACE_SECONDS;
        assert_eq!(overdue_review_advisory(20, 20), None);
        assert_eq!(overdue_review_advisory(due - 1, 20), None);
        assert_eq!(
            overdue_review_advisory(due, 20),
            Some(RuntimeSecurityAdvisory::review_overdue())
        );
        assert_eq!(
            overdue_review_advisory(u64::MAX, 20),
            Some(RuntimeSecurityAdvisory::review_overdue())
        );
    }
}
