//! Composition-root ownership for privileged WebView2 runtime generations.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use zephium_core::runtime_security::RuntimeSecurityAdvisory;
use zephium_core::webview2::{RuntimeGeneration, RuntimeGenerationKind, UnexpectedBrowser};

static RUNTIME_STATE: OnceLock<RuntimeGeneration> = OnceLock::new();

#[derive(Debug)]
pub struct PreparedDirectories {
    pub main: PathBuf,
    pub panel: PathBuf,
}

pub fn prepare(
    data_dir: &Path,
    main_label: &str,
    panel_label: &str,
) -> io::Result<PreparedDirectories> {
    if main_label.is_empty() || panel_label.is_empty() || main_label == panel_label {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "privileged WebView labels must be distinct and nonempty",
        ));
    }
    let runtime = RuntimeGeneration::prepare(
        &data_dir.join("privileged-runtime"),
        RuntimeGenerationKind::Privileged,
        &zephium_engine::WindowsLeftoverProcesses,
    )?;
    for line in zephium_engine::generation_report_lines("privileged", runtime.report()) {
        crate::write_diagnostic(format_args!("{line}"));
    }
    // Main and the panel each start one browser, before either is recorded.
    for _ in [main_label, panel_label] {
        match runtime.expect_browser() {
            Ok(()) => {}
            Err(error @ UnexpectedBrowser::Invalidated(_)) => {
                crate::write_diagnostic(format_args!(
                    "privacy: a privileged WebView2 browser starts unexpected, so its data waits for a reboot after an unclean quit: {error}"
                ));
            }
            Err(error) => {
                return Err(io::Error::other(format!(
                    "a privileged WebView2 browser cannot start, since its data could be cleared while it runs: {error}"
                )));
            }
        }
    }
    let main = runtime.root().join(main_label);
    let panel = runtime.root().join(panel_label);
    fs::create_dir(&main)?;
    fs::create_dir(&panel)?;
    let main = zephium_core::webview2::canonical_user_data_directory(&main)?;
    let panel = zephium_core::webview2::canonical_user_data_directory(&panel)?;
    if main.parent() != Some(runtime.root()) || panel.parent() != Some(runtime.root()) {
        return Err(io::Error::other(
            "privileged WebView2 UDF escaped its owned runtime generation",
        ));
    }
    RUNTIME_STATE
        .set(runtime)
        .map_err(|_| io::Error::other("privileged WebView2 runtime initialized twice"))?;
    Ok(PreparedDirectories { main, panel })
}

/// Records a privileged browser process in this run's generation, so the
/// next launch can clear it even if this run never proves the process gone.
pub fn record_browser_process(process_id: u32) {
    let Some(runtime) = RUNTIME_STATE.get() else {
        return;
    };
    let recorded = zephium_engine::browser_process_record(process_id)
        .ok_or_else(|| io::Error::other("the browser process could not be read"))
        .and_then(|record| runtime.record_browser_process(record));
    if let Err(error) = recorded {
        let invalidated = runtime.invalidate_process_record();
        crate::write_diagnostic(format_args!(
            "privacy: a privileged WebView2 browser process was not recorded for cleanup after an unclean quit: {error}"
        ));
        // Its expectation is still pending, which keeps the generation.
        if let Err(error) = invalidated {
            crate::write_diagnostic(format_args!(
                "privacy: the privileged cleanup record could not be invalidated: {error}"
            ));
        }
    }
}

/// Chrome's advisory when preparing this run's generation left earlier
/// runtime data waiting for a reboot; the reason is already in the log.
pub fn cleanup_advisory() -> Option<RuntimeSecurityAdvisory> {
    RUNTIME_STATE
        .get()?
        .report()
        .overdue
        .is_some()
        .then(RuntimeSecurityAdvisory::cleanup_overdue)
}

/// Delete this run's generation only after the composition root has proven
/// Environment5/PID/HANDLE exit for both privileged environments.
pub fn cleanup_current_after_proven_exit() -> bool {
    let Some(runtime) = RUNTIME_STATE.get() else {
        return true;
    };
    let cleanup = runtime.cleanup_ticket();
    match cleanup.cleanup_after_proven_exit() {
        Ok(()) => true,
        Err(error) => {
            crate::write_diagnostic(format_args!(
                "privacy: could not remove proven-exited privileged WebView2 runtime data at {}: {error}",
                cleanup.root().display()
            ));
            false
        }
    }
}
