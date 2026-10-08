//! Platform-independent macOS/WebKit runtime admission policy.
//!
//! Apple ships WebKit security fixes as Safari updates (Safari 27 reached
//! Sequoia and Tahoe on its own, not through a macOS release) and other system
//! fixes as macOS updates. Admission therefore checks both product versions
//! against what each macOS line can install, and compares the installed Safari
//! build with the WebKit framework build that actually owns `WKWebView`. Only an unsupported major
//! release blocks startup; a runtime below the reviewed security floor starts
//! with an update advisory, because refusing to open the browser protects
//! nobody and strands the user.

use std::fmt;
use std::str::FromStr;

use crate::runtime_security::{
    overdue_review_advisory, RuntimeSecurityAdvisories, RuntimeSecurityAdvisory,
    RuntimeSecurityUpdateTarget,
};

pub const SONOMA_SECURITY_FLOOR: ProductVersion = ProductVersion::new(14, 8, 9);
pub const SEQUOIA_SECURITY_FLOOR: ProductVersion = ProductVersion::new(15, 8, 1);
pub const TAHOE_SECURITY_FLOOR: ProductVersion = ProductVersion::new(26, 7, 1);
pub const GOLDEN_GATE_SECURITY_FLOOR: ProductVersion = ProductVersion::new(27, 0, 1);
/// The oldest Safari any supported line may run. Its major is the hard floor;
/// the point release still decides the update advisory on Sonoma, which Safari
/// 27 did not reach.
pub const SAFARI_SECURITY_FLOOR: ProductVersion = ProductVersion::new(26, 6, 1);
/// The newest Safari line this review covers. A later major is unreviewed.
pub const SAFARI_REVIEWED_MAJOR: u32 = 27;

pub const SONOMA_RECOMMENDED: ProductVersion = ProductVersion::new(14, 8, 9);
pub const SEQUOIA_RECOMMENDED: ProductVersion = ProductVersion::new(15, 8, 1);
pub const TAHOE_RECOMMENDED: ProductVersion = ProductVersion::new(26, 7, 1);
pub const GOLDEN_GATE_RECOMMENDED: ProductVersion = ProductVersion::new(27, 0, 1);
/// Sonoma's newest Safari. Safari 27 shipped for Sequoia and Tahoe only.
pub const SONOMA_SAFARI_RECOMMENDED: ProductVersion = ProductVersion::new(26, 6, 1);
/// Safari 27 for Sequoia and Tahoe, and the Safari macOS 27 ships with.
pub const SAFARI_RECOMMENDED: ProductVersion = ProductVersion::new(27, 0, 0);

pub const SONOMA_SECURITY_FLOOR_TEXT: &str = "14.8.9";
pub const SEQUOIA_SECURITY_FLOOR_TEXT: &str = "15.8.1";
pub const TAHOE_SECURITY_FLOOR_TEXT: &str = "26.7.1";
pub const GOLDEN_GATE_SECURITY_FLOOR_TEXT: &str = "27.0.1";
pub const SAFARI_SECURITY_FLOOR_TEXT: &str = "26.6.1";
pub const SONOMA_RECOMMENDED_TEXT: &str = "14.8.9";
pub const SEQUOIA_RECOMMENDED_TEXT: &str = "15.8.1";
pub const TAHOE_RECOMMENDED_TEXT: &str = "26.7.1";
pub const GOLDEN_GATE_RECOMMENDED_TEXT: &str = "27.0.1";
pub const SONOMA_SAFARI_RECOMMENDED_TEXT: &str = "26.6.1";
pub const SAFARI_RECOMMENDED_TEXT: &str = "27.0";
pub const SECURITY_FLOOR_PUBLISHED_ON: &str = "2026-09-28";
/// 2026-09-28T00:00:00Z. A wall clock before the reviewed Apple security
/// release cannot establish that this floor was published and must fail closed.
pub const SECURITY_FLOOR_PUBLISHED_UNIX_SECONDS: u64 = 1_790_553_600;
pub const RECOMMENDED_RELEASE_PUBLISHED_ON: &str = "2026-09-28";
/// 2026-09-28T00:00:00Z.
pub const RECOMMENDED_RELEASE_PUBLISHED_UNIX_SECONDS: u64 = 1_790_553_600;
pub const SECURITY_FLOOR_SOURCE_URL: &str = "https://support.apple.com/en-us/100100";
pub const SAFARI_SECURITY_SOURCE_URL: &str = "https://support.apple.com/en-us/149039";
pub const SONOMA_SAFARI_SECURITY_SOURCE_URL: &str = "https://support.apple.com/en-us/148286";
pub const TAHOE_SECURITY_SOURCE_URL: &str = "https://support.apple.com/en-us/149228";
pub const SEQUOIA_SECURITY_SOURCE_URL: &str = "https://support.apple.com/en-us/149229";

/// The last UTC date on which CI may accept this review without an update.
// Reviewed against Apple's security releases on 2026-10-06. Safari 27
// (2026-09-14, Sequoia and Tahoe, four WebKit CVEs) is now the recommended
// Safari there; macOS 26.7.1, 15.8.1 (2026-09-28) and 27.0.1 (no published
// CVEs) are the newest system releases. Sonoma got neither Safari 27 nor a
// September update; it stays admitted on Safari 26.6.1.
pub const SECURITY_FLOOR_REVIEW_BY: &str = "2026-10-13";
/// 2026-10-14T00:00:00Z. The human-readable review date above is inclusive.
pub const SECURITY_FLOOR_REVIEW_DEADLINE_EXCLUSIVE_UNIX_SECONDS: u64 = 1_791_936_000;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ProductVersion([u32; 3]);

impl ProductVersion {
    pub const fn new(major: u32, minor: u32, patch: u32) -> Self {
        Self([major, minor, patch])
    }

    pub const fn components(self) -> [u32; 3] {
        self.0
    }

    pub const fn major(self) -> u32 {
        self.0[0]
    }
}

impl fmt::Display for ProductVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}.{}.{}", self.0[0], self.0[1], self.0[2])
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VersionParseError {
    Empty,
    TooLong,
    WrongComponentCount,
    InvalidComponent,
    NonCanonicalComponent,
    ComponentOverflow,
}

impl fmt::Display for VersionParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Empty => "version is empty",
            Self::TooLong => "version is longer than 32 bytes",
            Self::WrongComponentCount => "version has an unsupported numeric component count",
            Self::InvalidComponent => "version component is not an unsigned ASCII integer",
            Self::NonCanonicalComponent => "version component has a leading zero",
            Self::ComponentOverflow => "version component exceeds u32",
        })
    }
}

impl std::error::Error for VersionParseError {}

impl FromStr for ProductVersion {
    type Err = VersionParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.is_empty() {
            return Err(VersionParseError::Empty);
        }
        if value.len() > 32 {
            return Err(VersionParseError::TooLong);
        }

        let mut components = [0_u32; 3];
        let mut parsed = value.split('.');
        for component in &mut components {
            let raw = parsed
                .next()
                .ok_or(VersionParseError::WrongComponentCount)?;
            if raw.is_empty() || !raw.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err(VersionParseError::InvalidComponent);
            }
            if raw.len() > 1 && raw.starts_with('0') {
                return Err(VersionParseError::NonCanonicalComponent);
            }
            *component = raw
                .parse()
                .map_err(|_| VersionParseError::ComponentOverflow)?;
        }
        if parsed.next().is_some() {
            return Err(VersionParseError::WrongComponentCount);
        }
        Ok(Self(components))
    }
}

fn parse_safari_version(value: &str) -> Result<ProductVersion, VersionParseError> {
    if value.is_empty() {
        return Err(VersionParseError::Empty);
    }
    if value.len() > 32 {
        return Err(VersionParseError::TooLong);
    }

    let mut parsed = value.split('.');
    let major = parse_version_component(
        parsed
            .next()
            .ok_or(VersionParseError::WrongComponentCount)?,
    )?;
    let minor = parse_version_component(
        parsed
            .next()
            .ok_or(VersionParseError::WrongComponentCount)?,
    )?;
    let patch = match parsed.next() {
        Some(raw) => parse_version_component(raw)?,
        None => 0,
    };
    if parsed.next().is_some() {
        return Err(VersionParseError::WrongComponentCount);
    }
    Ok(ProductVersion::new(major, minor, patch))
}

fn parse_version_component(raw: &str) -> Result<u32, VersionParseError> {
    if raw.is_empty() || !raw.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(VersionParseError::InvalidComponent);
    }
    if raw.len() > 1 && raw.starts_with('0') {
        return Err(VersionParseError::NonCanonicalComponent);
    }
    raw.parse()
        .map_err(|_| VersionParseError::ComponentOverflow)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BuildVersionError {
    Empty,
    TooLong,
    TooManyComponents,
    InvalidComponent,
    NonCanonicalComponent,
}

impl fmt::Display for BuildVersionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Empty => "build version is empty",
            Self::TooLong => "build version is longer than 64 bytes",
            Self::TooManyComponents => "build version has more than eight components",
            Self::InvalidComponent => "build version component is not an unsigned ASCII integer",
            Self::NonCanonicalComponent => "build version component has a leading zero",
        })
    }
}

impl std::error::Error for BuildVersionError {}

fn validate_build_version(value: &str) -> Result<(), BuildVersionError> {
    if value.is_empty() {
        return Err(BuildVersionError::Empty);
    }
    if value.len() > 64 {
        return Err(BuildVersionError::TooLong);
    }
    let mut count = 0_usize;
    for raw in value.split('.') {
        count += 1;
        if count > 8 {
            return Err(BuildVersionError::TooManyComponents);
        }
        if raw.is_empty() || !raw.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(BuildVersionError::InvalidComponent);
        }
        if raw.len() > 1 && raw.starts_with('0') {
            return Err(BuildVersionError::NonCanonicalComponent);
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdmissionError {
    InvalidOperatingSystemVersion(VersionParseError),
    InvalidSafariVersion(VersionParseError),
    InvalidSafariBuild(BuildVersionError),
    InvalidWebKitBuild(BuildVersionError),
    UnsupportedOperatingSystemMajor(u32),
    UnsupportedSafariMajor(u32),
}

impl fmt::Display for AdmissionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidOperatingSystemVersion(error) => {
                write!(formatter, "invalid operating-system version: {error}")
            }
            Self::InvalidSafariVersion(error) => {
                write!(formatter, "invalid Safari version: {error}")
            }
            Self::InvalidSafariBuild(error) => write!(formatter, "invalid Safari build: {error}"),
            Self::InvalidWebKitBuild(error) => write!(formatter, "invalid WebKit build: {error}"),
            Self::UnsupportedOperatingSystemMajor(major) => {
                write!(
                    formatter,
                    "macOS major {major} has not been security-reviewed"
                )
            }
            Self::UnsupportedSafariMajor(major) => {
                write!(
                    formatter,
                    "Safari major {major} has not been security-reviewed"
                )
            }
        }
    }
}

impl std::error::Error for AdmissionError {}

/// Assess one Apple-supplied macOS/WebKit combination without performing I/O.
///
/// `safari_build` must come from the protected system Safari bundle and
/// `webkit_build` from the bundle owning the loaded `WKWebView` class. Their
/// equality is what connects Safari's marketing version to the shared WebKit
/// framework used by the embedder. Unsupported majors are hard failures;
/// outdated point releases, review age and newer lines are advisories.
pub fn assess_runtime(
    operating_system: &str,
    safari: &str,
    safari_build: &str,
    webkit_build: &str,
    unix_seconds: u64,
) -> Result<RuntimeSecurityAdvisories, AdmissionError> {
    let operating_system = operating_system
        .parse::<ProductVersion>()
        .map_err(AdmissionError::InvalidOperatingSystemVersion)?;
    let safari = parse_safari_version(safari).map_err(AdmissionError::InvalidSafariVersion)?;
    validate_build_version(safari_build).map_err(AdmissionError::InvalidSafariBuild)?;
    validate_build_version(webkit_build).map_err(AdmissionError::InvalidWebKitBuild)?;
    let mut advisories = RuntimeSecurityAdvisories::new().with_optional(overdue_review_advisory(
        unix_seconds,
        SECURITY_FLOOR_REVIEW_DEADLINE_EXCLUSIVE_UNIX_SECONDS,
    ));
    // A mismatch usually means Safari was updated and macOS has not restarted
    // yet, so the loaded WebKit cannot be tied to Safari's version.
    if safari_build != webkit_build {
        advisories.insert(RuntimeSecurityAdvisory::update_recommended(
            RuntimeSecurityUpdateTarget::OperatingSystem,
        ));
    }
    let versions = match operating_system.major() {
        14 => Some((
            SONOMA_SECURITY_FLOOR,
            SONOMA_RECOMMENDED,
            SONOMA_SAFARI_RECOMMENDED,
        )),
        15 => Some((
            SEQUOIA_SECURITY_FLOOR,
            SEQUOIA_RECOMMENDED,
            SAFARI_RECOMMENDED,
        )),
        26 => Some((TAHOE_SECURITY_FLOOR, TAHOE_RECOMMENDED, SAFARI_RECOMMENDED)),
        27 => Some((
            GOLDEN_GATE_SECURITY_FLOOR,
            GOLDEN_GATE_RECOMMENDED,
            SAFARI_RECOMMENDED,
        )),
        major if major > 27 => {
            advisories.insert(RuntimeSecurityAdvisory::unreviewed_runtime());
            None
        }
        major => return Err(AdmissionError::UnsupportedOperatingSystemMajor(major)),
    };

    if safari.major() < SAFARI_SECURITY_FLOOR.major() {
        return Err(AdmissionError::UnsupportedSafariMajor(safari.major()));
    }
    if safari.major() > SAFARI_REVIEWED_MAJOR {
        advisories.insert(RuntimeSecurityAdvisory::unreviewed_runtime());
    }
    if let Some((required, recommended, safari_recommended)) = versions {
        // Software Update offers both, so either being behind is one advisory.
        if operating_system < required.max(recommended)
            || safari < SAFARI_SECURITY_FLOOR.max(safari_recommended)
        {
            advisories.insert(RuntimeSecurityAdvisory::update_recommended(
                RuntimeSecurityUpdateTarget::OperatingSystem,
            ));
        }
    }
    Ok(advisories)
}

pub const fn security_floor_review_is_current(unix_seconds: u64) -> bool {
    unix_seconds >= RECOMMENDED_RELEASE_PUBLISHED_UNIX_SECONDS
        && unix_seconds < SECURITY_FLOOR_REVIEW_DEADLINE_EXCLUSIVE_UNIX_SECONDS
}

#[cfg(test)]
mod tests {
    use super::*;

    const OVERDUE: u64 = SECURITY_FLOOR_REVIEW_DEADLINE_EXCLUSIVE_UNIX_SECONDS
        + crate::runtime_security::RUNTIME_REVIEW_GRACE_SECONDS;

    const BUILD: &str = "21624.3.4.5.6";

    fn assess_at_review(
        operating_system: &str,
        safari: &str,
        safari_build: &str,
        webkit_build: &str,
    ) -> Result<RuntimeSecurityAdvisories, AdmissionError> {
        assess_runtime(
            operating_system,
            safari,
            safari_build,
            webkit_build,
            RECOMMENDED_RELEASE_PUBLISHED_UNIX_SECONDS,
        )
    }

    #[test]
    fn strict_product_version_parser_rejects_ambiguous_input() {
        let version: ProductVersion = "26.5.2".parse().unwrap();
        assert_eq!(version.components(), [26, 5, 2]);
        assert_eq!(version.to_string(), "26.5.2");

        for value in [
            "",
            "26",
            "26.5",
            "26.5.2.0",
            ".5.2",
            "26..2",
            "26.5.",
            "026.5.2",
            "26.05.2",
            "26.5.02",
            "+26.5.2",
            "26.5.-2",
            "26.5.2 ",
            " 26.5.2",
            "２６.5.2",
            "4294967296.0.0",
        ] {
            assert!(
                value.parse::<ProductVersion>().is_err(),
                "unexpectedly accepted {value:?}"
            );
        }
        assert_eq!(
            "1".repeat(33).parse::<ProductVersion>(),
            Err(VersionParseError::TooLong)
        );
    }

    #[test]
    fn safari_parser_canonically_supports_apples_omitted_zero_patch() {
        assert_eq!(
            parse_safari_version("26.5").unwrap().components(),
            [26, 5, 0]
        );
        assert_eq!(
            parse_safari_version("26.5.2").unwrap().components(),
            [26, 5, 2]
        );
        for value in ["26", "26.5.2.0", "26.05", "26.5.", "26.5 ", "26.beta"] {
            assert!(parse_safari_version(value).is_err());
        }
    }

    #[test]
    fn admits_reviewed_release_lines_without_advisories() {
        for (os, safari) in [
            ("14.8.9", "26.6.1"),
            ("14.9.0", "26.6.2"),
            ("15.8.1", "27.0"),
            ("15.8.2", "27.0.1"),
            ("26.7.1", "27.0"),
            ("26.7.2", "27.1"),
            ("27.0.1", "27.0"),
        ] {
            assert_eq!(
                assess_at_review(os, safari, BUILD, BUILD),
                Ok(RuntimeSecurityAdvisories::new())
            );
        }
    }

    #[test]
    fn outdated_os_or_safari_recommends_an_update_instead_of_blocking() {
        let update =
            RuntimeSecurityAdvisories::from_advisory(RuntimeSecurityAdvisory::update_recommended(
                RuntimeSecurityUpdateTarget::OperatingSystem,
            ));
        for (os, safari) in [
            ("14.0.0", "26.6.1"),
            ("14.8.8", "26.6.1"),
            ("15.7.9", "26.6.1"),
            ("15.8.0", "26.6.1"),
            ("26.6.2", "26.6.1"),
            ("26.7.0", "26.6.1"),
            ("14.8.9", "26.0"),
            ("15.8.1", "26.6"),
            // Safari 27 carries WebKit fixes Sequoia and Tahoe cannot get
            // from their own point releases.
            ("15.8.1", "26.6.1"),
            ("26.7.1", "26.6.1"),
            ("27.0.0", "27.0"),
        ] {
            assert_eq!(assess_at_review(os, safari, BUILD, BUILD), Ok(update));
        }
    }

    #[test]
    fn rejects_unsupported_os_or_safari_majors() {
        for (os, safari) in [
            ("13.9.9", "26.6.1"),
            ("16.0.0", "26.6.1"),
            ("25.0.0", "27.0"),
        ] {
            assert!(matches!(
                assess_at_review(os, safari, BUILD, BUILD),
                Err(AdmissionError::UnsupportedOperatingSystemMajor(_))
            ));
        }
        assert_eq!(
            assess_at_review("15.8.1", "18.6", BUILD, BUILD),
            Err(AdmissionError::UnsupportedSafariMajor(18))
        );
    }

    #[test]
    fn current_floor_and_future_runtime_advisories_remain_distinct() {
        assert_eq!(
            assess_runtime(
                "26.7.1",
                "27.0",
                BUILD,
                BUILD,
                RECOMMENDED_RELEASE_PUBLISHED_UNIX_SECONDS,
            ),
            Ok(RuntimeSecurityAdvisories::new())
        );
        assert_eq!(
            assess_runtime(
                "28.0.0",
                "28.0",
                BUILD,
                BUILD,
                RECOMMENDED_RELEASE_PUBLISHED_UNIX_SECONDS,
            ),
            Ok(RuntimeSecurityAdvisories::from_advisory(
                RuntimeSecurityAdvisory::unreviewed_runtime(),
            ))
        );
        assert_eq!(
            assess_runtime(
                "15.8.1",
                "28.0",
                BUILD,
                BUILD,
                RECOMMENDED_RELEASE_PUBLISHED_UNIX_SECONDS,
            ),
            Ok(RuntimeSecurityAdvisories::from_advisory(
                RuntimeSecurityAdvisory::unreviewed_runtime(),
            ))
        );
        assert_eq!(
            assess_runtime("26.7.1", "27.0", BUILD, BUILD, OVERDUE,),
            Ok(RuntimeSecurityAdvisories::from_advisory(
                RuntimeSecurityAdvisory::review_overdue(),
            ))
        );

        let combined = assess_runtime("28.0.0", "28.0", BUILD, BUILD, OVERDUE).unwrap();
        assert!(combined.contains(RuntimeSecurityAdvisory::review_overdue()));
        assert!(combined.contains(RuntimeSecurityAdvisory::unreviewed_runtime()));
    }

    #[test]
    fn rejects_malformed_bundle_builds_and_flags_mismatched_ones() {
        assert_eq!(
            assess_at_review("26.7.1", "27.0", "21624.1", "21624.2"),
            Ok(RuntimeSecurityAdvisories::from_advisory(
                RuntimeSecurityAdvisory::update_recommended(
                    RuntimeSecurityUpdateTarget::OperatingSystem,
                ),
            ))
        );
        for malformed in [
            "",
            ".21624",
            "21624.",
            "21624..1",
            "021624.1",
            "21624.a",
            "21624-1",
            "1.2.3.4.5.6.7.8.9",
        ] {
            assert!(assess_at_review("26.7.1", "27.0", malformed, BUILD).is_err());
            assert!(assess_at_review("26.7.1", "27.0", BUILD, malformed).is_err());
        }
    }

    #[test]
    fn maintenance_deadline_is_an_exclusive_utc_boundary() {
        assert!(!security_floor_review_is_current(0));
        assert!(!security_floor_review_is_current(
            SECURITY_FLOOR_PUBLISHED_UNIX_SECONDS - 1
        ));
        assert!(!security_floor_review_is_current(
            RECOMMENDED_RELEASE_PUBLISHED_UNIX_SECONDS - 1
        ));
        assert!(security_floor_review_is_current(
            RECOMMENDED_RELEASE_PUBLISHED_UNIX_SECONDS
        ));
        assert!(security_floor_review_is_current(
            SECURITY_FLOOR_REVIEW_DEADLINE_EXCLUSIVE_UNIX_SECONDS - 1
        ));
        assert!(!security_floor_review_is_current(
            SECURITY_FLOOR_REVIEW_DEADLINE_EXCLUSIVE_UNIX_SECONDS
        ));
        assert!(!security_floor_review_is_current(u64::MAX));
    }
}
