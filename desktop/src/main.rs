// Prevents an extra console window on Windows in release.
#![cfg_attr(
    any(not(debug_assertions), feature = "webext-qa"),
    windows_subsystem = "windows"
)]

fn main() -> std::process::ExitCode {
    let profile = match profile_argument(std::env::args_os().skip(1)) {
        Ok(profile) => profile,
        Err(message) => {
            eprintln!("{message}");
            return std::process::ExitCode::from(2);
        }
    };
    #[cfg(all(target_os = "macos", feature = "work-product"))]
    zephium_app::work_commands::private_guard_main();
    #[cfg(all(target_os = "macos", feature = "work-integration-qa", debug_assertions))]
    if let Some((_, stage)) = [
        (
            "--probe-interactive-safari-government",
            zephium_engine::LivenessStage::Safari,
        ),
        (
            "--probe-interactive-scripts-government",
            zephium_engine::LivenessStage::BrowseScripts,
        ),
        (
            "--probe-interactive-gate-government",
            zephium_engine::LivenessStage::DocumentGate,
        ),
    ]
    .into_iter()
    .find(|(flag, _)| std::env::args_os().skip(1).eq([*flag]))
    {
        return if zephium_engine::run_interactive_liveness_probe(
            zephium_engine::LivenessSite::Government,
            stage,
        )
        .is_ok()
        {
            std::process::ExitCode::SUCCESS
        } else {
            std::process::ExitCode::FAILURE
        };
    }
    #[cfg(all(target_os = "macos", feature = "work-integration-qa", debug_assertions))]
    if std::env::args_os()
        .skip(1)
        .eq(["--probe-interactive-government"])
    {
        return if zephium_engine::run_interactive_government_probe().is_ok() {
            std::process::ExitCode::SUCCESS
        } else {
            std::process::ExitCode::FAILURE
        };
    }
    #[cfg(all(target_os = "macos", feature = "work-integration-qa", debug_assertions))]
    if let Some((_, stage)) = [
        (
            "--probe-bare-government",
            zephium_engine::LivenessStage::Bare,
        ),
        (
            "--probe-safari-government",
            zephium_engine::LivenessStage::Safari,
        ),
        (
            "--probe-scripts-government",
            zephium_engine::LivenessStage::BrowseScripts,
        ),
        (
            "--probe-gate-government",
            zephium_engine::LivenessStage::DocumentGate,
        ),
    ]
    .into_iter()
    .find(|(flag, _)| std::env::args_os().skip(1).eq([*flag]))
    {
        return if zephium_engine::run_liveness_probe(
            zephium_engine::LivenessSite::Government,
            stage,
        )
        .is_ok()
        {
            std::process::ExitCode::SUCCESS
        } else {
            std::process::ExitCode::FAILURE
        };
    }
    #[cfg(all(target_os = "macos", feature = "work-integration-qa", debug_assertions))]
    if std::env::args_os()
        .skip(1)
        .eq(["--check-provider-keychain"])
    {
        use std::io::Write as _;
        let started = std::time::Instant::now();
        let available = zephium_agentic::load_macos_development_openai_credential().is_ok();
        let _ = writeln!(
            std::io::stderr(),
            "keychain_check available={available} elapsed_ms={}",
            started.elapsed().as_millis()
        );
        return if available {
            std::process::ExitCode::SUCCESS
        } else {
            std::process::ExitCode::FAILURE
        };
    }
    zephium_desktop_lib::run_with_profile(profile);
    std::process::ExitCode::SUCCESS
}

fn profile_argument(
    args: impl IntoIterator<Item = std::ffi::OsString>,
) -> Result<Option<String>, String> {
    let mut args = args.into_iter();
    let mut profile = None;
    while let Some(argument) = args.next() {
        if argument == "--profile" {
            if profile.is_some() {
                return Err("--profile may be provided only once".into());
            }
            let value = args
                .next()
                .ok_or_else(|| "--profile requires a name".to_owned())?;
            let value = value
                .into_string()
                .map_err(|_| "--profile must be valid UTF-8".to_owned())?;
            if value.starts_with("--") {
                return Err("--profile requires a name".into());
            }
            let canonical = zephium_core::profiles::profile_name(&value)
                .filter(|name| name == &value)
                .ok_or_else(|| "--profile requires a valid, non-empty profile name".to_owned())?;
            profile = Some(canonical);
        } else if let Some(value) = argument
            .to_str()
            .and_then(|value| value.strip_prefix("--profile="))
        {
            if profile.is_some() {
                return Err("--profile may be provided only once".into());
            }
            let canonical = zephium_core::profiles::profile_name(value)
                .filter(|name| name == value)
                .ok_or_else(|| "--profile requires a valid, non-empty profile name".to_owned())?;
            profile = Some(canonical);
        }
    }
    Ok(profile)
}

#[cfg(test)]
mod profile_argument_tests {
    use super::profile_argument;

    fn args(values: &[&str]) -> Vec<std::ffi::OsString> {
        values.iter().map(std::ffi::OsString::from).collect()
    }

    #[test]
    fn profile_argument_accepts_name_and_equals_forms() {
        assert_eq!(
            profile_argument(args(&["--profile", "Work"]))
                .unwrap()
                .as_deref(),
            Some("Work")
        );
        assert_eq!(
            profile_argument(args(&["--profile=Work"]))
                .unwrap()
                .as_deref(),
            Some("Work")
        );
    }

    #[test]
    fn profile_argument_rejects_missing_empty_noncanonical_and_duplicate_values() {
        for values in [
            vec!["--profile"],
            vec!["--profile", "--user-data-dir", "/tmp"],
            vec!["--profile", ""],
            vec!["--profile", " Work "],
            vec!["--profile=Work", "--profile", "Home"],
        ] {
            assert!(profile_argument(args(&values)).is_err());
        }
    }
}
