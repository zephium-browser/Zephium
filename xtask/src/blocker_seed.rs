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
const EASYLIST_ASSET: &str = "easylist.txt.gz";
const EASYPRIVACY_ASSET: &str = "easyprivacy.txt.gz";
const LICENSE_FILE: &str = "LICENSE-CC-BY-SA-3.0.txt";
const NOTICE_FILE: &str = "NOTICE";
const EXPECTED_FILES: [&str; 7] = [
    CATALOG_FILE,
    EASYLIST_ASSET,
    EASYPRIVACY_ASSET,
    LICENSE_FILE,
    NOTICE_FILE,
    QUALITY_FILE,
    SEED_FILE,
];
const LICENSE_SHA256: &str = "3f941b3b89cf7b8370ceb83cc76d2120d471b58735d8ca60238a751a48d7f72f";
const LICENSE_EXPRESSION: &str = "CC-BY-SA-3.0";
const ATTRIBUTION: &str = "The EasyList authors (https://easylist.to/)";
const REDISTRIBUTION: &str = "Unmodified upstream subscription; deterministic gzip packaging only";
const LICENSE_URL: &str = "https://easylist.to/pages/licence.html";
const SOURCE_EXPIRY_SECONDS: u64 = 4 * 24 * 60 * 60;
/// The seed only protects the first hours, until the app fetches current lists
/// on its own; a release may ship one up to a month old, so a hotfix never
/// waits on a list refresh. The sources' own expiry still drives the app.
const RELEASE_SEED_MAX_AGE_SECONDS: u64 = 30 * 24 * 60 * 60;
const MAX_SOURCE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_LICENSE_BYTES: u64 = 64 * 1024;
const GZIP_HEADER: [u8; 10] = [0x1f, 0x8b, 0x08, 0x00, 0, 0, 0, 0, 0x02, 0xff];

#[derive(Clone, Debug)]
struct SourceInput {
    id: &'static str,
    target: &'static str,
    source_url: &'static str,
    raw: Vec<u8>,
    header: SourceHeader,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SourceHeader {
    title: String,
    version: String,
    commit: String,
    modified_unix: u64,
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
    upstream_commit: String,
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
    license_file: String,
    license_sha256: String,
    compilers: Vec<CompileGolden>,
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

pub(crate) fn check_release_freshness(repository: &Path, now: u64) -> Result<(), String> {
    let directory = repository.join(ASSET_DIRECTORY);
    validate_closed_file_set(&directory)?;
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
    if catalog.schema_version != 1 || catalog.sources.len() != 2 {
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
    validate_closed_file_set(&directory)?;

    let license = read_regular_file(&directory.join(LICENSE_FILE), MAX_LICENSE_BYTES)?;
    validate_license(&license)?;
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
    if quality.schema_version != 2
        || quality.package_revision != catalog.revision
        || quality.license_file != LICENSE_FILE
        || quality.license_sha256 != LICENSE_SHA256
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

pub(crate) fn update(
    repository: &Path,
    easylist: &Path,
    easyprivacy: &Path,
    license_path: &Path,
) -> Result<(), String> {
    let easylist = read_source(
        "easylist",
        "easylist.txt",
        "EasyList",
        "https://easylist.to/easylist/easylist.txt",
        easylist,
    )?;
    let easyprivacy = read_source(
        "easyprivacy",
        "easyprivacy.txt",
        "EasyPrivacy",
        "https://easylist.to/easylist/easyprivacy.txt",
        easyprivacy,
    )?;
    let sources = vec![easylist, easyprivacy];
    let license = read_regular_file(license_path, MAX_LICENSE_BYTES)?;
    validate_license(&license)?;

    let catalog = build_catalog(&sources)?;
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
        schema_version: 2,
        package_revision: catalog.revision,
        catalog_manifest_sha256: hex_sha256(&catalog_bytes),
        release_seed_manifest_sha256: hex_sha256(&seed_bytes),
        license_file: LICENSE_FILE.into(),
        license_sha256: LICENSE_SHA256.into(),
        compilers,
    };

    let mut files = BTreeMap::<&str, Vec<u8>>::new();
    files.insert(CATALOG_FILE, catalog_bytes);
    files.insert(SEED_FILE, seed_bytes);
    files.insert(QUALITY_FILE, canonical_json(&quality)?);
    files.insert(EASYLIST_ASSET, compressed[0].clone());
    files.insert(EASYPRIVACY_ASSET, compressed[1].clone());
    files.insert(LICENSE_FILE, license);
    files.insert(NOTICE_FILE, build_notice(&sources).into_bytes());
    let publication = replace_asset_directory(repository, &files)?;
    if let Err(error) = check(repository) {
        publication.rollback()?;
        return Err(format!(
            "generated blocker seed failed post-publication verification and was rolled back: {error}"
        ));
    }
    publication.commit()
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
    validate_closed_file_set(directory)?;
    let previous_catalog_bytes = read_regular_file(&directory.join(CATALOG_FILE), 128 * 1024)?;
    let previous_seed_bytes = read_regular_file(&directory.join(SEED_FILE), 128 * 1024)?;
    let previous_catalog: CatalogManifest =
        parse_canonical_json(&previous_catalog_bytes, CATALOG_FILE)?;
    let previous_seed: ReleaseSeedManifest = parse_canonical_json(&previous_seed_bytes, SEED_FILE)?;
    if previous_seed.catalog_manifest_sha256 != hex_sha256(&previous_catalog_bytes)
        || previous_catalog.sources.len() != catalog.sources.len()
        || previous_seed.assets.len() != seed.assets.len()
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
        if version == previous_version
            && (source != previous_source
                || asset.upstream_commit != previous_asset.upstream_commit)
        {
            return Err(format!(
                "{} version {} is being assigned different source bytes or provenance",
                asset.upstream_title, version
            ));
        }
    }
    Ok(())
}

fn build_catalog(sources: &[SourceInput]) -> Result<CatalogManifest, String> {
    let revision = sources
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
        .ok_or_else(|| "blocker seed has no sources".to_owned())?;
    let created_unix = sources
        .iter()
        .map(|source| source.header.modified_unix)
        .max()
        .ok_or_else(|| "blocker seed has no creation time".to_owned())?;
    let expires_unix = sources
        .iter()
        .map(|source| source.header.modified_unix + SOURCE_EXPIRY_SECONDS)
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
                id: source.id.into(),
                format: SourceFormat::Standard,
                target: source.target.into(),
                length: source.raw.len() as u64,
                sha256: hex_sha256(&source.raw),
                license: LicenseMetadata {
                    license_expression: LICENSE_EXPRESSION.into(),
                    attribution: ATTRIBUTION.into(),
                    redistribution: REDISTRIBUTION.into(),
                    source_url: source.source_url.into(),
                },
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
        schema_version: 1,
        catalog_manifest_sha256: hex_sha256(catalog_bytes),
        assets: sources
            .iter()
            .zip(compressed)
            .map(|(source, bytes)| ReleaseSeedAsset {
                target: source.target.into(),
                compression: CompressionFormat::Gzip,
                compressed_length: bytes.len() as u64,
                compressed_sha256: hex_sha256(bytes),
                upstream_title: source.header.title.clone(),
                upstream_version: source.header.version.clone(),
                upstream_commit: source.header.commit.clone(),
            })
            .collect(),
    }
}

fn validate_catalog(catalog: &CatalogManifest, sources: &[SourceInput]) -> Result<(), String> {
    if catalog != &build_catalog(sources)? {
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
        || seed.schema_version != 1
        || catalog.sources.len() != 2
        || seed.assets.len() != 2
    {
        return Err("blocker seed manifest schema or source count is invalid".into());
    }
    let expected = [
        (
            "easylist",
            "easylist.txt",
            EASYLIST_ASSET,
            "EasyList",
            "https://easylist.to/easylist/easylist.txt",
        ),
        (
            "easyprivacy",
            "easyprivacy.txt",
            EASYPRIVACY_ASSET,
            "EasyPrivacy",
            "https://easylist.to/easylist/easyprivacy.txt",
        ),
    ];
    let mut sources = Vec::with_capacity(expected.len());
    for ((catalog_source, asset), (id, target, file, title, source_url)) in
        catalog.sources.iter().zip(&seed.assets).zip(expected)
    {
        validate_catalog_source(catalog_source, id, target, source_url)?;
        if asset.target != target
            || asset.compression != CompressionFormat::Gzip
            || asset.upstream_title != title
        {
            return Err(format!("{title} release asset metadata is invalid"));
        }
        let compressed = read_regular_file(&directory.join(file), 16 * 1024 * 1024)?;
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
        let header = parse_source_header(&raw, title)?;
        if header.version != asset.upstream_version || header.commit != asset.upstream_commit {
            return Err(format!(
                "{title} header provenance does not match its envelope"
            ));
        }
        sources.push(SourceInput {
            id,
            target,
            source_url,
            raw,
            header,
        });
    }
    Ok(sources)
}

fn validate_catalog_source(
    source: &CatalogSource,
    id: &str,
    target: &str,
    source_url: &str,
) -> Result<(), String> {
    let expected_license = LicenseMetadata {
        license_expression: LICENSE_EXPRESSION.into(),
        attribution: ATTRIBUTION.into(),
        redistribution: REDISTRIBUTION.into(),
        source_url: source_url.into(),
    };
    if source.id != id
        || source.target != target
        || source.format != SourceFormat::Standard
        || source.length == 0
        || source.length > MAX_SOURCE_BYTES
        || !valid_sha256(&source.sha256)
        || source.license != expected_license
    {
        return Err(format!("{id} catalog descriptor is invalid"));
    }
    Ok(())
}

fn read_source(
    id: &'static str,
    target: &'static str,
    title: &str,
    source_url: &'static str,
    path: &Path,
) -> Result<SourceInput, String> {
    let raw = read_regular_file(path, MAX_SOURCE_BYTES)?;
    let header = parse_source_header(&raw, title)?;
    Ok(SourceInput {
        id,
        target,
        source_url,
        raw,
        header,
    })
}

fn parse_source_header(raw: &[u8], expected_title: &str) -> Result<SourceHeader, String> {
    if raw.is_empty() || raw.last() != Some(&b'\n') || raw.contains(&0) || raw.contains(&b'\r') {
        return Err(format!(
            "{expected_title} must be nonempty NUL-free UTF-8 with canonical LF endings"
        ));
    }
    let text = std::str::from_utf8(raw)
        .map_err(|_| format!("{expected_title} source is not valid UTF-8"))?;
    let mut lines = text.lines().take(32);
    let first = lines
        .next()
        .ok_or_else(|| format!("{expected_title} has no ABP header"))?;
    if !matches!(first, "[Adblock Plus 1.1]" | "[Adblock Plus 2.0]") {
        return Err(format!("{expected_title} has an unsupported ABP header"));
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
            "Licence",
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
    let title = field("Title")?;
    let version = field("Version")?;
    let commit = field("Commit")?;
    if title != expected_title
        || version.len() != 12
        || !version.bytes().all(|byte| byte.is_ascii_digit())
        || commit.len() != 40
        || !commit
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        || field("Expires")? != "4 days (update frequency)"
        || field("Homepage")? != "https://easylist.to/"
        || field("Licence")? != LICENSE_URL
    {
        return Err(format!("{expected_title} header provenance is invalid"));
    }
    let (modified_unix, formatted) = timestamp_from_version(version)?;
    if field("Last modified")? != formatted {
        return Err(format!(
            "{expected_title} version and Last modified headers disagree"
        ));
    }
    Ok(SourceHeader {
        title: title.into(),
        version: version.into(),
        commit: commit.into(),
        modified_unix,
    })
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

fn validate_license(bytes: &[u8]) -> Result<(), String> {
    if hex_sha256(bytes) != LICENSE_SHA256 {
        return Err(format!(
            "{LICENSE_FILE} differs from the reviewed CC BY-SA 3.0 legal code"
        ));
    }
    let text =
        std::str::from_utf8(bytes).map_err(|_| format!("{LICENSE_FILE} is not valid UTF-8"))?;
    if !text.contains("Attribution-ShareAlike 3.0 Unported")
        || !text.contains("CREATIVE COMMONS CORPORATION IS NOT A LAW FIRM")
    {
        return Err(format!(
            "{LICENSE_FILE} does not contain the expected legal code"
        ));
    }
    Ok(())
}

fn build_notice(sources: &[SourceInput]) -> String {
    let mut notice = String::from(
        "Zephium bundled blocker seed v1\n\n\
         This directory contains unmodified EasyList and EasyPrivacy subscription text.\n\
         Zephium changes only the storage representation by applying deterministic gzip\n\
         compression; the decompressed source bytes are unchanged.\n\n\
         Attribution: The EasyList authors (https://easylist.to/)\n\
         Selected license: CC BY-SA 3.0 Unported (CC-BY-SA-3.0)\n\
         License text: LICENSE-CC-BY-SA-3.0.txt\n\
         Upstream terms: https://easylist.to/pages/licence.html\n\n",
    );
    for source in sources {
        notice.push_str(&format!(
            "{}\nSource: {}\nVersion: {}\nCommit: {}\nRaw SHA-256: {}\n\n",
            source.header.title,
            source.source_url,
            source.header.version,
            source.header.commit,
            hex_sha256(&source.raw)
        ));
    }
    notice.push_str(
        "The upstream authors do not endorse Zephium. Release engineering must regenerate\n\
         and verify this record with `cargo xtask update-blocker-seed` whenever either\n\
         subscription changes.\n",
    );
    notice
}

fn validate_closed_file_set(directory: &Path) -> Result<(), String> {
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
    let expected = EXPECTED_FILES
        .iter()
        .map(|name| (*name).to_owned())
        .collect::<BTreeSet<_>>();
    if actual != expected {
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
    files: &BTreeMap<&str, Vec<u8>>,
) -> Result<PublishedDirectory, String> {
    if files.keys().copied().collect::<BTreeSet<_>>()
        != EXPECTED_FILES.into_iter().collect::<BTreeSet<_>>()
    {
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

fn write_compile_inputs(directory: &Path, sources: &[SourceInput]) -> Result<[PathBuf; 2], String> {
    if sources.len() != 2 {
        return Err("blocker seed compiler requires exactly two sources".into());
    }
    let mut paths = Vec::with_capacity(2);
    for source in sources {
        let path = directory.join(source.target);
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
    paths
        .try_into()
        .map_err(|_| "blocker seed compiler input count changed".to_owned())
}

fn compile_all(
    repository: &Path,
    paths: &[PathBuf; 2],
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
            )
        })
        .collect()
}

fn compile_one(
    repository: &Path,
    target: CompileTarget,
    paths: &[PathBuf; 2],
    artifact_output: Option<&Path>,
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
    command.arg(&paths[0]).arg(&paths[1]);
    if let Some(output) = artifact_output {
        command.arg("--artifact").arg(output);
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
    easylist: &Path,
    easyprivacy: &Path,
    artifact_output: Option<&Path>,
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
    let sources = [("easylist", easylist), ("easyprivacy", easyprivacy)]
        .into_iter()
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
    let compiled = Compiler::default()
        .compile(native_target, sources)
        .map_err(|error| format!("{error:?}"))?;
    #[cfg(feature = "blocker-seed-runtime")]
    verify_runtime_release_seed_smoke(&compiled)?;
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
            Ok::<_, String>(CosmeticGolden {
                accepted_rules: report.accepted,
                rejected_rules: report.rejected,
                generic_hide_controls: report.generic_controls,
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

#[cfg(feature = "blocker-seed-runtime")]
fn verify_runtime_release_seed_smoke(
    compiled: &zephium_blocker::CompiledRules,
) -> Result<(), String> {
    use zephium_blocker::{NetworkAction, NetworkRequest, RequestMethod, ResourceType};

    for (url, expected) in [
        (
            "https://ad.doubleclick.com/activity;src=1",
            NetworkAction::Block,
        ),
        (
            "https://www.googletagmanager.com/zephium-blocker-smoke.js",
            NetworkAction::Block,
        ),
        ("https://example.com/app.js", NetworkAction::Allow),
    ] {
        let decision = compiled
            .evaluate_source_independent(NetworkRequest::source_independent(
                url,
                ResourceType::Script,
                RequestMethod::Get,
            ))
            .map_err(|error| format!("runtime release-seed smoke probe failed: {error:?}"))?;
        if decision.action() != expected {
            return Err(format!(
                "runtime release-seed smoke probe returned {:?} for {url}, expected {expected:?}",
                decision.action()
            ));
        }
    }
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
            license_file: LICENSE_FILE.into(),
            license_sha256: LICENSE_SHA256.into(),
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
            license: LicenseMetadata {
                license_expression: LICENSE_EXPRESSION.into(),
                attribution: ATTRIBUTION.into(),
                redistribution: REDISTRIBUTION.into(),
                source_url: "https://example.invalid/fixture.txt".into(),
            },
        };
        let mut catalog = CatalogManifest {
            schema_version: 1,
            revision: 1,
            created_unix: now,
            expires_unix: now + 1,
            sources: vec![source.clone(), source],
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
    fn generation_rejects_revision_rollback_and_equivocation() {
        let directory = tempfile::tempdir().unwrap();
        for name in EXPECTED_FILES {
            std::fs::write(directory.path().join(name), b"fixture").unwrap();
        }
        let source = CatalogSource {
            id: "easylist".into(),
            format: SourceFormat::Standard,
            target: "easylist.txt".into(),
            length: 7,
            sha256: "0".repeat(64),
            license: LicenseMetadata {
                license_expression: LICENSE_EXPRESSION.into(),
                attribution: ATTRIBUTION.into(),
                redistribution: REDISTRIBUTION.into(),
                source_url: "https://easylist.to/easylist/easylist.txt".into(),
            },
        };
        let previous_catalog = CatalogManifest {
            schema_version: 1,
            revision: 2,
            created_unix: 2,
            expires_unix: 3,
            sources: vec![source.clone()],
        };
        let previous_catalog_bytes = canonical_json(&previous_catalog).unwrap();
        let previous_seed = ReleaseSeedManifest {
            schema_version: 1,
            catalog_manifest_sha256: hex_sha256(&previous_catalog_bytes),
            assets: vec![ReleaseSeedAsset {
                target: "easylist.txt".into(),
                compression: CompressionFormat::Gzip,
                compressed_length: 7,
                compressed_sha256: "1".repeat(64),
                upstream_title: "EasyList".into(),
                upstream_version: "202607240002".into(),
                upstream_commit: "2".repeat(40),
            }],
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
    }
}
