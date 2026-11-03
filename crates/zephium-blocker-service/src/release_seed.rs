use std::collections::HashSet;
use std::io::{Cursor, Read};
use std::sync::Arc;

use flate2::bufread::GzDecoder;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use zephium_blocker::Sha256;
use zephium_blocker::{PolicyCatalog, StaticPolicyCatalog};
use zephium_blocker_update::{
    CatalogIdentity, CatalogManifest, LicensePolicy, ManifestError, UpdateLimits,
};
use zephium_core::ports::blocker::BlockerCompileFailure;

const RELEASE_SEED_SCHEMA_VERSION: u32 = 2;
const MAX_RELEASE_SEED_MANIFEST_BYTES: usize = 128 * 1024;
const MAX_PROVENANCE_FIELD_BYTES: usize = 256;

/// Canonical release-only envelope for compressed embedded source assets.
///
/// The TUF catalog manifest continues to bind the uncompressed source bytes.
/// This envelope additionally binds deterministic packaging and upstream
/// header provenance without pretending that release-authenticated material
/// was delivered by the update repository.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseSeedManifest {
    /// Exact envelope schema.
    pub schema_version: u32,
    /// Lowercase SHA-256 of the exact canonical catalog manifest.
    pub catalog_manifest_sha256: String,
    /// Exact ordered compressed asset set.
    pub assets: Vec<ReleaseSeedAssetManifest>,
}

/// Exact packaging and upstream-header identity of one embedded source.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseSeedAssetManifest {
    /// Target name shared with the canonical catalog manifest.
    pub target: String,
    /// Deterministic on-disk encoding.
    pub compression: ReleaseSeedCompression,
    /// Exact compressed byte length.
    pub compressed_length: u64,
    /// Lowercase SHA-256 of the compressed bytes.
    pub compressed_sha256: String,
    /// Exact title from the source's header.
    pub upstream_title: String,
    /// Exact version from the source's ABP header, or the UTC minute of its
    /// `Last modified` date for lists without one.
    pub upstream_version: String,
    /// Exact upstream commit from the source's ABP header; lists without an
    /// ABP header name none.
    pub upstream_commit: Option<String>,
}

/// Compression formats accepted by the release-seed loader.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReleaseSeedCompression {
    /// One deterministic RFC 1952 gzip member.
    Gzip,
}

/// One immutable byte slice embedded in the signed executable.
#[derive(Clone, Copy, Debug)]
pub struct EmbeddedReleaseAsset {
    target: &'static str,
    bytes: &'static [u8],
}

impl EmbeddedReleaseAsset {
    /// Associates exact embedded bytes with one catalog target name.
    pub const fn new(target: &'static str, bytes: &'static [u8]) -> Self {
        Self { target, bytes }
    }
}

/// Release-authenticated catalog ready for the managed compiler service.
#[derive(Clone, Debug)]
pub struct ReleaseCatalogSeed {
    pub(crate) identity: CatalogIdentity,
    pub(crate) catalog: PolicyCatalog,
}

impl ReleaseCatalogSeed {
    /// Validates the canonical manifests and exact compressed asset set, then
    /// creates a deferred catalog whose raw sources are inflated only after a
    /// persistent compiled-cache miss.
    pub fn from_embedded_gzip(
        catalog_manifest_bytes: &'static [u8],
        seed_manifest_bytes: &'static [u8],
        assets: Vec<EmbeddedReleaseAsset>,
        limits: UpdateLimits,
        license_policy: LicensePolicy,
    ) -> Result<Self, ReleaseSeedError> {
        let catalog =
            CatalogManifest::parse_release_seed(catalog_manifest_bytes, limits, &license_policy)
                .map_err(|_| ReleaseSeedError::CatalogManifest)?;
        let seed = ReleaseSeedManifest::parse_canonical(seed_manifest_bytes)?;
        let catalog_manifest_sha256: [u8; 32] = Sha256::digest(catalog_manifest_bytes);
        if decode_sha256(&seed.catalog_manifest_sha256)? != catalog_manifest_sha256 {
            return Err(ReleaseSeedError::CatalogIdentity);
        }
        validate_exact_asset_set(&catalog, &seed, &assets)?;
        let identity = catalog
            .identity(catalog_manifest_sha256)
            .map_err(|_| ReleaseSeedError::CatalogManifest)?;

        let deferred_catalog = catalog.clone();
        let deferred_seed = seed.clone();
        let deferred_assets: Arc<[EmbeddedReleaseAsset]> = assets.into();
        let policy = PolicyCatalog::deferred_reloadable(catalog_manifest_sha256, move || {
            load_catalog(&deferred_catalog, &deferred_seed, &deferred_assets)
                .map_err(|_| BlockerCompileFailure::InvalidSource)
        });
        Ok(Self {
            identity,
            catalog: policy,
        })
    }

    /// Returns the immutable release package identity.
    pub fn identity(&self) -> &CatalogIdentity {
        &self.identity
    }
}

impl ReleaseSeedManifest {
    /// Serializes the one accepted canonical JSON representation.
    pub fn encode_canonical(&self) -> Result<Vec<u8>, ReleaseSeedError> {
        serde_json::to_vec(self).map_err(|_| ReleaseSeedError::Envelope)
    }

    /// Parses and fully validates exact canonical envelope bytes.
    pub fn parse_canonical(bytes: &[u8]) -> Result<Self, ReleaseSeedError> {
        if bytes.is_empty() || bytes.len() > MAX_RELEASE_SEED_MANIFEST_BYTES {
            return Err(ReleaseSeedError::Envelope);
        }
        let manifest: Self =
            serde_json::from_slice(bytes).map_err(|_| ReleaseSeedError::Envelope)?;
        if manifest.encode_canonical()?.as_slice() != bytes {
            return Err(ReleaseSeedError::Envelope);
        }
        manifest.validate()?;
        Ok(manifest)
    }

    fn validate(&self) -> Result<(), ReleaseSeedError> {
        if self.schema_version != RELEASE_SEED_SCHEMA_VERSION
            || self.assets.is_empty()
            || self.assets.len() > 32
        {
            return Err(ReleaseSeedError::Envelope);
        }
        decode_sha256(&self.catalog_manifest_sha256)?;
        let mut targets = HashSet::with_capacity(self.assets.len());
        for asset in &self.assets {
            if asset.target.is_empty()
                || !targets.insert(asset.target.as_str())
                || asset.compressed_length == 0
                || asset.compressed_length > 16 * 1024 * 1024
            {
                return Err(ReleaseSeedError::Envelope);
            }
            decode_sha256(&asset.compressed_sha256)?;
            validate_provenance_text(&asset.upstream_title)?;
            if !(8..=20).contains(&asset.upstream_version.len())
                || !asset
                    .upstream_version
                    .bytes()
                    .all(|byte| byte.is_ascii_digit())
                || asset.upstream_commit.as_ref().is_some_and(|commit| {
                    commit.len() != 40
                        || !commit
                            .bytes()
                            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                })
            {
                return Err(ReleaseSeedError::Provenance);
            }
        }
        Ok(())
    }
}

fn validate_exact_asset_set(
    catalog: &CatalogManifest,
    seed: &ReleaseSeedManifest,
    assets: &[EmbeddedReleaseAsset],
) -> Result<(), ReleaseSeedError> {
    if catalog.sources.len() != seed.assets.len() || seed.assets.len() != assets.len() {
        return Err(ReleaseSeedError::AssetSet);
    }
    for ((source, descriptor), embedded) in catalog.sources.iter().zip(&seed.assets).zip(assets) {
        if source.target != descriptor.target || descriptor.target != embedded.target {
            return Err(ReleaseSeedError::AssetSet);
        }
        if embedded.bytes.len() as u64 != descriptor.compressed_length {
            return Err(ReleaseSeedError::CompressedAsset);
        }
        let expected = decode_sha256(&descriptor.compressed_sha256)?;
        let actual: [u8; 32] = Sha256::digest(embedded.bytes);
        if actual != expected {
            return Err(ReleaseSeedError::CompressedAsset);
        }
    }
    Ok(())
}

fn load_catalog(
    manifest: &CatalogManifest,
    seed: &ReleaseSeedManifest,
    assets: &[EmbeddedReleaseAsset],
) -> Result<StaticPolicyCatalog, ReleaseSeedError> {
    let mut contents = Vec::with_capacity(assets.len());
    for ((source, descriptor), asset) in manifest.sources.iter().zip(&seed.assets).zip(assets) {
        let expected_length =
            usize::try_from(source.length).map_err(|_| ReleaseSeedError::SourceAsset)?;
        let mut decoder = GzDecoder::new(Cursor::new(asset.bytes));
        let limit = source
            .length
            .checked_add(1)
            .ok_or(ReleaseSeedError::SourceAsset)?;
        let mut output = Vec::with_capacity(expected_length);
        decoder
            .by_ref()
            .take(limit)
            .read_to_end(&mut output)
            .map_err(|_| ReleaseSeedError::SourceAsset)?;
        if output.len() != expected_length
            || decoder.into_inner().position() != asset.bytes.len() as u64
        {
            return Err(ReleaseSeedError::SourceAsset);
        }
        let text = String::from_utf8(output).map_err(|_| ReleaseSeedError::SourceAsset)?;
        validate_source_header(&text, descriptor)?;
        contents.push(Arc::<str>::from(text));
    }
    manifest
        .build_verified_catalog(&contents)
        .map_err(map_manifest_source_error)
}

fn validate_source_header(
    source: &str,
    descriptor: &ReleaseSeedAssetManifest,
) -> Result<(), ReleaseSeedError> {
    if source.as_bytes().contains(&0) {
        return Err(ReleaseSeedError::SourceAsset);
    }
    let mut lines = source.lines().take(32).peekable();
    let adblock = lines
        .next_if(|line| line.starts_with("[Adblock Plus ") && line.ends_with(']'))
        .is_some();
    let mut title = None;
    let mut version = None;
    let mut commit = None;
    let mut modified = None;
    for line in lines {
        title = title.or_else(|| line.strip_prefix("! Title: "));
        version = version.or_else(|| line.strip_prefix("! Version: "));
        commit = commit.or_else(|| line.strip_prefix("! Commit: "));
        modified = modified.or_else(|| line.strip_prefix("! Last modified: "));
    }
    let provenance = if adblock {
        version
            .zip(commit)
            .map(|(version, commit)| (version.to_owned(), Some(commit)))
    } else {
        modified
            .and_then(dated_version)
            .map(|version| (version, None))
    };
    let expected = (
        descriptor.upstream_version.as_str(),
        descriptor.upstream_commit.as_deref(),
    );
    if title != Some(descriptor.upstream_title.as_str())
        || provenance
            .as_ref()
            .map(|(version, commit)| (version.as_str(), *commit))
            != Some(expected)
    {
        return Err(ReleaseSeedError::Provenance);
    }
    Ok(())
}
fn validate_provenance_text(value: &str) -> Result<(), ReleaseSeedError> {
    if value.is_empty()
        || value.len() > MAX_PROVENANCE_FIELD_BYTES
        || value.chars().any(char::is_control)
    {
        return Err(ReleaseSeedError::Provenance);
    }
    Ok(())
}

/// An RFC 2822 date such as `Mon, 02 Nov 2026 12:27:09 +0000` as the UTC
/// `YYYYMMDDHHMM` version the official list updater assigns it.
fn dated_version(value: &str) -> Option<String> {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let date = value.split_once(", ").map_or(value, |(_, date)| date);
    let [day, month, year, time, zone] =
        <[&str; 5]>::try_from(date.split(' ').collect::<Vec<_>>()).ok()?;
    let number = |text: &str, digits: std::ops::RangeInclusive<usize>| {
        (digits.contains(&text.len()) && text.bytes().all(|byte| byte.is_ascii_digit()))
            .then(|| text.parse::<i64>().ok())
            .flatten()
    };
    let month = MONTHS.iter().position(|name| *name == month)? as i64 + 1;
    let (day, year) = (number(day, 1..=2)?, number(year, 4..=4)?);
    let mut clock = time.split(':');
    let hour = number(clock.next()?, 2..=2)?;
    let minute = number(clock.next()?, 2..=2)?;
    let second = clock
        .next()
        .map_or(Some(0), |second| number(second, 2..=2))?;
    let (sign, offset) = zone.split_at_checked(1)?;
    let sign = match sign {
        "+" => 1,
        "-" => -1,
        _ => return None,
    };
    let offset = number(offset, 4..=4)?;
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let month_days = match month {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    if clock.next().is_some()
        || !(1..=month_days).contains(&day)
        || hour > 23
        || minute > 59
        || second > 59
        || offset / 100 > 23
        || offset % 100 > 59
    {
        return None;
    }
    let minutes = days_from_civil(year, month, day) * 1440 + hour * 60 + minute
        - sign * (offset / 100 * 60 + offset % 100);
    let (year, month, day) = civil_from_days(minutes.div_euclid(1440));
    let minute_of_day = minutes.rem_euclid(1440);
    (1970..=9999).contains(&year).then(|| {
        format!(
            "{year:04}{month:02}{day:02}{:02}{:02}",
            minute_of_day / 60,
            minute_of_day % 60
        )
    })
}

fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let year_of_era = year.rem_euclid(400);
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let days = days + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    (year_of_era + era * 400 + i64::from(month <= 2), month, day)
}

fn decode_sha256(value: &str) -> Result<[u8; 32], ReleaseSeedError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(ReleaseSeedError::Digest);
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

fn map_manifest_source_error(error: ManifestError) -> ReleaseSeedError {
    match error {
        ManifestError::TargetMismatch | ManifestError::NotUtf8 => ReleaseSeedError::SourceAsset,
        ManifestError::Catalog => ReleaseSeedError::Catalog,
        _ => ReleaseSeedError::CatalogManifest,
    }
}

/// Stable release-seed rejection category.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum ReleaseSeedError {
    /// The canonical TUF-compatible catalog manifest is invalid.
    #[error("release seed catalog manifest is invalid")]
    CatalogManifest,
    /// The release-only compressed-asset envelope is malformed.
    #[error("release seed envelope is invalid")]
    Envelope,
    /// A lowercase SHA-256 field is malformed.
    #[error("release seed digest is invalid")]
    Digest,
    /// The envelope does not bind the exact catalog manifest.
    #[error("release seed catalog identity does not match")]
    CatalogIdentity,
    /// Catalog, envelope, and embedded target sets differ.
    #[error("release seed asset set does not match")]
    AssetSet,
    /// Exact compressed bytes differ from their release envelope.
    #[error("release seed compressed asset is invalid")]
    CompressedAsset,
    /// A source cannot be boundedly decoded or matched to its raw descriptor.
    #[error("release seed source asset is invalid")]
    SourceAsset,
    /// Structured upstream header provenance is invalid.
    #[error("release seed provenance is invalid")]
    Provenance,
    /// Verified sources cannot form a compiler catalog.
    #[error("release seed cannot form a compiler catalog")]
    Catalog,
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{Compression, GzBuilder};
    use std::io::Write;
    use zephium_blocker_update::{
        LicenseMetadata, ManifestSource, ManifestSourceFormat, CATALOG_MANIFEST_VERSION,
    };

    const LICENSE: &str = "CC-BY-SA-3.0";

    fn gzip(source: &[u8]) -> Vec<u8> {
        let mut encoder = GzBuilder::new()
            .mtime(0)
            .write(Vec::new(), Compression::best());
        encoder.write_all(source).unwrap();
        encoder.finish().unwrap()
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    fn fixture() -> (Vec<u8>, Vec<u8>, Vec<u8>) {
        let source = b"[Adblock Plus 2.0]\n! Version: 202607240001\n! Title: EasyList\n! Last modified: 24 Jul 2026 00:01 UTC\n! Expires: 4 days (update frequency)\n! Commit: 0123456789abcdef0123456789abcdef01234567\n||ads.invalid^\n";
        let compressed = gzip(source);
        let manifest = CatalogManifest {
            schema_version: CATALOG_MANIFEST_VERSION,
            revision: 202_607_240_001,
            created_unix: 1_784_851_260,
            expires_unix: 1_785_196_860,
            sources: vec![ManifestSource {
                id: "easylist".into(),
                format: ManifestSourceFormat::Standard,
                target: "easylist.txt".into(),
                length: source.len() as u64,
                sha256: hex(Sha256::digest(source).as_slice()),
                license: LicenseMetadata {
                    license_expression: LICENSE.into(),
                    attribution: "The EasyList authors (https://easylist.to/)".into(),
                    redistribution: "Unmodified upstream subscription".into(),
                    source_url: "https://easylist.to/easylist/easylist.txt".into(),
                },
            }],
        }
        .encode_canonical()
        .unwrap();
        let seed = ReleaseSeedManifest {
            schema_version: RELEASE_SEED_SCHEMA_VERSION,
            catalog_manifest_sha256: hex(Sha256::digest(&manifest).as_slice()),
            assets: vec![ReleaseSeedAssetManifest {
                target: "easylist.txt".into(),
                compression: ReleaseSeedCompression::Gzip,
                compressed_length: compressed.len() as u64,
                compressed_sha256: hex(Sha256::digest(&compressed).as_slice()),
                upstream_title: "EasyList".into(),
                upstream_version: "202607240001".into(),
                upstream_commit: Some("0123456789abcdef0123456789abcdef01234567".into()),
            }],
        }
        .encode_canonical()
        .unwrap();
        (manifest, seed, compressed)
    }

    #[test]
    fn exact_release_seed_builds_one_deferred_identity() {
        let (manifest, seed, compressed) = fixture();
        let manifest = Box::leak(manifest.into_boxed_slice());
        let seed = Box::leak(seed.into_boxed_slice());
        let compressed = Box::leak(compressed.into_boxed_slice());
        let release = ReleaseCatalogSeed::from_embedded_gzip(
            manifest,
            seed,
            vec![EmbeddedReleaseAsset::new("easylist.txt", compressed)],
            UpdateLimits::default(),
            LicensePolicy::new([LICENSE]).unwrap(),
        )
        .unwrap();
        assert_eq!(release.identity().revision, 202_607_240_001);
        assert_eq!(release.identity().source_count, 1);
    }

    #[test]
    fn dated_lists_carry_their_modification_minute_and_no_commit() {
        let adblock = ReleaseSeedAssetManifest {
            target: "easylist.txt".into(),
            compression: ReleaseSeedCompression::Gzip,
            compressed_length: 1,
            compressed_sha256: "0".repeat(64),
            upstream_title: "EasyList".into(),
            upstream_version: "202607240001".into(),
            upstream_commit: Some("0123456789abcdef0123456789abcdef01234567".into()),
        };
        let dated = ReleaseSeedAssetManifest {
            target: "ublock-quick-fixes.txt".into(),
            upstream_title: "uBlock\u{2080} filters \u{2013} Quick fixes".into(),
            upstream_version: "202611030030".into(),
            upstream_commit: None,
            ..adblock.clone()
        };
        let source = "! Title: uBlock\u{2080} filters \u{2013} Quick fixes\n! Last modified: Mon, 02 Nov 2026 23:30:00 -0100\n! Expires: 8 hours\nexample.invalid##.ad\n";
        assert_eq!(validate_source_header(source, &dated), Ok(()));
        let committed = ReleaseSeedAssetManifest {
            upstream_commit: adblock.upstream_commit.clone(),
            ..dated.clone()
        };
        assert_eq!(
            validate_source_header(source, &committed),
            Err(ReleaseSeedError::Provenance)
        );
        assert_eq!(
            validate_source_header(&source.replace("23:30", "23:31"), &dated),
            Err(ReleaseSeedError::Provenance)
        );
        let envelope = ReleaseSeedManifest {
            schema_version: RELEASE_SEED_SCHEMA_VERSION,
            catalog_manifest_sha256: "0".repeat(64),
            assets: vec![adblock, dated],
        };
        assert_eq!(envelope.validate(), Ok(()));
        assert_eq!(
            dated_version("01 Mar 2028 00:15 +0130").as_deref(),
            Some("202802292245")
        );
        assert_eq!(dated_version("Mon, 31 Nov 2026 12:27:09 +0000"), None);
    }

    #[test]
    fn compressed_corruption_and_target_drift_fail_before_worker_start() {
        let (manifest, seed, mut compressed) = fixture();
        compressed[0] ^= 1;
        let manifest = Box::leak(manifest.into_boxed_slice());
        let seed = Box::leak(seed.into_boxed_slice());
        let compressed = Box::leak(compressed.into_boxed_slice());
        assert!(matches!(
            ReleaseCatalogSeed::from_embedded_gzip(
                manifest,
                seed,
                vec![EmbeddedReleaseAsset::new("easylist.txt", compressed)],
                UpdateLimits::default(),
                LicensePolicy::new([LICENSE]).unwrap(),
            ),
            Err(ReleaseSeedError::CompressedAsset)
        ));
    }
}
