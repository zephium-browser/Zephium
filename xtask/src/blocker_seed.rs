use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use flate2::bufread::GzDecoder;
use flate2::{Compression, GzBuilder};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const ASSET_DIRECTORY: &str = "assets/blocker-seed/v1";
const CATALOG_FILE: &str = "catalog.json";
const SEED_FILE: &str = "release-seed.json";
const QUALITY_FILE: &str = "compile-report.json";
const NOTICE_FILE: &str = "NOTICE";
const METADATA_FILES: [&str; 4] = [CATALOG_FILE, NOTICE_FILE, QUALITY_FILE, SEED_FILE];
const SEED_SCHEMA_VERSION: u32 = 2;
const QUALITY_SCHEMA_VERSION: u32 = 4;
const REDISTRIBUTION: &str = "Unmodified upstream subscription; deterministic gzip packaging only";
const HOUR: u64 = 60 * 60;
const DAY: u64 = 24 * HOUR;
/// The seed only protects the first hours, until the app fetches current lists
/// on its own; a release may ship one whose oldest list is up to a month old,
/// so a hotfix never waits on a list refresh. The sources' own expiry still
/// drives the app.
const RELEASE_SEED_MAX_AGE_SECONDS: u64 = 30 * DAY;
const MAX_SOURCE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_SOURCE_RULES: usize = 500_000;
const MAX_LICENSE_BYTES: u64 = 64 * 1024;
const GZIP_HEADER: [u8; 10] = [0x1f, 0x8b, 0x08, 0x00, 0, 0, 0, 0, 0x02, 0xff];

#[derive(Debug)]
struct LicenseSpec {
    file: &'static str,
    sha256: &'static str,
    expression: &'static str,
    notice: &'static str,
    attribution: &'static str,
    /// The list header that names the upstream terms.
    terms_header: &'static str,
    terms_url: &'static str,
    legal_code: [&'static str; 2],
}

const CC_BY_SA: LicenseSpec = LicenseSpec {
    file: "LICENSE-CC-BY-SA-3.0.txt",
    sha256: "3f941b3b89cf7b8370ceb83cc76d2120d471b58735d8ca60238a751a48d7f72f",
    expression: "CC-BY-SA-3.0",
    notice: "Selected license: CC BY-SA 3.0 Unported (CC-BY-SA-3.0)\n",
    attribution: "The EasyList authors (https://easylist.to/)",
    terms_header: "Licence",
    terms_url: "https://easylist.to/pages/licence.html",
    legal_code: [
        "Attribution-ShareAlike 3.0 Unported",
        "CREATIVE COMMONS CORPORATION IS NOT A LAW FIRM",
    ],
};

const GPL_3: LicenseSpec = LicenseSpec {
    file: "LICENSE-GPL-3.0.txt",
    sha256: "589ed823e9a84c56feb95ac58e7cf384626b9cbf4fda2a907bc36e103de1bad2",
    expression: "GPL-3.0-only",
    notice: "License: GNU General Public License, version 3 (GPL-3.0-only)\n\
             These lists are conveyed in source form, unmodified, without additional\n\
             terms and without warranty.\n",
    attribution: "uBlock Origin's uAssets authors (https://github.com/uBlockOrigin/uAssets)",
    terms_header: "License",
    terms_url: "https://github.com/uBlockOrigin/uAssets/blob/master/LICENSE",
    legal_code: ["GNU GENERAL PUBLIC LICENSE", "Version 3, 29 June 2007"],
};

const LICENSES: [&LicenseSpec; 2] = [&CC_BY_SA, &GPL_3];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HeaderFormat {
    /// `[Adblock Plus]` first line, numeric `Version` and upstream `Commit`.
    Adblock,
    /// Versioned by `Last modified`, with no commit.
    Dated,
}

/// One bundled list, with the same header rules and bounds the app applies to
/// its official downloads.
#[derive(Debug)]
struct SourceSpec {
    id: &'static str,
    target: &'static str,
    title: &'static str,
    source_url: &'static str,
    /// uBlock Origin's assembled `.min` lists carry no homepage line.
    homepage: Option<&'static str>,
    license: &'static LicenseSpec,
    header: HeaderFormat,
    min_bytes: usize,
    min_rules: usize,
    follows_expires: bool,
    /// Whether the list's `Last modified` dates the seed. A list that changes
    /// only every few months would otherwise make every seed look stale.
    dates_seed: bool,
}

const SOURCES: [SourceSpec; 9] = [
    SourceSpec {
        id: "easylist",
        target: "easylist.txt",
        title: "EasyList",
        source_url: "https://easylist.to/easylist/easylist.txt",
        homepage: Some("https://easylist.to/"),
        license: &CC_BY_SA,
        header: HeaderFormat::Adblock,
        min_bytes: 256 * 1024,
        min_rules: 1000,
        follows_expires: false,
        dates_seed: true,
    },
    SourceSpec {
        id: "easyprivacy",
        target: "easyprivacy.txt",
        title: "EasyPrivacy",
        source_url: "https://easylist.to/easylist/easyprivacy.txt",
        homepage: Some("https://easylist.to/"),
        license: &CC_BY_SA,
        header: HeaderFormat::Adblock,
        min_bytes: 256 * 1024,
        min_rules: 1000,
        follows_expires: false,
        dates_seed: true,
    },
    SourceSpec {
        id: "ublock-filters",
        target: "ublock-filters.txt",
        title: "uBlock filters",
        source_url: "https://ublockorigin.github.io/uAssets/filters/filters.min.txt",
        homepage: None,
        license: &GPL_3,
        header: HeaderFormat::Dated,
        min_bytes: 128 * 1024,
        min_rules: 1000,
        follows_expires: false,
        dates_seed: true,
    },
    SourceSpec {
        id: "ublock-quick-fixes",
        target: "ublock-quick-fixes.txt",
        title: "uBlock\u{2080} filters \u{2013} Quick fixes",
        source_url: "https://ublockorigin.github.io/uAssets/filters/quick-fixes.txt",
        homepage: Some(UASSETS),
        license: &GPL_3,
        header: HeaderFormat::Dated,
        min_bytes: 8 * 1024,
        min_rules: 50,
        follows_expires: true,
        dates_seed: true,
    },
    SourceSpec {
        id: "ublock-link-shorteners",
        target: "ublock-link-shorteners.txt",
        title: "uBlock filters \u{2013} Link shorteners",
        source_url: "https://ublockorigin.github.io/uAssets/filters/ubo-link-shorteners.txt",
        homepage: Some(UASSETS),
        license: &GPL_3,
        header: HeaderFormat::Dated,
        min_bytes: 32 * 1024,
        min_rules: 500,
        follows_expires: false,
        dates_seed: true,
    },
    SourceSpec {
        id: "ublock-privacy",
        target: "ublock-privacy.txt",
        title: "uBlock filters \u{2013} Privacy",
        source_url: "https://ublockorigin.github.io/uAssets/filters/privacy.min.txt",
        homepage: None,
        license: &GPL_3,
        header: HeaderFormat::Dated,
        min_bytes: 32 * 1024,
        min_rules: 500,
        follows_expires: false,
        dates_seed: true,
    },
    SourceSpec {
        id: "ublock-resource-abuse",
        target: "ublock-resource-abuse.txt",
        title: "uBlock filters \u{2013} Resource abuse",
        source_url: "https://ublockorigin.github.io/uAssets/filters/resource-abuse.txt",
        homepage: Some(UASSETS),
        license: &GPL_3,
        header: HeaderFormat::Dated,
        min_bytes: 2 * 1024,
        min_rules: 20,
        follows_expires: false,
        dates_seed: false,
    },
    SourceSpec {
        id: "ublock-unbreak",
        target: "ublock-unbreak.txt",
        title: "uBlock filters \u{2013} Unbreak",
        source_url: "https://ublockorigin.github.io/uAssets/filters/unbreak.min.txt",
        homepage: None,
        license: &GPL_3,
        header: HeaderFormat::Dated,
        min_bytes: 32 * 1024,
        min_rules: 500,
        follows_expires: false,
        dates_seed: true,
    },
    SourceSpec {
        id: "ublock-badware",
        target: "ublock-badware.txt",
        title: "uBlock filters \u{2013} Badware risks",
        source_url: "https://ublockorigin.github.io/uAssets/filters/badware.min.txt",
        homepage: None,
        license: &GPL_3,
        header: HeaderFormat::Dated,
        min_bytes: 32 * 1024,
        min_rules: 1000,
        follows_expires: false,
        dates_seed: true,
    },
];

const UASSETS: &str = "https://github.com/uBlockOrigin/uAssets";

#[derive(Clone, Debug)]
struct SourceInput {
    spec: &'static SourceSpec,
    raw: Vec<u8>,
    header: SourceHeader,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SourceHeader {
    title: String,
    version: String,
    commit: Option<String>,
    /// The exact `Last modified` header of a list versioned by it.
    modified: Option<String>,
    modified_unix: u64,
    expiry_seconds: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct CatalogManifest {
    schema_version: u32,
    revision: u64,
    created_unix: u64,
    expires_unix: u64,
    sources: Vec<CatalogSource>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct CatalogSource {
    id: String,
    format: SourceFormat,
    target: String,
    length: u64,
    sha256: String,
    license: LicenseMetadata,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum SourceFormat {
    Standard,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct LicenseMetadata {
    license_expression: String,
    attribution: String,
    redistribution: String,
    source_url: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct ReleaseSeedManifest {
    schema_version: u32,
    catalog_manifest_sha256: String,
    assets: Vec<ReleaseSeedAsset>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct ReleaseSeedAsset {
    target: String,
    compression: CompressionFormat,
    compressed_length: u64,
    compressed_sha256: String,
    upstream_title: String,
    upstream_version: String,
    upstream_commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    upstream_modified: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum CompressionFormat {
    Gzip,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct QualityManifest {
    schema_version: u32,
    package_revision: u64,
    catalog_manifest_sha256: String,
    release_seed_manifest_sha256: String,
    licenses: Vec<LicenseRecord>,
    compilers: Vec<CompileGolden>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct LicenseRecord {
    file: String,
    sha256: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct CompileGolden {
    target: CompileTarget,
    feature_graph: String,
    policy_format_version: u32,
    webkit_artifact_format_version: u32,
    adblock_engine_version: String,
    limits: CompileBudgets,
    policy_sha256: String,
    native_artifact_sha256: Option<String>,
    total_source_bytes: usize,
    candidate_rules: usize,
    accepted_rules: usize,
    rejected_rules: usize,
    native_blocking_rule_entries: usize,
    attribution_sensitive_rules: usize,
    runtime_omitted_rules: usize,
    runtime_approximated_rules: usize,
    runtime_resource_approximated_rules: usize,
    runtime_source_kind_approximated_rules: usize,
    sources: Vec<SourceGolden>,
    runtime: Option<RuntimeGolden>,
    webkit: Option<WebKitGolden>,
    cosmetics: Option<CosmeticGolden>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct CosmeticGolden {
    accepted_rules: usize,
    rejected_rules: usize,
    generic_hide_controls: usize,
    scriptlet_rules: usize,
    unknown_scriptlet_rules: usize,
    unsupported_scriptlet_rules: usize,
    scriptlet_hosts: usize,
    scriptlet_calls: usize,
    scriptlet_program_bytes: usize,
    policy_sha256: String,
    policy_bytes: usize,
    native_artifact_sha256: Option<String>,
    native_json_bytes: Option<usize>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum CompileTarget {
    Runtime,
    Webkit,
}

impl CompileTarget {
    const fn feature(self) -> &'static str {
        match self {
            Self::Runtime => "blocker-seed-runtime",
            Self::Webkit => "blocker-seed-webkit",
        }
    }

    const fn argument(self) -> &'static str {
        match self {
            Self::Runtime => "runtime",
            Self::Webkit => "webkit",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct CompileBudgets {
    max_sources: usize,
    max_source_bytes: usize,
    max_total_source_bytes: usize,
    max_line_bytes: usize,
    max_rules: usize,
    max_physical_lines: usize,
    max_webkit_rules: usize,
    max_webkit_json_bytes: usize,
    max_request_url_bytes: usize,
    max_source_url_bytes: usize,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct SourceGolden {
    id: String,
    total_lines: usize,
    ignored_lines: usize,
    candidate_rules: usize,
    accepted_rules: usize,
    rejected_rules: usize,
    attribution_sensitive_rules: usize,
    runtime_omitted_rules: usize,
    runtime_approximated_rules: usize,
    runtime_resource_approximated_rules: usize,
    runtime_source_kind_approximated_rules: usize,
    dropped: BTreeMap<String, usize>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct RuntimeGolden {
    regexes: usize,
    omitted_regexes: usize,
    regex_limit: usize,
    pattern_bytes: usize,
    pattern_bytes_limit: usize,
    largest_regex_patterns: usize,
    patterns_per_regex_limit: usize,
    largest_regex_pattern_bytes: usize,
    pattern_bytes_per_regex_limit: usize,
    regex_size_limit_bytes: usize,
    regex_dfa_size_limit_bytes: usize,
    max_filter_checks_per_request: usize,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct WebKitGolden {
    accepted_input_rules: usize,
    converted_input_rules: usize,
    pruned_input_rules: usize,
    omitted_input_rules: usize,
    approximated_input_rules: usize,
    attribution_approximated_input_rules: usize,
    resource_approximated_input_rules: usize,
    blocking_rule_entries: usize,
    emitted_rules: usize,
    json_bytes: usize,
    dropped: BTreeMap<String, usize>,
}

pub(crate) fn check(repository: &Path) -> Result<(), String> {
    check_inner(repository, None)
}

pub(crate) fn materialize_webkit(repository: &Path, output: &Path) -> Result<(), String> {
    check_inner(repository, Some(output))
}

/// The bundled lists' WebKit artifact with every rule kept, as the pruned
/// rule list is measured against. Its compile report is not a golden.
pub(crate) fn materialize_webkit_unpruned(repository: &Path, output: &Path) -> Result<(), String> {
    let directory = repository.join(ASSET_DIRECTORY);
    validate_closed_file_set(&directory, &expected_files())?;
    let catalog_bytes = read_regular_file(&directory.join(CATALOG_FILE), 128 * 1024)?;
    let seed_bytes = read_regular_file(&directory.join(SEED_FILE), 128 * 1024)?;
    let catalog: CatalogManifest = parse_canonical_json(&catalog_bytes, CATALOG_FILE)?;
    let seed: ReleaseSeedManifest = parse_canonical_json(&seed_bytes, SEED_FILE)?;
    if seed.catalog_manifest_sha256 != hex_sha256(&catalog_bytes) {
        return Err("blocker seed manifest identities do not match exact bytes".into());
    }
    let raw_sources = verify_and_inflate_sources(&directory, &catalog, &seed)?;
    let temporary = tempfile::tempdir()
        .map_err(|error| format!("cannot create blocker seed directory: {error}"))?;
    let source_paths = write_compile_inputs(temporary.path(), &raw_sources)?;
    compile_one(
        repository,
        CompileTarget::Webkit,
        &source_paths,
        Some(output),
        true,
    )?;
    Ok(())
}

pub(crate) fn check_release_freshness(repository: &Path, now: u64) -> Result<(), String> {
    let directory = repository.join(ASSET_DIRECTORY);
    validate_closed_file_set(&directory, &expected_files())?;
    let catalog_bytes = read_regular_file(&directory.join(CATALOG_FILE), 128 * 1024)?;
    let seed_bytes = read_regular_file(&directory.join(SEED_FILE), 128 * 1024)?;
    let quality_bytes = read_regular_file(&directory.join(QUALITY_FILE), 256 * 1024)?;
    let catalog: CatalogManifest = parse_canonical_json(&catalog_bytes, CATALOG_FILE)?;
    let seed: ReleaseSeedManifest = parse_canonical_json(&seed_bytes, SEED_FILE)?;
    let quality: QualityManifest = parse_canonical_json(&quality_bytes, QUALITY_FILE)?;
    let catalog_digest = hex_sha256(&catalog_bytes);
    if seed.catalog_manifest_sha256 != catalog_digest
        || quality.catalog_manifest_sha256 != catalog_digest
        || quality.release_seed_manifest_sha256 != hex_sha256(&seed_bytes)
        || quality.package_revision != catalog.revision
    {
        return Err("blocker seed release manifests do not bind the exact catalog".into());
    }
    validate_release_freshness(&catalog, now)
}

fn validate_release_freshness(catalog: &CatalogManifest, now: u64) -> Result<(), String> {
    if catalog.schema_version != 1 || catalog.sources.len() != SOURCES.len() {
        return Err("blocker seed catalog schema or source count is invalid".into());
    }
    if catalog.created_unix > now {
        return Err("blocker seed was created in the future".into());
    }
    if catalog
        .created_unix
        .saturating_add(RELEASE_SEED_MAX_AGE_SECONDS)
        <= now
    {
        return Err(format!(
            "blocker seed was created at {} and is older than a release may ship; refresh it with `cargo xtask update-blocker-seed`",
            catalog.created_unix
        ));
    }
    Ok(())
}

fn check_inner(repository: &Path, webkit_output: Option<&Path>) -> Result<(), String> {
    let directory = repository.join(ASSET_DIRECTORY);
    validate_closed_file_set(&directory, &expected_files())?;

    for license in LICENSES {
        validate_license(
            license,
            &read_regular_file(&directory.join(license.file), MAX_LICENSE_BYTES)?,
        )?;
    }
    let catalog_bytes = read_regular_file(&directory.join(CATALOG_FILE), 128 * 1024)?;
    let seed_bytes = read_regular_file(&directory.join(SEED_FILE), 128 * 1024)?;
    let quality_bytes = read_regular_file(&directory.join(QUALITY_FILE), 256 * 1024)?;
    let catalog: CatalogManifest = parse_canonical_json(&catalog_bytes, CATALOG_FILE)?;
    let seed: ReleaseSeedManifest = parse_canonical_json(&seed_bytes, SEED_FILE)?;
    let quality: QualityManifest = parse_canonical_json(&quality_bytes, QUALITY_FILE)?;

    let catalog_digest = hex_sha256(&catalog_bytes);
    if seed.catalog_manifest_sha256 != catalog_digest
        || quality.catalog_manifest_sha256 != catalog_digest
        || quality.release_seed_manifest_sha256 != hex_sha256(&seed_bytes)
    {
        return Err("blocker seed manifest identities do not match exact bytes".into());
    }
    if quality.schema_version != QUALITY_SCHEMA_VERSION
        || quality.package_revision != catalog.revision
        || quality.licenses != license_records()
    {
        return Err("blocker seed quality manifest metadata is invalid".into());
    }

    let raw_sources = verify_and_inflate_sources(&directory, &catalog, &seed)?;
    validate_catalog(&catalog, &raw_sources)?;
    let notice = read_regular_file(&directory.join(NOTICE_FILE), 32 * 1024)?;
    let expected_notice = build_notice(&raw_sources);
    if notice != expected_notice.as_bytes() {
        return Err("blocker seed NOTICE is not the exact generated attribution record".into());
    }

    let temporary = tempfile::tempdir()
        .map_err(|error| format!("cannot create blocker seed verification directory: {error}"))?;
    let source_paths = write_compile_inputs(temporary.path(), &raw_sources)?;
    let staged_webkit = webkit_output.map(|_| temporary.path().join("webkit-content-rules.json"));
    let actual_compilers = compile_all(repository, &source_paths, staged_webkit.as_deref())?;
    if quality.compilers != actual_compilers {
        return Err(format!(
            "blocker seed compiler goldens drifted\nrecorded: {:#?}\nactual: {actual_compilers:#?}",
            quality.compilers
        ));
    }
    if let (Some(staged), Some(output)) = (staged_webkit.as_deref(), webkit_output) {
        publish_verified_webkit_artifact(staged, output, &actual_compilers)?;
    }
    eprintln!(
        "blocker seed {} verified offline: {} source bytes, Runtime + WebKit goldens exact",
        catalog.revision,
        raw_sources
            .iter()
            .map(|source| source.raw.len())
            .sum::<usize>()
    );
    Ok(())
}

/// Reads each list from `source_directory` under its bundled file name.
pub(crate) fn update(
    repository: &Path,
    source_directory: &Path,
    license_paths: [&Path; LICENSES.len()],
) -> Result<(), String> {
    let sources = SOURCES
        .iter()
        .map(|spec| {
            let raw = read_regular_file(&source_directory.join(spec.target), MAX_SOURCE_BYTES)?;
            let header = parse_source_header(&raw, spec)?;
            Ok(SourceInput { spec, raw, header })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let licenses = LICENSES
        .iter()
        .zip(license_paths)
        .map(|(license, path)| {
            let bytes = read_regular_file(path, MAX_LICENSE_BYTES)?;
            validate_license(license, &bytes)?;
            Ok(bytes)
        })
        .collect::<Result<Vec<_>, String>>()?;

    reject_future_sources(
        &sources,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| "the system clock is before 1970")?
            .as_secs(),
    )?;
    let previous = repository
        .join(ASSET_DIRECTORY)
        .join(CATALOG_FILE)
        .exists()
        .then(|| {
            let bytes = read_regular_file(
                &repository.join(ASSET_DIRECTORY).join(CATALOG_FILE),
                128 * 1024,
            )?;
            parse_canonical_json::<CatalogManifest>(&bytes, CATALOG_FILE)
        })
        .transpose()?;
    let catalog = build_catalog(&sources, next_revision(&sources, previous.as_ref())?)?;
    let catalog_bytes = canonical_json(&catalog)?;
    let compressed = sources
        .iter()
        .map(|source| deterministic_gzip(&source.raw))
        .collect::<Result<Vec<_>, _>>()?;
    let seed = build_release_seed(&sources, &compressed, &catalog_bytes);
    let seed_bytes = canonical_json(&seed)?;
    validate_generation_progression(
        &repository.join(ASSET_DIRECTORY),
        &catalog,
        &catalog_bytes,
        &seed,
    )?;

    let compile_directory = tempfile::tempdir()
        .map_err(|error| format!("cannot create blocker seed compile directory: {error}"))?;
    let source_paths = write_compile_inputs(compile_directory.path(), &sources)?;
    let compilers = compile_all(repository, &source_paths, None)?;
    let quality = QualityManifest {
        schema_version: QUALITY_SCHEMA_VERSION,
        package_revision: catalog.revision,
        catalog_manifest_sha256: hex_sha256(&catalog_bytes),
        release_seed_manifest_sha256: hex_sha256(&seed_bytes),
        licenses: license_records(),
        compilers,
    };

    let mut files = BTreeMap::<String, Vec<u8>>::new();
    files.insert(CATALOG_FILE.into(), catalog_bytes);
    files.insert(SEED_FILE.into(), seed_bytes);
    files.insert(QUALITY_FILE.into(), canonical_json(&quality)?);
    files.insert(NOTICE_FILE.into(), build_notice(&sources).into_bytes());
    for (source, bytes) in sources.iter().zip(compressed) {
        files.insert(asset_file(source.spec.target), bytes);
    }
    for (license, bytes) in LICENSES.iter().zip(licenses) {
        files.insert(license.file.into(), bytes);
    }
    let publication = replace_asset_directory(repository, &files)?;
    if let Err(error) = check(repository) {
        publication.rollback()?;
        return Err(format!(
            "generated blocker seed failed post-publication verification and was rolled back: {error}"
        ));
    }
    publication.commit()
}

fn license_records() -> Vec<LicenseRecord> {
    LICENSES
        .iter()
        .map(|license| LicenseRecord {
            file: license.file.into(),
            sha256: license.sha256.into(),
        })
        .collect()
}

fn asset_file(target: &str) -> String {
    format!("{target}.gz")
}

/// The closed file set of a seed holding these `(target, license)` sources.
fn file_set<'a>(sources: impl IntoIterator<Item = (&'a str, &'a LicenseSpec)>) -> BTreeSet<String> {
    let mut files = METADATA_FILES
        .iter()
        .map(|name| (*name).to_owned())
        .collect::<BTreeSet<_>>();
    for (target, license) in sources {
        files.insert(asset_file(target));
        files.insert(license.file.into());
    }
    files
}

fn expected_files() -> BTreeSet<String> {
    file_set(SOURCES.iter().map(|spec| (spec.target, spec.license)))
}

fn validate_generation_progression(
    directory: &Path,
    catalog: &CatalogManifest,
    catalog_bytes: &[u8],
    seed: &ReleaseSeedManifest,
) -> Result<(), String> {
    if !directory.exists() {
        return Ok(());
    }
    let previous_catalog_bytes = read_regular_file(&directory.join(CATALOG_FILE), 128 * 1024)?;
    let previous_seed_bytes = read_regular_file(&directory.join(SEED_FILE), 128 * 1024)?;
    let previous_catalog: CatalogManifest =
        parse_canonical_json(&previous_catalog_bytes, CATALOG_FILE)?;
    let previous_seed: ReleaseSeedManifest = parse_canonical_json(&previous_seed_bytes, SEED_FILE)?;
    let previous_files = previous_catalog
        .sources
        .iter()
        .map(|source| {
            LICENSES
                .into_iter()
                .find(|license| license.expression == source.license.license_expression)
                .map(|license| (source.target.as_str(), license))
                .ok_or_else(|| "existing blocker seed names an unknown license".to_owned())
        })
        .collect::<Result<Vec<_>, _>>()?;
    validate_closed_file_set(directory, &file_set(previous_files))?;
    // A seed may gain lists, which have no baseline; those it already held keep
    // their order and must not roll back.
    if previous_seed.catalog_manifest_sha256 != hex_sha256(&previous_catalog_bytes)
        || previous_seed.assets.len() != previous_catalog.sources.len()
        || previous_catalog.sources.len() > catalog.sources.len()
        || seed.assets.len() != catalog.sources.len()
    {
        return Err("existing blocker seed cannot be trusted as an update baseline".into());
    }
    if catalog.revision < previous_catalog.revision {
        return Err(format!(
            "blocker seed revision {} would roll back existing revision {}",
            catalog.revision, previous_catalog.revision
        ));
    }
    if catalog.revision == previous_catalog.revision && catalog_bytes != previous_catalog_bytes {
        return Err(format!(
            "blocker seed revision {} is being assigned different catalog bytes",
            catalog.revision
        ));
    }
    for (((previous_source, previous_asset), source), asset) in previous_catalog
        .sources
        .iter()
        .zip(&previous_seed.assets)
        .zip(&catalog.sources)
        .zip(&seed.assets)
    {
        if previous_source.id != source.id
            || previous_source.target != source.target
            || previous_asset.target != asset.target
        {
            return Err("blocker seed source identity changed across revisions".into());
        }
        let previous_version = previous_asset
            .upstream_version
            .parse::<u64>()
            .map_err(|_| "existing blocker seed has an invalid source version")?;
        let version = asset
            .upstream_version
            .parse::<u64>()
            .map_err(|_| "candidate blocker seed has an invalid source version")?;
        if version < previous_version {
            return Err(format!(
                "{} version {} would roll back existing version {}",
                asset.upstream_title, version, previous_version
            ));
        }
        // A list versioned by its modification date can change twice in a
        // minute, but only to a copy modified later within it; one with a
        // published version cannot change under it.
        let dated = SOURCES
            .iter()
            .any(|spec| spec.id == source.id && spec.header == HeaderFormat::Dated);
        let seconds =
            |asset: &ReleaseSeedAsset| asset.upstream_modified.as_deref().and_then(dated_seconds);
        let later = matches!(
            (seconds(asset), seconds(previous_asset)),
            (Some(new), Some(old)) if new > old
        );
        if version == previous_version
            && (source != previous_source
                || asset.upstream_commit != previous_asset.upstream_commit)
            && !(dated && later)
        {
            return Err(format!(
                "{} version {} is being assigned different source bytes or provenance{}",
                asset.upstream_title,
                version,
                if dated {
                    " without a later Last modified"
                } else {
                    ""
                }
            ));
        }
    }
    Ok(())
}

/// A source dated ahead of the clock (beyond five minutes of skew) is a bad
/// header or a bad clock. A dated list is judged by its exact time.
fn reject_future_sources(sources: &[SourceInput], now: u64) -> Result<(), String> {
    let modified = |header: &SourceHeader| {
        header
            .modified
            .as_deref()
            .and_then(dated_seconds)
            .and_then(|seconds| u64::try_from(seconds).ok())
            .unwrap_or(header.modified_unix)
    };
    match sources
        .iter()
        .find(|source| modified(&source.header) > now.saturating_add(300))
    {
        Some(source) => Err(format!("{} is dated in the future", source.header.title)),
        None => Ok(()),
    }
}

/// The newest source version, the least revision a catalog of them may carry.
fn newest_version(sources: &[SourceInput]) -> Result<u64, String> {
    sources
        .iter()
        .map(|source| {
            source
                .header
                .version
                .parse::<u64>()
                .map_err(|_| format!("{} has an invalid numeric version", source.header.title))
        })
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .max()
        .ok_or_else(|| "blocker seed has no sources".to_owned())
}

/// A new catalog's revision: unchanged sources keep the previous revision;
/// any change advances it past both the previous revision and the newest
/// source, in the same `YYYYMMDDHHMM` form, so a list with a lower version
/// changing still moves the revision.
fn next_revision(
    sources: &[SourceInput],
    previous: Option<&CatalogManifest>,
) -> Result<u64, String> {
    let newest = newest_version(sources)?;
    let Some(previous) = previous else {
        return Ok(newest);
    };
    let unchanged = previous.sources.len() == sources.len()
        && previous.sources.iter().zip(sources).all(|(old, new)| {
            old.id == new.spec.id
                && old.sha256 == hex_sha256(&new.raw)
                && old.length == new.raw.len() as u64
        });
    if unchanged {
        return Ok(previous.revision.max(newest));
    }
    let (unix, _) = timestamp_from_version(&previous.revision.to_string())?;
    let minutes = i64::try_from(unix / 60 + 1).map_err(|_| "revision overflows")?;
    let (year, month, day) = civil_from_days(minutes.div_euclid(1440));
    let minute = minutes.rem_euclid(1440);
    let after: u64 = format!(
        "{year:04}{month:02}{day:02}{:02}{:02}",
        minute / 60,
        minute % 60
    )
    .parse()
    .map_err(|_| "revision overflows")?;
    Ok(after.max(newest))
}

fn build_catalog(sources: &[SourceInput], revision: u64) -> Result<CatalogManifest, String> {
    if revision < newest_version(sources)? {
        return Err("blocker seed revision is older than its newest source".into());
    }
    // The oldest list dates the seed, so the release gate measures the
    // stalest list rather than the most recently refreshed one.
    let dating = || sources.iter().filter(|source| source.spec.dates_seed);
    let created_unix = dating()
        .map(|source| source.header.modified_unix)
        .min()
        .ok_or_else(|| "blocker seed has no creation time".to_owned())?;
    let expires_unix = dating()
        .map(|source| source.header.modified_unix + source.header.expiry_seconds)
        .min()
        .ok_or_else(|| "blocker seed has no expiry time".to_owned())?;
    if created_unix >= expires_unix {
        return Err("blocker seed source freshness windows do not overlap".into());
    }

    Ok(CatalogManifest {
        schema_version: 1,
        revision,
        created_unix,
        expires_unix,
        sources: sources
            .iter()
            .map(|source| CatalogSource {
                id: source.spec.id.into(),
                format: SourceFormat::Standard,
                target: source.spec.target.into(),
                length: source.raw.len() as u64,
                sha256: hex_sha256(&source.raw),
                license: license_metadata(source.spec),
            })
            .collect(),
    })
}

fn build_release_seed(
    sources: &[SourceInput],
    compressed: &[Vec<u8>],
    catalog_bytes: &[u8],
) -> ReleaseSeedManifest {
    ReleaseSeedManifest {
        schema_version: SEED_SCHEMA_VERSION,
        catalog_manifest_sha256: hex_sha256(catalog_bytes),
        assets: sources
            .iter()
            .zip(compressed)
            .map(|(source, bytes)| ReleaseSeedAsset {
                target: source.spec.target.into(),
                compression: CompressionFormat::Gzip,
                compressed_length: bytes.len() as u64,
                compressed_sha256: hex_sha256(bytes),
                upstream_title: source.header.title.clone(),
                upstream_version: source.header.version.clone(),
                upstream_commit: source.header.commit.clone(),
                upstream_modified: source.header.modified.clone(),
            })
            .collect(),
    }
}

fn validate_catalog(catalog: &CatalogManifest, sources: &[SourceInput]) -> Result<(), String> {
    if catalog != &build_catalog(sources, catalog.revision)? {
        return Err("blocker seed catalog differs from exact source metadata".into());
    }
    Ok(())
}

fn verify_and_inflate_sources(
    directory: &Path,
    catalog: &CatalogManifest,
    seed: &ReleaseSeedManifest,
) -> Result<Vec<SourceInput>, String> {
    if catalog.schema_version != 1
        || seed.schema_version != SEED_SCHEMA_VERSION
        || catalog.sources.len() != SOURCES.len()
        || seed.assets.len() != SOURCES.len()
    {
        return Err("blocker seed manifest schema or source count is invalid".into());
    }
    let mut sources = Vec::with_capacity(SOURCES.len());
    for ((catalog_source, asset), spec) in catalog.sources.iter().zip(&seed.assets).zip(&SOURCES) {
        let title = spec.title;
        validate_catalog_source(catalog_source, spec)?;
        if asset.target != spec.target
            || asset.compression != CompressionFormat::Gzip
            || asset.upstream_title != title
        {
            return Err(format!("{title} release asset metadata is invalid"));
        }
        let compressed =
            read_regular_file(&directory.join(asset_file(spec.target)), 16 * 1024 * 1024)?;
        if compressed.len() as u64 != asset.compressed_length
            || hex_sha256(&compressed) != asset.compressed_sha256
        {
            return Err(format!("{title} compressed asset identity is invalid"));
        }
        let raw = bounded_inflate(&compressed, catalog_source.length, title)?;
        if hex_sha256(&raw) != catalog_source.sha256 {
            return Err(format!("{title} raw source identity is invalid"));
        }
        if deterministic_gzip(&raw)? != compressed {
            return Err(format!(
                "{title} is not the exact deterministic gzip representation"
            ));
        }
        let header = parse_source_header(&raw, spec)?;
        if header.version != asset.upstream_version
            || header.commit != asset.upstream_commit
            || header.modified != asset.upstream_modified
        {
            return Err(format!(
                "{title} header provenance does not match its envelope"
            ));
        }
        sources.push(SourceInput { spec, raw, header });
    }
    Ok(sources)
}

fn license_metadata(spec: &SourceSpec) -> LicenseMetadata {
    LicenseMetadata {
        license_expression: spec.license.expression.into(),
        attribution: spec.license.attribution.into(),
        redistribution: REDISTRIBUTION.into(),
        source_url: spec.source_url.into(),
    }
}

fn validate_catalog_source(source: &CatalogSource, spec: &SourceSpec) -> Result<(), String> {
    if source.id != spec.id
        || source.target != spec.target
        || source.format != SourceFormat::Standard
        || source.length == 0
        || source.length > MAX_SOURCE_BYTES
        || !valid_sha256(&source.sha256)
        || source.license != license_metadata(spec)
    {
        return Err(format!("{} catalog descriptor is invalid", spec.id));
    }
    Ok(())
}

fn parse_source_header(raw: &[u8], spec: &SourceSpec) -> Result<SourceHeader, String> {
    let expected_title = spec.title;
    if raw.is_empty() || raw.last() != Some(&b'\n') || raw.contains(&0) || raw.contains(&b'\r') {
        return Err(format!(
            "{expected_title} must be nonempty NUL-free UTF-8 with canonical LF endings"
        ));
    }
    let text = std::str::from_utf8(raw)
        .map_err(|_| format!("{expected_title} source is not valid UTF-8"))?;
    let rules = text
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('!') && !line.starts_with('['))
        .count();
    if text.len() < spec.min_bytes || !(spec.min_rules..=MAX_SOURCE_RULES).contains(&rules) {
        return Err(format!(
            "{expected_title} has {} bytes and {rules} rules, outside its accepted bounds",
            text.len()
        ));
    }
    let mut lines = text.lines().take(32).peekable();
    let adblock = lines
        .next_if(|line| matches!(*line, "[Adblock Plus 1.1]" | "[Adblock Plus 2.0]"))
        .is_some();
    if adblock != (spec.header == HeaderFormat::Adblock) {
        return Err(format!("{expected_title} has an unexpected ABP header"));
    }
    let mut fields = BTreeMap::new();
    for line in lines {
        for name in [
            "Version",
            "Title",
            "Last modified",
            "Expires",
            "Commit",
            "Homepage",
            spec.license.terms_header,
        ] {
            if let Some(value) = line.strip_prefix(&format!("! {name}: ")) {
                if fields.insert(name, value).is_some() {
                    return Err(format!("{expected_title} duplicates its {name} header"));
                }
            }
        }
    }
    let field = |name| {
        fields
            .get(name)
            .copied()
            .ok_or_else(|| format!("{expected_title} has no {name} header"))
    };
    if field("Title")? != expected_title
        || fields.get("Homepage").copied() != spec.homepage
        || field(spec.license.terms_header)? != spec.license.terms_url
    {
        return Err(format!("{expected_title} header provenance is invalid"));
    }
    let (version, commit, modified, expiry_seconds) = match spec.header {
        HeaderFormat::Adblock => {
            let version = field("Version")?;
            let commit = field("Commit")?;
            if version.len() != 12
                || !version.bytes().all(|byte| byte.is_ascii_digit())
                || commit.len() != 40
                || !commit
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                || field("Expires")? != "4 days (update frequency)"
            {
                return Err(format!("{expected_title} header provenance is invalid"));
            }
            (version.to_owned(), Some(commit.to_owned()), None, 4 * DAY)
        }
        HeaderFormat::Dated => {
            let modified = field("Last modified")?;
            let version = dated_version(modified)
                .ok_or_else(|| format!("{expected_title} has an invalid Last modified header"))?;
            let expires = expires_seconds(field("Expires")?)
                .ok_or_else(|| format!("{expected_title} has an invalid Expires header"))?;
            let ceiling = if spec.follows_expires { DAY } else { 7 * DAY };
            (
                version,
                None,
                Some(modified.to_owned()),
                expires.clamp(4 * HOUR, ceiling),
            )
        }
    };
    let (modified_unix, formatted) = timestamp_from_version(&version)?;
    if spec.header == HeaderFormat::Adblock && field("Last modified")? != formatted {
        return Err(format!(
            "{expected_title} version and Last modified headers disagree"
        ));
    }
    Ok(SourceHeader {
        title: expected_title.into(),
        version,
        commit,
        modified,
        modified_unix,
        expiry_seconds,
    })
}

/// `8 hours`, `5 days` or `4 days (update frequency)` in seconds.
fn expires_seconds(value: &str) -> Option<u64> {
    let mut words = value.split_whitespace();
    let count: u64 = words.next()?.parse().ok()?;
    let unit = match words.next()?.trim_end_matches('s') {
        "hour" => HOUR,
        "day" => DAY,
        _ => return None,
    };
    count.checked_mul(unit)
}

/// An RFC 2822 date such as `Mon, 02 Nov 2026 12:27:09 +0000` as the UTC
/// `YYYYMMDDHHMM` version the app's list updater assigns it.
fn dated_version(value: &str) -> Option<String> {
    let minutes = dated_seconds(value)?.div_euclid(60);
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

/// The same date in seconds since the Unix epoch.
fn dated_seconds(value: &str) -> Option<i64> {
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
    if clock.next().is_some()
        || day == 0
        || day > i64::from(days_in_month(year as u32, month as u32))
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
    Some(minutes * 60 + second)
}

fn timestamp_from_version(version: &str) -> Result<(u64, String), String> {
    let number = |range: std::ops::Range<usize>| {
        version[range]
            .parse::<u32>()
            .map_err(|_| "source version has an invalid timestamp".to_owned())
    };
    let year = number(0..4)?;
    let month = number(4..6)?;
    let day = number(6..8)?;
    let hour = number(8..10)?;
    let minute = number(10..12)?;
    if !(2020..=2100).contains(&year)
        || !(1..=12).contains(&month)
        || day == 0
        || day > days_in_month(year, month)
        || hour > 23
        || minute > 59
    {
        return Err("source version timestamp is outside accepted bounds".into());
    }
    let days = days_from_civil(year as i64, month as i64, day as i64);
    if days < 0 {
        return Err("source version timestamp predates the Unix epoch".into());
    }
    let unix = (days as u64)
        .checked_mul(86_400)
        .and_then(|seconds| seconds.checked_add(u64::from(hour) * 3_600))
        .and_then(|seconds| seconds.checked_add(u64::from(minute) * 60))
        .ok_or_else(|| "source version timestamp overflows".to_owned())?;
    let months = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let formatted = format!(
        "{day:02} {} {year:04} {hour:02}:{minute:02} UTC",
        months[(month - 1) as usize]
    );
    Ok((unix, formatted))
}

const fn days_in_month(year: u32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400)) => {
            29
        }
        2 => 28,
        _ => 0,
    }
}

const fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let days = days + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let adjusted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * adjusted_month + 2) / 5 + 1;
    let month = if adjusted_month < 10 {
        adjusted_month + 3
    } else {
        adjusted_month - 9
    };
    let year = year_of_era + era * 400 + if month <= 2 { 1 } else { 0 };
    (year, month, day)
}

const fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let adjusted_year = year - if month <= 2 { 1 } else { 0 };
    let era = if adjusted_year >= 0 {
        adjusted_year
    } else {
        adjusted_year - 399
    } / 400;
    let year_of_era = adjusted_year - era * 400;
    let adjusted_month = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * adjusted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn deterministic_gzip(raw: &[u8]) -> Result<Vec<u8>, String> {
    let mut encoder = GzBuilder::new()
        .mtime(0)
        .operating_system(255)
        .write(Vec::new(), Compression::best());
    encoder
        .write_all(raw)
        .map_err(|error| format!("cannot compress blocker seed source: {error}"))?;
    let compressed = encoder
        .finish()
        .map_err(|error| format!("cannot finish blocker seed compression: {error}"))?;
    if compressed.get(..GZIP_HEADER.len()) != Some(GZIP_HEADER.as_slice()) {
        return Err("gzip encoder did not produce Zephium's canonical header".into());
    }
    Ok(compressed)
}

fn bounded_inflate(
    compressed: &[u8],
    expected_length: u64,
    title: &str,
) -> Result<Vec<u8>, String> {
    if compressed.get(..GZIP_HEADER.len()) != Some(GZIP_HEADER.as_slice())
        || expected_length == 0
        || expected_length > MAX_SOURCE_BYTES
    {
        return Err(format!("{title} gzip header or declared length is invalid"));
    }
    let expected_length =
        usize::try_from(expected_length).map_err(|_| format!("{title} source is too large"))?;
    let mut decoder = GzDecoder::new(Cursor::new(compressed));
    let mut raw = Vec::with_capacity(expected_length);
    decoder
        .by_ref()
        .take(expected_length as u64 + 1)
        .read_to_end(&mut raw)
        .map_err(|error| format!("cannot inflate {title}: {error}"))?;
    let consumed = decoder.into_inner().position();
    if raw.len() != expected_length || consumed != compressed.len() as u64 {
        return Err(format!(
            "{title} gzip is truncated, oversized, concatenated, or has trailing bytes"
        ));
    }
    Ok(raw)
}

fn validate_license(license: &LicenseSpec, bytes: &[u8]) -> Result<(), String> {
    let file = license.file;
    if hex_sha256(bytes) != license.sha256 {
        return Err(format!(
            "{file} differs from the reviewed {} legal code",
            license.expression
        ));
    }
    let text = std::str::from_utf8(bytes).map_err(|_| format!("{file} is not valid UTF-8"))?;
    if !license
        .legal_code
        .iter()
        .all(|marker| text.contains(marker))
    {
        return Err(format!("{file} does not contain the expected legal code"));
    }
    Ok(())
}

fn build_notice(sources: &[SourceInput]) -> String {
    let mut notice = String::from(
        "Zephium bundled blocker seed v1\n\n\
         This directory contains unmodified filter list subscription text. Zephium\n\
         changes only the storage representation by applying deterministic gzip\n\
         compression; the decompressed source bytes are unchanged.\n\n",
    );
    for license in LICENSES {
        let titles = sources
            .iter()
            .filter(|source| source.spec.license.expression == license.expression)
            .map(|source| source.spec.title)
            .collect::<Vec<_>>();
        notice.push_str(&format!(
            "{}\nAttribution: {}\n{}License text: {}\nUpstream terms: {}\n\n",
            titles.join(", "),
            license.attribution,
            license.notice,
            license.file,
            license.terms_url
        ));
    }
    for source in sources {
        notice.push_str(&format!(
            "{}\nSource: {}\nVersion: {}\n",
            source.header.title, source.spec.source_url, source.header.version
        ));
        if let Some(commit) = &source.header.commit {
            notice.push_str(&format!("Commit: {commit}\n"));
        }
        notice.push_str(&format!("Raw SHA-256: {}\n\n", hex_sha256(&source.raw)));
    }
    notice.push_str(
        "The upstream authors do not endorse Zephium. Release engineering must regenerate\n\
         and verify this record with `cargo xtask update-blocker-seed` whenever a\n\
         subscription changes.\n",
    );
    notice
}

fn validate_closed_file_set(directory: &Path, expected: &BTreeSet<String>) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(directory)
        .map_err(|error| format!("cannot inspect {}: {error}", directory.display()))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(format!("{} is not a real directory", directory.display()));
    }
    let mut actual = BTreeSet::new();
    for entry in std::fs::read_dir(directory)
        .map_err(|error| format!("cannot enumerate {}: {error}", directory.display()))?
    {
        let entry = entry.map_err(|error| {
            format!(
                "cannot enumerate an entry in {}: {error}",
                directory.display()
            )
        })?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| "blocker seed contains a non-UTF-8 filename".to_owned())?;
        let metadata = std::fs::symlink_metadata(entry.path())
            .map_err(|error| format!("cannot inspect blocker seed file {name}: {error}"))?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(format!("blocker seed entry {name} is not a regular file"));
        }
        actual.insert(name);
    }
    if &actual != expected {
        return Err(format!(
            "blocker seed file set is not closed; expected {expected:?}, found {actual:?}"
        ));
    }
    Ok(())
}

fn read_regular_file(path: &Path, maximum: u64) -> Result<Vec<u8>, String> {
    let path_metadata = std::fs::symlink_metadata(path)
        .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?;
    if path_metadata.file_type().is_symlink() || !path_metadata.is_file() {
        return Err(format!(
            "{} is not a regular non-symlink file",
            path.display()
        ));
    }
    let mut file =
        File::open(path).map_err(|error| format!("cannot open {}: {error}", path.display()))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("cannot inspect open {}: {error}", path.display()))?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > maximum {
        return Err(format!("{} has an invalid byte length", path.display()));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    Read::by_ref(&mut file)
        .take(maximum + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    if bytes.len() as u64 != metadata.len() || bytes.len() as u64 > maximum {
        return Err(format!("{} changed while being read", path.display()));
    }
    Ok(bytes)
}

fn publish_verified_webkit_artifact(
    staged: &Path,
    output: &Path,
    compilers: &[CompileGolden],
) -> Result<(), String> {
    let mut webkit_compilers = compilers
        .iter()
        .filter(|compiler| compiler.target == CompileTarget::Webkit);
    let compiler = webkit_compilers
        .next()
        .ok_or_else(|| "verified blocker seed has no WebKit compiler report".to_owned())?;
    if webkit_compilers.next().is_some() {
        return Err("verified blocker seed has duplicate WebKit compiler reports".into());
    }
    let coverage = compiler
        .webkit
        .as_ref()
        .ok_or_else(|| "verified blocker seed has no WebKit coverage report".to_owned())?;
    let expected_digest = compiler
        .native_artifact_sha256
        .as_deref()
        .filter(|digest| valid_sha256(digest))
        .ok_or_else(|| "verified blocker seed has no valid WebKit artifact identity".to_owned())?;
    let bytes = read_regular_file(
        staged,
        zephium_core::blocker::MAX_DECLARATIVE_RULE_BYTES as u64,
    )?;
    if bytes.len() != coverage.json_bytes
        || bytes.first() != Some(&b'[')
        || bytes.last() != Some(&b']')
        || std::str::from_utf8(&bytes).is_err()
    {
        return Err("materialized WebKit blocker artifact is structurally invalid".into());
    }
    let mut digest = Sha256::new();
    digest.update(b"zephium-webkit-content-rules");
    digest.update(compiler.webkit_artifact_format_version.to_be_bytes());
    digest.update(&bytes);
    if lower_hex(&digest.finalize()) != expected_digest {
        return Err("materialized WebKit blocker artifact identity is invalid".into());
    }
    write_new_file(output, &bytes, "verified WebKit blocker artifact")
}

fn write_new_file(path: &Path, bytes: &[u8], description: &str) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| format!("cannot create {description} {}: {error}", path.display()))?;
    let result = file
        .write_all(bytes)
        .and_then(|()| file.sync_all())
        .and_then(|()| file.metadata())
        .and_then(|metadata| {
            if metadata.is_file() && metadata.len() == bytes.len() as u64 {
                Ok(())
            } else {
                Err(std::io::Error::other(
                    "open output is not the expected regular file",
                ))
            }
        });
    if let Err(error) = result {
        drop(file);
        return Err(format!(
            "cannot write {description} {}: {error}; the create-new output was left in place for inspection",
            path.display()
        ));
    }
    Ok(())
}

fn replace_asset_directory(
    repository: &Path,
    files: &BTreeMap<String, Vec<u8>>,
) -> Result<PublishedDirectory, String> {
    if files.keys().cloned().collect::<BTreeSet<_>>() != expected_files() {
        return Err("generator attempted to publish a noncanonical blocker seed file set".into());
    }
    let parent = repository.join("assets/blocker-seed");
    std::fs::create_dir_all(&parent)
        .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    let lock_path = parent.join(".update.lock");
    let lock = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&lock_path)
        .map_err(|error| {
            format!(
                "cannot acquire blocker seed generation lock {}: {error}",
                lock_path.display()
            )
        })?;
    let guard = GenerationLock {
        file: lock,
        path: lock_path,
    };
    let staging = tempfile::Builder::new()
        .prefix(".v1.stage.")
        .tempdir_in(&parent)
        .map_err(|error| format!("cannot create blocker seed staging directory: {error}"))?;
    for (name, bytes) in files {
        let path = staging.path().join(name);
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|error| format!("cannot create {}: {error}", path.display()))?;
        file.write_all(bytes)
            .and_then(|()| file.sync_all())
            .map_err(|error| format!("cannot durably write {}: {error}", path.display()))?;
    }
    sync_directory(staging.path())?;
    let destination = parent.join("v1");
    let backup = parent.join(format!(".v1.backup.{}", std::process::id()));
    if backup.exists() {
        return Err(format!(
            "stale blocker seed backup exists at {}; inspect it before retrying",
            backup.display()
        ));
    }
    let had_destination = destination.exists();
    if had_destination {
        let metadata = std::fs::symlink_metadata(&destination)
            .map_err(|error| format!("cannot inspect {}: {error}", destination.display()))?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(format!("{} is not a real directory", destination.display()));
        }
    }
    let staging = staging.keep();
    if had_destination {
        std::fs::rename(&destination, &backup).map_err(|error| {
            format!(
                "cannot stage existing blocker seed {}: {error}",
                destination.display()
            )
        })?;
    }
    if let Err(error) = std::fs::rename(&staging, &destination) {
        if had_destination {
            let _ = std::fs::rename(&backup, &destination);
        }
        return Err(format!(
            "cannot publish blocker seed {}: {error}; staged files remain at {}",
            destination.display(),
            staging.display()
        ));
    }
    sync_directory(&parent)?;
    Ok(PublishedDirectory {
        destination,
        backup: had_destination.then_some(backup),
        parent,
        finished: false,
        _lock: guard,
    })
}

struct GenerationLock {
    file: File,
    path: PathBuf,
}

impl Drop for GenerationLock {
    fn drop(&mut self) {
        let _ = self.file.sync_all();
        let _ = std::fs::remove_file(&self.path);
    }
}

struct PublishedDirectory {
    destination: PathBuf,
    backup: Option<PathBuf>,
    parent: PathBuf,
    finished: bool,
    _lock: GenerationLock,
}

impl PublishedDirectory {
    fn commit(mut self) -> Result<(), String> {
        self.finished = true;
        if let Some(backup) = &self.backup {
            std::fs::remove_dir_all(backup).map_err(|error| {
                format!(
                    "verified blocker seed is published, but backup {} could not be removed: {error}",
                    backup.display()
                )
            })?;
            sync_directory(&self.parent)?;
        }
        Ok(())
    }

    fn rollback(mut self) -> Result<(), String> {
        self.rollback_inner()?;
        self.finished = true;
        Ok(())
    }

    fn rollback_inner(&mut self) -> Result<(), String> {
        if self.destination.exists() {
            std::fs::remove_dir_all(&self.destination).map_err(|error| {
                format!(
                    "cannot remove failed blocker seed {} during rollback: {error}",
                    self.destination.display()
                )
            })?;
        }
        if let Some(backup) = &self.backup {
            std::fs::rename(backup, &self.destination).map_err(|error| {
                format!(
                    "cannot restore blocker seed backup {}: {error}",
                    backup.display()
                )
            })?;
        }
        sync_directory(&self.parent)
    }
}

impl Drop for PublishedDirectory {
    fn drop(&mut self) {
        if !self.finished {
            let _ = self.rollback_inner();
        }
    }
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<(), String> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| format!("cannot synchronize directory {}: {error}", path.display()))
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> Result<(), String> {
    Ok(())
}

fn write_compile_inputs(directory: &Path, sources: &[SourceInput]) -> Result<Vec<PathBuf>, String> {
    if sources.len() != SOURCES.len() {
        return Err("blocker seed compiler requires every bundled source".into());
    }
    let mut paths = Vec::with_capacity(sources.len());
    for source in sources {
        let path = directory.join(source.spec.target);
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|error| format!("cannot create compiler input {}: {error}", path.display()))?;
        file.write_all(&source.raw)
            .and_then(|()| file.sync_all())
            .map_err(|error| format!("cannot write compiler input {}: {error}", path.display()))?;
        paths.push(path);
    }
    Ok(paths)
}

fn compile_all(
    repository: &Path,
    paths: &[PathBuf],
    webkit_output: Option<&Path>,
) -> Result<Vec<CompileGolden>, String> {
    [CompileTarget::Runtime, CompileTarget::Webkit]
        .into_iter()
        .map(|target| {
            compile_one(
                repository,
                target,
                paths,
                (target == CompileTarget::Webkit)
                    .then_some(webkit_output)
                    .flatten(),
                false,
            )
        })
        .collect()
}

fn compile_one(
    repository: &Path,
    target: CompileTarget,
    paths: &[PathBuf],
    artifact_output: Option<&Path>,
    unpruned: bool,
) -> Result<CompileGolden, String> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut command = Command::new(cargo);
    command.current_dir(repository).args([
        "run",
        "--quiet",
        "--offline",
        "--locked",
        "--package",
        "xtask",
        "--no-default-features",
        "--features",
        target.feature(),
        "--",
        "__compile-blocker-seed",
        target.argument(),
    ]);
    command.args(paths);
    if let Some(output) = artifact_output {
        command.arg("--artifact").arg(output);
    }
    if unpruned {
        command.arg("--unpruned");
    }
    let output = command.stdin(Stdio::null()).output().map_err(|error| {
        format!(
            "cannot start {} blocker seed compiler: {error}",
            target.argument()
        )
    })?;
    if !output.status.success() {
        return Err(format!(
            "{} blocker seed compilation failed:\n{}",
            target.argument(),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let stdout = std::str::from_utf8(&output.stdout)
        .map_err(|_| format!("{} compiler returned non-UTF-8 output", target.argument()))?
        .trim();
    serde_json::from_str(stdout).map_err(|error| {
        format!(
            "{} compiler returned invalid report: {error}",
            target.argument()
        )
    })
}

#[cfg(any(feature = "blocker-seed-runtime", feature = "blocker-seed-webkit"))]
pub(crate) fn compile_hidden(
    target: &str,
    paths: &[PathBuf],
    artifact_output: Option<&Path>,
    unpruned: bool,
) -> Result<(), String> {
    use zephium_blocker::{Compiler, FilterSource, SourceFormat, SourceId};

    let expected_target =
        if cfg!(feature = "blocker-seed-runtime") && !cfg!(feature = "blocker-seed-webkit") {
            CompileTarget::Runtime
        } else if cfg!(feature = "blocker-seed-webkit") && !cfg!(feature = "blocker-seed-runtime") {
            CompileTarget::Webkit
        } else {
            return Err("exactly one blocker seed compiler feature must be enabled".into());
        };
    if target != expected_target.argument() {
        return Err(format!(
            "compiler feature {} cannot build target {target}",
            expected_target.argument()
        ));
    }
    if artifact_output.is_some() && expected_target != CompileTarget::Webkit {
        return Err("only the WebKit compiler may materialize a native artifact".into());
    }
    if paths.len() != SOURCES.len() {
        return Err(format!(
            "the blocker seed compiler takes exactly {} sources",
            SOURCES.len()
        ));
    }
    let sources = SOURCES
        .iter()
        .map(|spec| spec.id)
        .zip(paths)
        .map(|(id, path)| {
            let bytes = read_regular_file(path, MAX_SOURCE_BYTES)?;
            let text =
                String::from_utf8(bytes).map_err(|_| format!("{} is not UTF-8", path.display()))?;
            Ok(FilterSource::new(
                SourceId::new(id).map_err(|error| format!("invalid source ID {id}: {error}"))?,
                SourceFormat::Standard,
                text,
            ))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let native_target = match expected_target {
        CompileTarget::Runtime => zephium_blocker::CompileTarget::Runtime,
        CompileTarget::Webkit => zephium_blocker::CompileTarget::WebKit,
    };
    let compiler = Compiler::default();
    #[cfg(feature = "blocker-seed-webkit")]
    let compiler = if unpruned {
        compiler.without_webkit_pruning()
    } else {
        compiler
    };
    #[cfg(not(feature = "blocker-seed-webkit"))]
    if unpruned {
        return Err("only the WebKit compiler prunes rules".into());
    }
    let compiled = compiler
        .compile(native_target, sources)
        .map_err(|error| format!("{error:?}"))?;
    #[cfg(feature = "blocker-seed-runtime")]
    compiled
        .verify_official_canary()
        .map_err(|error| format!("runtime release seed: {error}"))?;
    if let Some(output) = artifact_output {
        let webkit = compiled
            .webkit()
            .ok_or_else(|| "WebKit compiler produced no native artifact".to_owned())?;
        write_new_file(output, webkit.json().as_bytes(), "WebKit blocker artifact")?;
    }
    let report = compiled.report();
    let limits = Compiler::default().limits();
    let source_reports = report
        .sources()
        .iter()
        .map(|source| {
            let dropped = source
                .dropped()
                .iter()
                .map(|entry| (input_drop_name(entry.reason()).to_owned(), entry.count()))
                .collect();
            SourceGolden {
                id: source.id().as_str().into(),
                total_lines: source.total_lines(),
                ignored_lines: source.ignored_lines(),
                candidate_rules: source.candidate_rules(),
                accepted_rules: source.accepted_rules(),
                rejected_rules: source.rejected_rules(),
                attribution_sensitive_rules: source.attribution_sensitive_rules(),
                runtime_omitted_rules: source.runtime_omitted_rules(),
                runtime_approximated_rules: source.runtime_approximated_rules(),
                runtime_resource_approximated_rules: source.runtime_resource_approximated_rules(),
                runtime_source_kind_approximated_rules: source
                    .runtime_source_kind_approximated_rules(),
                dropped,
            }
        })
        .collect();
    #[cfg(feature = "blocker-seed-runtime")]
    let runtime = report.runtime().map(|coverage| RuntimeGolden {
        regexes: coverage.regexes(),
        omitted_regexes: coverage.omitted_regexes(),
        regex_limit: coverage.regex_limit(),
        pattern_bytes: coverage.pattern_bytes(),
        pattern_bytes_limit: coverage.pattern_bytes_limit(),
        largest_regex_patterns: coverage.largest_regex_patterns(),
        patterns_per_regex_limit: coverage.patterns_per_regex_limit(),
        largest_regex_pattern_bytes: coverage.largest_regex_pattern_bytes(),
        pattern_bytes_per_regex_limit: coverage.pattern_bytes_per_regex_limit(),
        regex_size_limit_bytes: coverage.regex_size_limit_bytes(),
        regex_dfa_size_limit_bytes: coverage.regex_dfa_size_limit_bytes(),
        max_filter_checks_per_request: coverage.max_filter_checks_per_request(),
    });
    #[cfg(not(feature = "blocker-seed-runtime"))]
    let runtime = None;
    let webkit = report.webkit().map(|coverage| WebKitGolden {
        accepted_input_rules: coverage.accepted_input_rules(),
        converted_input_rules: coverage.converted_input_rules(),
        pruned_input_rules: coverage.pruned_input_rules(),
        omitted_input_rules: coverage.omitted_input_rules(),
        approximated_input_rules: coverage.approximated_input_rules(),
        attribution_approximated_input_rules: coverage.attribution_approximated_input_rules(),
        resource_approximated_input_rules: coverage.resource_approximated_input_rules(),
        blocking_rule_entries: coverage.blocking_rule_entries(),
        emitted_rules: coverage.emitted_rules(),
        json_bytes: coverage.json_bytes(),
        dropped: coverage
            .dropped()
            .iter()
            .map(|entry| (webkit_drop_name(entry.reason()).to_owned(), entry.count()))
            .collect(),
    });
    let cosmetics = compiled
        .cosmetic_policy()
        .map(|policy| {
            let encoded = policy.encode().map_err(|error| error.to_string())?;
            let report = policy.report();
            let (scriptlet_hosts, scriptlet_calls) = policy.scriptlet_coverage();
            Ok::<_, String>(CosmeticGolden {
                accepted_rules: report.accepted,
                rejected_rules: report.rejected,
                generic_hide_controls: report.generic_controls,
                scriptlet_rules: report.scriptlet_rules,
                unknown_scriptlet_rules: report.unknown_scriptlet_rules,
                unsupported_scriptlet_rules: report.unsupported_scriptlet_rules,
                scriptlet_hosts,
                scriptlet_calls,
                scriptlet_program_bytes: policy.scriptlet_program().map_or(0, str::len),
                policy_sha256: hex_sha256(&encoded),
                policy_bytes: encoded.len(),
                native_artifact_sha256: None,
                native_json_bytes: None,
            })
        })
        .transpose()?;
    let golden = CompileGolden {
        target: expected_target,
        feature_graph: expected_target.argument().into(),
        policy_format_version: zephium_blocker::POLICY_FORMAT_VERSION,
        webkit_artifact_format_version: zephium_blocker::WEBKIT_ARTIFACT_FORMAT_VERSION,
        adblock_engine_version: zephium_blocker::ADBLOCK_ENGINE_VERSION.into(),
        limits: CompileBudgets {
            max_sources: limits.max_sources(),
            max_source_bytes: limits.max_source_bytes(),
            max_total_source_bytes: limits.max_total_source_bytes(),
            max_line_bytes: limits.max_line_bytes(),
            max_rules: limits.max_rules(),
            max_physical_lines: limits.max_physical_lines(),
            max_webkit_rules: limits.max_webkit_rules(),
            max_webkit_json_bytes: limits.max_webkit_json_bytes(),
            max_request_url_bytes: limits.max_request_url_bytes(),
            max_source_url_bytes: limits.max_source_url_bytes(),
        },
        policy_sha256: compiled.digest().to_string(),
        native_artifact_sha256: compiled.webkit().map(|rules| rules.digest().to_string()),
        total_source_bytes: report.total_source_bytes(),
        candidate_rules: report.candidate_rules(),
        accepted_rules: report.accepted_rules(),
        rejected_rules: report.rejected_rules(),
        native_blocking_rule_entries: report.native_blocking_rule_entries(),
        attribution_sensitive_rules: report.attribution_sensitive_rules(),
        runtime_omitted_rules: report.runtime_omitted_rules(),
        runtime_approximated_rules: report.runtime_approximated_rules(),
        runtime_resource_approximated_rules: report.runtime_resource_approximated_rules(),
        runtime_source_kind_approximated_rules: report.runtime_source_kind_approximated_rules(),
        sources: source_reports,
        runtime,
        webkit,
        cosmetics,
    };
    let encoded = canonical_json(&golden)?;
    println!(
        "{}",
        String::from_utf8(encoded).map_err(|_| "compiler report is not UTF-8")?
    );
    Ok(())
}

#[cfg(any(feature = "blocker-seed-runtime", feature = "blocker-seed-webkit"))]
const fn input_drop_name(reason: zephium_blocker::InputDropReason) -> &'static str {
    use zephium_blocker::InputDropReason;
    match reason {
        InputDropReason::InvalidNetworkRule => "invalid_network_rule",
        InputDropReason::InvalidCosmeticRule => "invalid_cosmetic_rule",
        InputDropReason::UnsupportedRule => "unsupported_rule",
        InputDropReason::UnsupportedCosmeticRule => "unsupported_cosmetic_rule",
        InputDropReason::UnsupportedTag => "unsupported_tag",
        InputDropReason::ForbiddenRedirect => "forbidden_redirect",
        InputDropReason::ForbiddenCsp => "forbidden_csp",
        InputDropReason::ForbiddenRemoveParam => "forbidden_remove_param",
        InputDropReason::ForbiddenGenericHide => "forbidden_generic_hide",
        InputDropReason::UnsupportedMethodPredicate => "unsupported_method_predicate",
        InputDropReason::InvalidDomainPredicate => "invalid_domain_predicate",
        InputDropReason::UnsupportedRegularExpression => "unsupported_regular_expression",
    }
}

#[cfg(any(feature = "blocker-seed-runtime", feature = "blocker-seed-webkit"))]
const fn webkit_drop_name(reason: zephium_blocker::WebKitDropReason) -> &'static str {
    use zephium_blocker::WebKitDropReason;
    match reason {
        WebKitDropReason::MixedDomainConditions => "mixed_domain_conditions",
        WebKitDropReason::UnsupportedResourceTypes => "unsupported_resource_types",
        WebKitDropReason::BadFilterControl => "badfilter_control",
        WebKitDropReason::FullRegularExpression => "full_regular_expression",
        WebKitDropReason::OptimizedRule => "optimized_rule",
        WebKitDropReason::CosmeticEntity => "cosmetic_entity",
        WebKitDropReason::NonAscii => "non_ascii",
        WebKitDropReason::InvalidDomain => "invalid_domain",
        WebKitDropReason::FromAlias => "from_alias",
        WebKitDropReason::RequestMethod => "request_method",
        WebKitDropReason::ImportantPriority => "important_priority",
        WebKitDropReason::SuppressedByRuleSemantics => "suppressed_by_rule_semantics",
        WebKitDropReason::UrlFilterTooLarge => "url_filter_too_large",
        WebKitDropReason::NavigationChecked => "navigation_checked",
    }
}

fn canonical_json<T: Serialize>(value: &T) -> Result<Vec<u8>, String> {
    serde_json::to_vec(value).map_err(|error| format!("cannot encode canonical JSON: {error}"))
}

fn parse_canonical_json<T>(bytes: &[u8], name: &str) -> Result<T, String>
where
    T: for<'de> Deserialize<'de> + Serialize,
{
    let value = serde_json::from_slice(bytes)
        .map_err(|error| format!("{name} is malformed JSON: {error}"))?;
    if canonical_json(&value)? != bytes {
        return Err(format!("{name} is not exact canonical JSON"));
    }
    Ok(value)
}

fn hex_sha256(bytes: &[u8]) -> String {
    lower_hex(&Sha256::digest(bytes))
}

fn lower_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_version_timestamp_is_exact() {
        assert_eq!(
            timestamp_from_version("202607241759").unwrap(),
            (1_784_915_940, "24 Jul 2026 17:59 UTC".into())
        );
        assert!(timestamp_from_version("202602301200").is_err());
        assert!(timestamp_from_version("202613011200").is_err());
    }

    #[test]
    fn dated_lists_are_versioned_by_their_utc_modification_minute() {
        assert_eq!(
            dated_version("Mon, 02 Nov 2026 12:27:09 +0000").as_deref(),
            Some("202611021227")
        );
        assert_eq!(
            dated_version("Mon, 02 Nov 2026 23:30:00 -0100").as_deref(),
            Some("202611030030")
        );
        assert_eq!(
            dated_version("01 Mar 2028 00:15 +0130").as_deref(),
            Some("202802292245")
        );
        assert_eq!(dated_version("yesterday"), None);
        assert_eq!(dated_version("Mon, 31 Nov 2026 12:27:09 +0000"), None);
        assert_eq!(expires_seconds("8 hours"), Some(8 * HOUR));
        assert_eq!(expires_seconds("4 days (update frequency)"), Some(4 * DAY));
    }

    #[test]
    fn source_headers_follow_each_lists_own_format() {
        let quick_fixes = &SOURCES[3];
        let mut list = format!(
            "! Title: {}\n! Last modified: Mon, 02 Nov 2026 14:08:18 +0000\n! Expires: 8 hours\n! License: {}\n! Homepage: {}\n",
            quick_fixes.title,
            GPL_3.terms_url,
            quick_fixes.homepage.unwrap()
        );
        for rule in 0..600 {
            list.push_str(&format!("example{rule}.invalid##.advertisement\n"));
        }
        let header = parse_source_header(list.as_bytes(), quick_fixes).unwrap();
        assert_eq!(header.version, "202611021408");
        assert_eq!(header.commit, None);
        assert_eq!(header.expiry_seconds, 8 * HOUR);
        assert!(parse_source_header(
            format!("[Adblock Plus 2.0]\n{list}").as_bytes(),
            quick_fixes
        )
        .unwrap_err()
        .contains("unexpected ABP header"));
        assert!(parse_source_header(list.as_bytes(), &SOURCES[2]).is_err());
    }

    #[test]
    fn deterministic_gzip_rejects_trailing_data_and_round_trips() {
        let raw = b"[Adblock Plus 2.0]\n||ads.example^\n";
        let compressed = deterministic_gzip(raw).unwrap();
        assert_eq!(
            bounded_inflate(&compressed, raw.len() as u64, "fixture").unwrap(),
            raw
        );
        let mut trailing = compressed;
        trailing.push(0);
        assert!(bounded_inflate(&trailing, raw.len() as u64, "fixture").is_err());

        let mut concatenated = deterministic_gzip(raw).unwrap();
        concatenated.extend(deterministic_gzip(raw).unwrap());
        assert!(bounded_inflate(&concatenated, raw.len() as u64, "fixture").is_err());
    }

    #[test]
    fn canonical_json_rejects_formatting_variants() {
        let value = QualityManifest {
            schema_version: 1,
            package_revision: 1,
            catalog_manifest_sha256: "0".repeat(64),
            release_seed_manifest_sha256: "1".repeat(64),
            licenses: license_records(),
            compilers: Vec::new(),
        };
        let canonical = canonical_json(&value).unwrap();
        assert_eq!(
            parse_canonical_json::<QualityManifest>(&canonical, "fixture").unwrap(),
            value
        );
        let mut noncanonical = canonical;
        noncanonical.push(b'\n');
        assert!(parse_canonical_json::<QualityManifest>(&noncanonical, "fixture").is_err());
    }

    #[test]
    fn materialized_artifacts_never_replace_an_existing_path() {
        let directory = tempfile::tempdir().unwrap();
        let output = directory.path().join("artifact.json");
        write_new_file(&output, b"first", "fixture").unwrap();
        assert!(write_new_file(&output, b"second", "fixture")
            .unwrap_err()
            .contains("cannot create"));
        assert_eq!(std::fs::read(output).unwrap(), b"first");
    }

    #[test]
    fn production_release_requires_source_material_current_at_publication() {
        let now = 1_800_000_000;
        let source = CatalogSource {
            id: "fixture".into(),
            format: SourceFormat::Standard,
            target: "fixture.txt".into(),
            length: 1,
            sha256: "0".repeat(64),
            license: license_metadata(&SOURCES[0]),
        };
        let mut catalog = CatalogManifest {
            schema_version: 1,
            revision: 1,
            created_unix: now,
            expires_unix: now + 1,
            sources: vec![source; SOURCES.len()],
        };
        assert!(validate_release_freshness(&catalog, now).is_ok());

        catalog.expires_unix = now;
        assert!(
            validate_release_freshness(&catalog, now).is_ok(),
            "a source past its refresh hint still ships"
        );
        assert!(
            validate_release_freshness(&catalog, now + RELEASE_SEED_MAX_AGE_SECONDS - 1).is_ok()
        );
        assert!(
            validate_release_freshness(&catalog, now + RELEASE_SEED_MAX_AGE_SECONDS)
                .unwrap_err()
                .contains("older than a release may ship")
        );

        catalog.expires_unix = now + 1;
        catalog.created_unix = now + 1;
        assert_eq!(
            validate_release_freshness(&catalog, now).unwrap_err(),
            "blocker seed was created in the future"
        );
    }

    #[test]
    fn generation_rejects_rollback_and_equivocation_but_accepts_new_lists() {
        let directory = tempfile::tempdir().unwrap();
        for name in file_set([("easylist.txt", &CC_BY_SA)]) {
            std::fs::write(directory.path().join(name), b"fixture").unwrap();
        }
        let source = CatalogSource {
            id: "easylist".into(),
            format: SourceFormat::Standard,
            target: "easylist.txt".into(),
            length: 7,
            sha256: "0".repeat(64),
            license: license_metadata(&SOURCES[0]),
        };
        let previous_catalog = CatalogManifest {
            schema_version: 1,
            revision: 2,
            created_unix: 2,
            expires_unix: 3,
            sources: vec![source.clone()],
        };
        let previous_catalog_bytes = canonical_json(&previous_catalog).unwrap();
        let asset = ReleaseSeedAsset {
            target: "easylist.txt".into(),
            compression: CompressionFormat::Gzip,
            compressed_length: 7,
            compressed_sha256: "1".repeat(64),
            upstream_title: "EasyList".into(),
            upstream_version: "202607240002".into(),
            upstream_commit: Some("2".repeat(40)),
            upstream_modified: None,
        };
        let previous_seed = ReleaseSeedManifest {
            schema_version: 1,
            catalog_manifest_sha256: hex_sha256(&previous_catalog_bytes),
            assets: vec![asset.clone()],
        };
        std::fs::write(directory.path().join(CATALOG_FILE), &previous_catalog_bytes).unwrap();
        std::fs::write(
            directory.path().join(SEED_FILE),
            canonical_json(&previous_seed).unwrap(),
        )
        .unwrap();

        let mut rollback_catalog = previous_catalog.clone();
        rollback_catalog.revision = 1;
        let rollback_bytes = canonical_json(&rollback_catalog).unwrap();
        assert!(validate_generation_progression(
            directory.path(),
            &rollback_catalog,
            &rollback_bytes,
            &previous_seed,
        )
        .is_err());

        let mut equivocation_catalog = previous_catalog.clone();
        equivocation_catalog.sources[0].sha256 = "f".repeat(64);
        let equivocation_bytes = canonical_json(&equivocation_catalog).unwrap();
        assert!(validate_generation_progression(
            directory.path(),
            &equivocation_catalog,
            &equivocation_bytes,
            &previous_seed,
        )
        .is_err());

        let added = CatalogSource {
            id: "ublock-filters".into(),
            target: "ublock-filters.txt".into(),
            license: license_metadata(&SOURCES[2]),
            ..source.clone()
        };
        let added_asset = ReleaseSeedAsset {
            target: "ublock-filters.txt".into(),
            upstream_title: "uBlock filters".into(),
            upstream_version: "202611021227".into(),
            upstream_commit: None,
            upstream_modified: Some("Mon, 02 Nov 2026 12:27:09 +0000".into()),
            ..asset.clone()
        };
        let mut grown_catalog = previous_catalog.clone();
        grown_catalog.revision = 202_611_021_227;
        grown_catalog.sources.push(added.clone());
        let grown_seed = ReleaseSeedManifest {
            schema_version: SEED_SCHEMA_VERSION,
            catalog_manifest_sha256: "3".repeat(64),
            assets: vec![asset.clone(), added_asset.clone()],
        };
        let grown_bytes = canonical_json(&grown_catalog).unwrap();
        validate_generation_progression(
            directory.path(),
            &grown_catalog,
            &grown_bytes,
            &grown_seed,
        )
        .unwrap();

        grown_catalog.sources.swap(0, 1);
        let reordered_seed = ReleaseSeedManifest {
            assets: vec![added_asset, asset],
            ..grown_seed
        };
        let reordered_bytes = canonical_json(&grown_catalog).unwrap();
        assert!(validate_generation_progression(
            directory.path(),
            &grown_catalog,
            &reordered_bytes,
            &reordered_seed,
        )
        .unwrap_err()
        .contains("identity changed"));
    }

    fn dated_input(index: usize, version: &str, raw: &[u8]) -> SourceInput {
        SourceInput {
            spec: &SOURCES[index],
            raw: raw.to_vec(),
            header: SourceHeader {
                title: SOURCES[index].id.into(),
                version: version.into(),
                commit: None,
                modified: None,
                modified_unix: timestamp_from_version(version).unwrap().0,
                expiry_seconds: 4 * 24 * 60 * 60,
            },
        }
    }

    fn catalog_of(inputs: &[SourceInput], revision: u64) -> CatalogManifest {
        build_catalog(inputs, revision).unwrap()
    }

    #[test]
    fn any_source_change_advances_the_revision_by_at_least_a_minute() {
        let old = [
            dated_input(0, "202611021000", b"a"),
            dated_input(3, "202611021459", b"b"),
        ];
        let previous = catalog_of(&old, 202611021459);
        assert_eq!(next_revision(&old, Some(&previous)).unwrap(), 202611021459);
        // The lower-versioned list changes; the revision still moves, with the minute carried.
        let changed = [
            dated_input(0, "202611021000", b"a2"),
            dated_input(3, "202611021459", b"b"),
        ];
        assert_eq!(
            next_revision(&changed, Some(&previous)).unwrap(),
            202611021500
        );
        let newer = [
            dated_input(0, "202611021000", b"a"),
            dated_input(3, "202611031200", b"b2"),
        ];
        assert_eq!(
            next_revision(&newer, Some(&previous)).unwrap(),
            202611031200
        );
        assert_eq!(next_revision(&old, None).unwrap(), 202611021459);
        assert!(build_catalog(&newer, 202611021459).is_err());
    }

    #[test]
    fn a_rarely_changed_list_does_not_date_the_seed() {
        let abuse = SOURCES
            .iter()
            .position(|spec| spec.id == "ublock-resource-abuse")
            .unwrap();
        let inputs = [
            dated_input(0, "202611031000", b"a"),
            dated_input(3, "202611041000", b"b"),
            dated_input(abuse, "202610020529", b"c"),
        ];
        let catalog = catalog_of(&inputs, 202611041000);
        assert_eq!(
            catalog.created_unix,
            timestamp_from_version("202611031000").unwrap().0
        );
        assert!(catalog.expires_unix > catalog.created_unix);
    }

    #[test]
    fn a_source_dated_in_the_future_is_refused() {
        let input = [dated_input(3, "202611021200", b"q")];
        let modified = input[0].header.modified_unix;
        assert!(reject_future_sources(&input, modified).is_ok());
        assert!(reject_future_sources(&input, modified - 300).is_ok());
        assert!(reject_future_sources(&input, modified - 301).is_err());

        let mut exact = dated_input(3, "202611021200", b"q");
        exact.header.modified = Some("Mon, 02 Nov 2026 12:00:59 +0000".into());
        let input = [exact];
        // The minute is within the skew; its 59th second is not.
        assert!(reject_future_sources(&input, modified - 290).is_err());
        assert!(reject_future_sources(&input, modified - 241).is_ok());
    }

    #[test]
    fn a_dated_list_may_change_within_its_minute_only_to_a_later_copy() {
        const PREVIOUS: &str = "Mon, 02 Nov 2026 14:08:18 +0000";
        for (index, target, license, dated, modified, accepted) in [
            (
                3,
                "ublock-quick-fixes.txt",
                &GPL_3,
                true,
                "Mon, 02 Nov 2026 14:08:40 +0000",
                true,
            ),
            (3, "ublock-quick-fixes.txt", &GPL_3, true, PREVIOUS, false),
            (
                3,
                "ublock-quick-fixes.txt",
                &GPL_3,
                true,
                "Mon, 02 Nov 2026 14:08:02 +0000",
                false,
            ),
            (0, "easylist.txt", &CC_BY_SA, false, "", false),
        ] {
            let directory = tempfile::tempdir().unwrap();
            for name in file_set([(target, license)]) {
                std::fs::write(directory.path().join(name), b"fixture").unwrap();
            }
            let source = CatalogSource {
                id: SOURCES[index].id.into(),
                format: SourceFormat::Standard,
                target: target.into(),
                length: 7,
                sha256: "0".repeat(64),
                license: license_metadata(&SOURCES[index]),
            };
            let previous = CatalogManifest {
                schema_version: 1,
                revision: 202_611_021_408,
                created_unix: 2,
                expires_unix: 3,
                sources: vec![source.clone()],
            };
            let previous_bytes = canonical_json(&previous).unwrap();
            let asset = ReleaseSeedAsset {
                target: target.into(),
                compression: CompressionFormat::Gzip,
                compressed_length: 7,
                compressed_sha256: "1".repeat(64),
                upstream_title: SOURCES[index].id.into(),
                upstream_version: "202611021408".into(),
                upstream_commit: (!dated).then(|| "2".repeat(40)),
                upstream_modified: dated.then(|| PREVIOUS.into()),
            };
            let seed = ReleaseSeedManifest {
                schema_version: SEED_SCHEMA_VERSION,
                catalog_manifest_sha256: hex_sha256(&previous_bytes),
                assets: vec![asset.clone()],
            };
            std::fs::write(directory.path().join(CATALOG_FILE), &previous_bytes).unwrap();
            std::fs::write(
                directory.path().join(SEED_FILE),
                canonical_json(&seed).unwrap(),
            )
            .unwrap();
            let mut changed = previous.clone();
            changed.revision = 202_611_021_409;
            changed.sources[0].sha256 = "f".repeat(64);
            let changed_bytes = canonical_json(&changed).unwrap();
            let changed_seed = ReleaseSeedManifest {
                assets: vec![ReleaseSeedAsset {
                    upstream_modified: dated.then(|| modified.into()),
                    ..asset
                }],
                ..seed
            };
            let result = validate_generation_progression(
                directory.path(),
                &changed,
                &changed_bytes,
                &changed_seed,
            );
            assert_eq!(result.is_ok(), accepted, "{target} {modified}");
        }
    }
}
