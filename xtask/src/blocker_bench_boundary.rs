//! Mechanical checks that the protection QA benchmark driver stays inside the
//! isolated `adblock-qa` application (a debug or `protection-qa` profile build):
//! the engine feature behind it is enabled by that one desktop feature and
//! nothing else, and the sources keep every definition and hook behind the
//! matching `cfg`.

use std::path::{Path, PathBuf};

use toml::Value;

const DESKTOP_MANIFEST: &str = "desktop/Cargo.toml";
const ENGINE_MANIFEST: &str = "crates/zephium-engine/Cargo.toml";
const DESKTOP_LIB: &str = "desktop/src/lib.rs";
const ENGINE_ROOT: &str = "crates/zephium-engine";
const QA_FEATURE: &str = "adblock-qa";
const ENGINE_FEATURE: &str = "blocker-bench";
const ENGINE_FEATURE_IN_DESKTOP: &str = "zephium-engine/blocker-bench";
const DRIVER_FILE: &str = "blocker_bench.rs";
const TOKENS: [&str; 2] = ["blocker_bench", "blocker-bench"];
/// Trees without workspace manifests or sources of ours.
const SKIPPED: [&str; 6] = ["target", "node_modules", ".git", "vendor", "dist", "gen"];
const DESKTOP_RELEASE_REFUSAL: &str = concat!(
    "#[cfg(all(feature=\"adblock-qa\",any(not(any(debug_assertions,zephium_protection_qa_profile)),",
    "not(any(target_os=\"macos\",target_os=\"windows\")))))]",
    "compile_error!(\"protectionQArequiresamacOSorWindowsdebugorprotection-qabuild\");"
);
const ENGINE_RELEASE_REFUSAL: &str = concat!(
    "#[cfg(all(feature=\"blocker-bench\",not(any(debug_assertions,zephium_protection_qa_profile))))]",
    "compile_error!(\"theblockerbenchmarkdriverisforbiddeninoptimizedbuildsoutsideprotection-qa\");"
);
const ENGINE_DRIVER_MODULE: &str = concat!(
    "#[cfg(all(feature=\"blocker-bench\",",
    "any(target_os=\"macos\",target_os=\"windows\")))]modblocker_bench;"
);

pub(crate) fn check(repository: &Path) -> Result<(), String> {
    for path in files(repository, "Cargo.toml")? {
        let relative = relative(repository, &path);
        validate_manifest(&relative, &read(&path)?)?;
    }
    for path in files(&repository.join("crates"), "rs")?
        .into_iter()
        .chain(files(&repository.join("desktop"), "rs")?)
    {
        let relative = relative(repository, &path);
        let source = read(&path)?;
        if relative.starts_with(ENGINE_ROOT) {
            validate_engine_source(&relative, &source)?;
        } else if relative.starts_with("desktop/src/") {
            validate_desktop_source(&relative, &source)?;
        } else if mentions_driver(&source) {
            return Err(format!("{relative} may not know the benchmark driver"));
        }
    }
    let engine_root = compact(&read(&repository.join(ENGINE_ROOT).join("src/lib.rs"))?);
    let host = compact(&read(
        &repository.join(ENGINE_ROOT).join("src/host/mod.rs"),
    )?);
    let desktop = compact(&read(&repository.join(DESKTOP_LIB))?);
    for (label, source, required) in [
        ("engine root", &engine_root, ENGINE_RELEASE_REFUSAL),
        ("engine host", &host, ENGINE_DRIVER_MODULE),
        ("desktop root", &desktop, DESKTOP_RELEASE_REFUSAL),
        (
            "desktop root",
            &desktop,
            "#[cfg(feature=\"adblock-qa\")]modblocker_bench;",
        ),
    ] {
        if !source.contains(required) {
            return Err(format!("{label} lost its benchmark driver gate {required}"));
        }
    }
    Ok(())
}

/// The desktop `adblock-qa` feature is the only way in: no other manifest, and
/// no other feature or dependency table, may name the engine feature or the
/// QA feature.
fn validate_manifest(relative: &str, source: &str) -> Result<(), String> {
    let manifest: Value = toml::from_str(source)
        .map_err(|error| format!("{relative} is invalid TOML for the benchmark gate: {error}"))?;
    let table = manifest
        .as_table()
        .ok_or_else(|| format!("{relative} is not a manifest table"))?;
    for (section, value) in table {
        if section == "features" && relative == DESKTOP_MANIFEST {
            validate_desktop_features(value)?;
        } else if section == "features" && relative == ENGINE_MANIFEST {
            validate_engine_features(value)?;
        } else if mentions_feature(value) {
            return Err(format!(
                "{relative} [{section}] reaches the benchmark driver outside adblock-qa"
            ));
        }
    }
    Ok(())
}

fn validate_desktop_features(features: &Value) -> Result<(), String> {
    let features = features
        .as_table()
        .ok_or("desktop features are not a table")?;
    let qa = features
        .get(QA_FEATURE)
        .and_then(Value::as_array)
        .ok_or("desktop lost its adblock-qa feature")?;
    if !qa
        .iter()
        .any(|entry| entry.as_str() == Some(ENGINE_FEATURE_IN_DESKTOP))
    {
        return Err("adblock-qa no longer enables the engine benchmark feature".into());
    }
    for (name, entries) in features {
        if name == QA_FEATURE {
            continue;
        }
        if name == ENGINE_FEATURE || mentions_feature(entries) {
            return Err(format!(
                "desktop feature {name} reaches the benchmark driver; only adblock-qa may"
            ));
        }
    }
    Ok(())
}

fn validate_engine_features(features: &Value) -> Result<(), String> {
    let features = features
        .as_table()
        .ok_or("engine features are not a table")?;
    if !features
        .get(ENGINE_FEATURE)
        .and_then(Value::as_array)
        .is_some_and(Vec::is_empty)
    {
        return Err("the engine benchmark feature must enable nothing else".into());
    }
    for (name, entries) in features {
        if name != ENGINE_FEATURE && mentions_feature(entries) {
            return Err(format!(
                "engine feature {name} enables the benchmark driver; no feature may"
            ));
        }
    }
    Ok(())
}

/// Any string, key or nested value naming the QA feature or the engine feature,
/// bare or as `crate/feature`.
fn mentions_feature(value: &Value) -> bool {
    let names = |text: &str| {
        [QA_FEATURE, ENGINE_FEATURE]
            .iter()
            .any(|feature| text == *feature || text.ends_with(&format!("/{feature}")))
    };
    match value {
        Value::String(text) => names(text),
        Value::Array(items) => items.iter().any(mentions_feature),
        Value::Table(table) => table
            .iter()
            .any(|(key, value)| names(key) || mentions_feature(value)),
        _ => false,
    }
}

fn validate_engine_source(relative: &str, source: &str) -> Result<(), String> {
    // The driver's own file is gated by its `mod` declaration, checked above.
    if relative.ends_with(&format!("/{DRIVER_FILE}")) {
        return Ok(());
    }
    let lines: Vec<&str> = source.lines().collect();
    for (index, line) in lines.iter().enumerate() {
        if is_definition(line) && mentions_driver(line) {
            let gate = compact(&attributes_above(&lines, index));
            if !gate.contains("feature=\"blocker-bench\"") {
                return Err(format!(
                    "{relative}:{} defines benchmark code outside feature blocker-bench",
                    index + 1
                ));
            }
        }
    }
    Ok(())
}

fn validate_desktop_source(relative: &str, source: &str) -> Result<(), String> {
    if relative.ends_with(&format!("/{DRIVER_FILE}")) {
        return Ok(());
    }
    if relative != DESKTOP_LIB {
        return if mentions_driver(source) {
            Err(format!("{relative} may not know the benchmark driver"))
        } else {
            Ok(())
        };
    }
    let lines: Vec<&str> = source.lines().collect();
    for (index, line) in lines.iter().enumerate() {
        if line.trim_start().starts_with("//") || !mentions_driver(line) {
            continue;
        }
        let gate = compact(&attributes_above(&lines, index));
        if !gate.contains("#[cfg(feature=\"adblock-qa\")]") {
            return Err(format!(
                "{relative}:{} reaches the benchmark driver outside feature adblock-qa",
                index + 1
            ));
        }
    }
    Ok(())
}

fn mentions_driver(text: &str) -> bool {
    text.lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .any(|line| TOKENS.iter().any(|token| line.contains(token)))
}

fn is_definition(line: &str) -> bool {
    let line = line.trim_start();
    let line = line.strip_prefix("pub(crate) ").unwrap_or(line);
    let line = line.strip_prefix("pub ").unwrap_or(line);
    [
        "fn ", "mod ", "struct ", "enum ", "static ", "const ", "type ", "trait ", "use ",
    ]
    .iter()
    .any(|keyword| line.starts_with(keyword))
}

/// The attributes and comments directly above an item or statement: every
/// contiguous line up to a blank one or the end of the previous item.
fn attributes_above(lines: &[&str], index: usize) -> String {
    let mut block = lines[index].to_owned();
    for previous in lines[..index].iter().rev() {
        let trimmed = previous.trim();
        let attribute_like = trimmed.starts_with("//")
            || trimmed
                .chars()
                .last()
                .is_some_and(|last| !matches!(last, ';' | '{' | '}'));
        if !attribute_like {
            break;
        }
        block.push('\n');
        block.push_str(trimmed);
    }
    block
}

fn compact(source: &str) -> String {
    source.chars().filter(|c| !c.is_whitespace()).collect()
}

fn relative(repository: &Path, path: &Path) -> String {
    path.strip_prefix(repository)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn read(path: &Path) -> Result<String, String> {
    std::fs::read_to_string(path)
        .map_err(|error| format!("benchmark gate cannot read {}: {error}", path.display()))
}

/// Every file with this name or extension under `directory` that the
/// repository tracks or would track. Ignored trees, such as other checkouts
/// under `.claude/worktrees`, and the skipped ones hold none of ours.
fn files(directory: &Path, name_or_extension: &str) -> Result<Vec<PathBuf>, String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(directory)
        .args([
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
        ])
        .output()
        .map_err(|error| {
            format!(
                "benchmark gate cannot run git in {}: {error}",
                directory.display()
            )
        })?;
    if !output.status.success() {
        return Err(format!(
            "benchmark gate cannot list the files in {}: {}",
            directory.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let listing = String::from_utf8(output.stdout).map_err(|_| {
        format!(
            "benchmark gate got a non-UTF-8 path under {}",
            directory.display()
        )
    })?;
    let mut found = Vec::new();
    for relative in listing.split('\0').filter(|path| !path.is_empty()) {
        let relative = Path::new(relative);
        let name = relative
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        let wanted = name == name_or_extension
            || relative
                .extension()
                .is_some_and(|extension| extension == name_or_extension);
        let skipped = relative
            .components()
            .any(|part| SKIPPED.iter().any(|skip| part.as_os_str() == *skip));
        if !wanted || skipped {
            continue;
        }
        let path = directory.join(relative);
        // A tracked file deleted from the working tree is listed but gone.
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(format!(
                    "benchmark gate cannot inspect {}: {error}",
                    path.display()
                ))
            }
        };
        if metadata.file_type().is_file() {
            found.push(path);
        }
    }
    found.sort();
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DESKTOP: &str = "[features]\ndefault = [\"work-product\"]\nwork-product = []\nadblock-qa = [\"tauri/custom-protocol\", \"zephium-engine/blocker-bench\"]\n[dependencies]\nzephium-engine = { workspace = true }\n";
    const ENGINE: &str = "[features]\nagentic-browser-qa = []\nblocker-bench = []\n";

    #[test]
    fn only_files_the_repository_tracks_or_would_track_are_checked() {
        let root = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            let status = std::process::Command::new("git")
                .arg("-C")
                .arg(root.path())
                .args(args)
                .status()
                .unwrap();
            assert!(status.success());
        };
        git(&["init", "-q"]);
        for (path, text) in [
            (".gitignore", ".claude/\n"),
            ("tracked/Cargo.toml", ""),
            ("new/Cargo.toml", ""),
            (".claude/worktrees/agent-1/Cargo.toml", ""),
            ("vendor/Cargo.toml", ""),
        ] {
            let path = root.path().join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        git(&["add", "tracked/Cargo.toml"]);
        let found: Vec<String> = files(root.path(), "Cargo.toml")
            .unwrap()
            .iter()
            .map(|path| relative(root.path(), path))
            .collect();
        assert_eq!(found, ["new/Cargo.toml", "tracked/Cargo.toml"]);
    }

    #[test]
    fn repository_passes() {
        let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        check(&repository).expect("benchmark driver boundary");
        assert!(files(&repository.join("crates"), "rs").unwrap().len() > 100);
    }

    #[test]
    fn only_the_qa_feature_may_enable_the_engine_feature() {
        assert!(validate_manifest(DESKTOP_MANIFEST, DESKTOP).is_ok());
        assert!(validate_manifest(ENGINE_MANIFEST, ENGINE).is_ok());
        for desktop in [
            DESKTOP.replace("default = [\"work-product\"]", "default = [\"adblock-qa\"]"),
            DESKTOP.replace(
                "work-product = []",
                "work-product = [\"zephium-engine/blocker-bench\"]",
            ),
            DESKTOP.replace(
                "{ workspace = true }",
                "{ workspace = true, features = [\"blocker-bench\"] }",
            ),
            DESKTOP.replace(", \"zephium-engine/blocker-bench\"", ""),
        ] {
            assert!(
                validate_manifest(DESKTOP_MANIFEST, &desktop).is_err(),
                "{desktop}"
            );
        }
        for engine in [
            ENGINE.replace(
                "blocker-bench = []",
                "blocker-bench = [\"agentic-browser-qa\"]",
            ),
            ENGINE.replace(
                "agentic-browser-qa = []",
                "agentic-browser-qa = [\"blocker-bench\"]",
            ),
        ] {
            assert!(
                validate_manifest(ENGINE_MANIFEST, &engine).is_err(),
                "{engine}"
            );
        }
    }

    #[test]
    fn no_other_manifest_may_reach_either_feature() {
        let other = "[features]\nqa = [\"zephium-engine/blocker-bench\"]\n";
        assert!(validate_manifest("crates/other/Cargo.toml", other).is_err());
        let dependency =
            "[dependencies]\nzephium-desktop = { path = \"../desktop\", features = [\"adblock-qa\"] }\n";
        assert!(validate_manifest("xtask/Cargo.toml", dependency).is_err());
        assert!(validate_manifest("xtask/Cargo.toml", "[features]\ndefault = []\n").is_ok());
    }

    #[test]
    fn engine_definitions_need_the_feature_attribute() {
        let gated = "#[cfg(all(\n    feature = \"blocker-bench\",\n    any(target_os = \"macos\", target_os = \"windows\")\n))]\n#[doc(hidden)]\npub fn blocker_bench_evaluate() {}\n";
        assert!(validate_engine_source("crates/zephium-engine/src/lib.rs", gated).is_ok());
        let ungated =
            "fn other() {}\n\n/// QA only.\n#[doc(hidden)]\npub fn blocker_bench_evaluate() {}\n";
        assert!(validate_engine_source("crates/zephium-engine/src/lib.rs", ungated).is_err());
        let wrong_feature = "#[cfg(feature = \"agentic-browser-qa\")]\nmod blocker_bench;\n";
        assert!(
            validate_engine_source("crates/zephium-engine/src/host/mod.rs", wrong_feature).is_err()
        );
    }

    #[test]
    fn desktop_hooks_need_the_qa_feature_attribute() {
        let gated =
            "fn a() {\n    #[cfg(feature = \"adblock-qa\")]\n    blocker_bench::start();\n}\n";
        assert!(validate_desktop_source(DESKTOP_LIB, gated).is_ok());
        let ungated = "fn a() {\n    let x = 1;\n    blocker_bench::start();\n}\n";
        assert!(validate_desktop_source(DESKTOP_LIB, ungated).is_err());
        assert!(validate_desktop_source(
            "desktop/src/other.rs",
            "fn a() { blocker_bench::start(); }"
        )
        .is_err());
        assert!(validate_desktop_source("desktop/src/blocker_bench.rs", ungated).is_ok());
    }

    #[test]
    fn crates_other_than_the_engine_may_not_know_the_driver() {
        assert!(mentions_driver("let flag = \"--blocker-bench\";"));
        assert!(!mentions_driver("// the blocker-bench driver\nlet x = 1;"));
    }
}
