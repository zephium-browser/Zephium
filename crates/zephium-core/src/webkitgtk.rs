//! Platform-independent WebKitGTK runtime admission and review policy.
//!
//! The installed Linux library is part of the browser's security boundary.
//! Keep the enforced minimum tied to a published security advisory. The
//! reviewed stable line receives patch recommendations, while a newer stable
//! even-minor 2.x line is admitted with an explicit unreviewed-runtime warning.

use std::fmt;

use crate::runtime_security::{
    overdue_review_advisory, RuntimeSecurityAdvisories, RuntimeSecurityAdvisory,
    RuntimeSecurityUpdateTarget,
};

pub const SECURITY_FLOOR: [u32; 3] = [2, 54, 0];
pub const SECURITY_FLOOR_TEXT: &str = "2.54.0";
pub const SECURITY_FLOOR_PUBLISHED_ON: &str = "2026-09-29";
pub const SECURITY_FLOOR_SOURCE_URL: &str = "https://webkitgtk.org/security/WSA-2026-0006.html";
/// 2026-09-29T00:00:00Z. A wall clock before the reviewed advisory cannot
/// establish that the security floor was published and must fail closed.
pub const SECURITY_FLOOR_PUBLISHED_UNIX_SECONDS: u64 = 1_790_640_000;

pub const REVIEWED_STABLE_RELEASE_LINE: [u32; 2] = [2, 54];
pub const REVIEWED_STABLE_RELEASE_LINE_TEXT: &str = "2.54";

pub const LATEST_REVIEWED: [u32; 3] = [2, 54, 1];
pub const LATEST_REVIEWED_TEXT: &str = "2.54.1";
pub const LATEST_REVIEWED_PUBLISHED_ON: &str = "2026-10-02";
pub const LATEST_REVIEWED_SOURCE_URL: &str =
    "https://webkitgtk.org/2026/10/02/webkitgtk2.54.1-released.html";

/// Re-reviewed on 2026-10-04 against the official security-advisory index and
/// release feed. WSA-2026-0006 lists fixes in 2.54.0, the first stable 2.54
/// release, and no 2.52 release after 2.52.6 carries them, so 2.54.0 is the
/// newest stable security boundary. 2.54.1 is a bug-fix release; 2.53.92 is an
/// odd-minor development release.
///
/// The last UTC date on which CI may accept this review without an update.
pub const SECURITY_FLOOR_REVIEW_BY: &str = "2026-11-03";
/// 2026-11-04T00:00:00Z. The human-readable review date above is inclusive.
pub const SECURITY_FLOOR_REVIEW_DEADLINE_EXCLUSIVE_UNIX_SECONDS: u64 = 1_793_750_400;

/// Environment switches that can disable/replace renderer confinement,
/// expose a remote inspector, pause a child for a debugger, or turn off
/// JavaScriptCore allocator/JIT mitigations. The current stable WebKitGTK no
/// longer lets `WEBKIT_FORCE_SANDBOX=0` disable the sandbox, but rejecting the
/// legacy switch keeps admission independent of version-specific parsing and
/// prevents it becoming dangerous again after a runtime change.
pub const SECURITY_RELEVANT_ENVIRONMENT_OVERRIDES: [&str; 10] = [
    "WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS",
    "WEBKIT_FORCE_SANDBOX",
    "WEBKIT_INSPECTOR_SERVER",
    "WEBKIT_INSPECTOR_HTTP_SERVER",
    "WEBKIT2_PAUSE_WEB_PROCESS_ON_LAUNCH",
    "WEBKIT_SAMPLE_MEMORY",
    "WEBKIT_DISABLE_MEMORY_PRESSURE_MONITOR",
    "GIGACAGE_ENABLED",
    "JavaScriptCoreUseJIT",
    "Malloc",
];

pub fn environment_override_is_security_relevant(name: &str) -> bool {
    name.starts_with("JSC_") || SECURITY_RELEVANT_ENVIRONMENT_OVERRIDES.contains(&name)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdmissionError {
    BelowSecurityFloor { found: [u32; 3], required: [u32; 3] },
    UnreviewedReleaseLine { found: [u32; 3], reviewed: [u32; 2] },
}

impl fmt::Display for AdmissionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BelowSecurityFloor { found, required } => write!(
                formatter,
                "WebKitGTK {}.{}.{} is below security floor {}.{}.{}",
                found[0], found[1], found[2], required[0], required[1], required[2],
            ),
            Self::UnreviewedReleaseLine { found, reviewed } => write!(
                formatter,
                "WebKitGTK {}.{}.{} is outside the reviewed stable {}.{} release line",
                found[0], found[1], found[2], reviewed[0], reviewed[1],
            ),
        }
    }
}

impl std::error::Error for AdmissionError {}

pub fn admit_runtime(major: u32, minor: u32, micro: u32) -> Result<(), AdmissionError> {
    let found = [major, minor, micro];
    if found < SECURITY_FLOOR {
        return Err(AdmissionError::BelowSecurityFloor {
            found,
            required: SECURITY_FLOOR,
        });
    }
    // WebKitGTK uses even minor numbers for stable release lines. A newer
    // stable 2.x line retains the hard ABI/security postconditions and is
    // admitted with an advisory by `assess_runtime`; development or unrelated
    // major lines remain hard failures.
    if major != REVIEWED_STABLE_RELEASE_LINE[0]
        || minor < REVIEWED_STABLE_RELEASE_LINE[1]
        || !minor.is_multiple_of(2)
    {
        return Err(AdmissionError::UnreviewedReleaseLine {
            found,
            reviewed: REVIEWED_STABLE_RELEASE_LINE,
        });
    }
    Ok(())
}

pub fn assess_runtime(
    major: u32,
    minor: u32,
    micro: u32,
    unix_seconds: u64,
) -> Result<RuntimeSecurityAdvisories, AdmissionError> {
    admit_runtime(major, minor, micro)?;
    Ok(runtime_advisories(
        [major, minor, micro],
        REVIEWED_STABLE_RELEASE_LINE,
        LATEST_REVIEWED,
        unix_seconds,
        SECURITY_FLOOR_REVIEW_DEADLINE_EXCLUSIVE_UNIX_SECONDS,
    ))
}

fn runtime_advisories(
    found: [u32; 3],
    reviewed_line: [u32; 2],
    latest_reviewed: [u32; 3],
    unix_seconds: u64,
    review_deadline_exclusive: u64,
) -> RuntimeSecurityAdvisories {
    let mut advisories = RuntimeSecurityAdvisories::new().with_optional(overdue_review_advisory(
        unix_seconds,
        review_deadline_exclusive,
    ));
    if [found[0], found[1]] != reviewed_line {
        advisories.insert(RuntimeSecurityAdvisory::unreviewed_runtime());
    } else if found < latest_reviewed {
        advisories.insert(RuntimeSecurityAdvisory::update_recommended(
            RuntimeSecurityUpdateTarget::OperatingSystem,
        ));
    }
    advisories
}

pub const fn security_floor_review_is_current(unix_seconds: u64) -> bool {
    unix_seconds >= SECURITY_FLOOR_PUBLISHED_UNIX_SECONDS
        && unix_seconds < SECURITY_FLOOR_REVIEW_DEADLINE_EXCLUSIVE_UNIX_SECONDS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admission_rejects_obsolete_and_development_release_lines() {
        assert_eq!(
            admit_runtime(2, 52, 6),
            Err(AdmissionError::BelowSecurityFloor {
                found: [2, 52, 6],
                required: SECURITY_FLOOR,
            })
        );
        assert_eq!(admit_runtime(2, 54, 0), Ok(()));
        assert_eq!(admit_runtime(2, 54, u32::MAX), Ok(()));
        assert_eq!(admit_runtime(2, 56, 0), Ok(()));

        for found in [[2, 55, 0], [2, 57, 0], [3, 0, 0]] {
            assert_eq!(
                admit_runtime(found[0], found[1], found[2]),
                Err(AdmissionError::UnreviewedReleaseLine {
                    found,
                    reviewed: REVIEWED_STABLE_RELEASE_LINE,
                })
            );
        }
        assert_eq!(
            assess_runtime(2, 54, 1, SECURITY_FLOOR_PUBLISHED_UNIX_SECONDS),
            Ok(RuntimeSecurityAdvisories::new())
        );
        assert_eq!(
            assess_runtime(2, 54, 0, SECURITY_FLOOR_PUBLISHED_UNIX_SECONDS),
            Ok(RuntimeSecurityAdvisories::from_advisory(
                RuntimeSecurityAdvisory::update_recommended(
                    RuntimeSecurityUpdateTarget::OperatingSystem,
                ),
            ))
        );
        assert_eq!(
            assess_runtime(2, 56, 0, SECURITY_FLOOR_PUBLISHED_UNIX_SECONDS),
            Ok(RuntimeSecurityAdvisories::from_advisory(
                RuntimeSecurityAdvisory::unreviewed_runtime(),
            ))
        );
        assert_eq!(
            assess_runtime(
                2,
                54,
                1,
                SECURITY_FLOOR_REVIEW_DEADLINE_EXCLUSIVE_UNIX_SECONDS
                    + crate::runtime_security::RUNTIME_REVIEW_GRACE_SECONDS,
            ),
            Ok(RuntimeSecurityAdvisories::from_advisory(
                RuntimeSecurityAdvisory::review_overdue(),
            ))
        );
    }

    #[test]
    fn reviewed_line_patch_updates_and_independent_review_age_are_reported() {
        let recommended = runtime_advisories([2, 54, 0], [2, 54], [2, 54, 1], 10, 20);
        assert!(
            recommended.contains(RuntimeSecurityAdvisory::update_recommended(
                RuntimeSecurityUpdateTarget::OperatingSystem,
            ))
        );

        let combined = runtime_advisories(
            [2, 56, 0],
            [2, 54],
            [2, 54, 1],
            20 + crate::runtime_security::RUNTIME_REVIEW_GRACE_SECONDS,
            20,
        );
        assert!(combined.contains(RuntimeSecurityAdvisory::review_overdue()));
        assert!(combined.contains(RuntimeSecurityAdvisory::unreviewed_runtime()));
    }

    #[test]
    fn maintenance_deadline_is_an_exclusive_utc_boundary() {
        assert!(!security_floor_review_is_current(0));
        assert!(!security_floor_review_is_current(
            SECURITY_FLOOR_PUBLISHED_UNIX_SECONDS - 1
        ));
        assert!(security_floor_review_is_current(
            SECURITY_FLOOR_PUBLISHED_UNIX_SECONDS
        ));
        assert!(security_floor_review_is_current(
            SECURITY_FLOOR_REVIEW_DEADLINE_EXCLUSIVE_UNIX_SECONDS - 1
        ));
        assert!(!security_floor_review_is_current(
            SECURITY_FLOOR_REVIEW_DEADLINE_EXCLUSIVE_UNIX_SECONDS
        ));
        assert!(!security_floor_review_is_current(u64::MAX));
    }

    #[test]
    fn security_relevant_environment_overrides_are_fail_closed() {
        for name in SECURITY_RELEVANT_ENVIRONMENT_OVERRIDES {
            assert!(environment_override_is_security_relevant(name), "{name}");
        }
        assert!(environment_override_is_security_relevant("JSC_useJITCage"));
        assert!(environment_override_is_security_relevant("JSC_dumpOptions"));
        assert!(!environment_override_is_security_relevant("GTK_THEME"));
        assert!(!environment_override_is_security_relevant("WEBKIT_DEBUG"));
    }
}
