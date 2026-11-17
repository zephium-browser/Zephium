//! Explicit Windows Work qualification against the shared loopback replicas.
#![forbid(unsafe_code)]
#![deny(clippy::dbg_macro, clippy::print_stderr, clippy::print_stdout)]

#[cfg(not(debug_assertions))]
compile_error!("Windows Work qualification is forbidden in optimized builds");

#[cfg(target_os = "windows")]
mod assist_bench;
#[cfg(target_os = "windows")]
mod assist_scenarios;
#[cfg(target_os = "windows")]
mod loopback_support;
#[cfg(target_os = "windows")]
use loopback_support as acceptance;
#[cfg(target_os = "windows")]
mod windows_retained_probe;
#[cfg(target_os = "windows")]
mod windows_work_storage_fixture;
#[cfg(target_os = "windows")]
mod work_app_editors;
#[cfg(target_os = "windows")]
mod work_app_views;
#[cfg(target_os = "windows")]
#[path = "windows_work_probe.rs"]
mod work_durable;
#[cfg(target_os = "windows")]
mod work_site;

#[cfg(target_os = "windows")]
#[derive(Debug)]
enum ProbeFailure {
    Authority,
    Runtime,
    Keychain,
    Verification,
    Engine(&'static str),
}

#[cfg(target_os = "windows")]
fn main() {
    use std::io::Write as _;
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let result = match args.as_slice() {
        [flag, scenario] if flag == "--assist-bench" => assist_bench::run(scenario),
        [flag] if flag == "--assist-bench-list" => {
            for scenario in assist_scenarios::SCENARIOS.iter().filter(|s| !s.live) {
                let _ = writeln!(
                    std::io::stdout().lock(),
                    "{}\tloopback\t{}",
                    scenario.name,
                    scenario.request
                );
            }
            Ok(())
        }
        [flag, scenario] if flag == "--loopback-site" => {
            windows_work_storage_fixture::run(scenario)
        }
        [flag] if flag == "--native-retained-lifecycle" => windows_retained_probe::run(),
        [flag] if flag == "--native-first-preview" => windows_retained_probe::run_first_preview(),
        [flag] if flag == "--native-wikipedia-read" => windows_retained_probe::run_wikipedia_read(),
        [flag] if flag == "--native-cookie-session" => windows_retained_probe::run_cookie_session(),
        [flag] if flag == "--native-cookie-session-no-observation" => {
            windows_retained_probe::run_cookie_session_without_observation()
        }
        [flag] if flag == "--cookie-persistence" || flag == "--long-udf" => {
            zephium_engine::run_windows_cookie_persistence_control().map_err(|reason| {
                let _ = writeln!(
                    std::io::stderr().lock(),
                    "windows-cookie-control: failure={reason}; content=redacted"
                );
                ProbeFailure::Verification
            })
        }
        [flag] if flag == "--check-provider-keychain" => {
            zephium_agentic::load_probe_openai_credential()
                .map(|_| ())
                .map_err(|reason| {
                    let _ = writeln!(
                        std::io::stderr().lock(),
                        "windows-work-probe: keychain_failure={reason:?}; content=redacted"
                    );
                    ProbeFailure::Keychain
                })
        }
        [flag] if flag == "--native-action-guard" => {
            match zephium_engine::run_windows_semantic_action_guard_probe(1) {
                Ok([true, true, true, true, true]) => {
                    let _ = writeln!(std::io::stdout().lock(),
                        "windows-action-guard: refused=true trusted_beforeinput=true admitted_unchanged=true decoy_unchanged=true teardown_complete=true; content=redacted");
                    Ok(())
                }
                Ok(_) => Err(ProbeFailure::Verification),
                Err(reason) => {
                    let _ = writeln!(
                        std::io::stderr().lock(),
                        "windows-action-guard: failure={reason:?}; content=redacted"
                    );
                    Err(ProbeFailure::Verification)
                }
            }
        }
        _ => Err(ProbeFailure::Authority),
    };
    match result {
        Ok(()) => {
            let _ = writeln!(
                std::io::stdout().lock(),
                "windows-work-probe: passed; content=redacted"
            );
        }
        Err(ProbeFailure::Engine(stage)) => {
            let _ = writeln!(
                std::io::stderr().lock(),
                "windows-work-probe: failed; reason=Engine; stage={stage}; content=redacted"
            );
            std::process::exit(1);
        }
        Err(reason) => {
            let _ = writeln!(
                std::io::stderr().lock(),
                "windows-work-probe: failed; reason={reason:?}; content=redacted"
            );
            std::process::exit(1);
        }
    }
}

#[cfg(not(target_os = "windows"))]
fn main() {
    std::process::exit(2);
}
