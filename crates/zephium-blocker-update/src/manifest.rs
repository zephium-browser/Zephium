use std::collections::HashSet;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use url::Url;
use zephium_blocker::Sha256;
use zephium_blocker::{PolicySource, SourceFormat, SourceId, StaticPolicyCatalog};

use crate::types::{CatalogIdentity, LicensePolicy, UpdateLimits};

/// Exact TUF target containing the package catalog.
pub const CATALOG_MANIFEST_TARGET: &str = "zephium-filter-catalog-v1.json";
/// Current canonical package-manifest schema.
pub const CATALOG_MANIFEST_VERSION: u32 = 1;
const MAX_LICENSE_FIELD_BYTES: usize = 2 * 1024;
const MAX_SOURCE_URL_BYTES: usize = 4 * 1024;
const MAX_TARGET_NAME_BYTES: usize = 128;
const MAX_PACKAGE_LIFETIME_SECONDS: u64 = 93 * 24 * 60 * 60;
const MAX_FUTURE_CLOCK_SKEW_SECONDS: u64 = 24 * 60 * 60;
const MIN_PACKAGE_REMAINING_VALIDITY_SECONDS: u64 = 24 * 60 * 60;

/// Canonical, signed description of one filter package.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogManifest {
    /// Exact schema version.
    pub schema_version: u32,
    /// Strictly monotonic repository package revision.
    pub revision: u64,
    /// Package creation time as Unix seconds.
    pub created_unix: u64,
    /// Package expiry time as Unix seconds.
    pub expires_unix: u64,
    /// Exact ordered source descriptors.
    pub sources: Vec<ManifestSource>,
}

/// One immutable source target.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestSource {
    /// Stable compiler source identifier.
    pub id: String,
    /// Source syntax.
    pub format: ManifestSourceFormat,
    /// Exact non-normalized TUF target name.
    pub target: String,
    /// Exact UTF-8 byte length.
    pub length: u64,
    /// Lowercase SHA-256 hexadecimal digest.
    pub sha256: String,
    /// Mandatory distribution and attribution record.
    pub license: LicenseMetadata,
}

/// Source syntax encoded in the package manifest.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ManifestSourceFormat {
    /// Adblock Plus/uBlock-compatible network rules.
    Standard,
    /// Hosts-file entries.
    Hosts,
}

impl From<ManifestSourceFormat> for SourceFormat {
    fn from(value: ManifestSourceFormat) -> Self {
        match value {
            ManifestSourceFormat::Standard => Self::Standard,
            ManifestSourceFormat::Hosts => Self::Hosts,
        }
    }
}

/// Mandatory legal and provenance record for one redistributed source.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LicenseMetadata {
    /// SPDX expression or a stable project-owned license identifier.
    pub license_expression: String,
    /// Human-readable attribution required by the source.
    pub attribution: String,
    /// Stable record of redistribution terms or approval.
    pub redistribution: String,
    /// Canonical upstream source page.
    pub source_url: String,
}

impl CatalogManifest {
    /// Serializes the sole accepted canonical JSON representation.
    pub fn encode_canonical(&self) -> Result<Vec<u8>, ManifestError> {
        serde_json::to_vec(self).map_err(|_| ManifestError::Malformed)
    }

    /// Parses and fully validates exact canonical manifest bytes.
    pub fn parse_canonical(
        bytes: &[u8],
        now_unix: u64,
        limits: UpdateLimits,
        license_policy: &LicensePolicy,
    ) -> Result<Self, ManifestError> {
        if bytes.is_empty() || bytes.len() > limits.max_manifest_bytes {
            return Err(ManifestError::Size);
        }
        let manifest: Self = serde_json::from_slice(bytes).map_err(|_| ManifestError::Malformed)?;
        if manifest.encode_canonical()?.as_slice() != bytes {
            return Err(ManifestError::NonCanonical);
        }
        manifest.validate(now_unix, limits, license_policy)?;
        Ok(manifest)
    }

    #[cfg(feature = "tuf")]
    pub(crate) fn parse_cached(
        bytes: &[u8],
        limits: UpdateLimits,
        license_policy: &LicensePolicy,
    ) -> Result<Self, ManifestError> {
        Self::parse_release_seed(bytes, limits, license_policy)
    }

    /// Parses an exact canonical manifest embedded in a signed application
    /// release.
    ///
    /// A signed release remains authoritative after the sources' recommended
    /// refresh interval ends, so this validates the complete structural,
    /// resource, and license policy without applying the online candidate's
    /// current-time admission check. Callers must expose
    /// [`CatalogManifest::expires_unix`] as refresh advice, not as release
    /// policy invalidation.
    pub fn parse_release_seed(
        bytes: &[u8],
        limits: UpdateLimits,
        license_policy: &LicensePolicy,
    ) -> Result<Self, ManifestError> {
        if bytes.is_empty() || bytes.len() > limits.max_manifest_bytes {
            return Err(ManifestError::Size);
        }
        let manifest: Self = serde_json::from_slice(bytes).map_err(|_| ManifestError::Malformed)?;
        if manifest.encode_canonical()?.as_slice() != bytes {
            return Err(ManifestError::NonCanonical);
        }
        manifest.validate_structure(limits, license_policy)?;
        Ok(manifest)
    }

    fn validate(
        &self,
        now_unix: u64,
        limits: UpdateLimits,
        license_policy: &LicensePolicy,
    ) -> Result<(), ManifestError> {
        self.validate_structure(limits, license_policy)?;
        if self.created_unix > now_unix.saturating_add(MAX_FUTURE_CLOCK_SKEW_SECONDS) {
            return Err(ManifestError::Time);
        }
        if self.expires_unix <= now_unix {
            return Err(ManifestError::Expired);
        }
        if self.expires_unix.saturating_sub(now_unix) < MIN_PACKAGE_REMAINING_VALIDITY_SECONDS {
            return Err(ManifestError::ValidityTooShort);
        }
        Ok(())
    }

    fn validate_structure(
        &self,
        limits: UpdateLimits,
        license_policy: &LicensePolicy,
    ) -> Result<(), ManifestError> {
        if self.schema_version != CATALOG_MANIFEST_VERSION {
            return Err(ManifestError::Schema);
        }
        if self.revision == 0 {
            return Err(ManifestError::Revision);
        }
        let valid_times = self.created_unix <= self.expires_unix
            && self.expires_unix.saturating_sub(self.created_unix) <= MAX_PACKAGE_LIFETIME_SECONDS;
        if !valid_times {
            return Err(ManifestError::Time);
        }
        if self.sources.is_empty() || self.sources.len() > limits.max_sources {
            return Err(ManifestError::SourceCount);
        }

        let mut ids = HashSet::with_capacity(self.sources.len());
        let mut targets = HashSet::with_capacity(self.sources.len());
        let mut total = 0u64;
        for source in &self.sources {
            SourceId::new(source.id.as_str()).map_err(|_| ManifestError::SourceId)?;
            if !ids.insert(source.id.as_str()) {
                return Err(ManifestError::DuplicateSourceId);
            }
            validate_target_name(&source.target)?;
            if !targets.insert(source.target.as_str()) {
                return Err(ManifestError::DuplicateTarget);
            }
            if source.length == 0 || source.length > limits.max_source_bytes {
                return Err(ManifestError::SourceSize);
            }
            total = total
                .checked_add(source.length)
                .ok_or(ManifestError::TotalSize)?;
            if total > limits.max_total_source_bytes {
                return Err(ManifestError::TotalSize);
            }
            decode_sha256(&source.sha256)?;
            source.license.validate(license_policy)?;
        }
        Ok(())
    }

    /// Builds a compiler catalog after revalidating every exact source body
    /// against the signed manifest.
    ///
    /// TUF target verification and embedded-release verification are separate
    /// trust paths, but both converge on this final length, digest, UTF-8, and
    /// catalog boundary.
    pub fn build_verified_catalog(
        &self,
        source_contents: &[Arc<str>],
    ) -> Result<StaticPolicyCatalog, ManifestError> {
        if source_contents.len() != self.sources.len() {
            return Err(ManifestError::TargetMismatch);
        }
        let mut sources = Vec::with_capacity(source_contents.len());
        for (descriptor, contents) in self.sources.iter().zip(source_contents) {
            if contents.len() as u64 != descriptor.length {
                return Err(ManifestError::TargetMismatch);
            }
            let expected = decode_sha256(&descriptor.sha256)?;
            if Sha256::digest(contents.as_bytes()).as_slice() != expected {
                return Err(ManifestError::TargetMismatch);
            }
            let id = SourceId::new(descriptor.id.as_str()).map_err(|_| ManifestError::SourceId)?;
            let trusted = zephium_blocker::official_source_trusts_scriptlets(&id);
            let source = PolicySource::new(id, descriptor.format.into(), Arc::clone(contents));
            sources.push(if trusted {
                source.trusting_scriptlets()
            } else {
                source
            });
        }
        StaticPolicyCatalog::new(sources).map_err(|_| ManifestError::Catalog)
    }

    /// Derives the immutable package identity bound to exact canonical
    /// manifest bytes.
    pub fn identity(&self, manifest_sha256: [u8; 32]) -> Result<CatalogIdentity, ManifestError> {
        let source_count =
            u32::try_from(self.sources.len()).map_err(|_| ManifestError::SourceCount)?;
        let source_bytes = self.sources.iter().try_fold(0u64, |total, source| {
            total
                .checked_add(source.length)
                .ok_or(ManifestError::TotalSize)
        })?;
        Ok(CatalogIdentity {
            revision: self.revision,
            manifest_sha256,
            created_unix: self.created_unix,
            expires_unix: self.expires_unix,
            source_count,
            source_bytes,
        })
    }
}

impl LicenseMetadata {
    fn validate(&self, license_policy: &LicensePolicy) -> Result<(), ManifestError> {
        validate_bounded_text(&self.license_expression)?;
        if !license_policy.accepts(&self.license_expression) {
            return Err(ManifestError::License);
        }
        validate_bounded_text(&self.attribution)?;
        validate_bounded_text(&self.redistribution)?;
        if self.source_url.is_empty() || self.source_url.len() > MAX_SOURCE_URL_BYTES {
            return Err(ManifestError::License);
        }
        let source_url = Url::parse(&self.source_url).map_err(|_| ManifestError::License)?;
        if source_url.scheme() != "https"
            || source_url.host_str().is_none()
            || !source_url.username().is_empty()
            || source_url.password().is_some()
            || source_url.query().is_some()
            || source_url.fragment().is_some()
        {
            return Err(ManifestError::License);
        }
        Ok(())
    }
}

fn validate_bounded_text(value: &str) -> Result<(), ManifestError> {
    if value.is_empty()
        || value.len() > MAX_LICENSE_FIELD_BYTES
        || value.chars().any(char::is_control)
    {
        return Err(ManifestError::License);
    }
    Ok(())
}

fn validate_target_name(value: &str) -> Result<(), ManifestError> {
    if value.is_empty() || value.len() > MAX_TARGET_NAME_BYTES || value == CATALOG_MANIFEST_TARGET {
        return Err(ManifestError::TargetName);
    }
    let bytes = value.as_bytes();
    let endpoint = |byte: u8| byte.is_ascii_lowercase() || byte.is_ascii_digit();
    if !endpoint(bytes[0])
        || !endpoint(bytes[bytes.len() - 1])
        || !bytes.iter().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'-' | b'_')
        })
    {
        return Err(ManifestError::TargetName);
    }
    Ok(())
}

pub(crate) fn decode_sha256(value: &str) -> Result<[u8; 32], ManifestError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(ManifestError::Digest);
    }
    let mut digest = [0u8; 32];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        digest[index] = (hex_nibble(pair[0]) << 4) | hex_nibble(pair[1]);
    }
    Ok(digest)
}

fn hex_nibble(byte: u8) -> u8 {
    match byte {
        b'0'..=b'9' => byte - b'0',
        b'a'..=b'f' => byte - b'a' + 10,
        _ => 0,
    }
}

/// Stable manifest rejection reason.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum ManifestError {
    /// Manifest byte size is empty or above its bound.
    #[error("manifest size is invalid")]
    Size,
    /// JSON decoding failed or unknown fields were present.
    #[error("manifest is malformed")]
    Malformed,
    /// JSON was valid but not in the exact canonical representation.
    #[error("manifest is not canonical")]
    NonCanonical,
    /// Schema version is not supported.
    #[error("manifest schema is unsupported")]
    Schema,
    /// Revision is not a valid positive revision.
    #[error("manifest revision is invalid")]
    Revision,
    /// Creation or expiry times violate package policy.
    #[error("manifest time range is invalid")]
    Time,
    /// Package validity has ended.
    #[error("manifest is expired")]
    Expired,
    /// Package validity is too short for safe staged compilation and rollout.
    #[error("manifest remaining validity is too short")]
    ValidityTooShort,
    /// Source count is empty or exceeds its bound.
    #[error("manifest source count is invalid")]
    SourceCount,
    /// A source identifier is invalid.
    #[error("manifest source identifier is invalid")]
    SourceId,
    /// Source identifiers are not unique.
    #[error("manifest source identifier is duplicated")]
    DuplicateSourceId,
    /// A source target name is unsafe or non-canonical.
    #[error("manifest target name is invalid")]
    TargetName,
    /// Source target names are not unique.
    #[error("manifest target name is duplicated")]
    DuplicateTarget,
    /// One source size is empty or exceeds its bound.
    #[error("manifest source size is invalid")]
    SourceSize,
    /// Combined source sizes overflow or exceed their bound.
    #[error("manifest total source size is invalid")]
    TotalSize,
    /// A SHA-256 digest is not exact lowercase hexadecimal.
    #[error("manifest digest is invalid")]
    Digest,
    /// Required license, attribution, redistribution, or source metadata is invalid.
    #[error("manifest license metadata is invalid")]
    License,
    /// Downloaded targets do not correspond exactly to the manifest.
    #[error("manifest target set does not match downloaded targets")]
    TargetMismatch,
    /// A completely verified target is not UTF-8.
    #[error("verified source target is not UTF-8")]
    NotUtf8,
    /// Verified sources exceed the compiler catalog boundary.
    #[error("verified sources cannot form a compiler catalog")]
    Catalog,
}

#[cfg(test)]
mod tests {
    use super::validate_target_name;

    #[test]
    fn source_target_names_are_single_segment_and_canonical() {
        for valid in ["easylist.txt", "easyprivacy-v2.txt", "list_01.dat"] {
            assert_eq!(validate_target_name(valid), Ok(()), "{valid}");
        }
        for invalid in [
            "",
            ".hidden",
            "trailing.",
            "Uppercase.txt",
            "nested/list.txt",
            r"nested\list.txt",
            "../list.txt",
            "list%2etxt",
            "white space.txt",
            "zephium-filter-catalog-v1.json",
        ] {
            assert!(validate_target_name(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn source_target_name_length_is_bounded() {
        assert!(validate_target_name(&format!("a{}z", "x".repeat(126))).is_ok());
        assert!(validate_target_name(&format!("a{}z", "x".repeat(127))).is_err());
    }
}
