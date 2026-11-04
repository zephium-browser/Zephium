use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs::File;
use std::io::Read as _;
use std::path::{Component, Path};
use std::process::Command;

pub(crate) const SHIPPING_FEATURE_SETS: [&str; 2] = [
    "full-regex-handling,css-validation,embedded-domain-resolver",
    "full-regex-handling,css-validation,content-blocking,embedded-domain-resolver",
];
pub(crate) const OPTIONAL_EXACT_FEATURES: &str =
    "full-regex-handling,css-validation,embedded-domain-resolver";

const UPSTREAM_COMMIT: &str = "00b19a06508ddd4f3453779f8618983421ddf32b";
const UPSTREAM_TREE: &str = "17f03b25091db8ce9ef88408e4be9d11f3e6f755";
const CRATE_ARCHIVE_SHA256: &str =
    "77420e48225975c472eaea1b7c767af6caebea02d910043b0af7a1271d47ec9c";
const SOURCE_MANIFEST_GIT_BLOB: &str = "720abe77fa0fedfef0859f9f5ce3499cebb7aa14";
const SOURCE_MANIFEST_SHA256: &str =
    "6204e3f481bfdebf263fc7c3eda963362e2353ba282919779f3e3ea8eba1b374";
const SUPPORT_GIT_INVENTORY_SHA256: &str =
    "8710e4f62d933f7dd25b6954a5dcfacbecbaa244e030bc1d3da0f1f265ac9142";
const SUPPORT_BYTE_INVENTORY_SHA256: &str =
    "fda4cd4c4a5f2c37ebd55fdd8713e3d5f788e48af23c72ff9a97c60e097e9ac6";
const CODE_GIT_INVENTORY_SHA256: &str =
    "c0f4eca4b0b0c0416ac8dedf4a0b40e4128b71f116ae8fc1b4616624f8fe28b6";
const CODE_BYTE_INVENTORY_SHA256: &str =
    "e9e789ef794ef226194194c5d5066b29d03611d368d7c252c4212c67129cbc8b";
const SUPPORT_FILE_COUNT: usize = 4;
const CODE_FILE_COUNT: usize = 73;

const CODE_ROOTS: [&str; 3] = ["src", "benches", "tests"];
const SUPPORT_PATHS: [&str; 4] = [".gitattributes", "LICENSE", "README.md", "rustfmt.toml"];
const FORK_ROOT_FILES: [&str; 11] = [
    ".gitattributes",
    "Cargo.lock",
    "Cargo.toml",
    "Cargo.toml.orig",
    "FORK.toml",
    "LICENSE",
    "README.md",
    "REBASE.md",
    "UPSTREAM.md",
    "UPSTREAM_FILES.toml",
    "rustfmt.toml",
];
const FORK_ROOT_DIRECTORIES: [&str; 3] = ["benches", "src", "tests"];
const FORK_MANIFEST_PATH: &str = "Cargo.toml";
const EXCLUDED_COMPONENTS: [&str; 11] = [
    ".github",
    ".gitignore",
    ".npmignore",
    "CHANGELOG.md",
    "SECURITY.md",
    "examples",
    "fuzz",
    "js",
    "npm package metadata",
    "rust-toolchain.toml",
    "all upstream data fixtures",
];
const PATCH_SET_NAMES: [&str; 9] = [
    "bounded prepared runtime matcher",
    "source-independent request construction",
    "conservative unknown-attribution matching",
    "serialized matcher compatibility boundary",
    "empty removeparam fast path",
    "fallible WebKit content-rule conversion",
    "strict request test compatibility",
    "fork regression tests",
    "fixture-free test surface",
];
const PATCH_PATHS: [&str; 21] = [
    "Cargo.toml",
    "src/blocker.rs",
    "src/content_blocking.rs",
    "src/data_format/mod.rs",
    "src/engine.rs",
    "src/filters/fb_network.rs",
    "src/filters/fb_network_builder.rs",
    "src/filters/network.rs",
    "src/filters/network_matchers.rs",
    "src/network_filter_list.rs",
    "src/regex_manager.rs",
    "src/request.rs",
    "src/url_parser/mod.rs",
    "tests/ublock-coverage.rs",
    "tests/test_utils.rs",
    "tests/unit/blocker.rs",
    "tests/unit/content_blocking.rs",
    "tests/unit/engine.rs",
    "tests/unit/filters/network_matchers.rs",
    "tests/unit/request.rs",
    "tests/unit/resources/resource_assembler.rs",
];

#[derive(Debug)]
struct UpstreamEntry {
    git_blob: String,
    sha256: String,
}

#[derive(Debug)]
struct ReviewedFork {
    support: BTreeMap<String, UpstreamEntry>,
    code: BTreeMap<String, UpstreamEntry>,
    patch_paths: BTreeSet<String>,
}

pub(crate) fn check(repository: &Path) -> Result<(), String> {
    let fork_root = repository.join("vendor/adblock");
    let fork_source = read_text(&fork_root.join("FORK.toml"))?;
    let inventory_source = read_text(&fork_root.join("UPSTREAM_FILES.toml"))?;
    let reviewed = validate_records(&fork_source, &inventory_source)?;

    validate_manifests(repository, &fork_root)?;
    verify_imported_files(&fork_root, &reviewed)?;
    verify_lock_alignment(repository)?;
    verify_blocker_product_lock(repository)?;
    Ok(())
}

fn read_text(path: &Path) -> Result<String, String> {
    std::fs::read_to_string(path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))
}

fn validate_records(fork_source: &str, inventory_source: &str) -> Result<ReviewedFork, String> {
    let fork = parse_table(fork_source, "FORK.toml")?;
    require_integer(&fork, "schema", 1)?;
    let upstream = require_table(&fork, "upstream")?;
    require_string(
        upstream,
        "repository",
        "https://github.com/brave/adblock-rust",
    )?;
    require_string(upstream, "tag", "v0.13.2")?;
    require_string(upstream, "version", "0.13.2")?;
    require_string(upstream, "commit", UPSTREAM_COMMIT)?;
    require_string(upstream, "tree", UPSTREAM_TREE)?;
    require_string(upstream, "license", "MPL-2.0")?;

    let import = require_table(&fork, "import")?;
    require_string(import, "source_manifest", "Cargo.toml.orig")?;
    require_string(import, "fork_manifest", FORK_MANIFEST_PATH)?;
    require_string(import, "fork_lockfile", "Cargo.lock")?;
    require_string(import, "application_lockfile", "../../Cargo.lock")?;
    require_string(import, "upstream_file_inventory", "UPSTREAM_FILES.toml")?;
    require_string_array(import, "code_paths", &CODE_ROOTS)?;
    require_string_array(import, "support_paths", &SUPPORT_PATHS)?;
    require_string_array(import, "excluded_upstream_components", &EXCLUDED_COMPONENTS)?;

    let production = require_table(&fork, "production_graphs")?;
    require_key_set(
        production,
        &[
            "default_features",
            "optional_exact_attribution",
            "test_threading",
            "verification",
            "webkit_native",
            "windows_runtime",
        ],
        "adblock production-graph table",
    )?;
    require_string(production, "windows_runtime", SHIPPING_FEATURE_SETS[0])?;
    require_string(production, "webkit_native", SHIPPING_FEATURE_SETS[1])?;
    require_string(
        production,
        "optional_exact_attribution",
        OPTIONAL_EXACT_FEATURES,
    )?;
    require_string(
        production,
        "default_features",
        "not used by Zephium production builds",
    )?;
    require_string(
        production,
        "test_threading",
        "upstream single-thread feature is disabled in every production-graph gate",
    )?;
    require_string(
        production,
        "verification",
        "cargo xtask check-security-fork-locks resolves exact desktop graphs for Windows, both macOS architectures, and Linux",
    )?;
    let lock_alignment = require_table(&fork, "lock_alignment")?;
    require_string(
        lock_alignment,
        "policy",
        "every registry package reachable from the union of Zephium production graphs must have the same version, source, and checksum in the standalone and application lockfiles",
    )?;
    require_string(
        lock_alignment,
        "standalone_dev_graph",
        "may contain additional independently pinned test-only packages",
    )?;

    validate_release_gates(require_table(&fork, "release_gates")?)?;

    let patch_sets = fork
        .get("patch_sets")
        .and_then(toml::Value::as_array)
        .ok_or_else(|| "FORK.toml patch_sets must be an array".to_owned())?;
    if patch_sets.len() != PATCH_SET_NAMES.len() {
        return Err(format!(
            "FORK.toml must contain exactly {} reviewed patch sets, found {}",
            PATCH_SET_NAMES.len(),
            patch_sets.len()
        ));
    }
    let mut patch_paths = BTreeSet::new();
    let mut names = BTreeSet::new();
    for (index, value) in patch_sets.iter().enumerate() {
        let patch = value
            .as_table()
            .ok_or_else(|| format!("FORK.toml patch set {index} is not a table"))?;
        let name = patch
            .get("name")
            .and_then(toml::Value::as_str)
            .filter(|name| !name.trim().is_empty())
            .ok_or_else(|| format!("FORK.toml patch set {index} has no name"))?;
        if name != PATCH_SET_NAMES[index] {
            return Err(format!(
                "FORK.toml patch set {index} is `{name}`, expected `{}`",
                PATCH_SET_NAMES[index]
            ));
        }
        if !names.insert(name) {
            return Err(format!("FORK.toml duplicates patch set `{name}`"));
        }
        let paths = string_array(patch, "paths")?;
        if paths.is_empty() {
            return Err(format!("FORK.toml patch set `{name}` has no paths"));
        }
        for path in paths {
            validate_relative_path(path)?;
            patch_paths.insert(path.to_owned());
        }
        let invariants = string_array(patch, "invariants")?;
        if invariants.is_empty() || invariants.iter().any(|value| value.trim().is_empty()) {
            return Err(format!(
                "FORK.toml patch set `{name}` must record non-empty invariants"
            ));
        }
    }
    let expected_patch_paths = PATCH_PATHS
        .iter()
        .map(|path| (*path).to_owned())
        .collect::<BTreeSet<_>>();
    if patch_paths != expected_patch_paths {
        return Err(format!(
            "FORK.toml patch path union differs from the reviewed inventory: {}",
            describe_set_difference(&expected_patch_paths, &patch_paths)
        ));
    }

    let inventory = parse_table(inventory_source, "UPSTREAM_FILES.toml")?;
    require_integer(&inventory, "schema", 1)?;
    require_string(&inventory, "upstream_commit", UPSTREAM_COMMIT)?;
    require_string(&inventory, "upstream_tree", UPSTREAM_TREE)?;
    require_string(&inventory, "crate_archive_sha256", CRATE_ARCHIVE_SHA256)?;
    require_string(
        &inventory,
        "source_manifest_git_blob",
        SOURCE_MANIFEST_GIT_BLOB,
    )?;
    require_string(&inventory, "source_manifest_sha256", SOURCE_MANIFEST_SHA256)?;
    require_integer(&inventory, "support_file_count", SUPPORT_FILE_COUNT as i64)?;
    require_integer(&inventory, "code_file_count", CODE_FILE_COUNT as i64)?;
    require_string(
        &inventory,
        "support_git_inventory_sha256",
        SUPPORT_GIT_INVENTORY_SHA256,
    )?;
    require_string(
        &inventory,
        "support_byte_inventory_sha256",
        SUPPORT_BYTE_INVENTORY_SHA256,
    )?;
    require_string(
        &inventory,
        "code_git_inventory_sha256",
        CODE_GIT_INVENTORY_SHA256,
    )?;
    require_string(
        &inventory,
        "code_byte_inventory_sha256",
        CODE_BYTE_INVENTORY_SHA256,
    )?;
    let support = parse_inventory_group(
        require_table(&inventory, "support")?,
        SUPPORT_FILE_COUNT,
        SUPPORT_GIT_INVENTORY_SHA256,
        SUPPORT_BYTE_INVENTORY_SHA256,
    )?;
    let code = parse_inventory_group(
        require_table(&inventory, "code")?,
        CODE_FILE_COUNT,
        CODE_GIT_INVENTORY_SHA256,
        CODE_BYTE_INVENTORY_SHA256,
    )?;
    let manifest_patch_paths = patch_paths
        .iter()
        .filter(|path| !code.contains_key(*path))
        .map(String::as_str)
        .collect::<Vec<_>>();
    if manifest_patch_paths != [FORK_MANIFEST_PATH] {
        return Err(format!(
            "FORK.toml manifest patch inventory is {manifest_patch_paths:?}, expected [{FORK_MANIFEST_PATH:?}]"
        ));
    }
    let imported_patch_paths = patch_paths
        .into_iter()
        .filter(|path| code.contains_key(path))
        .collect();
    Ok(ReviewedFork {
        support,
        code,
        patch_paths: imported_patch_paths,
    })
}

fn validate_release_gates(gates: &toml::Table) -> Result<(), String> {
    const GATES: [(&str, &str); 19] = [
        ("fork_provenance", "cargo xtask check-security-fork-locks"),
        (
            "fork_fmt",
            "cargo fmt --manifest-path vendor/adblock/Cargo.toml -- --check",
        ),
        (
            "fork_windows_check",
            "cargo check --manifest-path vendor/adblock/Cargo.toml --locked --lib --no-default-features --features full-regex-handling,css-validation,embedded-domain-resolver",
        ),
        (
            "fork_windows_clippy",
            "cargo clippy --manifest-path vendor/adblock/Cargo.toml --locked --lib --tests --no-default-features --features full-regex-handling,css-validation,embedded-domain-resolver -- -D warnings",
        ),
        (
            "fork_windows_tests",
            "cargo test --manifest-path vendor/adblock/Cargo.toml --locked --lib --test fork_contract --no-default-features --features full-regex-handling,css-validation,embedded-domain-resolver",
        ),
        (
            "fork_webkit_check",
            "cargo check --manifest-path vendor/adblock/Cargo.toml --locked --lib --no-default-features --features full-regex-handling,css-validation,content-blocking,embedded-domain-resolver",
        ),
        (
            "fork_webkit_clippy",
            "cargo clippy --manifest-path vendor/adblock/Cargo.toml --locked --lib --tests --no-default-features --features full-regex-handling,css-validation,content-blocking,embedded-domain-resolver -- -D warnings",
        ),
        (
            "fork_webkit_tests",
            "cargo test --manifest-path vendor/adblock/Cargo.toml --locked --lib --test fork_contract --no-default-features --features full-regex-handling,css-validation,content-blocking,embedded-domain-resolver",
        ),
        (
            "fork_optional_exact_check",
            "cargo check --manifest-path vendor/adblock/Cargo.toml --locked --lib --no-default-features --features full-regex-handling,css-validation,embedded-domain-resolver",
        ),
        (
            "fork_optional_exact_clippy",
            "cargo clippy --manifest-path vendor/adblock/Cargo.toml --locked --lib --tests --no-default-features --features full-regex-handling,css-validation,embedded-domain-resolver -- -D warnings",
        ),
        (
            "fork_optional_exact_tests",
            "cargo test --manifest-path vendor/adblock/Cargo.toml --locked --lib --test fork_contract --no-default-features --features full-regex-handling,css-validation,embedded-domain-resolver",
        ),
        (
            "fork_upstream_compatibility_tests",
            "cargo test --manifest-path vendor/adblock/Cargo.toml --locked --lib --test fork_contract",
        ),
        (
            "fork_dependency_policy",
            "cargo deny --manifest-path vendor/adblock/Cargo.toml --config deny.toml --locked check -A advisory-not-detected",
        ),
        ("workspace_fmt", "cargo fmt --all -- --check"),
        (
            "workspace_check",
            "cargo check --workspace --locked --all-targets",
        ),
        (
            "workspace_clippy",
            "cargo clippy --workspace --locked --all-targets -- -D warnings",
        ),
        (
            "adapter_tests",
            "check, test, and Clippy each runtime-only, runtime-exact, WebKit-only, runtime+WebKit, and runtime-exact+WebKit zephium-blocker graph",
        ),
        (
            "feature_matrix",
            "runtime; runtime-exact; webkit; runtime,webkit; runtime-exact,webkit",
        ),
        (
            "differential_tests",
            "the explicit fork_contract target runs self-authored exact-attribution, unknown-attribution, bounded-matcher, and serialization vectors under every reviewed graph",
        ),
    ];
    require_key_set(
        gates,
        &GATES.map(|(key, _)| key),
        "adblock release-gate table",
    )?;
    for (key, expected) in GATES {
        require_string(gates, key, expected)?;
    }
    Ok(())
}

fn parse_inventory_group(
    table: &toml::Table,
    expected_count: usize,
    expected_git_digest: &str,
    expected_byte_digest: &str,
) -> Result<BTreeMap<String, UpstreamEntry>, String> {
    let git_rows = parse_hash_rows(table, "files", 40)?;
    let sha_rows = parse_hash_rows(table, "sha256", 64)?;
    if git_rows.len() != expected_count || sha_rows.len() != expected_count {
        return Err(format!(
            "upstream inventory expected {expected_count} entries, found {} Git rows and {} SHA-256 rows",
            git_rows.len(),
            sha_rows.len()
        ));
    }
    if digest_rows(&git_rows) != expected_git_digest {
        return Err("upstream Git inventory aggregate digest is not reviewed".to_owned());
    }
    if digest_rows(&sha_rows) != expected_byte_digest {
        return Err("upstream byte inventory aggregate digest is not reviewed".to_owned());
    }

    let git = git_rows.into_iter().collect::<BTreeMap<_, _>>();
    let sha = sha_rows.into_iter().collect::<BTreeMap<_, _>>();
    if git.keys().ne(sha.keys()) {
        return Err("Git and SHA-256 upstream inventories name different paths".to_owned());
    }
    Ok(git
        .into_iter()
        .map(|(path, git_blob)| {
            let sha256 = sha
                .get(&path)
                .expect("key equality was established")
                .to_owned();
            (path, UpstreamEntry { git_blob, sha256 })
        })
        .collect())
}

fn parse_hash_rows(
    table: &toml::Table,
    key: &str,
    hash_len: usize,
) -> Result<Vec<(String, String)>, String> {
    let values = table
        .get(key)
        .and_then(toml::Value::as_array)
        .ok_or_else(|| format!("upstream inventory `{key}` must be an array"))?;
    let mut rows = Vec::with_capacity(values.len());
    let mut previous_path: Option<String> = None;
    for value in values {
        let row = value
            .as_str()
            .ok_or_else(|| format!("upstream inventory `{key}` contains a non-string row"))?;
        let (hash, path) = row
            .split_once("  ")
            .ok_or_else(|| format!("invalid `{key}` inventory row `{row}`"))?;
        if hash.len() != hash_len
            || !hash
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(format!("invalid `{key}` hash for `{path}`"));
        }
        validate_relative_path(path)?;
        if previous_path
            .as_deref()
            .is_some_and(|previous| previous >= path)
        {
            return Err(format!(
                "upstream inventory `{key}` paths are duplicated or not bytewise sorted at `{path}`"
            ));
        }
        previous_path = Some(path.to_owned());
        rows.push((path.to_owned(), hash.to_owned()));
    }
    Ok(rows)
}

fn digest_rows(rows: &[(String, String)]) -> String {
    let mut hasher = Sha256::new();
    for (path, hash) in rows {
        hasher.update(hash.as_bytes());
        hasher.update(b"  ");
        hasher.update(path.as_bytes());
        hasher.update(b"\n");
    }
    hex(&hasher.finalize())
}

fn validate_manifests(repository: &Path, fork_root: &Path) -> Result<(), String> {
    validate_fork_root_layout(fork_root)?;

    let source_manifest = fork_root.join("Cargo.toml.orig");
    verify_regular_file(&source_manifest)?;
    let source_sha = sha256_file(&source_manifest)?;
    let source_blob = git_blob(&source_manifest)?;
    if source_sha != SOURCE_MANIFEST_SHA256 || source_blob != SOURCE_MANIFEST_GIT_BLOB {
        return Err("Cargo.toml.orig is not the recorded upstream source manifest".to_owned());
    }

    let manifest = parse_table(
        &read_text(&fork_root.join("Cargo.toml"))?,
        "adblock Cargo.toml",
    )?;
    require_key_set(
        &manifest,
        &[
            "bench",
            "dependencies",
            "dev-dependencies",
            "features",
            "lib",
            "lints",
            "package",
            "test",
        ],
        "adblock manifest",
    )?;
    let package = require_table(&manifest, "package")?;
    require_key_set(
        package,
        &[
            "authors",
            "autobenches",
            "autobins",
            "autoexamples",
            "autolib",
            "autotests",
            "build",
            "description",
            "edition",
            "exclude",
            "license",
            "name",
            "readme",
            "repository",
            "version",
        ],
        "adblock package",
    )?;
    require_string(package, "name", "adblock")?;
    require_string(package, "version", "0.13.2")?;
    require_string(package, "edition", "2024")?;
    require_string(package, "license", "MPL-2.0")?;
    require_string(
        package,
        "repository",
        "https://github.com/brave/adblock-rust/",
    )?;
    for key in [
        "build",
        "autolib",
        "autobins",
        "autoexamples",
        "autotests",
        "autobenches",
    ] {
        require_bool(package, key, false)?;
    }

    let library = require_table(&manifest, "lib")?;
    require_key_set(
        library,
        &["bench", "name", "path"],
        "adblock library target",
    )?;
    require_string(library, "name", "adblock")?;
    require_string(library, "path", "src/lib.rs")?;
    require_bool(library, "bench", false)?;
    validate_test_targets(&manifest)?;
    validate_bench_targets(&manifest)?;

    let features = require_table(&manifest, "features")?;
    require_key_set(
        features,
        &[
            "content-blocking",
            "css-validation",
            "debug-info",
            "default",
            "embedded-domain-resolver",
            "full-regex-handling",
            "resource-assembler",
            "single-thread",
        ],
        "adblock feature table",
    )?;
    require_string_array(
        features,
        "default",
        &[
            "embedded-domain-resolver",
            "full-regex-handling",
            "single-thread",
        ],
    )?;
    require_string_array(features, "content-blocking", &[])?;
    require_string_array(features, "full-regex-handling", &[])?;
    require_string_array(features, "embedded-domain-resolver", &["addr"])?;
    require_string_array(features, "single-thread", &[])?;
    require_string_array(features, "css-validation", &["cssparser", "selectors"])?;
    require_string_array(features, "debug-info", &[])?;
    require_string_array(features, "resource-assembler", &[])?;

    let lints = require_table(&manifest, "lints")?;
    require_key_set(lints, &["clippy"], "adblock lint table")?;
    let clippy_lints = require_table(lints, "clippy")?;
    require_key_set(
        clippy_lints,
        &["len_zero", "uninlined_format_args"],
        "adblock Clippy lint table",
    )?;
    require_string(clippy_lints, "len_zero", "allow")?;
    require_string(clippy_lints, "uninlined_format_args", "warn")?;

    let dependencies = require_table(&manifest, "dependencies")?;
    require_key_set(
        dependencies,
        &[
            "addr",
            "arrayvec",
            "base64",
            "bitflags",
            "cssparser",
            "flatbuffers",
            "idna",
            "itertools",
            "memchr",
            "percent-encoding",
            "precomputed-hash",
            "regex",
            "rustc-hash",
            "seahash",
            "selectors",
            "serde",
            "serde_json",
            "thiserror",
            "url",
        ],
        "adblock dependency table",
    )?;
    validate_dependency_sources(dependencies, "dependency")?;

    let dev_dependencies = require_table(&manifest, "dev-dependencies")?;
    require_key_set(
        dev_dependencies,
        &["addr", "criterion", "mock_instant", "sha2"],
        "adblock dev-dependency table",
    )?;
    validate_dependency_sources(dev_dependencies, "dev-dependency")?;
    let test_resolver = dev_dependencies
        .get("addr")
        .and_then(toml::Value::as_table)
        .ok_or_else(|| "adblock manifest must pin the no-resolver test harness".to_owned())?;
    require_string(test_resolver, "version", "=0.15.6")?;
    if test_resolver
        .get("default-features")
        .and_then(toml::Value::as_bool)
        != Some(false)
    {
        return Err("test-only addr must disable default features".to_owned());
    }
    require_string_array(test_resolver, "features", &["psl"])?;

    let root = parse_table(
        &read_text(&repository.join("Cargo.toml"))?,
        "root Cargo.toml",
    )?;
    let workspace = require_table(&root, "workspace")?;
    if !string_array(workspace, "exclude")?.contains(&"vendor/adblock") {
        return Err("root workspace must exclude vendor/adblock".to_owned());
    }
    let patch = root
        .get("patch")
        .and_then(toml::Value::as_table)
        .and_then(|patch| patch.get("crates-io"))
        .and_then(toml::Value::as_table)
        .and_then(|registry| registry.get("adblock"))
        .and_then(toml::Value::as_table)
        .ok_or_else(|| "root manifest does not patch crates.io adblock".to_owned())?;
    require_key_set(patch, &["path"], "root adblock patch")?;
    require_string(patch, "path", "vendor/adblock")?;
    validate_zephium_feature_manifests(repository, &root)?;
    Ok(())
}

fn validate_zephium_feature_manifests(repository: &Path, root: &toml::Table) -> Result<(), String> {
    let workspace_dependencies = require_table(root, "workspace")?
        .get("dependencies")
        .and_then(toml::Value::as_table)
        .ok_or_else(|| "root workspace dependency table is missing".to_owned())?;
    reject_dependency_aliases(
        workspace_dependencies,
        "root workspace dependency table",
        &["adblock"],
        &[
            "zephium-blocker",
            "zephium-blocker-service",
            "zephium-blocker-update",
            "zephium-update-transport",
        ],
    )?;
    let blocker_workspace = workspace_dependencies
        .get("zephium-blocker")
        .and_then(toml::Value::as_table)
        .ok_or_else(|| "root workspace has no structured zephium-blocker dependency".to_owned())?;
    require_key_set(
        blocker_workspace,
        &["default-features", "path", "version"],
        "root zephium-blocker dependency",
    )?;
    require_string(blocker_workspace, "path", "crates/zephium-blocker")?;
    require_string(blocker_workspace, "version", "=0.1.0")?;
    require_bool(blocker_workspace, "default-features", false)?;
    for (name, path) in [
        ("zephium-blocker-service", "crates/zephium-blocker-service"),
        ("zephium-blocker-update", "crates/zephium-blocker-update"),
        (
            "zephium-update-transport",
            "crates/zephium-update-transport",
        ),
    ] {
        let dependency = workspace_dependencies
            .get(name)
            .and_then(toml::Value::as_table)
            .ok_or_else(|| format!("root workspace has no structured {name} dependency"))?;
        require_key_set(
            dependency,
            &["default-features", "path", "version"],
            &format!("root {name} dependency"),
        )?;
        require_string(dependency, "path", path)?;
        require_string(dependency, "version", "=0.1.0")?;
        require_bool(dependency, "default-features", false)?;
    }
    let tough = workspace_dependencies
        .get("tough")
        .and_then(toml::Value::as_table)
        .ok_or_else(|| "root workspace has no structured tough dependency".to_owned())?;
    require_key_set(
        tough,
        &["default-features", "version"],
        "root tough dependency",
    )?;
    require_string(tough, "version", "=0.24.0")?;
    require_bool(tough, "default-features", false)?;

    let blocker = parse_table(
        &read_text(&repository.join("crates/zephium-blocker/Cargo.toml"))?,
        "zephium-blocker Cargo.toml",
    )?;
    let blocker_features = require_table(&blocker, "features")?;
    require_key_set(
        blocker_features,
        &["default", "runtime", "runtime-exact", "webkit"],
        "zephium-blocker feature table",
    )?;
    require_string_array(
        blocker_features,
        "default",
        &["runtime", "runtime-exact", "webkit"],
    )?;
    require_string_array(blocker_features, "runtime", &[])?;
    require_string_array(
        blocker_features,
        "runtime-exact",
        &["runtime", "adblock/embedded-domain-resolver"],
    )?;
    require_string_array(blocker_features, "webkit", &["adblock/content-blocking"])?;
    let blocker_adblock = require_table(&blocker, "dependencies")?
        .get("adblock")
        .and_then(toml::Value::as_table)
        .ok_or_else(|| "zephium-blocker has no structured adblock dependency".to_owned())?;
    require_key_set(
        blocker_adblock,
        &["default-features", "features", "version"],
        "zephium-blocker adblock dependency",
    )?;
    require_string(blocker_adblock, "version", "=0.13.2")?;
    require_bool(blocker_adblock, "default-features", false)?;
    require_string_array(
        blocker_adblock,
        "features",
        &[
            "full-regex-handling",
            "css-validation",
            "embedded-domain-resolver",
        ],
    )?;
    reject_dependency_aliases(
        require_table(&blocker, "dependencies")?,
        "zephium-blocker dependency table",
        &[],
        &["adblock"],
    )?;
    validate_blocker_update_manifest(repository)?;
    validate_update_transport_manifest(repository)?;
    validate_private_fs_validation_manifest(repository)?;
    validate_blocker_service_manifest(repository)?;
    validate_blocker_fuzz_manifest(repository)?;

    let desktop = parse_table(
        &read_text(&repository.join("desktop/Cargo.toml"))?,
        "desktop Cargo.toml",
    )?;
    for kind in ["dependencies", "build-dependencies", "dev-dependencies"] {
        if let Some(dependencies) = desktop.get(kind).and_then(toml::Value::as_table) {
            let allowed = if kind == "dependencies" {
                ["zephium-blocker-service"].as_slice()
            } else {
                [].as_slice()
            };
            reject_dependency_aliases(
                dependencies,
                &format!("desktop root {kind}"),
                &["adblock"],
                allowed,
            )?;
            reject_blocker_packages_except(dependencies, &format!("desktop root {kind}"), allowed)?;
        }
    }
    let desktop_dependencies = require_table(&desktop, "dependencies")?;
    let service = desktop_dependencies
        .get("zephium-blocker-service")
        .and_then(toml::Value::as_table)
        .ok_or_else(|| "desktop has no structured zephium-blocker-service dependency".to_owned())?;
    require_key_set(
        service,
        &["features", "workspace"],
        "desktop zephium-blocker-service dependency",
    )?;
    require_bool(service, "workspace", true)?;
    require_string_array(service, "features", &["official-https"])?;
    let targets = require_table(&desktop, "target")?;
    let reviewed_targets = BTreeSet::from([
        "cfg(target_os = \"windows\")",
        "cfg(all(unix, not(target_os = \"macos\")))",
        "cfg(target_os = \"macos\")",
    ]);
    for (target, target_table) in targets {
        let target_table = target_table
            .as_table()
            .ok_or_else(|| format!("desktop target `{target}` is not a table"))?;
        for kind in ["dependencies", "build-dependencies", "dev-dependencies"] {
            let Some(dependencies) = target_table.get(kind).and_then(toml::Value::as_table) else {
                continue;
            };
            for (name, value) in dependencies {
                let package = dependency_package(name, value);
                if matches!(
                    package,
                    "adblock"
                        | "zephium-blocker-service"
                        | "zephium-blocker-update"
                        | "zephium-update-transport"
                ) || (package == "zephium-blocker"
                    && (!reviewed_targets.contains(target.as_str())
                        || kind != "dependencies"
                        || name != "zephium-blocker"))
                {
                    return Err(format!(
                        "desktop target `{target}` {kind} contains unreviewed `{name}` -> `{package}`"
                    ));
                }
            }
        }
    }
    for (target, expected_features) in [
        (
            "cfg(target_os = \"windows\")",
            ["runtime", "runtime-exact"].as_slice(),
        ),
        (
            "cfg(all(unix, not(target_os = \"macos\")))",
            ["webkit"].as_slice(),
        ),
        ("cfg(target_os = \"macos\")", ["webkit"].as_slice()),
    ] {
        let dependency = targets
            .get(target)
            .and_then(toml::Value::as_table)
            .and_then(|table| table.get("dependencies"))
            .and_then(toml::Value::as_table)
            .and_then(|dependencies| dependencies.get("zephium-blocker"))
            .and_then(toml::Value::as_table)
            .ok_or_else(|| {
                format!("desktop target `{target}` has no structured zephium-blocker dependency")
            })?;
        require_key_set(
            dependency,
            &["features", "workspace"],
            &format!("desktop `{target}` zephium-blocker dependency"),
        )?;
        require_bool(dependency, "workspace", true)?;
        require_string_array(dependency, "features", expected_features)?;
    }
    Ok(())
}

fn validate_blocker_update_manifest(repository: &Path) -> Result<(), String> {
    let update = parse_table(
        &read_text(&repository.join("crates/zephium-blocker-update/Cargo.toml"))?,
        "zephium-blocker-update Cargo.toml",
    )?;
    let package = require_table(&update, "package")?;
    require_string(package, "name", "zephium-blocker-update")?;
    let features = require_table(&update, "features")?;
    require_key_set(
        features,
        &["default", "official-https", "tuf"],
        "zephium-blocker-update feature table",
    )?;
    require_string_array(features, "default", &[])?;
    require_string_array(
        features,
        "tuf",
        &[
            "dep:async-trait",
            "dep:rustix",
            "dep:tokio",
            "dep:tough",
            "dep:windows",
            "dep:zephium-update-transport",
            "zephium-update-transport/tough",
        ],
    )?;

    require_string_array(
        features,
        "official-https",
        &[
            "dep:rustix",
            "dep:tokio",
            "dep:windows",
            "dep:zephium-update-transport",
        ],
    )?;
    let dependencies = require_table(&update, "dependencies")?;
    require_key_set(
        dependencies,
        &[
            "async-trait",
            "serde",
            "serde_json",
            "thiserror",
            "tokio",
            "tough",
            "url",
            "zephium-update-transport",
        ],
        "zephium-blocker-update dependency table",
    )?;
    let async_trait = require_dependency_table(dependencies, "async-trait")?;
    require_key_set(
        async_trait,
        &["optional", "version"],
        "zephium-blocker-update async-trait dependency",
    )?;
    require_string(async_trait, "version", "=0.1.89")?;
    require_bool(async_trait, "optional", true)?;
    for name in ["serde", "serde_json", "thiserror", "url"] {
        require_workspace_dependency(dependencies, name, &[])?;
    }
    let tough = require_dependency_table(dependencies, "tough")?;
    require_key_set(
        tough,
        &["optional", "workspace"],
        "zephium-blocker-update tough dependency",
    )?;
    require_bool(tough, "workspace", true)?;
    require_bool(tough, "optional", true)?;
    let transport = require_dependency_table(dependencies, "zephium-update-transport")?;
    require_key_set(
        transport,
        &["optional", "workspace"],
        "zephium-blocker-update transport dependency",
    )?;
    require_bool(transport, "workspace", true)?;
    require_bool(transport, "optional", true)?;
    let tokio = require_dependency_table(dependencies, "tokio")?;
    require_key_set(
        tokio,
        &["features", "optional", "workspace"],
        "zephium-blocker-update tokio dependency",
    )?;
    require_bool(tokio, "workspace", true)?;
    require_string_array(tokio, "features", &["fs", "io-util", "rt", "time"])?;
    require_bool(tokio, "optional", true)?;

    let dev_dependencies = require_table(&update, "dev-dependencies")?;
    require_key_set(
        dev_dependencies,
        &["aws-lc-rs", "jiff", "tempfile", "zephium-core"],
        "zephium-blocker-update dev-dependency table",
    )?;
    require_dependency_version(dev_dependencies, "aws-lc-rs", "=1.18.1")?;
    require_dependency_version(dev_dependencies, "jiff", "0.2")?;
    require_workspace_dependency(dev_dependencies, "tempfile", &[])?;

    let targets = require_table(&update, "target")?;
    require_key_set(
        targets,
        &[
            "cfg(not(target_os = \"windows\"))",
            "cfg(target_os = \"windows\")",
            "cfg(unix)",
        ],
        "zephium-blocker-update target table",
    )?;
    validate_target_dependency(
        targets,
        "cfg(not(target_os = \"windows\"))",
        "zephium-blocker",
        &["webkit"],
    )?;
    validate_target_dependency(
        targets,
        "cfg(target_os = \"windows\")",
        "zephium-blocker",
        &["runtime"],
    )?;
    let unix = target_dependencies(targets, "cfg(unix)")?;
    require_key_set(
        unix,
        &["rustix"],
        "zephium-blocker-update Unix dependency table",
    )?;
    let rustix = require_dependency_table(unix, "rustix")?;
    require_key_set(
        rustix,
        &["features", "optional", "version"],
        "zephium-blocker-update rustix dependency",
    )?;
    require_string(rustix, "version", "=1.1.4")?;
    require_string_array(rustix, "features", &["fs", "process"])?;
    require_bool(rustix, "optional", true)?;

    let windows = target_dependencies(targets, "cfg(target_os = \"windows\")")?;
    require_key_set(
        windows,
        &["windows", "zephium-blocker"],
        "zephium-blocker-update Windows dependency table",
    )?;
    let windows_crate = require_dependency_table(windows, "windows")?;
    require_key_set(
        windows_crate,
        &["features", "optional", "version"],
        "zephium-blocker-update windows dependency",
    )?;
    require_string(windows_crate, "version", "=0.61.3")?;
    require_string_array(
        windows_crate,
        "features",
        &[
            "Win32_Foundation",
            "Win32_Storage_FileSystem",
            "Win32_System_IO",
        ],
    )?;
    require_bool(windows_crate, "optional", true)?;
    Ok(())
}

fn validate_update_transport_manifest(repository: &Path) -> Result<(), String> {
    let transport = parse_table(
        &read_text(&repository.join("crates/zephium-update-transport/Cargo.toml"))?,
        "zephium-update-transport Cargo.toml",
    )?;
    require_string(
        require_table(&transport, "package")?,
        "name",
        "zephium-update-transport",
    )?;
    let features = require_table(&transport, "features")?;
    require_key_set(
        features,
        &["default", "tough"],
        "zephium-update-transport feature table",
    )?;
    require_string_array(features, "default", &[])?;
    require_string_array(features, "tough", &["dep:async-trait", "dep:tough"])?;
    let dependencies = require_table(&transport, "dependencies")?;
    require_key_set(
        dependencies,
        &[
            "async-trait",
            "futures-util",
            "reqwest",
            "thiserror",
            "time",
            "tough",
            "url",
        ],
        "zephium-update-transport dependency table",
    )?;
    let time = require_dependency_table(dependencies, "time")?;
    require_key_set(
        time,
        &["default-features", "features", "version"],
        "transport time dependency",
    )?;
    require_string(time, "version", "=0.3.49")?;
    require_bool(time, "default-features", false)?;
    require_string_array(time, "features", &["parsing"])?;
    let async_trait = require_dependency_table(dependencies, "async-trait")?;
    require_key_set(
        async_trait,
        &["optional", "version"],
        "zephium-update-transport async-trait dependency",
    )?;
    require_string(async_trait, "version", "=0.1.89")?;
    require_bool(async_trait, "optional", true)?;
    let futures = require_dependency_table(dependencies, "futures-util")?;
    require_key_set(
        futures,
        &["default-features", "features", "version"],
        "zephium-update-transport futures-util dependency",
    )?;
    require_string(futures, "version", "=0.3.32")?;
    require_bool(futures, "default-features", false)?;
    require_string_array(futures, "features", &["std"])?;
    let reqwest = require_dependency_table(dependencies, "reqwest")?;
    require_key_set(
        reqwest,
        &["default-features", "features", "version"],
        "zephium-update-transport reqwest dependency",
    )?;
    require_string(reqwest, "version", "=0.13.4")?;
    require_bool(reqwest, "default-features", false)?;
    require_string_array(reqwest, "features", &["rustls", "stream", "system-proxy"])?;
    for name in ["thiserror", "url"] {
        require_workspace_dependency(dependencies, name, &[])?;
    }
    let tough = require_dependency_table(dependencies, "tough")?;
    require_key_set(
        tough,
        &["optional", "workspace"],
        "zephium-update-transport tough dependency",
    )?;
    require_bool(tough, "workspace", true)?;
    require_bool(tough, "optional", true)?;
    Ok(())
}

fn validate_private_fs_validation_manifest(repository: &Path) -> Result<(), String> {
    let manifest = parse_table(
        &read_text(&repository.join("crates/zephium-private-fs/Cargo.toml"))?,
        "zephium-private-fs Cargo.toml",
    )?;
    let features = require_table(&manifest, "features")?;
    require_key_set(
        features,
        &["windows-namespace-validation", "windows-work-test-fixtures"],
        "private filesystem validation feature table",
    )?;
    // There is deliberately no default feature or production forwarding alias.
    // The crate independently rejects optimized builds with this gate enabled.
    require_string_array(features, "windows-namespace-validation", &[])?;
    require_string_array(features, "windows-work-test-fixtures", &[])
}

fn validate_blocker_service_manifest(repository: &Path) -> Result<(), String> {
    let service = parse_table(
        &read_text(&repository.join("crates/zephium-blocker-service/Cargo.toml"))?,
        "zephium-blocker-service Cargo.toml",
    )?;
    require_string(
        require_table(&service, "package")?,
        "name",
        "zephium-blocker-service",
    )?;
    let features = require_table(&service, "features")?;
    require_key_set(
        features,
        &["default", "official-https", "tuf"],
        "zephium-blocker-service feature table",
    )?;
    require_string_array(features, "default", &[])?;
    require_string_array(features, "tuf", &["zephium-blocker-update/tuf"])?;
    require_string_array(
        features,
        "official-https",
        &["zephium-blocker-update/official-https"],
    )?;
    let dependencies = require_table(&service, "dependencies")?;
    require_key_set(
        dependencies,
        &[
            "flate2",
            "ring",
            "serde",
            "serde_json",
            "thiserror",
            "zephium-blocker",
            "zephium-blocker-update",
            "zephium-core",
        ],
        "zephium-blocker-service dependency table",
    )?;
    require_dependency_version(dependencies, "flate2", "=1.1.9")?;
    for name in [
        "ring",
        "serde",
        "serde_json",
        "thiserror",
        "zephium-blocker",
        "zephium-blocker-update",
        "zephium-core",
    ] {
        require_workspace_dependency(dependencies, name, &[])?;
    }
    let dev_dependencies = require_table(&service, "dev-dependencies")?;
    require_key_set(
        dev_dependencies,
        &["tempfile"],
        "zephium-blocker-service dev-dependency table",
    )?;
    require_workspace_dependency(dev_dependencies, "tempfile", &[])
}

fn validate_blocker_fuzz_manifest(repository: &Path) -> Result<(), String> {
    let root = repository.join("crates/zephium-blocker/fuzz");
    validate_blocker_fuzz_layout(&root)?;
    validate_blocker_fuzz_deny(&root)?;
    let fuzz = parse_table(
        &read_text(&root.join("Cargo.toml"))?,
        "zephium-blocker fuzz Cargo.toml",
    )?;
    let package = require_table(&fuzz, "package")?;
    require_string(package, "name", "zephium-blocker-fuzz")?;
    require_string(package, "version", "0.0.0")?;
    require_string(package, "edition", "2021")?;
    require_string(package, "license", "MPL-2.0")?;
    require_bool(package, "publish", false)?;
    let metadata = require_table(package, "metadata")?;
    require_bool(metadata, "cargo-fuzz", true)?;

    let dependencies = require_table(&fuzz, "dependencies")?;
    require_key_set(
        dependencies,
        &["libfuzzer-sys", "zephium-blocker"],
        "zephium-blocker fuzz dependency table",
    )?;
    require_dependency_version(dependencies, "libfuzzer-sys", "=0.4.13")?;
    let blocker = require_dependency_table(dependencies, "zephium-blocker")?;
    require_key_set(
        blocker,
        &["default-features", "features", "path", "version"],
        "zephium-blocker fuzz blocker dependency",
    )?;
    require_string(blocker, "path", "..")?;
    require_string(blocker, "version", "=0.1.0")?;
    require_bool(blocker, "default-features", false)?;
    require_string_array(blocker, "features", &["runtime", "runtime-exact", "webkit"])?;

    require_string_array(require_table(&fuzz, "workspace")?, "members", &["."])?;
    let patch = fuzz
        .get("patch")
        .and_then(toml::Value::as_table)
        .and_then(|patch| patch.get("crates-io"))
        .and_then(toml::Value::as_table)
        .and_then(|registry| registry.get("adblock"))
        .and_then(toml::Value::as_table)
        .ok_or_else(|| {
            "blocker fuzz manifest does not patch the reviewed adblock fork".to_owned()
        })?;
    require_key_set(patch, &["path"], "blocker fuzz adblock patch")?;
    require_string(patch, "path", "../../../vendor/adblock")?;

    const TARGETS: [(&str, &str); 3] = [
        ("source_admission", "fuzz_targets/source_admission.rs"),
        ("request_match", "fuzz_targets/request_match.rs"),
        ("webkit_canonical", "fuzz_targets/webkit_canonical.rs"),
    ];
    let bins = fuzz
        .get("bin")
        .and_then(toml::Value::as_array)
        .ok_or_else(|| "blocker fuzz manifest has no binary targets".to_owned())?;
    if bins.len() != TARGETS.len() {
        return Err(format!(
            "blocker fuzz manifest has {} targets, expected {}",
            bins.len(),
            TARGETS.len()
        ));
    }
    for (index, ((name, path), value)) in TARGETS.iter().zip(bins).enumerate() {
        let bin = value
            .as_table()
            .ok_or_else(|| format!("blocker fuzz target {index} is not a table"))?;
        require_key_set(
            bin,
            &["bench", "doc", "name", "path", "test"],
            &format!("blocker fuzz target {index}"),
        )?;
        require_string(bin, "name", name)?;
        require_string(bin, "path", path)?;
        for key in ["bench", "doc", "test"] {
            require_bool(bin, key, false)?;
        }
    }

    let packages = parse_lock(&read_text(&root.join("Cargo.lock"))?)?;
    for (name, version) in [
        ("adblock", "0.13.2"),
        ("zephium-blocker", "0.1.0"),
        ("zephium-blocker-fuzz", "0.0.0"),
    ] {
        let package = require_single_lock_package(&packages, name, version)?;
        if package.contains_key("source") || package.contains_key("checksum") {
            return Err(format!(
                "blocker fuzz lock `{name} {version}` is not a reviewed local package"
            ));
        }
    }
    let libfuzzer = require_single_lock_package(&packages, "libfuzzer-sys", "0.4.13")?;
    if libfuzzer.get("source").and_then(toml::Value::as_str)
        != Some("registry+https://github.com/rust-lang/crates.io-index")
        || libfuzzer
            .get("checksum")
            .and_then(toml::Value::as_str)
            .is_none_or(|checksum| {
                checksum.len() != 64
                    || !checksum
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            })
    {
        return Err("blocker fuzz lock has an unreviewed libfuzzer-sys identity".to_owned());
    }
    Ok(())
}

fn validate_blocker_fuzz_deny(root: &Path) -> Result<(), String> {
    validate_blocker_fuzz_deny_source(&read_text(&root.join("deny.toml"))?)
}

fn validate_blocker_fuzz_deny_source(source: &str) -> Result<(), String> {
    let policy = parse_table(source, "zephium-blocker fuzz deny.toml")?;
    require_key_set(
        &policy,
        &["advisories", "bans", "graph", "licenses", "sources"],
        "zephium-blocker fuzz deny policy",
    )?;

    let graph = require_table(&policy, "graph")?;
    require_key_set(graph, &["all-features"], "blocker fuzz deny graph")?;
    require_bool(graph, "all-features", true)?;

    let advisories = require_table(&policy, "advisories")?;
    require_key_set(
        advisories,
        &[
            "ignore",
            "unmaintained",
            "unsound",
            "unused-ignored-advisory",
            "version",
            "yanked",
        ],
        "blocker fuzz deny advisories",
    )?;
    require_integer(advisories, "version", 2)?;
    require_string(advisories, "yanked", "deny")?;
    require_string(advisories, "unmaintained", "all")?;
    require_string(advisories, "unsound", "all")?;
    require_string(advisories, "unused-ignored-advisory", "deny")?;
    require_string_array(advisories, "ignore", &[])?;

    let licenses = require_table(&policy, "licenses")?;
    require_key_set(
        licenses,
        &["allow", "version"],
        "blocker fuzz deny licenses",
    )?;
    require_integer(licenses, "version", 2)?;
    require_string_array(
        licenses,
        "allow",
        &[
            "Apache-2.0",
            "BSD-2-Clause",
            "ISC",
            "MIT",
            "MPL-2.0",
            "NCSA",
            "Unicode-3.0",
        ],
    )?;

    let bans = require_table(&policy, "bans")?;
    require_key_set(
        bans,
        &["multiple-versions", "wildcards"],
        "blocker fuzz deny bans",
    )?;
    require_string(bans, "multiple-versions", "allow")?;
    require_string(bans, "wildcards", "deny")?;

    let sources = require_table(&policy, "sources")?;
    require_key_set(
        sources,
        &["required-git-spec", "unknown-git", "unknown-registry"],
        "blocker fuzz deny sources",
    )?;
    require_string(sources, "unknown-registry", "deny")?;
    require_string(sources, "unknown-git", "deny")?;
    require_string(sources, "required-git-spec", "rev")
}

fn validate_blocker_fuzz_layout(root: &Path) -> Result<(), String> {
    const FILES: [&str; 4] = ["Cargo.lock", "Cargo.toml", "README.md", "deny.toml"];
    const DIRECTORIES: [&str; 2] = ["corpus", "fuzz_targets"];
    const TARGETS: [&str; 3] = [
        "request_match.rs",
        "source_admission.rs",
        "webkit_canonical.rs",
    ];
    const CORPUS: [(&str, &str); 3] = [
        (
            "request_match/basic-invalid-path",
            "eb89658e8cc6820f918c7958f03ad8d69df19fba5c94f1a31583488da4e6c36c",
        ),
        (
            "source_admission/basic-invalid-rule",
            "0f1eff9ba3a1cecc6a3ac16a3b66545e31bfc77dee099674f83ba444d29be20e",
        ),
        (
            "webkit_canonical/basic-invalid-modifiers",
            "4834d226545b39f44f12b6fb142046ec194d171d31ccdd7240d8efd75fa786b6",
        ),
    ];

    let mut files = BTreeSet::new();
    let mut directories = BTreeSet::new();
    for entry in std::fs::read_dir(root)
        .map_err(|error| format!("cannot read blocker fuzz root {}: {error}", root.display()))?
    {
        let entry = entry.map_err(|error| {
            format!(
                "cannot enumerate blocker fuzz root {}: {error}",
                root.display()
            )
        })?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| "blocker fuzz root contains a non-UTF-8 entry".to_owned())?;
        let kind = entry
            .file_type()
            .map_err(|error| format!("cannot inspect blocker fuzz entry `{name}`: {error}"))?;
        if kind.is_symlink() {
            return Err(format!("blocker fuzz root entry `{name}` is a symlink"));
        }
        if kind.is_file() {
            files.insert(name);
        } else if kind.is_dir() {
            if matches!(name.as_str(), "artifacts" | "target") {
                continue;
            }
            directories.insert(name);
        } else {
            return Err(format!(
                "blocker fuzz root entry `{name}` is not a regular file or directory"
            ));
        }
    }
    if files != FILES.into_iter().map(str::to_owned).collect()
        || directories != DIRECTORIES.into_iter().map(str::to_owned).collect()
    {
        return Err(format!(
            "blocker fuzz root layout is not closed: files={files:?}; directories={directories:?}"
        ));
    }
    require_flat_regular_files(&root.join("fuzz_targets"), &TARGETS)?;

    let corpus_root = root.join("corpus");
    let expected_directories = CORPUS
        .iter()
        .filter_map(|(path, _)| path.split_once('/').map(|(directory, _)| directory))
        .collect::<BTreeSet<_>>();
    let actual_directories = std::fs::read_dir(&corpus_root)
        .map_err(|error| format!("cannot read blocker fuzz corpus: {error}"))?
        .map(|entry| {
            let entry =
                entry.map_err(|error| format!("cannot enumerate blocker corpus: {error}"))?;
            let kind = entry
                .file_type()
                .map_err(|error| format!("cannot inspect blocker corpus entry: {error}"))?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| "blocker corpus contains a non-UTF-8 entry".to_owned())?;
            if !kind.is_dir() || kind.is_symlink() {
                return Err(format!("blocker corpus entry `{name}` is not a directory"));
            }
            Ok(name)
        })
        .collect::<Result<BTreeSet<_>, String>>()?;
    if actual_directories
        != expected_directories
            .into_iter()
            .map(str::to_owned)
            .collect::<BTreeSet<_>>()
    {
        return Err("blocker fuzz corpus target directories are not closed".to_owned());
    }
    for directory in &actual_directories {
        let expected = CORPUS
            .iter()
            .filter_map(|(path, _)| {
                path.split_once('/')
                    .filter(|(parent, _)| parent == directory)
                    .map(|(_, file)| file)
            })
            .collect::<Vec<_>>();
        require_flat_regular_files(&corpus_root.join(directory), &expected)?;
    }
    for (relative, expected_digest) in CORPUS {
        let path = corpus_root.join(relative);
        let bytes = std::fs::read(&path)
            .map_err(|error| format!("cannot read blocker corpus {}: {error}", path.display()))?;
        if bytes.is_empty() || bytes.len() > 4 * 1024 {
            return Err(format!(
                "blocker corpus {} is empty or exceeds 4 KiB",
                path.display()
            ));
        }
        let text = std::str::from_utf8(&bytes)
            .map_err(|_| format!("blocker corpus {} is not UTF-8", path.display()))?;
        if !text.contains(".invalid") || sha256_file(&path)? != expected_digest {
            return Err(format!(
                "blocker corpus {} is not the reviewed Zephium-authored seed",
                path.display()
            ));
        }
    }
    Ok(())
}

fn require_flat_regular_files(root: &Path, expected: &[&str]) -> Result<(), String> {
    let mut actual = BTreeSet::new();
    for entry in std::fs::read_dir(root)
        .map_err(|error| format!("cannot read {}: {error}", root.display()))?
    {
        let entry =
            entry.map_err(|error| format!("cannot enumerate {}: {error}", root.display()))?;
        let kind = entry
            .file_type()
            .map_err(|error| format!("cannot inspect {}: {error}", entry.path().display()))?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| format!("{} contains a non-UTF-8 entry", root.display()))?;
        if !kind.is_file() || kind.is_symlink() {
            return Err(format!(
                "{} entry `{name}` is not a regular file",
                root.display()
            ));
        }
        actual.insert(name);
    }
    let expected = expected.iter().map(|name| (*name).to_owned()).collect();
    if actual != expected {
        return Err(format!(
            "{} file set is not closed: {actual:?}",
            root.display()
        ));
    }
    Ok(())
}

fn validate_target_dependency(
    targets: &toml::Table,
    target: &str,
    name: &str,
    features: &[&str],
) -> Result<(), String> {
    let dependencies = target_dependencies(targets, target)?;
    let dependency = require_dependency_table(dependencies, name)?;
    require_key_set(
        dependency,
        &["features", "workspace"],
        &format!("{target} {name} dependency"),
    )?;
    require_bool(dependency, "workspace", true)?;
    require_string_array(dependency, "features", features)
}

fn target_dependencies<'a>(
    targets: &'a toml::Table,
    target: &str,
) -> Result<&'a toml::Table, String> {
    targets
        .get(target)
        .and_then(toml::Value::as_table)
        .and_then(|table| table.get("dependencies"))
        .and_then(toml::Value::as_table)
        .ok_or_else(|| format!("{target} has no dependency table"))
}

fn require_dependency_table<'a>(
    dependencies: &'a toml::Table,
    name: &str,
) -> Result<&'a toml::Table, String> {
    dependencies
        .get(name)
        .and_then(toml::Value::as_table)
        .ok_or_else(|| format!("dependency `{name}` is not a structured table"))
}

fn require_dependency_version(
    dependencies: &toml::Table,
    name: &str,
    expected: &str,
) -> Result<(), String> {
    match dependencies.get(name) {
        Some(toml::Value::String(version)) if version == expected => Ok(()),
        Some(toml::Value::Table(table)) => require_string(table, "version", expected),
        value => Err(format!(
            "dependency `{name}` is {value:?}, expected registry version `{expected}`"
        )),
    }
}

fn require_workspace_dependency(
    dependencies: &toml::Table,
    name: &str,
    features: &[&str],
) -> Result<(), String> {
    let dependency = require_dependency_table(dependencies, name)?;
    let expected_keys = if features.is_empty() {
        ["workspace"].as_slice()
    } else {
        ["features", "workspace"].as_slice()
    };
    require_key_set(
        dependency,
        expected_keys,
        &format!("workspace dependency `{name}`"),
    )?;
    require_bool(dependency, "workspace", true)?;
    if features.is_empty() {
        Ok(())
    } else {
        require_string_array(dependency, "features", features)
    }
}

fn dependency_package<'a>(name: &'a str, value: &'a toml::Value) -> &'a str {
    value
        .as_table()
        .and_then(|dependency| dependency.get("package"))
        .and_then(toml::Value::as_str)
        .unwrap_or(name)
}

fn reject_dependency_aliases(
    dependencies: &toml::Table,
    label: &str,
    forbidden_packages: &[&str],
    allowed_exact_keys: &[&str],
) -> Result<(), String> {
    for (name, value) in dependencies {
        let package = dependency_package(name, value);
        if forbidden_packages.contains(&package)
            || (allowed_exact_keys.contains(&package) && name != package)
        {
            return Err(format!(
                "{label} contains unreviewed dependency alias `{name}` -> `{package}`"
            ));
        }
    }
    Ok(())
}

fn reject_blocker_packages_except(
    dependencies: &toml::Table,
    label: &str,
    allowed: &[&str],
) -> Result<(), String> {
    for (name, value) in dependencies {
        let package = dependency_package(name, value);
        if matches!(
            package,
            "adblock"
                | "zephium-blocker"
                | "zephium-blocker-service"
                | "zephium-blocker-update"
                | "zephium-update-transport"
        ) && (!allowed.contains(&package) || name != package)
        {
            return Err(format!(
                "{label} directly depends on unreviewed package `{package}` as `{name}`"
            ));
        }
    }
    Ok(())
}

fn validate_fork_root_layout(fork_root: &Path) -> Result<(), String> {
    let expected_files = FORK_ROOT_FILES
        .iter()
        .map(|name| (*name).to_owned())
        .collect::<BTreeSet<_>>();
    let expected_directories = FORK_ROOT_DIRECTORIES
        .iter()
        .map(|name| (*name).to_owned())
        .collect::<BTreeSet<_>>();
    let mut files = BTreeSet::new();
    let mut directories = BTreeSet::new();
    let entries = std::fs::read_dir(fork_root)
        .map_err(|error| format!("cannot read {}: {error}", fork_root.display()))?;
    for entry in entries {
        let entry =
            entry.map_err(|error| format!("cannot enumerate {}: {error}", fork_root.display()))?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| format!("{} contains a non-UTF-8 entry", fork_root.display()))?;
        let metadata = std::fs::symlink_metadata(entry.path())
            .map_err(|error| format!("cannot inspect {}: {error}", entry.path().display()))?;
        if metadata.file_type().is_symlink() {
            return Err(format!(
                "adblock fork root entry {} is a symlink",
                entry.path().display()
            ));
        }
        if name == "target" {
            if !metadata.is_dir() {
                return Err("adblock fork generated `target` entry is not a directory".to_owned());
            }
            continue;
        }
        if metadata.is_file() {
            if !expected_files.contains(&name) {
                return Err(format!("adblock fork root has unreviewed file `{name}`"));
            }
            files.insert(name);
        } else if metadata.is_dir() {
            if !expected_directories.contains(&name) {
                return Err(format!(
                    "adblock fork root has unreviewed directory `{name}`"
                ));
            }
            directories.insert(name);
        } else {
            return Err(format!(
                "adblock fork root entry {} is not a regular file or directory",
                entry.path().display()
            ));
        }
    }
    if files != expected_files {
        return Err(format!(
            "adblock fork root file set is not closed: {}",
            describe_set_difference(&expected_files, &files)
        ));
    }
    if directories != expected_directories {
        return Err(format!(
            "adblock fork root directory set is not closed: {}",
            describe_set_difference(&expected_directories, &directories)
        ));
    }

    Ok(())
}

fn verify_imported_files(fork_root: &Path, reviewed: &ReviewedFork) -> Result<(), String> {
    let actual_support = collect_explicit_files(fork_root, &SUPPORT_PATHS)?;
    let expected_support = reviewed.support.keys().cloned().collect::<BTreeSet<_>>();
    if actual_support != expected_support {
        return Err(format!(
            "imported support-file inventory differs from upstream record: {}",
            describe_set_difference(&expected_support, &actual_support)
        ));
    }
    let actual_code = collect_roots(fork_root, &CODE_ROOTS)?;
    let expected_code = reviewed.code.keys().cloned().collect::<BTreeSet<_>>();
    if actual_code != expected_code {
        return Err(format!(
            "imported code inventory differs from upstream record: {}",
            describe_set_difference(&expected_code, &actual_code)
        ));
    }
    for (relative, expected) in &reviewed.support {
        let path = fork_root.join(relative);
        let actual_sha = sha256_file(&path)?;
        let actual_blob = git_blob(&path)?;
        if actual_sha != expected.sha256 || actual_blob != expected.git_blob {
            return Err(format!(
                "support file {} is not byte-identical to recorded upstream",
                path.display()
            ));
        }
    }

    let mut actual_divergence = BTreeSet::new();
    for (relative, expected) in &reviewed.code {
        let path = fork_root.join(relative);
        let actual_sha = sha256_file(&path)?;
        let actual_blob = git_blob(&path)?;
        let sha_differs = actual_sha != expected.sha256;
        let blob_differs = actual_blob != expected.git_blob;
        if sha_differs != blob_differs {
            return Err(format!(
                "{} disagrees between Git-blob and SHA-256 identity",
                path.display()
            ));
        }
        if sha_differs {
            actual_divergence.insert(relative.to_owned());
        }
    }
    if actual_divergence != reviewed.patch_paths {
        return Err(format!(
            "actual adblock fork divergence differs from FORK.toml: {}",
            describe_set_difference(&reviewed.patch_paths, &actual_divergence)
        ));
    }

    Ok(())
}

fn collect_explicit_files(root: &Path, paths: &[&str]) -> Result<BTreeSet<String>, String> {
    let mut files = BTreeSet::new();
    for relative in paths {
        validate_relative_path(relative)?;
        verify_regular_file(&root.join(relative))?;
        if !files.insert((*relative).to_owned()) {
            return Err(format!("duplicate explicit import path `{relative}`"));
        }
    }
    Ok(files)
}

fn collect_roots(root: &Path, roots: &[&str]) -> Result<BTreeSet<String>, String> {
    let mut files = BTreeSet::new();
    for relative in roots {
        validate_relative_path(relative)?;
        collect_directory(root, Path::new(relative), &mut files)?;
    }
    Ok(files)
}

fn collect_directory(
    root: &Path,
    relative: &Path,
    files: &mut BTreeSet<String>,
) -> Result<(), String> {
    let directory = root.join(relative);
    let metadata = std::fs::symlink_metadata(&directory)
        .map_err(|error| format!("cannot inspect {}: {error}", directory.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(format!(
            "import root {} is not a real directory",
            directory.display()
        ));
    }
    let mut children = std::fs::read_dir(&directory)
        .map_err(|error| format!("cannot read {}: {error}", directory.display()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("cannot enumerate {}: {error}", directory.display()))?;
    children.sort_by_key(std::fs::DirEntry::file_name);
    for child in children {
        let name = child
            .file_name()
            .into_string()
            .map_err(|_| format!("{} contains a non-UTF-8 path", directory.display()))?;
        let child_relative = relative.join(name);
        let child_path = root.join(&child_relative);
        let metadata = std::fs::symlink_metadata(&child_path)
            .map_err(|error| format!("cannot inspect {}: {error}", child_path.display()))?;
        if metadata.file_type().is_symlink() {
            return Err(format!(
                "imported path {} is a symlink",
                child_path.display()
            ));
        }
        if metadata.is_dir() {
            collect_directory(root, &child_relative, files)?;
        } else if metadata.is_file() {
            let text = path_text(&child_relative)?;
            if !files.insert(text.clone()) {
                return Err(format!("import roots overlap at `{text}`"));
            }
        } else {
            return Err(format!(
                "imported path {} is not a regular file or directory",
                child_path.display()
            ));
        }
    }
    Ok(())
}

fn verify_regular_file(path: &Path) -> Result<std::fs::Metadata, String> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(format!("{} is not a regular file", path.display()));
    }
    Ok(metadata)
}

fn sha256_file(path: &Path) -> Result<String, String> {
    verify_regular_file(path)?;
    let mut file =
        File::open(path).map_err(|error| format!("cannot open {}: {error}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|error| format!("cannot hash {}: {error}", path.display()))?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(hex(&hasher.finalize()))
}

fn git_blob(path: &Path) -> Result<String, String> {
    verify_regular_file(path)?;
    let output = Command::new("git")
        .arg("hash-object")
        .arg("--no-filters")
        .arg(path)
        .output()
        .map_err(|error| format!("cannot hash {} with Git: {error}", path.display()))?;
    if !output.status.success() {
        return Err(format!("git hash-object failed for {}", path.display()));
    }
    let hash = std::str::from_utf8(&output.stdout)
        .map(str::trim)
        .map_err(|_| format!("git hash-object returned non-UTF-8 for {}", path.display()))?;
    if hash.len() != 40
        || !hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(format!(
            "git hash-object returned an invalid identity for {}",
            path.display()
        ));
    }
    Ok(hash.to_owned())
}

fn verify_lock_alignment(repository: &Path) -> Result<(), String> {
    verify_desktop_feature_graphs(repository)?;

    let vendor = cargo_tree(
        repository,
        &[
            "tree",
            "--manifest-path",
            "vendor/adblock/Cargo.toml",
            "--locked",
            "--offline",
            "--no-default-features",
            "--features",
            SHIPPING_FEATURE_SETS[1],
            "--edges",
            "normal,build",
            "--prefix",
            "depth",
            "--format",
            "{p}",
        ],
    )?;
    let root = cargo_tree(
        repository,
        &[
            "tree",
            "-p",
            "zephium-blocker",
            "--locked",
            "--offline",
            "--no-default-features",
            "--features",
            "runtime,webkit",
            "--edges",
            "normal,build",
            "--prefix",
            "depth",
            "--format",
            "{p}",
        ],
    )?;
    let vendor_tree = parse_depth_tree(&vendor)?;
    let root_tree = extract_adblock_subtree(&root)?;
    if vendor_tree != root_tree {
        let first = vendor_tree
            .iter()
            .zip(&root_tree)
            .position(|(vendor, root)| vendor != root)
            .unwrap_or_else(|| vendor_tree.len().min(root_tree.len()));
        return Err(format!(
            "standalone/application adblock production graphs differ at row {first}: vendor={:?}, application={:?}",
            vendor_tree.get(first),
            root_tree.get(first)
        ));
    }

    let vendor_lock = parse_lock(&read_text(&repository.join("vendor/adblock/Cargo.lock"))?)?;
    let root_lock = parse_lock(&read_text(&repository.join("Cargo.lock"))?)?;
    let packages = vendor_tree
        .iter()
        .filter_map(|(_, package)| parse_package_spec(package))
        .collect::<BTreeSet<_>>();
    for (name, version) in packages {
        if name == "adblock" {
            continue;
        }
        let vendor_identity = lock_identity(&vendor_lock, &name, &version)?;
        let root_identity = lock_identity(&root_lock, &name, &version)?;
        if vendor_identity != root_identity {
            return Err(format!(
                "`{name} {version}` differs between standalone and application lockfiles"
            ));
        }
    }
    Ok(())
}

fn verify_desktop_feature_graphs(repository: &Path) -> Result<(), String> {
    const TARGETS: [(&str, &[&str], &[&str]); 4] = [
        (
            "x86_64-pc-windows-msvc",
            &["runtime", "runtime-exact"],
            &[
                "addr",
                "css-validation",
                "cssparser",
                "embedded-domain-resolver",
                "full-regex-handling",
                "selectors",
            ],
        ),
        (
            "x86_64-apple-darwin",
            &["webkit"],
            &[
                "content-blocking",
                "addr",
                "css-validation",
                "cssparser",
                "embedded-domain-resolver",
                "full-regex-handling",
                "selectors",
            ],
        ),
        (
            "aarch64-apple-darwin",
            &["webkit"],
            &[
                "content-blocking",
                "addr",
                "css-validation",
                "cssparser",
                "embedded-domain-resolver",
                "full-regex-handling",
                "selectors",
            ],
        ),
        (
            "x86_64-unknown-linux-gnu",
            &["webkit"],
            &[
                "content-blocking",
                "addr",
                "css-validation",
                "cssparser",
                "embedded-domain-resolver",
                "full-regex-handling",
                "selectors",
            ],
        ),
    ];

    for (target, blocker_features, adblock_features) in TARGETS {
        let output = cargo_tree(
            repository,
            &[
                "tree",
                "-p",
                "zephium-desktop",
                "--locked",
                "--offline",
                "--target",
                target,
                "--edges",
                "normal,build",
                "--prefix",
                "depth",
                "--format",
                "{p}|{f}",
            ],
        )?;
        verify_desktop_feature_graph(target, &output, blocker_features, adblock_features)?;
        // The library-only, no-feature service remains network-free. Desktop
        // explicitly selects official HTTPS; TUF must stay out of that graph.
        let blocker = cargo_tree(
            repository,
            &[
                "tree",
                "-p",
                "zephium-blocker-service",
                "--locked",
                "--offline",
                "--target",
                target,
                "--edges",
                "normal,build",
                "--prefix",
                "depth",
                "--format",
                "{p}|{f}",
            ],
        )?;
        let blocker = parse_feature_tree(&blocker)?;
        for package in [
            "reqwest",
            "aws-lc-rs",
            "rustls-platform-verifier",
            "tough",
            "zephium-update-transport",
        ] {
            reject_package(&blocker, target, package)?;
        }
    }
    Ok(())
}

fn verify_desktop_feature_graph(
    target: &str,
    source: &str,
    expected_blocker_features: &[&str],
    expected_adblock_features: &[&str],
) -> Result<(), String> {
    let tree = parse_feature_tree(source)?;
    require_direct_product_package(&tree, target, "zephium-blocker", "0.1.0")?;
    require_direct_product_package(&tree, target, "zephium-blocker-service", "0.1.0")?;
    require_package_features(
        &tree,
        target,
        "zephium-blocker",
        "0.1.0",
        expected_blocker_features,
    )?;
    require_package_features(
        &tree,
        target,
        "adblock",
        "0.13.2",
        expected_adblock_features,
    )?;
    require_package_features(
        &tree,
        target,
        "zephium-blocker-service",
        "0.1.0",
        &["official-https"],
    )?;
    require_package_features(
        &tree,
        target,
        "zephium-blocker-update",
        "0.1.0",
        &["official-https"],
    )?;
    require_direct_product_package(&tree, target, "reqwest", "0.13.4")?;
    require_package_features(&tree, target, "zephium-update-transport", "0.1.0", &[])?;
    reject_package(&tree, target, "tough")?;
    Ok(())
}

fn reject_package(tree: &[FeatureTreeRow], target: &str, package: &str) -> Result<(), String> {
    if tree
        .iter()
        .any(|row| parse_package_spec(&row.package).is_some_and(|(name, _)| name == package))
    {
        return Err(format!(
            "desktop target `{target}` unexpectedly resolves dormant `{package}` code"
        ));
    }
    Ok(())
}

fn require_direct_product_package(
    tree: &[FeatureTreeRow],
    target: &str,
    name: &str,
    version: &str,
) -> Result<(), String> {
    let direct = tree
        .iter()
        .filter(|row| row.depth == 1 && package_is(&row.package, name, version))
        .count();
    if direct != 1 {
        return Err(format!(
            "desktop target `{target}` must resolve one direct `{name} {version}` node, found {direct}"
        ));
    }
    Ok(())
}

fn require_package_features(
    tree: &[FeatureTreeRow],
    target: &str,
    package: &str,
    version: &str,
    expected: &[&str],
) -> Result<(), String> {
    let named = tree
        .iter()
        .filter_map(|row| {
            parse_package_spec(&row.package)
                .filter(|(name, _)| name == package)
                .map(|(_, observed_version)| (row, observed_version))
        })
        .collect::<Vec<_>>();
    if named.is_empty() {
        return Err(format!(
            "desktop target `{target}` does not resolve `{package} {version}`"
        ));
    }
    if named
        .iter()
        .any(|(_, observed_version)| observed_version != version)
    {
        return Err(format!(
            "desktop target `{target}` resolves an unreviewed `{package}` version: {:?}",
            named
                .iter()
                .map(|(_, observed_version)| observed_version)
                .collect::<BTreeSet<_>>()
        ));
    }
    let actual = named
        .iter()
        .flat_map(|(row, _)| row.features.iter().cloned())
        .collect::<BTreeSet<_>>();
    let expected = expected
        .iter()
        .map(|feature| (*feature).to_owned())
        .collect::<BTreeSet<_>>();
    if actual != expected {
        return Err(format!(
            "desktop target `{target}` resolves `{package}` features {actual:?}, expected {expected:?}"
        ));
    }
    Ok(())
}

fn verify_blocker_product_lock(repository: &Path) -> Result<(), String> {
    const LOCAL: [(&str, &str); 4] = [
        ("zephium-blocker", "0.1.0"),
        ("zephium-blocker-service", "0.1.0"),
        ("zephium-blocker-update", "0.1.0"),
        ("zephium-update-transport", "0.1.0"),
    ];
    const REGISTRY: [(&str, &str); 5] = [
        ("aws-lc-rs", "1.18.1"),
        ("reqwest", "0.13.4"),
        ("rustls", "0.23.46"),
        ("rustls-platform-verifier", "0.7.0"),
        ("tough", "0.24.0"),
    ];
    const CRATES_IO: &str = "registry+https://github.com/rust-lang/crates.io-index";

    let packages = parse_lock(&read_text(&repository.join("Cargo.lock"))?)?;
    for (name, version) in LOCAL {
        let package = require_single_lock_package(&packages, name, version)?;
        if package.contains_key("source") || package.contains_key("checksum") {
            return Err(format!(
                "Cargo.lock `{name} {version}` is not the reviewed local workspace package"
            ));
        }
    }
    for (name, version) in REGISTRY {
        let package = require_single_lock_package(&packages, name, version)?;
        if package.get("source").and_then(toml::Value::as_str) != Some(CRATES_IO) {
            return Err(format!(
                "Cargo.lock `{name} {version}` is not sourced from the reviewed crates.io registry"
            ));
        }
        let checksum = package
            .get("checksum")
            .and_then(toml::Value::as_str)
            .filter(|value| {
                value.len() == 64
                    && value
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            });
        if checksum.is_none() {
            return Err(format!(
                "Cargo.lock `{name} {version}` has no canonical SHA-256 registry checksum"
            ));
        }
    }
    Ok(())
}

fn require_single_lock_package<'a>(
    packages: &'a [toml::Table],
    name: &str,
    version: &str,
) -> Result<&'a toml::Table, String> {
    let named = packages
        .iter()
        .filter(|package| package.get("name").and_then(toml::Value::as_str) == Some(name))
        .collect::<Vec<_>>();
    if named.len() != 1 {
        return Err(format!(
            "Cargo.lock expected one `{name}` package, found {}",
            named.len()
        ));
    }
    if named[0].get("version").and_then(toml::Value::as_str) != Some(version) {
        return Err(format!(
            "Cargo.lock `{name}` is not the reviewed version {version}"
        ));
    }
    Ok(named[0])
}

#[derive(Debug)]
struct FeatureTreeRow {
    depth: usize,
    package: String,
    features: BTreeSet<String>,
}

fn parse_feature_tree(source: &str) -> Result<Vec<FeatureTreeRow>, String> {
    source
        .lines()
        .map(|line| {
            let line = strip_ansi_sgr(line)?;
            let line = line.strip_suffix(" (*)").unwrap_or(&line);
            let digits = line.bytes().take_while(u8::is_ascii_digit).count();
            if digits == 0 || digits == line.len() {
                return Err(format!("invalid cargo feature-tree depth row `{line}`"));
            }
            let depth = line[..digits]
                .parse::<usize>()
                .map_err(|_| format!("invalid cargo feature-tree depth row `{line}`"))?;
            let (package, features) = line[digits..].rsplit_once('|').ok_or_else(|| {
                format!("cargo feature-tree row has no feature delimiter `{line}`")
            })?;
            if package.is_empty() {
                return Err(format!("cargo feature-tree row has no package `{line}`"));
            }
            let features = features
                .split(',')
                .filter(|feature| !feature.is_empty())
                .map(str::to_owned)
                .collect::<BTreeSet<_>>();
            Ok(FeatureTreeRow {
                depth,
                package: package.to_owned(),
                features,
            })
        })
        .collect()
}

/// Cargo uses ANSI SGR sequences for duplicated feature rows when its colour
/// mode is inherited from the surrounding CI environment. The tree is parsed
/// as a machine-readable security boundary, so normalise that presentation
/// layer before interpreting a feature name. Reject every non-SGR escape
/// sequence rather than silently accepting an unknown terminal control code.
fn strip_ansi_sgr(line: &str) -> Result<String, String> {
    let bytes = line.as_bytes();
    let mut normalized = String::with_capacity(line.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != 0x1b {
            let character = line[index..]
                .chars()
                .next()
                .expect("index must remain on a UTF-8 boundary");
            normalized.push(character);
            index += character.len_utf8();
            continue;
        }

        let escape = index;
        index += 1;
        if bytes.get(index) != Some(&b'[') {
            return Err(format!(
                "cargo feature-tree row has an unsupported ANSI escape at byte {escape}"
            ));
        }
        index += 1;
        while matches!(bytes.get(index), Some(b'0'..=b'9' | b';')) {
            index += 1;
        }
        if bytes.get(index) != Some(&b'm') {
            return Err(format!(
                "cargo feature-tree row has a non-SGR ANSI escape at byte {escape}"
            ));
        }
        index += 1;
    }
    Ok(normalized)
}

fn cargo_tree(repository: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("cargo")
        // Do not permit CARGO_TERM_COLOR from a CI action to change the
        // grammar consumed by parse_feature_tree. strip_ansi_sgr above is
        // retained as a defensive parser boundary for Cargo presentation
        // changes or a future caller that supplies recorded output.
        .args(["--color", "never"])
        .args(args)
        .current_dir(repository)
        .output()
        .map_err(|error| format!("cannot run cargo tree: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "cargo {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    String::from_utf8(output.stdout).map_err(|_| "cargo tree returned non-UTF-8".to_owned())
}

fn parse_depth_tree(source: &str) -> Result<Vec<(usize, String)>, String> {
    source
        .lines()
        .map(|line| {
            let digits = line.bytes().take_while(u8::is_ascii_digit).count();
            if digits == 0 || digits == line.len() {
                return Err(format!("invalid cargo tree depth row `{line}`"));
            }
            let depth = line[..digits]
                .parse::<usize>()
                .map_err(|_| format!("invalid cargo tree depth row `{line}`"))?;
            Ok((depth, line[digits..].to_owned()))
        })
        .collect()
}

fn extract_adblock_subtree(source: &str) -> Result<Vec<(usize, String)>, String> {
    let tree = parse_depth_tree(source)?;
    let start = tree
        .iter()
        .position(|(depth, package)| *depth == 1 && package.starts_with("adblock v0.13.2 "))
        .ok_or_else(|| {
            "application cargo tree has no direct local adblock dependency".to_owned()
        })?;
    let mut subtree = Vec::new();
    for (depth, package) in tree.into_iter().skip(start) {
        if !subtree.is_empty() && depth <= 1 {
            break;
        }
        subtree.push((depth - 1, package));
    }
    Ok(subtree)
}

fn parse_package_spec(spec: &str) -> Option<(String, String)> {
    let mut fields = spec.split_whitespace();
    let name = fields.next()?;
    let version = fields.next()?.strip_prefix('v')?;
    Some((name.to_owned(), version.to_owned()))
}

fn package_is(spec: &str, expected_name: &str, expected_version: &str) -> bool {
    parse_package_spec(spec)
        .is_some_and(|(name, version)| name == expected_name && version == expected_version)
}

fn parse_lock(source: &str) -> Result<Vec<toml::Table>, String> {
    let lock = parse_table(source, "Cargo.lock")?;
    lock.get("package")
        .and_then(toml::Value::as_array)
        .ok_or_else(|| "Cargo.lock has no package array".to_owned())?
        .iter()
        .map(|package| {
            package
                .as_table()
                .cloned()
                .ok_or_else(|| "Cargo.lock contains a non-table package".to_owned())
        })
        .collect()
}

fn lock_identity(
    packages: &[toml::Table],
    name: &str,
    version: &str,
) -> Result<(Option<String>, Option<String>), String> {
    let matches = packages
        .iter()
        .filter(|package| {
            package.get("name").and_then(toml::Value::as_str) == Some(name)
                && package.get("version").and_then(toml::Value::as_str) == Some(version)
        })
        .collect::<Vec<_>>();
    if matches.len() != 1 {
        return Err(format!(
            "Cargo.lock expected one `{name} {version}` package, found {}",
            matches.len()
        ));
    }
    Ok((
        matches[0]
            .get("source")
            .and_then(toml::Value::as_str)
            .map(str::to_owned),
        matches[0]
            .get("checksum")
            .and_then(toml::Value::as_str)
            .map(str::to_owned),
    ))
}

fn parse_table(source: &str, label: &str) -> Result<toml::Table, String> {
    source
        .parse::<toml::Table>()
        .map_err(|error| format!("{label} is invalid TOML: {error}"))
}

fn require_table<'a>(table: &'a toml::Table, key: &str) -> Result<&'a toml::Table, String> {
    table
        .get(key)
        .and_then(toml::Value::as_table)
        .ok_or_else(|| format!("required TOML table `{key}` is missing"))
}

fn require_string(table: &toml::Table, key: &str, expected: &str) -> Result<(), String> {
    let actual = table.get(key).and_then(toml::Value::as_str);
    if actual != Some(expected) {
        return Err(format!("TOML `{key}` is {actual:?}, expected `{expected}`"));
    }
    Ok(())
}

fn require_bool(table: &toml::Table, key: &str, expected: bool) -> Result<(), String> {
    let actual = table.get(key).and_then(toml::Value::as_bool);
    if actual != Some(expected) {
        return Err(format!("TOML `{key}` is {actual:?}, expected {expected}"));
    }
    Ok(())
}

fn require_integer(table: &toml::Table, key: &str, expected: i64) -> Result<(), String> {
    let actual = table.get(key).and_then(toml::Value::as_integer);
    if actual != Some(expected) {
        return Err(format!("TOML `{key}` is {actual:?}, expected {expected}"));
    }
    Ok(())
}

fn require_key_set(table: &toml::Table, expected: &[&str], label: &str) -> Result<(), String> {
    let actual = table.keys().map(String::as_str).collect::<BTreeSet<_>>();
    let expected = expected.iter().copied().collect::<BTreeSet<_>>();
    if actual != expected {
        let missing = expected.difference(&actual).copied().collect::<Vec<_>>();
        let unexpected = actual.difference(&expected).copied().collect::<Vec<_>>();
        return Err(format!(
            "{label} has an unreviewed key set: missing={missing:?}; unexpected={unexpected:?}"
        ));
    }
    Ok(())
}

fn validate_bench_targets(manifest: &toml::Table) -> Result<(), String> {
    const BENCHES: [(&str, &str); 1] = [("bench_regex", "benches/bench_regex.rs")];
    let benches = manifest
        .get("bench")
        .and_then(toml::Value::as_array)
        .ok_or_else(|| "adblock manifest must contain the reviewed bench targets".to_owned())?;
    if benches.len() != BENCHES.len() {
        return Err(format!(
            "adblock manifest has {} bench targets, expected {}",
            benches.len(),
            BENCHES.len()
        ));
    }
    for (index, ((expected_name, expected_path), value)) in BENCHES.iter().zip(benches).enumerate()
    {
        let bench = value
            .as_table()
            .ok_or_else(|| format!("adblock bench target {index} is not a table"))?;
        require_key_set(
            bench,
            &["harness", "name", "path"],
            &format!("adblock bench target {index}"),
        )?;
        require_string(bench, "name", expected_name)?;
        require_string(bench, "path", expected_path)?;
        require_bool(bench, "harness", false)?;
    }
    Ok(())
}

fn validate_test_targets(manifest: &toml::Table) -> Result<(), String> {
    let tests = manifest
        .get("test")
        .and_then(toml::Value::as_array)
        .ok_or_else(|| "adblock manifest must contain the reviewed test target".to_owned())?;
    if tests.len() != 1 {
        return Err(format!(
            "adblock manifest has {} explicit test targets, expected 1",
            tests.len()
        ));
    }
    let test = tests[0]
        .as_table()
        .ok_or_else(|| "adblock test target is not a table".to_owned())?;
    require_key_set(test, &["name", "path"], "adblock test target")?;
    require_string(test, "name", "fork_contract")?;
    require_string(test, "path", "tests/ublock-coverage.rs")
}

fn validate_dependency_sources(dependencies: &toml::Table, kind: &str) -> Result<(), String> {
    const ALLOWED_KEYS: [&str; 4] = ["default-features", "features", "optional", "version"];
    for (name, value) in dependencies {
        let dependency = value.as_table().ok_or_else(|| {
            format!("adblock {kind} `{name}` must use a normalized dependency table")
        })?;
        for forbidden in ["git", "package", "path", "registry", "registry-index"] {
            if dependency.contains_key(forbidden) {
                return Err(format!(
                    "adblock {kind} `{name}` uses forbidden `{forbidden}` metadata"
                ));
            }
        }
        for key in dependency.keys() {
            if !ALLOWED_KEYS.contains(&key.as_str()) {
                return Err(format!(
                    "adblock {kind} `{name}` uses unreviewed `{key}` metadata"
                ));
            }
        }
        if dependency
            .get("version")
            .and_then(toml::Value::as_str)
            .is_none_or(str::is_empty)
        {
            return Err(format!(
                "adblock {kind} `{name}` must have a non-empty registry version"
            ));
        }
        if dependency
            .get("features")
            .is_some_and(|_| string_array(dependency, "features").is_err())
        {
            return Err(format!(
                "adblock {kind} `{name}` has an invalid feature array"
            ));
        }
        for key in ["default-features", "optional"] {
            if dependency
                .get(key)
                .is_some_and(|value| value.as_bool().is_none())
            {
                return Err(format!("adblock {kind} `{name}` has a non-boolean `{key}`"));
            }
        }
    }
    Ok(())
}

fn string_array<'a>(table: &'a toml::Table, key: &str) -> Result<Vec<&'a str>, String> {
    table
        .get(key)
        .and_then(toml::Value::as_array)
        .ok_or_else(|| format!("required TOML array `{key}` is missing"))?
        .iter()
        .map(|value| {
            value
                .as_str()
                .ok_or_else(|| format!("TOML array `{key}` contains a non-string"))
        })
        .collect()
}

fn require_string_array(table: &toml::Table, key: &str, expected: &[&str]) -> Result<(), String> {
    let actual = string_array(table, key)?;
    if actual != expected {
        return Err(format!(
            "TOML array `{key}` is {actual:?}, expected {expected:?}"
        ));
    }
    Ok(())
}

fn validate_relative_path(path: &str) -> Result<(), String> {
    if path.is_empty()
        || path.contains('\\')
        || path.contains('\n')
        || path.contains('\r')
        || path.contains('\0')
    {
        return Err(format!("unsafe inventory path `{path}`"));
    }
    let parsed = Path::new(path);
    if parsed.is_absolute()
        || !parsed
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
    {
        return Err(format!("unsafe inventory path `{path}`"));
    }
    Ok(())
}

fn path_text(path: &Path) -> Result<String, String> {
    let mut text = String::new();
    for component in path.components() {
        let Component::Normal(component) = component else {
            return Err(format!("unsafe imported path {}", path.display()));
        };
        let component = component
            .to_str()
            .ok_or_else(|| format!("non-UTF-8 imported path {}", path.display()))?;
        if !text.is_empty() {
            text.push('/');
        }
        text.push_str(component);
    }
    validate_relative_path(&text)?;
    Ok(text)
}

fn describe_set_difference(expected: &BTreeSet<String>, actual: &BTreeSet<String>) -> String {
    let missing = expected.difference(actual).cloned().collect::<Vec<_>>();
    let unexpected = actual.difference(expected).cloned().collect::<Vec<_>>();
    format!("missing={missing:?}; unexpected={unexpected:?}")
}

fn hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(&mut output, "{byte:02x}").expect("writing to String cannot fail");
    }
    output
}

#[cfg(test)]
mod tests {
    use super::{
        git_blob, parse_feature_tree, sha256_file, validate_blocker_fuzz_deny_source,
        validate_fork_root_layout, validate_relative_path, verify_imported_files, UpstreamEntry,
        FORK_ROOT_DIRECTORIES, FORK_ROOT_FILES,
    };
    use std::collections::{BTreeMap, BTreeSet};

    #[test]
    fn inventory_paths_are_strictly_relative() {
        for invalid in ["", "../escape", "/absolute", "src\\windows", "src/\nfile"] {
            assert!(validate_relative_path(invalid).is_err(), "{invalid}");
        }
        assert!(validate_relative_path("tests/unit/request.rs").is_ok());
    }

    #[test]
    fn fuzz_dependency_policy_cannot_silently_weaken() {
        let strict = include_str!("../../crates/zephium-blocker/fuzz/deny.toml");
        validate_blocker_fuzz_deny_source(strict).unwrap();
        let weakened = strict.replace("unknown-git = \"deny\"", "unknown-git = \"allow\"");
        assert!(validate_blocker_fuzz_deny_source(&weakened).is_err());
    }

    #[test]
    fn feature_tree_normalizes_colored_duplicate_feature_rows() {
        let tree = parse_feature_tree(
            "0zephium-desktop v0.1.0 (/workspace/desktop)|\n\
             1zephium-blocker v0.1.0 (/workspace/crates/zephium-blocker)|runtime\n\
             2zephium-blocker v0.1.0 (/workspace/crates/zephium-blocker)|runtime \x1b[33m\x1b[2m(*)\x1b[39m\x1b[22m\n",
        )
        .expect("Cargo colour must not become a feature name");

        assert_eq!(tree.len(), 3);
        assert_eq!(tree[2].features, BTreeSet::from(["runtime".to_owned()]));
    }

    #[test]
    fn fork_root_layout_rejects_every_unreviewed_entry() {
        let temp = tempfile::tempdir().unwrap();
        for file in FORK_ROOT_FILES {
            std::fs::write(temp.path().join(file), b"reviewed").unwrap();
        }
        for directory in FORK_ROOT_DIRECTORIES {
            std::fs::create_dir(temp.path().join(directory)).unwrap();
        }
        std::fs::create_dir(temp.path().join("target")).unwrap();
        validate_fork_root_layout(temp.path()).unwrap();

        let extra_file = temp.path().join("unreviewed.rs");
        std::fs::write(&extra_file, b"unreviewed").unwrap();
        assert!(validate_fork_root_layout(temp.path()).is_err());
        std::fs::remove_file(extra_file).unwrap();

        let extra_directory = temp.path().join("unreviewed");
        std::fs::create_dir(&extra_directory).unwrap();
        assert!(validate_fork_root_layout(temp.path()).is_err());
        std::fs::remove_dir(extra_directory).unwrap();

        let data_payload = temp.path().join("data");
        std::fs::create_dir(&data_payload).unwrap();
        assert!(validate_fork_root_layout(temp.path()).is_err());
        std::fs::remove_dir(data_payload).unwrap();

        std::fs::remove_dir(temp.path().join("target")).unwrap();
        std::fs::write(temp.path().join("target"), b"not a directory").unwrap();
        assert!(validate_fork_root_layout(temp.path()).is_err());

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;

            std::fs::remove_file(temp.path().join("target")).unwrap();
            symlink(".", temp.path().join("target")).unwrap();
            assert!(validate_fork_root_layout(temp.path()).is_err());
        }
    }

    #[test]
    fn imported_files_reject_mutation_addition_and_stale_patches() {
        let temp = tempfile::tempdir().unwrap();
        for directory in ["src", "benches", "tests"] {
            std::fs::create_dir_all(temp.path().join(directory)).unwrap();
        }
        let support_files: [(&str, &[u8]); 4] = [
            (".gitattributes", b"* text"),
            ("LICENSE", b"license"),
            ("README.md", b"readme"),
            ("rustfmt.toml", b"edition = \"2021\""),
        ];
        for (relative, bytes) in support_files {
            std::fs::write(temp.path().join(relative), bytes).unwrap();
        }
        let code_path = temp.path().join("src/patched.rs");
        std::fs::write(&code_path, b"patched").unwrap();
        let reviewed = super::ReviewedFork {
            support: support_files
                .into_iter()
                .map(|(relative, _)| {
                    let path = temp.path().join(relative);
                    (
                        relative.to_owned(),
                        UpstreamEntry {
                            git_blob: git_blob(&path).unwrap(),
                            sha256: sha256_file(&path).unwrap(),
                        },
                    )
                })
                .collect(),
            code: BTreeMap::from([(
                "src/patched.rs".to_owned(),
                UpstreamEntry {
                    git_blob: "2a767e25cb5716598ee67b64019031e1c8a8d5c5".to_owned(),
                    sha256: "1581e27de87bffae0bd4d745cd7964e68528d7a83e2e4c259a782d275df6f558"
                        .to_owned(),
                },
            )]),
            patch_paths: BTreeSet::from(["src/patched.rs".to_owned()]),
        };

        verify_imported_files(temp.path(), &reviewed).unwrap();

        std::fs::write(temp.path().join("LICENSE"), b"mutation").unwrap();
        let support_mutation = verify_imported_files(temp.path(), &reviewed).unwrap_err();
        assert!(support_mutation.contains("support file"));
        std::fs::write(temp.path().join("LICENSE"), b"license").unwrap();

        std::fs::write(temp.path().join("src/unexpected.rs"), b"extra").unwrap();
        let addition = verify_imported_files(temp.path(), &reviewed).unwrap_err();
        assert!(addition.contains("inventory differs"));
        std::fs::remove_file(temp.path().join("src/unexpected.rs")).unwrap();

        let mut undeclared = reviewed;
        undeclared.patch_paths.clear();
        let error = verify_imported_files(temp.path(), &undeclared).unwrap_err();
        assert!(error.contains("divergence differs"));

        undeclared.patch_paths.insert("src/patched.rs".to_owned());
        std::fs::write(&code_path, b"upstream").unwrap();
        let stale = verify_imported_files(temp.path(), &undeclared).unwrap_err();
        assert!(stale.contains("divergence differs"));
    }
}
