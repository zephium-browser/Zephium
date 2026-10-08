//! Single deterministic entrypoint for the workspace gate: `cargo xtask ci`.

mod adblock_provenance;
mod agent_controller_boundary;
mod agent_model_catalog_boundary;
mod agent_runtime_boundary;
mod agentic_evidence;
mod agentic_probe_boundary;
mod blocker_seed;
mod foreground_rendering_boundary;
mod macos_process_family;
mod password_manager_qa;
mod webext_suite;
mod work_composition_boundary;
mod work_persistence_boundary;
mod work_resource_boundary;

use std::process::{exit, Command};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

const NATIVE_ADAPTERS: [(&str, Option<&str>); 3] = [
    ("vendor/wry/Cargo.toml", None),
    (
        "vendor/tauri-runtime-wry/Cargo.toml",
        Some("macos-private-api"),
    ),
    ("vendor/tauri/Cargo.toml", Some("macos-private-api,specta")),
];
const ADBLOCK_MANIFEST: &str = "vendor/adblock/Cargo.toml";
const BLOCKER_FUZZ_MANIFEST: &str = "crates/zephium-blocker/fuzz/Cargo.toml";
const BLOCKER_FEATURE_SETS: [&str; 5] = [
    "runtime",
    "runtime-exact",
    "webkit",
    "runtime,webkit",
    "runtime-exact,webkit",
];
static CI_RESOURCE_PROFILE: AtomicBool = AtomicBool::new(false);

fn main() {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    match arguments.first().map(String::as_str) {
        Some("ci") => {
            CI_RESOURCE_PROFILE.store(true, Ordering::Release);
            ci();
        }
        Some("check-frame-styles") => run("node", &["frame/scripts/check-styles.mjs"]),
        Some("check-engine-floors") => {
            check_engine_floors(strict_flag(&arguments[1..], "check-engine-floors"))
        }
        Some("check-release-engine-security") => check_release_engine_security(),
        Some("check-agentic-probe-boundary") => check_agentic_probe_boundary(),
        Some("check-agent-model-catalog-boundary") => check_agent_model_catalog_boundary(),
        Some("check-agent-controller-boundary") => check_agent_controller_boundary(),
        Some("check-agent-runtime-boundary") => check_agent_runtime_boundary(),
        Some("check-advisory-exceptions") => {
            check_advisory_exceptions(strict_flag(&arguments[1..], "check-advisory-exceptions"))
        }
        Some("check-security-fork-locks") | Some("check-native-adapter-locks") => {
            check_security_fork_locks()
        }
        Some("check-blocker-security-fork") => check_blocker_security_fork(),
        Some("webext-suite") => {
            if let Err(error) = webext_suite::run(&arguments[1..]) {
                eprintln!("extension suite: {error}");
                exit(1);
            }
        }
        Some("measure-macos-process-family") => {
            if let Err(error) = macos_process_family::run(&arguments[1..]) {
                eprintln!("macOS process-family measurement failed: {error}");
                exit(1);
            }
        }
        Some("serve-password-manager-webauthn-qa") => {
            if let Err(error) = password_manager_qa::run(&arguments[1..]) {
                eprintln!("password-manager WebAuthn QA failed: {error}");
                exit(1);
            }
        }
        Some("check-blocker-seed") if arguments.len() == 1 => {
            let repository = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
            if let Err(error) = blocker_seed::check(&repository) {
                eprintln!("bundled blocker seed policy failed: {error}");
                exit(1);
            }
        }
        Some("materialize-blocker-seed-webkit")
            if arguments.len() == 3 && arguments[1] == "--output" =>
        {
            let repository = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
            if let Err(error) =
                blocker_seed::materialize_webkit(&repository, std::path::Path::new(&arguments[2]))
            {
                eprintln!("bundled blocker seed materialization failed: {error}");
                exit(1);
            }
        }
        Some("update-blocker-seed") => update_blocker_seed(&arguments[1..]),
        #[cfg(any(feature = "blocker-seed-runtime", feature = "blocker-seed-webkit"))]
        Some("__compile-blocker-seed")
            if arguments.len() == 4 || (arguments.len() == 6 && arguments[4] == "--artifact") =>
        {
            if let Err(error) = blocker_seed::compile_hidden(
                &arguments[1],
                std::path::Path::new(&arguments[2]),
                std::path::Path::new(&arguments[3]),
                arguments.get(5).map(std::path::Path::new),
            ) {
                eprintln!("bundled blocker seed compilation failed: {error}");
                exit(1);
            }
        }
        // Retain the old entrypoint for local automation while making it run
        // every engine-floor deadline, not only Windows.
        Some("check-webview2-floor") => {
            check_engine_floors(strict_flag(&arguments[1..], "check-webview2-floor"))
        }
        _ => {
            eprintln!(
                "usage: cargo xtask <ci|webext-suite [--only NAME,...]|check-frame-styles|check-engine-floors [--strict]|check-release-engine-security|check-agentic-probe-boundary|check-advisory-exceptions [--strict]|check-security-fork-locks|check-native-adapter-locks|check-blocker-security-fork|measure-macos-process-family --bundle-id ID --duration-seconds N [--interval-millis N] [--label LABEL]|serve-password-manager-webauthn-qa [--port PORT]|check-blocker-seed|materialize-blocker-seed-webkit --output PATH|update-blocker-seed --easylist PATH --easyprivacy PATH --license PATH|check-webview2-floor [--strict]>"
            );
            exit(2);
        }
    }
}

fn update_blocker_seed(arguments: &[String]) {
    if arguments.len() != 6
        || arguments[0] != "--easylist"
        || arguments[2] != "--easyprivacy"
        || arguments[4] != "--license"
    {
        eprintln!(
            "usage: cargo xtask update-blocker-seed --easylist PATH --easyprivacy PATH --license PATH"
        );
        exit(2);
    }
    let repository = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    if let Err(error) = blocker_seed::update(
        &repository,
        std::path::Path::new(&arguments[1]),
        std::path::Path::new(&arguments[3]),
        std::path::Path::new(&arguments[5]),
    ) {
        eprintln!("bundled blocker seed update failed: {error}");
        exit(1);
    }
}

fn strict_flag(arguments: &[String], command: &str) -> bool {
    match arguments {
        [] => false,
        [flag] if flag == "--strict" => true,
        _ => {
            eprintln!("usage: cargo xtask {command} [--strict]");
            exit(2);
        }
    }
}

/// An expired review is a maintenance signal, not a defect in the change
/// under test, so it warns everywhere except where `--strict` is requested:
/// the release gate and the weekly security review.
fn report_expired_reviews(title: &str, expired: &[String], strict: bool) {
    let level = if strict { "error" } else { "warning" };
    let annotate = std::env::var("GITHUB_ACTIONS").is_ok_and(|value| value == "true");
    for message in expired {
        if annotate {
            println!("::{level} title={title}::{message}");
        } else {
            eprintln!("{level}: {message}");
        }
    }
    if strict && !expired.is_empty() {
        exit(1);
    }
}

fn check_advisory_exceptions(strict: bool) {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|error| {
            eprintln!("advisory-exception check cannot read UTC time: {error}");
            exit(1);
        })
        .as_secs();
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../deny.toml");
    let source = std::fs::read_to_string(&path).unwrap_or_else(|error| {
        eprintln!("cannot read {}: {error}", path.display());
        exit(1);
    });
    match validate_advisory_exceptions(&source, now) {
        Ok(expired) => report_expired_reviews("Advisory exception expired", &expired, strict),
        Err(error) => {
            eprintln!("cargo-deny advisory exception policy failed: {error}");
            exit(1);
        }
    }
}

fn check_security_fork_locks() {
    const ROOT_NATIVE_ADAPTERS: &[(&str, &str)] = &[
        ("tauri", "2.11.3"),
        ("tauri-runtime-wry", "2.11.3"),
        ("wry", "0.55.1"),
    ];
    const ROOT_FORKS: &[(&str, &str)] = &[
        ("adblock", "0.13.2"),
        ("tauri", "2.11.3"),
        ("tauri-runtime-wry", "2.11.3"),
        ("wry", "0.55.1"),
    ];
    const ADBLOCK_FORK: &[(&str, &str)] = &[("adblock", "0.13.2")];
    const RUNTIME_ADAPTERS: &[(&str, &str)] = &[("tauri-runtime-wry", "2.11.3"), ("wry", "0.55.1")];
    const WRY_ADAPTERS: &[(&str, &str)] = &[("wry", "0.55.1")];

    let repository = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    for (relative, forks) in [
        ("Cargo.lock", ROOT_FORKS),
        ("vendor/adblock/Cargo.lock", ADBLOCK_FORK),
        ("vendor/tauri/Cargo.lock", ROOT_NATIVE_ADAPTERS),
        ("vendor/tauri-runtime-wry/Cargo.lock", RUNTIME_ADAPTERS),
        ("vendor/wry/Cargo.lock", WRY_ADAPTERS),
    ] {
        let path = repository.join(relative);
        let source = std::fs::read_to_string(&path).unwrap_or_else(|error| {
            eprintln!("cannot read {}: {error}", path.display());
            exit(1);
        });
        if let Err(error) = validate_security_fork_lock(&source, forks) {
            eprintln!("vendored security-fork lock policy failed for {relative}: {error}");
            exit(1);
        }
    }
    if let Err(error) = adblock_provenance::check(&repository) {
        eprintln!("adblock fork provenance policy failed: {error}");
        exit(1);
    }
    check_tauri_fixture_blobs(&repository);
}

fn check_tauri_fixture_blobs(repository: &std::path::Path) {
    const EXPECTED_COMMIT: &str = "6f6ab1207bb3923c2721fbc67d2fdb1c8deb0c7a";
    const EXPECTED_FILES: [(&str, &str); 5] = [
        (
            "test/fixture/src-tauri/tauri.conf.json",
            "f5b75e3eb0554e617a862d78194c02176c811fcd",
        ),
        (
            "test/fixture/dist/index.html",
            "698a3577914d350e1ebcd9279fe325563553ba24",
        ),
        (
            "test/fixture/src-tauri/icons/icon.ico",
            "b3636e4b22ba65db9061cd60a77b02c92022dfd6",
        ),
        (
            "test/fixture/src-tauri/icons/icon.ico~dev",
            "db7fd98204424b6b9b02fa06ad18f05c089f93b5",
        ),
        (
            "test/fixture/src-tauri/icons/icon.png",
            "a437dd51741e9e56e14b5d6024493cb2abfd5259",
        ),
    ];
    let fork_root = repository.join("vendor/tauri");
    let record_path = fork_root.join("TEST_FIXTURE.toml");
    let source = std::fs::read_to_string(&record_path).unwrap_or_else(|error| {
        eprintln!("cannot read {}: {error}", record_path.display());
        exit(1);
    });
    let document = source.parse::<toml::Table>().unwrap_or_else(|error| {
        eprintln!("{} is invalid TOML: {error}", record_path.display());
        exit(1);
    });
    if document
        .get("upstream_commit")
        .and_then(toml::Value::as_str)
        != Some(EXPECTED_COMMIT)
    {
        eprintln!("Tauri fixture record does not match the reviewed upstream commit");
        exit(1);
    }
    let files = document
        .get("files")
        .and_then(toml::Value::as_array)
        .filter(|files| files.len() == EXPECTED_FILES.len())
        .unwrap_or_else(|| {
            eprintln!(
                "Tauri fixture record must contain exactly {} files",
                EXPECTED_FILES.len()
            );
            exit(1);
        });
    let mut seen = std::collections::HashSet::with_capacity(EXPECTED_FILES.len());
    for file in files {
        let file = file.as_table().unwrap_or_else(|| {
            eprintln!("Tauri fixture record contains a non-table file entry");
            exit(1);
        });
        let relative = file
            .get("path")
            .and_then(toml::Value::as_str)
            .map(std::path::Path::new)
            .filter(|path| {
                !path.is_absolute()
                    && path
                        .components()
                        .all(|component| matches!(component, std::path::Component::Normal(_)))
            })
            .unwrap_or_else(|| {
                eprintln!("Tauri fixture record contains an unsafe path");
                exit(1);
            });
        let recorded = file
            .get("git_blob")
            .and_then(toml::Value::as_str)
            .filter(|hash| hash.len() == 40 && hash.bytes().all(|byte| byte.is_ascii_hexdigit()))
            .unwrap_or_else(|| {
                eprintln!("Tauri fixture record contains an invalid Git blob identity");
                exit(1);
            });
        let relative_text = relative.to_string_lossy();
        let expected = EXPECTED_FILES
            .iter()
            .find_map(|(path, hash)| (*path == relative_text.as_ref()).then_some(*hash))
            .unwrap_or_else(|| {
                eprintln!("unexpected Tauri fixture path {relative_text}");
                exit(1);
            });
        if !seen.insert(relative_text.into_owned()) {
            eprintln!("duplicate Tauri fixture path {}", relative.display());
            exit(1);
        }
        if recorded != expected {
            eprintln!(
                "Tauri fixture record assigns {recorded} to {}, expected {expected}",
                relative.display()
            );
            exit(1);
        }
        let path = fork_root.join(relative);
        let metadata = std::fs::symlink_metadata(&path).unwrap_or_else(|error| {
            eprintln!("cannot inspect {}: {error}", path.display());
            exit(1);
        });
        if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
            eprintln!("Tauri fixture {} is not a regular file", path.display());
            exit(1);
        }
        let output = Command::new("git")
            .arg("hash-object")
            .arg(&path)
            .output()
            .unwrap_or_else(|error| {
                eprintln!("cannot hash {}: {error}", path.display());
                exit(1);
            });
        let actual = std::str::from_utf8(&output.stdout)
            .ok()
            .map(str::trim)
            .filter(|_| output.status.success())
            .unwrap_or_else(|| {
                eprintln!("git hash-object failed for {}", path.display());
                exit(1);
            });
        if actual != expected {
            eprintln!(
                "Tauri fixture {} has Git blob {actual}, expected {expected}",
                path.display()
            );
            exit(1);
        }
    }
}

fn validate_security_fork_lock(source: &str, required: &[(&str, &str)]) -> Result<(), String> {
    let document = source
        .parse::<toml::Table>()
        .map_err(|error| format!("invalid Cargo.lock TOML: {error}"))?;
    let packages = document
        .get("package")
        .and_then(toml::Value::as_array)
        .ok_or_else(|| "Cargo.lock has no package array".to_owned())?;

    for &(name, expected_version) in required {
        let matching = packages
            .iter()
            .filter_map(toml::Value::as_table)
            .filter(|package| package.get("name").and_then(toml::Value::as_str) == Some(name))
            .collect::<Vec<_>>();
        if matching.len() != 1 {
            return Err(format!(
                "expected exactly one `{name}` package, found {}",
                matching.len()
            ));
        }
        let package = matching[0];
        let version = package
            .get("version")
            .and_then(toml::Value::as_str)
            .ok_or_else(|| format!("`{name}` has no version"))?;
        if version != expected_version {
            return Err(format!(
                "`{name}` resolved to {version}, expected {expected_version}"
            ));
        }
        if package.contains_key("source") || package.contains_key("checksum") {
            return Err(format!(
                "`{name}` is registry/git sourced instead of the reviewed local adapter"
            ));
        }
    }
    Ok(())
}

/// Returns one message per expired exception; malformed or over-long
/// exceptions are hard errors.
fn validate_advisory_exceptions(source: &str, now: u64) -> Result<Vec<String>, String> {
    const MAX_EXCEPTION_LIFETIME: u64 = 120 * 24 * 60 * 60;
    let document = source
        .parse::<toml::Table>()
        .map_err(|error| format!("deny.toml is not valid TOML: {error}"))?;
    let ignore = document
        .get("advisories")
        .and_then(toml::Value::as_table)
        .and_then(|advisories| advisories.get("ignore"))
        .and_then(toml::Value::as_array)
        .ok_or_else(|| "deny.toml advisories.ignore must be an array".to_owned())?;
    if ignore.is_empty() {
        return Err("deny.toml contains no advisory exception entries".into());
    }

    let mut ids = std::collections::HashSet::with_capacity(ignore.len());
    let mut expired = Vec::new();
    for (index, exception) in ignore.iter().enumerate() {
        let entry = index + 1;
        let exception = exception.as_table().ok_or_else(|| {
            format!(
                "advisory exception {entry} must be a table with id and reason; bare-string ignores are forbidden"
            )
        })?;
        let id = exception
            .get("id")
            .and_then(toml::Value::as_str)
            .filter(|id| valid_rustsec_id(id))
            .ok_or_else(|| format!("advisory exception {entry} has no valid RUSTSEC id"))?;
        if !ids.insert(id) {
            return Err(format!("advisory exception {entry} duplicates {id}"));
        }
        let reason = exception
            .get("reason")
            .and_then(toml::Value::as_str)
            .ok_or_else(|| format!("advisory exception {entry} has no machine-readable reason"))?;
        let owner = reason
            .split(';')
            .find_map(|part| part.trim().strip_prefix("owner="))
            .filter(|owner| !owner.is_empty())
            .ok_or_else(|| format!("advisory exception {entry} has no owner"))?;
        let expiry = reason
            .split(';')
            .find_map(|part| part.trim().strip_prefix("expires-unix="))
            .and_then(|value| value.parse::<u64>().ok())
            .ok_or_else(|| format!("advisory exception {entry} has no valid expires-unix"))?;
        let human_expiry = reason
            .split(';')
            .find_map(|part| part.trim().strip_prefix("expires="))
            .filter(|value| valid_iso_date(value))
            .ok_or_else(|| format!("advisory exception {entry} has no valid ISO expiry"))?;
        if expiry <= now {
            expired.push(format!(
                "deny.toml advisory exception {id} owned by {owner} expired at {human_expiry} ({expiry}); remove or upgrade the dependency, or re-review the exception"
            ));
        }
        if expiry.saturating_sub(now) > MAX_EXCEPTION_LIFETIME {
            return Err(format!(
                "advisory exception {entry} owned by {owner} exceeds the 120-day review horizon"
            ));
        }
    }
    Ok(expired)
}

fn valid_rustsec_id(value: &str) -> bool {
    let Some((year, sequence)) = value
        .strip_prefix("RUSTSEC-")
        .and_then(|tail| tail.split_once('-'))
    else {
        return false;
    };
    year.len() == 4
        && sequence.len() == 4
        && year.bytes().all(|byte| byte.is_ascii_digit())
        && sequence.bytes().all(|byte| byte.is_ascii_digit())
}

fn valid_iso_date(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| matches!(index, 4 | 7) || byte.is_ascii_digit())
}

fn check_engine_floors(strict: bool) {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|error| {
            eprintln!("engine security-floor check cannot read UTC time: {error}");
            exit(1);
        })
        .as_secs();
    let mut expired = Vec::new();
    if !zephium_core::webview2::security_floor_review_is_current(now) {
        expired.push(format!(
            "WebView2 security review (hard floor {}, latest reviewed {}) expired after {}. Review {}, {}, and exact runtime availability at {}, then update the versions, publication dates, and review deadline together.",
            zephium_core::webview2::SECURITY_FLOOR_TEXT,
            zephium_core::webview2::LATEST_REVIEWED_TEXT,
            zephium_core::webview2::SECURITY_FLOOR_REVIEW_BY,
            zephium_core::webview2::SECURITY_FLOOR_SOURCE_URL,
            zephium_core::webview2::LATEST_REVIEWED_SOURCE_URL,
            zephium_core::webview2::RUNTIME_AVAILABILITY_SOURCE_URL,
        ));
    } else {
        eprintln!(
            "WebView2 hard floor {} and latest reviewed Stable {} are reviewed through {}",
            zephium_core::webview2::SECURITY_FLOOR_TEXT,
            zephium_core::webview2::LATEST_REVIEWED_TEXT,
            zephium_core::webview2::SECURITY_FLOOR_REVIEW_BY,
        );
    }

    if !zephium_core::macos::security_floor_review_is_current(now) {
        expired.push(format!(
            "macOS/WebKit security floors expired after {}. Review {}, {}, {}, and {} and update the OS/Safari versions, publication date, and review deadline together.",
            zephium_core::macos::SECURITY_FLOOR_REVIEW_BY,
            zephium_core::macos::SECURITY_FLOOR_SOURCE_URL,
            zephium_core::macos::SAFARI_SECURITY_SOURCE_URL,
            zephium_core::macos::TAHOE_SECURITY_SOURCE_URL,
            zephium_core::macos::SEQUOIA_SECURITY_SOURCE_URL,
        ));
    } else {
        eprintln!(
        "macOS/WebKit hard floors Sonoma {}, Sequoia {}, Tahoe {}, macOS 27 {}, and Safari {}; latest recommendations Sonoma {} + Safari {}, Sequoia {}, Tahoe {}, macOS 27 {}, with Safari {}; reviewed through {}",
        zephium_core::macos::SONOMA_SECURITY_FLOOR_TEXT,
        zephium_core::macos::SEQUOIA_SECURITY_FLOOR_TEXT,
        zephium_core::macos::TAHOE_SECURITY_FLOOR_TEXT,
        zephium_core::macos::GOLDEN_GATE_SECURITY_FLOOR_TEXT,
        zephium_core::macos::SAFARI_SECURITY_FLOOR_TEXT,
        zephium_core::macos::SONOMA_RECOMMENDED_TEXT,
        zephium_core::macos::SONOMA_SAFARI_RECOMMENDED_TEXT,
        zephium_core::macos::SEQUOIA_RECOMMENDED_TEXT,
        zephium_core::macos::TAHOE_RECOMMENDED_TEXT,
        zephium_core::macos::GOLDEN_GATE_RECOMMENDED_TEXT,
        zephium_core::macos::SAFARI_RECOMMENDED_TEXT,
        zephium_core::macos::SECURITY_FLOOR_REVIEW_BY,
    );
    }

    if !zephium_core::webkitgtk::security_floor_review_is_current(now) {
        expired.push(format!(
            "WebKitGTK security floor {} expired after {}. Review {} and {} and update the advisory floor, latest-reviewed release, and deadline together.",
            zephium_core::webkitgtk::SECURITY_FLOOR_TEXT,
            zephium_core::webkitgtk::SECURITY_FLOOR_REVIEW_BY,
            zephium_core::webkitgtk::SECURITY_FLOOR_SOURCE_URL,
            zephium_core::webkitgtk::LATEST_REVIEWED_SOURCE_URL,
        ));
    } else {
        eprintln!(
            "WebKitGTK security floor {} (latest reviewed {}) is reviewed through {}",
            zephium_core::webkitgtk::SECURITY_FLOOR_TEXT,
            zephium_core::webkitgtk::LATEST_REVIEWED_TEXT,
            zephium_core::webkitgtk::SECURITY_FLOOR_REVIEW_BY,
        );
    }
    report_expired_reviews("Engine security review expired", &expired, strict);
}

/// Release-only publication gate. Runtime admission uses the best stable
/// engine that actually exists; publishing additionally requires current
/// engine and advisory reviews, that no vendor has acknowledged an
/// outstanding stable-channel security fix, and that the immutable blocker
/// sources are still within their upstream recommended refresh cadence at
/// the exact publication boundary.
fn check_release_engine_security() {
    check_agentic_probe_boundary();
    check_engine_floors(true);
    check_advisory_exceptions(true);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|error| {
            eprintln!("release engine-security check cannot read UTC time: {error}");
            exit(1);
        })
        .as_secs();
    let repository = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    if let Err(error) = blocker_seed::check_release_freshness(&repository, now) {
        eprintln!("production release is blocked by bundled blocker seed freshness: {error}");
        exit(1);
    }
    if !zephium_core::webview2::production_release_security_is_current(now) {
        eprintln!(
            "production release is blocked: Microsoft acknowledged an outstanding Chromium security fix on {} (status reviewed {}); review {} and publish only after a fixed Stable WebView2 runtime is available and the floor is updated",
            zephium_core::webview2::OUTSTANDING_VENDOR_FIX_NOTICE_ON,
            zephium_core::webview2::OUTSTANDING_VENDOR_FIX_REVIEWED_ON,
            zephium_core::webview2::OUTSTANDING_VENDOR_FIX_SOURCE_URL,
        );
        exit(1);
    }
}

fn ci() {
    share_workspace_target_dir();
    check_agentic_probe_boundary();
    check_agent_model_catalog_boundary();
    check_agent_controller_boundary();
    check_agent_runtime_boundary();
    check_engine_floors(false);
    check_advisory_exceptions(false);
    check_blocker_security_fork();
    run("cargo", &["fmt", "--all", "--", "--check"]);
    for (manifest, _) in NATIVE_ADAPTERS {
        run(
            "cargo",
            &["fmt", "--manifest-path", manifest, "--", "--check"],
        );
    }
    run(
        "cargo",
        &[
            "clippy",
            "--workspace",
            "--all-targets",
            "--",
            "-D",
            "warnings",
        ],
    );
    // The ordinary desktop graph intentionally excludes Work execution. Test
    // and lint its enabled production path explicitly; probe-harness here only
    // permits deterministic localhost fixtures, never a live provider run.
    for target in ["--all-targets", "--lib"] {
        run(
            "cargo",
            &[
                "clippy",
                "--locked",
                "-p",
                "zephium-agent-controller",
                "--features",
                "provider-transport",
                target,
                "--",
                "-D",
                "warnings",
            ],
        );
    }
    run(
        "cargo",
        &[
            "test",
            "--locked",
            "-p",
            "zephium-agent-controller",
            "--features",
            "probe-harness",
        ],
    );
    run(
        "cargo",
        &[
            "check",
            "--locked",
            "--release",
            "-p",
            "zephium-agent-controller",
            "--features",
            "provider-transport",
        ],
    );
    // Work admission and Store fencing are intentionally absent from the
    // default desktop graph; exercise their real shipping paths explicitly.
    for package in ["zephium-app", "zephium-store"] {
        for target in ["--all-targets", "--lib"] {
            run(
                "cargo",
                &[
                    "clippy",
                    "--locked",
                    "-p",
                    package,
                    "--features",
                    "work-execution",
                    target,
                    "--",
                    "-D",
                    "warnings",
                ],
            );
        }
        run(
            "cargo",
            &[
                "test",
                "--locked",
                "-p",
                package,
                "--features",
                "work-execution",
            ],
        );
        run(
            "cargo",
            &[
                "check",
                "--locked",
                "--release",
                "-p",
                package,
                "--features",
                "work-execution",
            ],
        );
    }
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    for (package, features) in [
        ("zephium-work-composition", "durable-runtime"),
        ("zephium-desktop", "work-product"),
    ] {
        for target in ["--all-targets", "--lib"] {
            run(
                "cargo",
                &[
                    "clippy",
                    "--locked",
                    "-p",
                    package,
                    "--features",
                    features,
                    target,
                    "--",
                    "-D",
                    "warnings",
                ],
            );
        }
        run(
            "cargo",
            &[
                "check",
                "--locked",
                "--release",
                "-p",
                package,
                "--features",
                features,
            ],
        );
    }
    // `--all-targets` enables test-only references while linting library
    // artifacts, which can hide dead production paths behind cfg(test).
    run(
        "cargo",
        &[
            "clippy",
            "--workspace",
            "--lib",
            "--locked",
            "--",
            "-D",
            "warnings",
        ],
    );
    for (manifest, features) in NATIVE_ADAPTERS {
        run_native_adapter_clippy(manifest, features, "--all-targets");
        run_native_adapter_clippy(manifest, features, "--lib");
    }
    #[cfg(target_os = "macos")]
    check_macos_page_permission_probe();
    // Run display-dependent AppKit/WebKit gates before the long workspace and
    // vendored-adapter test inventory. On macOS, a heavily exercised test
    // process cohort can leave LaunchServices/GPU helper admission transiently
    // unavailable even though the exact same probe passes in a fresh turn.
    // Reordering preserves one strict attempt and avoids hiding regressions
    // behind retries.
    #[cfg(target_os = "macos")]
    run_macos_principal_isolation_probe();
    // The desktop test suite regenerates frame/src/shared/ipc/bindings.ts, so the
    // frontend typecheck after it doubles as a Rust/TS drift check.
    run("cargo", &["test", "--workspace"]);
    for (manifest, features) in NATIVE_ADAPTERS {
        run_native_adapter_tests(manifest, features);
    }
    run("pnpm", &["--dir", "frame", "run", "check"]);
    run("pnpm", &["--dir", "frame", "run", "build"]);
    run("pnpm", &["--dir", "frame", "run", "check:styles"]);
}

fn check_agentic_probe_boundary() {
    let repository = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    if let Err(error) = agentic_probe_boundary::check(&repository)
        .and_then(|()| work_composition_boundary::check(&repository))
        .and_then(|()| work_persistence_boundary::check(&repository))
        .and_then(|()| work_resource_boundary::check(&repository))
        .and_then(|()| agent_runtime_boundary::check(&repository))
    {
        eprintln!("agentic diagnostic release boundary failed: {error}");
        exit(1);
    }
    let runtime = repository.join("crates/zephium-agentic/assets/semantic-runtime-v1.js");
    let runtime = runtime.to_str().unwrap_or_else(|| {
        eprintln!("semantic runtime asset path is not valid UTF-8");
        exit(1);
    });
    run("node", &["--check", runtime]);
    let smoke = repository.join("eval/agentic-browsing/semantic-runtime-smoke-v1.js");
    let smoke = smoke.to_str().unwrap_or_else(|| {
        eprintln!("semantic runtime smoke path is not valid UTF-8");
        exit(1);
    });
    run("node", &[smoke, runtime]);
    run("node", &[smoke, runtime, "--without-document-parent-brand"]);
}

fn check_agent_model_catalog_boundary() {
    let repository = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    if let Err(error) = agent_model_catalog_boundary::check(&repository) {
        eprintln!("agent model catalog architecture boundary failed: {error}");
        exit(1);
    }
}

fn check_agent_controller_boundary() {
    let repository = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    if let Err(error) = agent_controller_boundary::check(&repository) {
        eprintln!("agent controller architecture boundary failed: {error}");
        exit(1);
    }
}

fn check_agent_runtime_boundary() {
    let repository = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    if let Err(error) = agent_runtime_boundary::check(&repository) {
        eprintln!("agent runtime architecture boundary failed: {error}");
        exit(1);
    }
}

#[cfg(target_os = "macos")]
fn run_macos_principal_isolation_probe() {
    const COMMON: [&str; 7] = [
        "--locked",
        "-p",
        "zephium-engine",
        "--features",
        "native-isolation-probes",
        "--bin",
        "macos-principal-isolation-probe",
    ];

    let mut clippy = vec!["clippy"];
    clippy.extend(COMMON);
    clippy.extend(["--", "-D", "warnings"]);
    run("cargo", &clippy);

    let mut execute = vec!["run"];
    execute.extend(COMMON);
    run("cargo", &execute);
}

#[cfg(target_os = "macos")]
fn check_macos_page_permission_probe() {
    const COMMON: [&str; 7] = [
        "--locked",
        "-p",
        "zephium-engine",
        "--features",
        "native-page-permission-probes",
        "--bin",
        "macos-page-permission-probe",
    ];

    let mut clippy = vec!["clippy"];
    clippy.extend(COMMON);
    clippy.extend(["--", "-D", "warnings"]);
    run("cargo", &clippy);

    // WebKit performs real system TCC validation before it invokes
    // WKUIDelegate, even when the embedder will ultimately deny the page
    // request. Shipping WebKit does not expose the mock-device bypass used by
    // upstream's own automated build. Compile and lint the feature here, but
    // keep execution in the signed, TCC-authorized packaged release gate;
    // unattended CI must never prompt, hang, or mutate device consent.
}

fn check_blocker_security_fork() {
    share_workspace_target_dir();
    check_security_fork_locks();
    let repository = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    if let Err(error) = blocker_seed::check(&repository) {
        eprintln!("bundled blocker seed policy failed: {error}");
        exit(1);
    }
    for package in [
        "zephium-blocker",
        "zephium-blocker-service",
        "zephium-blocker-update",
    ] {
        run("cargo", &["fmt", "--package", package, "--", "--check"]);
    }
    run(
        "cargo",
        &["fmt", "--manifest-path", ADBLOCK_MANIFEST, "--", "--check"],
    );
    run(
        "cargo",
        &[
            "fmt",
            "--manifest-path",
            BLOCKER_FUZZ_MANIFEST,
            "--",
            "--check",
        ],
    );
    run_blocker_feature_gates();
    run_blocker_product_gates();
    check_blocker_dependency_graphs();
    run_adblock_fork_gates();
}

fn share_workspace_target_dir() {
    // Excluded fork manifests otherwise create independent multi-gigabyte
    // target trees. Preserve an explicit caller override, but make local gates
    // share Cargo's fingerprinted workspace output. Keep the value lexically
    // canonical: AppKit/WebKit child probes use their executable path while
    // issuing helper-process sandbox extensions, and a `..` component can make
    // that native admission fail even though the path resolves to the same
    // inode.
    if std::env::var_os("CARGO_TARGET_DIR").is_none() {
        let workspace = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("xtask must remain directly beneath the workspace root");
        let target = workspace.join("target");
        std::env::set_var("CARGO_TARGET_DIR", target);
    }
}

fn run_blocker_feature_gates() {
    for features in BLOCKER_FEATURE_SETS {
        run(
            "cargo",
            &[
                "check",
                "-p",
                "zephium-blocker",
                "--lib",
                "--locked",
                "--no-default-features",
                "--features",
                features,
            ],
        );
        run(
            "cargo",
            &[
                "clippy",
                "-p",
                "zephium-blocker",
                "--lib",
                "--tests",
                "--locked",
                "--no-default-features",
                "--features",
                features,
                "--",
                "-D",
                "warnings",
            ],
        );
        run(
            "cargo",
            &[
                "test",
                "-p",
                "zephium-blocker",
                "--lib",
                "--locked",
                "--no-default-features",
                "--features",
                features,
            ],
        );
    }
}

fn run_blocker_product_gates() {
    for (package, features) in [
        ("zephium-update-transport", None),
        ("zephium-blocker-update", None),
        ("zephium-blocker-update", Some("tuf")),
        ("zephium-blocker-update", Some("official-https")),
        ("zephium-blocker-update", Some("tuf,official-https")),
        ("zephium-blocker-service", None),
        ("zephium-blocker-service", Some("tuf")),
        ("zephium-blocker-service", Some("official-https")),
        ("zephium-blocker-service", Some("tuf,official-https")),
    ] {
        let mut common = vec![
            "-p",
            package,
            "--all-targets",
            "--locked",
            "--no-default-features",
        ];
        if let Some(features) = features {
            common.extend(["--features", features]);
        }

        let mut check = vec!["check"];
        check.extend(common.iter().copied());
        run("cargo", &check);

        let mut clippy = vec!["clippy"];
        clippy.extend(common.iter().copied());
        clippy.extend(["--", "-D", "warnings"]);
        run("cargo", &clippy);

        let mut test = vec!["test"];
        test.extend(common.iter().copied());
        run("cargo", &test);
    }
    run(
        "cargo",
        &[
            "test",
            "-p",
            "zephium-blocker",
            "--test",
            "synthetic_quality",
            "--locked",
        ],
    );
    run(
        "cargo",
        &[
            "check",
            "--manifest-path",
            BLOCKER_FUZZ_MANIFEST,
            "--locked",
            "--bins",
        ],
    );
    run(
        "cargo",
        &[
            "clippy",
            "--manifest-path",
            BLOCKER_FUZZ_MANIFEST,
            "--locked",
            "--bins",
            "--",
            "-D",
            "warnings",
        ],
    );
    run(
        "cargo",
        &[
            "run",
            "-p",
            "zephium-blocker",
            "--example",
            "synthetic_blocker_lab",
            "--locked",
            "--",
            "--rules",
            "256",
            "--requests",
            "1024",
            "--target",
            "all",
        ],
    );
}

fn check_blocker_dependency_graphs() {
    let bundled = cargo_tree(&[
        "-p",
        "zephium-desktop",
        "--no-default-features",
        "--locked",
        "-e",
        "features",
        "--prefix",
        "none",
    ]);
    for forbidden in ["zephium-blocker-update feature \"tuf\"", "tough v"] {
        if bundled.lines().any(|line| line.starts_with(forbidden)) {
            eprintln!("official HTTPS desktop graph unexpectedly contains `{forbidden}`");
            exit(1);
        }
    }
    for required in [
        "zephium-blocker-service feature \"official-https\"",
        "zephium-blocker-update feature \"official-https\"",
        "zephium-update-transport v",
        "reqwest v",
        "rustls-platform-verifier v",
    ] {
        if !bundled.lines().any(|line| line.starts_with(required)) {
            eprintln!("official HTTPS desktop graph is missing `{required}`");
            exit(1);
        }
    }
    if !bundled
        .lines()
        .any(|line| line.starts_with("zephium-blocker-update v"))
    {
        eprintln!("bundled desktop graph lost canonical blocker package validation");
        exit(1);
    }

    let tuf = cargo_tree(&[
        "-p",
        "zephium-blocker-update",
        "--no-default-features",
        "--features",
        "tuf",
        "--locked",
        "-e",
        "features",
        "--prefix",
        "none",
    ]);
    for required in [
        "zephium-blocker-update v",
        "zephium-update-transport v",
        "tough v",
        "reqwest v",
        "rustls-platform-verifier v",
        "aws-lc-rs v",
    ] {
        if !tuf.lines().any(|line| line.starts_with(required)) {
            eprintln!("TUF verification graph is missing `{required}`");
            exit(1);
        }
    }
}

fn cargo_tree(arguments: &[&str]) -> String {
    let output = Command::new("cargo")
        .arg("tree")
        .args(arguments)
        .output()
        .unwrap_or_else(|error| {
            eprintln!("failed to execute cargo tree: {error}");
            exit(1);
        });
    if !output.status.success() {
        eprintln!(
            "cargo tree failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        exit(1);
    }
    String::from_utf8(output.stdout).unwrap_or_else(|error| {
        eprintln!("cargo tree emitted non-UTF-8 output: {error}");
        exit(1);
    })
}

fn run_adblock_fork_gates() {
    for features in adblock_provenance::SHIPPING_FEATURE_SETS {
        run(
            "cargo",
            &[
                "check",
                "--manifest-path",
                ADBLOCK_MANIFEST,
                "--lib",
                "--locked",
                "--no-default-features",
                "--features",
                features,
            ],
        );
        run(
            "cargo",
            &[
                "clippy",
                "--manifest-path",
                ADBLOCK_MANIFEST,
                "--lib",
                "--locked",
                "--no-default-features",
                "--features",
                features,
                "--",
                "-D",
                "warnings",
            ],
        );
        run(
            "cargo",
            &[
                "clippy",
                "--manifest-path",
                ADBLOCK_MANIFEST,
                "--lib",
                "--tests",
                "--locked",
                "--no-default-features",
                "--features",
                features,
                "--",
                "-D",
                "warnings",
            ],
        );
        run(
            "cargo",
            &[
                "test",
                "--manifest-path",
                ADBLOCK_MANIFEST,
                "--lib",
                "--test",
                "fork_contract",
                "--locked",
                "--no-default-features",
                "--features",
                features,
            ],
        );
    }
    let exact = adblock_provenance::OPTIONAL_EXACT_FEATURES;
    run(
        "cargo",
        &[
            "check",
            "--manifest-path",
            ADBLOCK_MANIFEST,
            "--lib",
            "--locked",
            "--no-default-features",
            "--features",
            exact,
        ],
    );
    run(
        "cargo",
        &[
            "clippy",
            "--manifest-path",
            ADBLOCK_MANIFEST,
            "--lib",
            "--tests",
            "--locked",
            "--no-default-features",
            "--features",
            exact,
            "--",
            "-D",
            "warnings",
        ],
    );
    run(
        "cargo",
        &[
            "test",
            "--manifest-path",
            ADBLOCK_MANIFEST,
            "--lib",
            "--test",
            "fork_contract",
            "--locked",
            "--no-default-features",
            "--features",
            exact,
        ],
    );
    // Retain an upstream-default compatibility run, but never let its
    // single-thread/embedded-resolver graph substitute for the exact shipped
    // graph tests above.
    run(
        "cargo",
        &[
            "test",
            "--manifest-path",
            ADBLOCK_MANIFEST,
            "--lib",
            "--test",
            "fork_contract",
            "--locked",
        ],
    );
}

fn run_native_adapter_clippy(manifest: &str, features: Option<&str>, target: &str) {
    let mut args = vec!["clippy", "--manifest-path", manifest, target, "--locked"];
    if let Some(features) = features {
        args.extend(["--features", features]);
    }
    args.extend(["--", "-D", "warnings"]);
    run("cargo", &args);
}

fn run_native_adapter_tests(manifest: &str, features: Option<&str>) {
    let mut args = vec![
        "test",
        "--manifest-path",
        manifest,
        "--all-targets",
        "--locked",
    ];
    if let Some(features) = features {
        args.extend(["--features", features]);
    }
    run("cargo", &args);
}

fn run(cmd: &str, args: &[&str]) {
    eprintln!("> {cmd} {}", args.join(" "));
    let mut command = Command::new(cmd);
    command.args(args);
    if cmd == "cargo" && CI_RESOURCE_PROFILE.load(Ordering::Acquire) {
        apply_ci_cargo_resource_profile(&mut command);
    }
    let status = command
        .status()
        .unwrap_or_else(|e| panic!("failed to spawn {cmd}: {e}"));
    if !status.success() {
        exit(status.code().unwrap_or(1));
    }
}

fn apply_ci_cargo_resource_profile(command: &mut Command) {
    // The gate intentionally compiles many mutually exclusive feature graphs.
    // Incremental caches and unpacked debug information cannot be reused
    // meaningfully across those graphs and previously grew `target/` beyond
    // 100 GiB on macOS. Tests, Clippy, compile-time policy, and native runtime
    // behavior are unchanged; release-profile measurements remain governed by
    // their explicit release profile.
    command
        .env("CARGO_INCREMENTAL", "0")
        .env("CARGO_PROFILE_DEV_DEBUG", "0")
        .env("CARGO_PROFILE_TEST_DEBUG", "0");
}

#[cfg(test)]
mod tests {
    use super::{
        apply_ci_cargo_resource_profile, validate_advisory_exceptions, validate_security_fork_lock,
    };

    #[test]
    fn ci_cargo_profile_disables_nonreusable_disk_heavy_artifacts() {
        let mut command = std::process::Command::new("cargo");
        apply_ci_cargo_resource_profile(&mut command);
        let environment = command
            .get_envs()
            .map(|(key, value)| {
                (
                    key.to_string_lossy().into_owned(),
                    value
                        .expect("CI resource variables are never removed")
                        .to_string_lossy()
                        .into_owned(),
                )
            })
            .collect::<std::collections::BTreeMap<_, _>>();
        assert_eq!(
            environment.get("CARGO_INCREMENTAL").map(String::as_str),
            Some("0")
        );
        assert_eq!(
            environment
                .get("CARGO_PROFILE_DEV_DEBUG")
                .map(String::as_str),
            Some("0")
        );
        assert_eq!(
            environment
                .get("CARGO_PROFILE_TEST_DEBUG")
                .map(String::as_str),
            Some("0")
        );
    }

    #[test]
    fn advisory_exceptions_require_owner_and_short_live_expiry() {
        let now = 1_000_000;
        let valid = r#"[advisories]
ignore = [
  { id = "RUSTSEC-2026-0001", reason = "owner=security; expires=2026-01-01; expires-unix=1000100; tracked" },
]"#;
        assert_eq!(validate_advisory_exceptions(valid, now), Ok(Vec::new()));
        assert!(validate_advisory_exceptions(
            r#"[advisories]
ignore = [{ id = "RUSTSEC-2026-0001", reason = "expires=2026-01-01; expires-unix=1000100; tracked" }]"#,
            now,
        )
        .unwrap_err()
        .contains("owner"));
        assert!(validate_advisory_exceptions(
            r#"[advisories]
ignore = [{ id = "RUSTSEC-2026-0001", reason = "owner=security; expires=2026-01-01; expires-unix=1000000000; tracked" }]"#,
            now,
        )
        .unwrap_err()
        .contains("120-day"));
    }

    #[test]
    fn expired_advisory_exceptions_are_reported_not_rejected() {
        let expired = validate_advisory_exceptions(
            r#"[advisories]
ignore = [
  { id = "RUSTSEC-2026-0001", reason = "owner=security; expires=2026-01-01; expires-unix=999999; tracked" },
  { id = "RUSTSEC-2026-0002", reason = "owner=security; expires=2026-01-02; expires-unix=1000100; tracked" },
]"#,
            1_000_000,
        )
        .expect("expiry alone is not a format error");
        assert_eq!(expired.len(), 1);
        assert!(expired[0].contains("RUSTSEC-2026-0001"));
        assert!(expired[0].contains("expired at 2026-01-01"));
    }

    #[test]
    fn security_fork_locks_require_exact_local_packages() {
        let local = r#"
version = 4

[[package]]
name = "tauri"
version = "2.11.3"

[[package]]
name = "tauri-runtime-wry"
version = "2.11.3"
"#;
        let required = &[("tauri", "2.11.3"), ("tauri-runtime-wry", "2.11.3")];
        assert!(validate_security_fork_lock(local, required).is_ok());

        let registry = format!(
            "{local}\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\n"
        );
        assert!(validate_security_fork_lock(&registry, required)
            .unwrap_err()
            .contains("registry/git sourced"));

        let duplicate = format!("{local}\n[[package]]\nname = \"tauri\"\nversion = \"2.11.3\"\n");
        assert!(validate_security_fork_lock(&duplicate, required)
            .unwrap_err()
            .contains("exactly one"));
    }

    #[test]
    fn advisory_exception_policy_is_toml_structural_not_format_sensitive() {
        let now = 1_000_000;
        let multiline = r#"
[advisories]
ignore = [
  {
    id = "RUSTSEC-2026-0001",
    reason = "owner=security; expires=2026-01-01; expires-unix=1000100; tracked",
  },
]
"#;
        assert!(validate_advisory_exceptions(multiline, now).is_ok());

        let bare = r#"[advisories]
ignore = ["RUSTSEC-2026-0001"]"#;
        assert!(validate_advisory_exceptions(bare, now)
            .unwrap_err()
            .contains("bare-string"));

        let duplicate = r#"[advisories]
ignore = [
  { id = "RUSTSEC-2026-0001", reason = "owner=security; expires=2026-01-01; expires-unix=1000100; tracked" },
  { id = "RUSTSEC-2026-0001", reason = "owner=security; expires=2026-01-01; expires-unix=1000100; tracked" },
]"#;
        assert!(validate_advisory_exceptions(duplicate, now)
            .unwrap_err()
            .contains("duplicates"));
    }
}
