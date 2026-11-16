// Prevents an extra console window on Windows in release.
#![cfg_attr(
    any(not(debug_assertions), feature = "webext-qa"),
    windows_subsystem = "windows"
)]

#[cfg(target_os = "linux")]
mod linux_graphics;

fn main() -> std::process::ExitCode {
    #[cfg(target_os = "linux")]
    if let Some(status) = linux_graphics::prepare() {
        return status;
    }
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
    zephium_desktop_lib::run();
    std::process::ExitCode::SUCCESS
}
