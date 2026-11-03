use std::{env, error::Error, fs, io, path::Path};

use serde_json::Value;
use tauri_utils::platform::Target;

#[allow(dead_code)]
mod foreground_probe_config;
mod navigation_probe_config;

const MAIN_LABEL: &str = "main";
const DEVELOPMENT_IDENTIFIER: &str = "app.zephium.dev";
const LINUX_DESKTOP_TEMPLATE: &str = "linux/zephium.desktop.hbs";
const PACKAGE_LICENSE: &str = "MPL-2.0 AND CC-BY-SA-3.0";
const LEGAL_RESOURCES: [(&str, &str); 3] = [
    ("../LICENSE", "licenses/Zephium-MPL-2.0.txt"),
    (
        "../assets/blocker-seed/v1/LICENSE-CC-BY-SA-3.0.txt",
        "licenses/blocker/CC-BY-SA-3.0.txt",
    ),
    (
        "../assets/blocker-seed/v1/NOTICE",
        "licenses/blocker/EasyList-EasyPrivacy-NOTICE.txt",
    ),
];

// A sandboxed package (Flatpak) must use its own application ID, so the
// effective Linux ID can be set at build time; desktop/src reads it back. The
// repository configuration keeps the default.
const LINUX_APP_ID: &str = "app.zephium";

fn linux_app_id() -> String {
    env::var("ZEPHIUM_LINUX_APP_ID").unwrap_or_else(|_| LINUX_APP_ID.into())
}

fn main() {
    println!("cargo:rerun-if-env-changed=ZEPHIUM_LINUX_APP_ID");
    println!("cargo:rustc-env=ZEPHIUM_LINUX_APP_ID={}", linux_app_id());
    record_source_revision();
    if let Err(error) = validate_privileged_window_ownership() {
        eprintln!("Tauri privileged-window ownership check failed: {error}");
        std::process::exit(1);
    }
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc")
    {
        // Tauri's default manifest is this Common Controls dependency, but its
        // resource library only reaches bin targets. Emit it through the linker
        // for standalone unit tests too, without embedding a duplicate in bins.
        tauri_build::try_build(
            tauri_build::Attributes::new()
                .windows_attributes(tauri_build::WindowsAttributes::new_without_app_manifest()),
        )
        .expect("Tauri Windows build configuration");
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg=/MANIFESTDEPENDENCY:type='win32' name='Microsoft.Windows.Common-Controls' version='6.0.0.0' processorArchitecture='*' publicKeyToken='6595b64144ccf1df' language='*'");
    } else {
        tauri_build::build();
    }
}

fn record_source_revision() {
    let git = |args: &[&str]| std::process::Command::new("git").args(args).output().ok();
    let revision = git(&["rev-parse", "HEAD"])
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|revision| revision.trim().to_owned())
        .filter(|revision| {
            revision.len() == 40 && revision.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
        .unwrap_or_else(|| "unknown".into());
    let dirty = git(&["status", "--porcelain", "--untracked-files=no"])
        .filter(|output| output.status.success())
        .map(|output| !output.stdout.is_empty());
    println!(
        "cargo:rustc-env=ZEPHIUM_BUILD_REVISION={revision}{}",
        match dirty {
            Some(true) => "+dirty",
            Some(false) => "",
            None => "+state-unknown",
        }
    );
    for name in ["HEAD", "index"] {
        if let Some(path) = git(&["rev-parse", "--git-path", name])
            .filter(|output| output.status.success())
            .and_then(|output| String::from_utf8(output.stdout).ok())
        {
            println!("cargo:rerun-if-changed={}", path.trim());
        }
    }
    if let Some(reference) = git(&["symbolic-ref", "-q", "HEAD"])
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
    {
        if let Some(path) = git(&["rev-parse", "--git-path", reference.trim()])
            .filter(|output| output.status.success())
            .and_then(|output| String::from_utf8(output.stdout).ok())
        {
            println!("cargo:rerun-if-changed={}", path.trim());
        }
    }
}

fn validate_privileged_window_ownership() -> Result<(), Box<dyn Error>> {
    println!("cargo:rerun-if-env-changed=TAURI_CONFIG");
    let root = env::current_dir()?;
    for name in [
        "tauri.conf.json",
        "tauri.macos.conf.json",
        "tauri.linux.conf.json",
        "tauri.windows.conf.json",
    ] {
        // Register absent platform overlays too: adding one must immediately
        // rerun this guard instead of reusing a previously successful build.
        println!("cargo:rerun-if-changed={}", root.join(name).display());
    }
    println!(
        "cargo:rerun-if-changed={}",
        root.join(LINUX_DESKTOP_TEMPLATE).display()
    );
    for (source, _) in LEGAL_RESOURCES {
        println!("cargo:rerun-if-changed={}", root.join(source).display());
    }
    let config_override = match env::var("TAURI_CONFIG") {
        Ok(raw) => Some(serde_json::from_str::<Value>(&raw)?),
        Err(env::VarError::NotPresent) => None,
        Err(error) => return Err(Box::new(error)),
    };
    let adblock_qa = env::var_os("CARGO_FEATURE_ADBLOCK_QA").is_some();
    if adblock_qa && env::var("OPT_LEVEL").as_deref() == Ok("0") {
        return Err("protection QA must be optimized: set CARGO_PROFILE_DEV_OPT_LEVEL=2".into());
    }
    let resource_ui_qa = env::var_os("CARGO_FEATURE_RESOURCE_UI_QA").is_some();
    let work_integration_qa = env::var_os("CARGO_FEATURE_WORK_INTEGRATION_QA").is_some();
    let file_workflows_qa = env::var_os("CARGO_FEATURE_FILE_WORKFLOWS_QA").is_some();
    let isolated_ui_qa = resource_ui_qa || work_integration_qa || file_workflows_qa;
    let rendering_probe = env::var_os("CARGO_FEATURE_MACOS_WORK_RENDERING_PROBE").is_some();
    let navigation_probe = env::var_os("CARGO_FEATURE_MACOS_WORK_NAVIGATION_PROBE").is_some();
    if env::var_os("CARGO_FEATURE_MACOS_WORK_RETAINED_PRODUCT_PROBE").is_some()
        && env::var_os("CARGO_FEATURE_MACOS_WORK_DISCOVERY_PROBE").is_some()
    {
        return Err(
            "retained product and navigation discovery witnesses are mutually exclusive".into(),
        );
    }
    if navigation_probe {
        if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos")
            || env::var("PROFILE").as_deref() != Ok("debug")
            || rendering_probe
        {
            return Err(
                "navigation qualification is an isolated macOS debug-only application".into(),
            );
        }
        navigation_probe_config::validate(
            config_override
                .as_ref()
                .ok_or("navigation qualification requires its isolated configuration override")?,
        )?;
    }
    let development_identity = config_override
        .as_ref()
        .and_then(|config| config.get("identifier"))
        .and_then(Value::as_str)
        == Some(DEVELOPMENT_IDENTIFIER);
    if development_identity && env::var("PROFILE").as_deref() != Ok("debug") {
        return Err("the development identity is for debug builds only".into());
    }
    let webext_qa = env::var_os("CARGO_FEATURE_WEBEXT_QA").is_some();
    if webext_qa {
        let config = config_override
            .as_ref()
            .ok_or("the extensions QA app requires its isolated configuration override")?;
        if config.get("identifier").and_then(Value::as_str) != Some("app.zephium.webext-qa")
            || config.get("productName").and_then(Value::as_str) != Some("Zephium Extensions QA")
        {
            return Err("the extensions QA app must use its exact isolated identity".into());
        }
    }
    if [
        resource_ui_qa,
        work_integration_qa,
        file_workflows_qa,
        adblock_qa,
        webext_qa,
    ]
    .into_iter()
    .filter(|enabled| *enabled)
    .count()
        > 1
    {
        return Err("product QA identities are mutually exclusive".into());
    }
    if isolated_ui_qa || adblock_qa {
        let target = env::var("CARGO_CFG_TARGET_OS")?;
        let supported_target =
            target == "macos" || ((file_workflows_qa || adblock_qa) && target == "windows");
        if !supported_target
            || env::var("PROFILE").as_deref() != Ok("debug")
            || rendering_probe
            || navigation_probe
        {
            return Err("product UI QA requires an isolated supported-platform debug build".into());
        }
        let (qa_id, qa_name) = if adblock_qa {
            ("app.zephium.protection-qa", "Zephium Protection QA")
        } else if file_workflows_qa {
            (
                "app.zephium.files-integration-qa",
                "Zephium Files Integration QA",
            )
        } else if work_integration_qa {
            ("app.zephium.work-integration", "Zephium Work Integration")
        } else {
            ("app.zephium.resources-qa", "Zephium Resources QA")
        };
        let config = config_override
            .as_ref()
            .ok_or("product UI QA requires an explicit isolated configuration")?;
        if config.get("identifier").and_then(Value::as_str) != Some(qa_id)
            || config.get("productName").and_then(Value::as_str) != Some(qa_name)
            || config
                .pointer("/app/windows/0/title")
                .and_then(Value::as_str)
                != Some(qa_name)
            || config.pointer("/build/devUrl") != Some(&Value::Null)
        {
            return Err(
                "product UI QA requires its exact isolated identity and bundled frontend".into(),
            );
        }
    }
    if rendering_probe {
        if env::var_os("CARGO_FEATURE_MACOS_WORK_RESOURCE_PROBE").is_some()
            && env::var_os("CARGO_FEATURE_MACOS_WORK_RETAINED_CONTROLLER_PROBE").is_some()
        {
            return Err("provider-free retention and paid retained-controller witnesses are mutually exclusive".into());
        }
        if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos")
            || env::var("PROFILE").as_deref() != Ok("debug")
            || env::var_os("CARGO_FEATURE_MACOS_WORK").is_some()
        {
            return Err("the rendering probe is an isolated macOS debug-only application".into());
        }
        foreground_probe_config::validate(
            config_override
                .as_ref()
                .ok_or("the rendering probe requires its isolated configuration override")?,
        )?;
    }
    for target in [Target::MacOS, Target::Linux, Target::Windows] {
        let (mut config, paths) = tauri_utils::config::parse::read_from(target, &root)?;
        for path in paths {
            println!("cargo:rerun-if-changed={}", path.display());
        }
        validate_target_window(target, "repository", &config)?;
        validate_legal_resources("repository", &config, &root)?;
        if matches!(target, Target::Linux) {
            validate_linux_identity("repository", LINUX_APP_ID, &config, &root)?;
        }
        if let Some(config_override) = &config_override {
            // Keep this identical to tauri-build's TAURI_CONFIG merge order.
            json_patch::merge(&mut config, config_override);
            validate_target_window(target, "effective", &config)?;
            validate_legal_resources("effective", &config, &root)?;
            // The override is supplied for the current build target. A Windows
            // measurement identity does not rename the Linux desktop package;
            // the repository Linux identity was validated independently above.
            if matches!(target, Target::Linux)
                && env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux")
                && !rendering_probe
                && !navigation_probe
                && !isolated_ui_qa
                && !webext_qa
                && !adblock_qa
                && !development_identity
            {
                validate_linux_identity("effective", &linux_app_id(), &config, &root)?;
            }
        }
    }
    Ok(())
}

fn validate_legal_resources(source: &str, config: &Value, root: &Path) -> io::Result<()> {
    if config.pointer("/bundle/license").and_then(Value::as_str) != Some(PACKAGE_LICENSE) {
        return Err(io::Error::other(format!(
            "{source} configuration must declare package license `{PACKAGE_LICENSE}`"
        )));
    }
    let resources = config
        .pointer("/bundle/resources")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            io::Error::other(format!(
                "{source} configuration must package the application and blocker legal resources"
            ))
        })?;
    for (path, destination) in LEGAL_RESOURCES {
        if resources.get(path).and_then(Value::as_str) != Some(destination) {
            return Err(io::Error::other(format!(
                "{source} configuration must map legal resource `{path}` to `{destination}`"
            )));
        }
        let metadata = fs::symlink_metadata(root.join(path))?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(io::Error::other(format!(
                "legal resource `{path}` must be a regular non-symlink file"
            )));
        }
    }
    Ok(())
}

fn validate_linux_identity(
    source: &str,
    linux_app_id: &str,
    config: &Value,
    root: &Path,
) -> io::Result<()> {
    // tauri-bundler 2.9.4 derives the installed desktop basename from
    // productName. Keep basename, GTK application id, and portal Registry id
    // on one canonical value while the template retains the visible name.
    if config.get("productName").and_then(Value::as_str) != Some(linux_app_id) {
        return Err(io::Error::other(format!(
            "Linux {source} productName must remain `{linux_app_id}` so the installed desktop entry is exactly {linux_app_id}.desktop"
        )));
    }
    if config.get("identifier").and_then(Value::as_str) != Some(linux_app_id) {
        return Err(io::Error::other(format!(
            "Linux {source} identifier must remain `{linux_app_id}`"
        )));
    }
    if config
        .pointer("/app/enableGTKAppId")
        .and_then(Value::as_bool)
        != Some(true)
    {
        return Err(io::Error::other(format!(
            "Linux {source} configuration must set app.enableGTKAppId=true"
        )));
    }
    for pointer in [
        "/bundle/linux/deb/desktopTemplate",
        "/bundle/linux/rpm/desktopTemplate",
    ] {
        if config.pointer(pointer).and_then(Value::as_str) != Some(LINUX_DESKTOP_TEMPLATE) {
            return Err(io::Error::other(format!(
                "Linux {source} configuration must set {pointer} to `{LINUX_DESKTOP_TEMPLATE}`"
            )));
        }
    }
    validate_linux_desktop_template(&root.join(LINUX_DESKTOP_TEMPLATE))
}

fn validate_linux_desktop_template(path: &Path) -> io::Result<()> {
    let template = fs::read_to_string(path)?;
    // `%U` is how the desktop hands a clicked link to the default browser.
    for exact in [
        "Exec={{exec}} %U",
        "StartupWMClass=app.zephium",
        "Icon={{icon}}",
        "Name=Zephium",
        "Terminal=false",
        "Type=Application",
    ] {
        if template.lines().filter(|line| *line == exact).count() != 1 {
            return Err(io::Error::other(format!(
                "Linux desktop template must contain exactly one `{exact}` entry"
            )));
        }
    }
    let handles_web = template.lines().any(|line| {
        line.strip_prefix("MimeType=").is_some_and(|types| {
            let types: Vec<_> = types.split(';').collect();
            types.contains(&"x-scheme-handler/http") && types.contains(&"x-scheme-handler/https")
        })
    });
    if !handles_web {
        return Err(io::Error::other(
            "Linux desktop template must declare the http and https scheme handlers",
        ));
    }
    if template.contains("Name={{name}}") {
        return Err(io::Error::other(
            "Linux desktop template must not expose productName as the visible application name",
        ));
    }
    Ok(())
}

fn validate_target_window(target: Target, source: &str, config: &Value) -> io::Result<()> {
    let windows = config
        .pointer("/app/windows")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            io::Error::other(format!(
                "{target} {source} configuration has no app.windows array"
            ))
        })?;
    if windows.len() != 1 {
        return Err(io::Error::other(format!(
            "{target} {source} configuration must expose exactly one Rust-owned main window template"
        )));
    }
    let main = &windows[0];
    if main.get("label").and_then(Value::as_str) != Some(MAIN_LABEL) {
        return Err(io::Error::other(format!(
            "{target} {source} configuration must preserve the `{MAIN_LABEL}` window label"
        )));
    }
    if main.get("create").and_then(Value::as_bool) != Some(false) {
        return Err(io::Error::other(format!(
            "{target} {source} configuration must set app.windows[0].create=false; Rust exclusively owns privileged WebView construction"
        )));
    }
    Ok(())
}
