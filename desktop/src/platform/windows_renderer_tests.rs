//! Opt-in crash qualification with an owned hidden blank WebView and fresh UDF.
use super::*;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[test]
#[ignore = "crashes only an owned blank WebView2 renderer; run alone on Windows"]
fn privileged_renderer_recovers_twice_through_native_process_failed() {
    let profile = tempfile::tempdir().unwrap();
    let mut context = tauri::generate_context!();
    context.config_mut().app.windows.clear();
    context.config_mut().identifier = "app.zephium.renderer-recovery-test".into();
    let app = tauri::Builder::default()
        .any_thread()
        .build(context)
        .unwrap();
    let expected = tauri::Url::parse("about:blank").unwrap();
    let gate = crate::UiStartupGate::new(expected.clone());
    // Exercise document readiness without mapping or focusing a top-level window.
    gate.visible.store(true, Ordering::Release);
    app.manage(gate.clone());
    app.manage(Renderers::default());
    let loads = Arc::new(AtomicUsize::new(0));
    let loaded = loads.clone();
    let window = tauri::WebviewWindowBuilder::new(
        app.handle(),
        crate::MAIN_LABEL,
        tauri::WebviewUrl::External(expected),
    )
    .visible(false)
    .incognito(true)
    .data_directory(profile.path().to_owned())
    .on_page_load(move |window, payload| {
        if matches!(payload.event(), tauri::webview::PageLoadEvent::Finished) {
            gate.mark_document_loaded(&window, payload.url());
            if payload.url() == &gate.expected() && gate.mark_frontend_ready(&window) {
                loaded.fetch_add(1, Ordering::Release);
            }
        }
    })
    .build()
    .unwrap();
    let handle = app.handle().clone();
    window
        .with_webview(move |view| unsafe {
            let mut version = windows::core::PWSTR::null();
            view.environment()
                .BrowserVersionString(&mut version)
                .unwrap();
            let version = super::super::take_pwstr_bounded(version, 256, 256).unwrap();
            eprintln!("fixture: WebView2 {version}");
            install(
                &view.controller().CoreWebView2().unwrap(),
                handle,
                crate::MAIN_LABEL.into(),
            )
            .unwrap();
        })
        .unwrap();
    // Constructing at about:blank does not necessarily issue a navigation.
    window
        .navigate(tauri::Url::parse("about:blank").unwrap())
        .unwrap();
    let passed = Arc::new(AtomicBool::new(false));
    let result = passed.clone();
    let handle = app.handle().clone();
    let driver = std::thread::spawn(move || {
        let wait_for = |count| {
            let deadline = Instant::now() + Duration::from_secs(30);
            while loads.load(Ordering::Acquire) < count
                || handle
                    .state::<Renderers>()
                    .main
                    .lock()
                    .unwrap()
                    .settled_attempts()
                    < (count - 1) as u8
            {
                if Instant::now() >= deadline {
                    eprintln!(
                        "fixture: expected {count} loads; observed {}",
                        loads.load(Ordering::Acquire)
                    );
                    return false;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            true
        };
        let mut recovered = wait_for(1);
        for count in 2..=3 {
            if !recovered {
                break;
            }
            let dispatched = window.with_webview(|view| unsafe {
                let core = view.controller().CoreWebView2().unwrap();
                let done = webview2_com::CallDevToolsProtocolMethodCompletedHandler::create(
                    Box::new(|_, _| Ok(())),
                );
                // The method deliberately terminates this fixture's renderer;
                // its completion need not succeed. ProcessFailed is the witness.
                let _ = core.CallDevToolsProtocolMethod(
                    windows::core::w!("Page.crash"),
                    windows::core::w!("{}"),
                    &done,
                );
            });
            recovered = dispatched.is_ok() && wait_for(count);
        }
        if recovered {
            let renderers = handle.state::<Renderers>();
            recovered = renderers.main.lock().unwrap().crashed() == Action::Stop;
        }
        result.store(recovered, Ordering::Release);
        handle.exit(0);
    });
    app.run_return(|_, _| {});
    driver.join().unwrap();
    assert!(
        passed.load(Ordering::Acquire),
        "native renderer recovery did not settle twice"
    );
}
