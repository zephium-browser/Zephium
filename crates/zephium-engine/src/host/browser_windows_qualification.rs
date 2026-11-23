//! Opt-in native boundary checks using disposable files and a loopback server.
use std::cell::RefCell;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::num::NonZeroIsize;
use std::os::windows::io::{FromRawHandle, OwnedHandle};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use raw_window_handle::{
    HandleError, HasWindowHandle, RawWindowHandle, Win32WindowHandle, WindowHandle,
};
use webview2_com::Microsoft::Web::WebView2::Win32::*;
use webview2_com::StateChangedEventHandler;
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
use windows::Win32::UI::WindowsAndMessaging::*;
use wry::{WebViewBuilder, WebViewBuilderExtWindows, WebViewExtWindows};

struct Host(HWND);
impl Host {
    fn new() -> Self {
        // The built-in window class needs no process-global registration.
        Self(unsafe {
            CreateWindowExW(
                WS_EX_TOOLWINDOW,
                windows::core::w!("STATIC"),
                windows::core::w!("Zephium native qualification"),
                WS_OVERLAPPEDWINDOW,
                0,
                0,
                800,
                600,
                None,
                None,
                None,
                None,
            )
            .unwrap()
        })
    }
}
impl HasWindowHandle for Host {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let handle = NonZeroIsize::new(self.0 .0 as isize).ok_or(HandleError::Unavailable)?;
        // This borrowed handle cannot outlive the owned native window.
        Ok(unsafe {
            WindowHandle::borrow_raw(RawWindowHandle::Win32(Win32WindowHandle::new(handle)))
        })
    }
}
impl Drop for Host {
    fn drop(&mut self) {
        let _ = unsafe { DestroyWindow(self.0) };
    }
}
fn until(mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !ready() {
        assert!(Instant::now() < deadline, "native boundary timed out");
        unsafe {
            let mut message = MSG::default();
            while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
            MsgWaitForMultipleObjectsEx(None, 20, QS_ALLINPUT, MWMO_INPUTAVAILABLE);
        }
    }
}

#[test]
#[ignore = "requires installed WebView2; uses a disposable profile and inline pages"]
fn native_page_memory_survives_tab_switches() {
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok().unwrap();
    }
    let profile = tempfile::tempdir().unwrap();
    let host = Host::new();
    // A child view is only really visible inside a shown parent.
    unsafe {
        let _ = ShowWindow(host.0, SW_SHOWNOACTIVATE);
    }
    let mut context = wry::WebContext::new(Some(profile.path().to_owned()));
    let mut views = Vec::new();
    for _ in 0..2 {
        let loaded = Arc::new(Mutex::new(false));
        let page_loaded = loaded.clone();
        let view = WebViewBuilder::new_with_web_context(&mut context)
            .with_visible(false)
            .with_focused(false)
            .with_html("<!doctype html><title>Memory fixture</title><iframe srcdoc='<p>Child frame</p>'></iframe>")
            .with_on_page_load_handler(move |event, _| {
                if matches!(event, wry::PageLoadEvent::Finished) {
                    *page_loaded.lock().unwrap() = true;
                }
            })
            .build_as_child(&host)
            .unwrap();
        until(|| *loaded.lock().unwrap());
        views.push(view);
    }
    let frames: Vec<_> = views
        .iter()
        .map(|view| crate::platform::imp::main_frame_id(view).unwrap())
        .collect();
    assert_ne!(frames[0], frames[1]);
    for turn in 0..12 {
        for (index, view) in views.iter().enumerate() {
            view.set_visible(index == turn % 2).unwrap();
        }
        let result = Rc::new(RefCell::new(None));
        let completed = result.clone();
        assert!(crate::platform::imp::sample_renderers(
            &views[0].environment(),
            move |renderers| *completed.borrow_mut() = Some(renderers),
        ));
        until(|| result.borrow().is_some());
        let renderers = result.borrow_mut().take().unwrap();
        for frame in &frames {
            assert!(
                renderers
                    .iter()
                    .any(|(bytes, pages)| *bytes > 0 && pages.contains(frame)),
                "memory sample must include both the visible and hidden page",
            );
        }
    }
}

#[test]
#[ignore = "requires installed WebView2; uses a disposable profile and loopback download"]
fn native_download_staging_and_media_capabilities() {
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok().unwrap();
    }
    let profile = tempfile::tempdir().unwrap();
    let output = tempfile::tempdir().unwrap();
    let host = Host::new();
    unsafe {
        let _ = ShowWindow(host.0, SW_SHOWNOACTIVATE);
    }
    let mut context = wry::WebContext::new(Some(profile.path().to_owned()));
    let destination = super::download_files::Destination::prepare(
        zephium_core::ids::DownloadId::generate(),
        output.path().join("fixture.txt"),
        None,
    )
    .unwrap();
    let payload = destination.payload();
    let result = Rc::new(RefCell::new(None::<Result<(), String>>));
    let callback = result.clone();
    let loaded = Arc::new(Mutex::new(false));
    let page_loaded = loaded.clone();
    let menu = Rc::new(RefCell::new(Vec::new()));
    let menu_report = menu.clone();
    let view = WebViewBuilder::new_with_web_context(&mut context)
        .with_visible(true)
        .with_focused(false)
        .with_devtools(false)
        .with_picture_in_picture_enabled(true)
        .with_native_context_menu_handler(move |_, args| {
            let items = unsafe { args.MenuItems() }.unwrap();
            let mut count = 0;
            unsafe { items.Count(&mut count) }.unwrap();
            for index in 0..count.min(64) {
                let item = unsafe { items.GetValueAtIndex(index) }.unwrap();
                let mut name = windows::core::PWSTR::null();
                unsafe { item.Name(&mut name) }.unwrap();
                if !name.is_null() {
                    let text = unsafe { name.to_string() }.unwrap();
                    unsafe {
                        windows::Win32::System::Com::CoTaskMemFree(Some(name.0.cast()));
                    }
                    if matches!(text.as_str(), "pictureInPciture" | "pictureInPicture") {
                        let mut command = 0;
                        unsafe { item.CommandId(&mut command) }.unwrap();
                        unsafe { args.SetSelectedCommandId(command) }.unwrap();
                    }
                    menu_report.borrow_mut().push(text);
                }
            }
            false
        })
        .with_on_page_load_handler(move |event, url| {
            if matches!(event, wry::PageLoadEvent::Finished) && url.starts_with("http://127.0.0.1:")
            {
                *page_loaded.lock().unwrap() = true;
            }
        })
        .with_native_download_handler(move |_, args| {
            println!("DOWNLOAD_START");
            let outcome = (|| -> windows::core::Result<()> {
                let operation = unsafe { args.DownloadOperation()? };
                let state_result = callback.clone();
                let handler = StateChangedEventHandler::create(Box::new(move |operation, _| {
                    let Some(operation) = operation else {
                        return Ok(());
                    };
                    let mut state = COREWEBVIEW2_DOWNLOAD_STATE::default();
                    unsafe {
                        operation.State(&mut state)?;
                    }
                    if state == COREWEBVIEW2_DOWNLOAD_STATE_COMPLETED {
                        *state_result.borrow_mut() = Some(Ok(()));
                    } else if state == COREWEBVIEW2_DOWNLOAD_STATE_INTERRUPTED {
                        let mut reason = COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON::default();
                        unsafe {
                            operation.InterruptReason(&mut reason)?;
                        }
                        *state_result.borrow_mut() =
                            Some(Err(format!("native interrupt {}", reason.0)));
                    }
                    Ok(())
                }));
                let mut token = 0;
                unsafe {
                    operation.add_StateChanged(&handler, &mut token)?;
                    args.SetResultFilePath(&windows::core::HSTRING::from(payload.as_os_str()))?;
                    args.SetCancel(false)?;
                }
                Ok(())
            })();
            if let Err(error) = outcome {
                *callback.borrow_mut() = Some(Err(format!("destination API: {error}")));
            }
        })
        .build_as_child(&host)
        .unwrap();
    let capabilities = Arc::new(Mutex::new(None));
    let reported = capabilities.clone();
    view.evaluate_script_with_callback("JSON.stringify({video:document.pictureInPictureEnabled,request:typeof HTMLVideoElement.prototype.requestPictureInPicture,document:typeof documentPictureInPicture})", move |value| {
        *reported.lock().unwrap() = Some(value);
    }).unwrap();
    until(|| capabilities.lock().unwrap().is_some());
    println!(
        "PIP_CAPABILITIES {}",
        capabilities.lock().unwrap().as_ref().unwrap()
    );
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = format!(
        "http://127.0.0.1:{}/fixture",
        listener.local_addr().unwrap().port()
    );
    let server = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if let Ok((mut socket, _)) = listener.accept() {
                socket.set_nonblocking(false).unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                socket
                    .set_write_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut request = [0; 4096];
                let count = socket.read(&mut request).unwrap();
                if String::from_utf8_lossy(&request[..count]).starts_with("GET / HTTP") {
                    socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: 15\r\nConnection: close\r\n\r\n<!doctype html>").unwrap();
                    continue;
                }
                if !String::from_utf8_lossy(&request[..count]).starts_with("GET /fixture HTTP") {
                    socket.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
                    continue;
                }
                socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Disposition: attachment; filename=fixture.txt\r\nContent-Length: 8\r\nConnection: close\r\n\r\ndownload").unwrap();
                println!("DOWNLOAD_RESPONSE");
                return;
            }
            assert!(Instant::now() < deadline, "fixture received no request");
            std::thread::sleep(Duration::from_millis(5));
        }
    });
    view.load_url(address.trim_end_matches("fixture")).unwrap();
    until(|| *loaded.lock().unwrap());
    view.evaluate_script("location.href='/fixture'").unwrap();
    until(|| result.borrow().is_some());
    server.join().unwrap();
    assert_eq!(result.borrow_mut().take().unwrap(), Ok(()));
    let (saved, receipt) = destination.finish("http://127.0.0.1", Some(8)).unwrap();
    assert_eq!(std::fs::read(&saved).unwrap(), b"download");
    super::download_files::verify_file(&saved, &receipt).unwrap();
    let pip = protocol(
        &view,
        "Runtime.evaluate",
        serde_json::json!({
            "expression": r#"(async () => {
          const c = document.createElement('canvas'); c.width=32; c.height=32;
          const paint = () => c.getContext('2d').fillRect(0,0,32,32);
          const timer = setInterval(paint, 30); paint();
          const stream = c.captureStream(30);
          const v = document.createElement('video'); v.muted=true; v.srcObject=stream;
          v.style.cssText='position:fixed;left:0;top:0;width:320px;height:240px';
          document.body.append(v);
          try {
            await v.play(); await v.requestPictureInPicture();
            const opened = document.pictureInPictureElement === v;
            await document.exitPictureInPicture();
            return opened && !document.pictureInPictureElement;
          } finally { window.stopFixtureVideo = () => { clearInterval(timer); stream.getTracks().forEach(t => t.stop()); v.remove(); }; }
        })()"#,
            "awaitPromise": true, "returnByValue": true, "userGesture": true
        }),
    );
    println!("PIP_ROUNDTRIP {pip}");
    assert_eq!(
        pip.pointer("/result/value"),
        Some(&serde_json::Value::Bool(true))
    );
    for kind in ["mousePressed", "mouseReleased"] {
        protocol(
            &view,
            "Input.dispatchMouseEvent",
            serde_json::json!({
                "type": kind, "x": 50, "y": 50, "button": "right", "clickCount": 1
            }),
        );
    }
    until(|| !menu.borrow().is_empty());
    println!("VIDEO_MENU {:?}", menu.borrow());
    assert!(menu.borrow().iter().any(|name| {
        matches!(name.as_str(), "pictureInPciture" | "pictureInPicture")
            && super::page_open::windows_menu_item_allowed(name)
    }));
    let menu_pip = protocol(
        &view,
        "Runtime.evaluate",
        serde_json::json!({
            "expression": r#"(async () => {
            const until = performance.now() + 5000;
            while (!document.pictureInPictureElement && performance.now() < until)
                await new Promise(resolve => setTimeout(resolve, 16));
            if (!document.pictureInPictureElement) return false;
            await document.exitPictureInPicture(); return true;
        })()"#,
            "awaitPromise": true, "returnByValue": true
        }),
    );
    assert_eq!(
        menu_pip.pointer("/result/value"),
        Some(&serde_json::Value::Bool(true))
    );
    view.evaluate_script("document.querySelector('video').muted=false")
        .unwrap();
    view.set_visible(false).unwrap();
    let automatic = include_str!("../platform/automatic_pip.js");
    crate::platform::imp::automatic_pip(
        &view,
        format!("({automatic})('leave',1)"),
        Rc::new(|| true),
    );
    let opened = protocol(
        &view,
        "Runtime.evaluate",
        serde_json::json!({
            "expression": "(async()=>{const end=performance.now()+5000;while(!document.pictureInPictureElement&&performance.now()<end)await new Promise(r=>setTimeout(r,30));return !!document.pictureInPictureElement})()",
            "awaitPromise":true,"returnByValue":true,
        }),
    );
    assert_eq!(
        opened.pointer("/result/value"),
        Some(&serde_json::Value::Bool(true)),
        "hidden-tab automatic PiP: {opened}"
    );
    view.set_visible(true).unwrap();
    crate::platform::imp::automatic_pip(
        &view,
        format!("({automatic})('return',2)"),
        Rc::new(|| true),
    );
    let closed = protocol(
        &view,
        "Runtime.evaluate",
        serde_json::json!({
            "expression": "(async()=>{const end=performance.now()+5000;while(document.pictureInPictureElement&&performance.now()<end)await new Promise(r=>setTimeout(r,30));return !document.pictureInPictureElement})()",
            "awaitPromise":true,"returnByValue":true,
        }),
    );
    assert_eq!(
        closed.pointer("/result/value"),
        Some(&serde_json::Value::Bool(true)),
        "automatic PiP return: {closed}"
    );
    view.evaluate_script("window.stopFixtureVideo()").unwrap();
    let mut process_id = 0;
    unsafe { view.webview().BrowserProcessId(&mut process_id) }.unwrap();
    use windows::Win32::System::Threading::{
        OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE,
    };
    let process = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, process_id) }.unwrap();
    let _process = unsafe { OwnedHandle::from_raw_handle(process.0) };
    drop(view);
    drop(context);
    until(
        || unsafe { WaitForSingleObject(process, 0) } == windows::Win32::Foundation::WAIT_OBJECT_0,
    );
    profile.close().unwrap();
    // The native writer must not remove provenance after reporting completion
    // or while its browser process exits.
    super::download_files::verify_file(&saved, &receipt).unwrap();
    unsafe {
        windows::Win32::System::Com::CoUninitialize();
    }
}

fn protocol(view: &wry::WebView, method: &str, parameters: serde_json::Value) -> serde_json::Value {
    let result = Rc::new(RefCell::new(None));
    let reported = result.clone();
    let handler = webview2_com::CallDevToolsProtocolMethodCompletedHandler::create(Box::new(
        move |status, response| {
            *reported.borrow_mut() = Some(status.map(|_| response));
            Ok(())
        },
    ));
    unsafe {
        view.webview()
            .CallDevToolsProtocolMethod(
                &windows::core::HSTRING::from(method),
                &windows::core::HSTRING::from(parameters.to_string()),
                &handler,
            )
            .unwrap();
    }
    until(|| result.borrow().is_some());
    let response = result.borrow_mut().take().unwrap().unwrap();
    serde_json::from_str(&response).unwrap()
}

#[test]
#[ignore = "requires installed WebView2; uses disposable image fixtures and downloads"]
fn native_resource_save_reaches_broker_only_for_current_foreground_menu() {
    use std::cell::Cell;
    use windows::core::Interface;
    unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok().unwrap() };
    for (link, policy, foreground, stale, expected) in [
        (false, wry::DownloadPolicy::UseHandlers, true, false, true),
        (true, wry::DownloadPolicy::UseHandlers, true, false, true),
        (false, wry::DownloadPolicy::UseHandlers, false, false, false),
        (false, wry::DownloadPolicy::UseHandlers, true, true, false),
        (
            false,
            wry::DownloadPolicy::DenyWithoutMetadata,
            true,
            false,
            false,
        ),
    ] {
        let profile = tempfile::tempdir().unwrap();
        let output = tempfile::tempdir().unwrap();
        let host = Host::new();
        unsafe {
            let _ = ShowWindow(host.0, SW_SHOWNOACTIVATE);
        }
        let destination = super::download_files::Destination::prepare(
            zephium_core::ids::DownloadId::generate(),
            output
                .path()
                .join(if link { "fixture.txt" } else { "image.png" }),
            None,
        )
        .unwrap();
        let payload = destination.payload();
        let navigation = crate::navigation_epoch::NavigationEpochTracker::new();
        navigation.begin("https://fixture.example/image").unwrap();
        let resource = Rc::new(super::page_open::ResourceSave::default());
        let menu_resource = resource.clone();
        let save_resource = resource.clone();
        let download_resource = resource.clone();
        let menu_navigation = navigation.clone();
        let save_navigation = navigation.clone();
        let download_navigation = navigation.clone();
        let loaded = Rc::new(Cell::new(false));
        let page_loaded = loaded.clone();
        let selected = Rc::new(Cell::new(false));
        let chosen = selected.clone();
        let finished = Rc::new(RefCell::new(None));
        let completed = finished.clone();
        let started = Rc::new(Cell::new(false));
        let download_started = started.clone();
        let save_decisions = Rc::new(RefCell::new(Vec::new()));
        let decisions = save_decisions.clone();
        let mut context = wry::WebContext::new(Some(profile.path().to_owned()));
        let view = WebViewBuilder::new_with_web_context(&mut context)
            .with_permission_handler(|_| wry::PermissionResponse::Deny)
            .with_download_policy(policy)
            .with_on_page_load_handler(move |event, _| {
                if matches!(event, wry::PageLoadEvent::Finished) { page_loaded.set(true); }
            })
            .with_html(if link {
                "<a style='display:block;width:200px;height:200px' href='data:text/plain;base64,Zml4dHVyZQ=='>Save link fixture</a>"
            } else {
                "<img style='width:200px;height:200px' src='data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jD1sAAAAASUVORK5CYII='>"
            })
            .with_native_context_menu_handler(move |_, args| {
                menu_resource.menu(&menu_navigation, args, true);
                if stale { menu_navigation.begin("https://fixture.example/replaced").unwrap(); }
                let items = unsafe { args.MenuItems() }.unwrap();
                let mut count = 0;
                unsafe { items.Count(&mut count) }.unwrap();
                for index in 0..count {
                    let item = unsafe { items.GetValueAtIndex(index) }.unwrap();
                    let mut name = windows::core::PWSTR::null();
                    unsafe { item.Name(&mut name) }.unwrap();
                    let text = unsafe { name.to_string() }.unwrap();
                    unsafe { windows::Win32::System::Com::CoTaskMemFree(Some(name.0.cast())) };
                    if text == if link { "saveLinkAs" } else { "saveImageAs" } {
                        let mut command = 0;
                        unsafe { item.CommandId(&mut command).unwrap(); args.SetSelectedCommandId(command).unwrap(); }
                        chosen.set(true);
                    }
                }
                false
            })
            .with_native_resource_save_handler(move |_| {
                save_resource.authorize(&save_navigation, foreground)
            })
            .with_native_download_handler(move |_, args| {
                assert!(download_resource.take_download(&download_navigation), "Save As must request a destination picker");
                assert!(!download_resource.take_download(&download_navigation), "the picker request is consumed exactly once");
                download_started.set(true);
                let operation = unsafe { args.DownloadOperation() }.unwrap();
                let completed = completed.clone();
                let handler = StateChangedEventHandler::create(Box::new(move |operation, _| {
                    let Some(operation) = operation else { return Ok(()); };
                    let mut state = COREWEBVIEW2_DOWNLOAD_STATE::default();
                    unsafe { operation.State(&mut state)?; }
                    if state == COREWEBVIEW2_DOWNLOAD_STATE_COMPLETED {
                        *completed.borrow_mut() = Some(true);
                    } else if state == COREWEBVIEW2_DOWNLOAD_STATE_INTERRUPTED {
                        *completed.borrow_mut() = Some(false);
                    }
                    Ok(())
                }));
                let mut token = 0;
                unsafe {
                    operation.add_StateChanged(&handler, &mut token).unwrap();
                    args.SetResultFilePath(&windows::core::HSTRING::from(payload.as_os_str())).unwrap();
                    args.SetCancel(false).unwrap();
                }
            })
            .build_as_child(&host).unwrap();
        let core = view.webview().cast::<ICoreWebView2_25>().unwrap();
        let mut token = 0;
        unsafe {
            core.add_SaveAsUIShowing(
                &webview2_com::SaveAsUIShowingEventHandler::create(Box::new(move |_, args| {
                    let args = args.unwrap();
                    let mut cancelled = windows::core::BOOL::default();
                    let mut suppressed = windows::core::BOOL::default();
                    args.Cancel(&mut cancelled)?;
                    args.SuppressDefaultDialog(&mut suppressed)?;
                    decisions
                        .borrow_mut()
                        .push((cancelled.as_bool(), suppressed.as_bool()));
                    Ok(())
                })),
                &mut token,
            )
            .unwrap();
        }
        until(|| loaded.get());
        protocol(
            &view,
            "Runtime.evaluate",
            serde_json::json!({
            "expression":"new Promise(r=>{let i=document.querySelector('img');if(!i||i.complete)r(true);else i.onload=()=>r(true)})",
                "awaitPromise":true
            }),
        );
        // A Save As without a menu or an explicit browser command remains denied.
        let result = Rc::new(Cell::new(false));
        let done = result.clone();
        unsafe {
            core.ShowSaveAsUI(&webview2_com::ShowSaveAsUICompletedHandler::create(
                Box::new(move |_, _| {
                    done.set(true);
                    Ok(())
                }),
            ))
            .unwrap();
        }
        until(|| result.get());
        assert_eq!(save_decisions.borrow().as_slice(), &[(true, true)]);
        save_decisions.borrow_mut().clear();
        for kind in ["mousePressed", "mouseReleased"] {
            protocol(
                &view,
                "Input.dispatchMouseEvent",
                serde_json::json!({
                    "type":kind,"x":50,"y":50,"button":"right","clickCount":1
                }),
            );
        }
        until(|| selected.get());
        until(|| !save_decisions.borrow().is_empty());
        assert_eq!(save_decisions.borrow().as_slice(), &[(!expected, true)]);
        if expected {
            until(|| finished.borrow().is_some());
            assert_eq!(*finished.borrow(), Some(true));
            let (saved, receipt) = destination.finish("https://fixture.example", None).unwrap();
            let bytes = std::fs::read(&saved).unwrap();
            if link {
                assert_eq!(bytes, b"fixture");
            } else {
                assert!(bytes.starts_with(b"\x89PNG\r\n\x1a\n"));
            }
            super::download_files::verify_file(&saved, &receipt).unwrap();
        }
        assert_eq!(started.get(), expected);
    }
}

struct QualificationDownloadStore {
    origin: zephium_core::permissions::PageOrigin,
    preferences: zephium_core::downloads::DownloadPreferences,
    records: Mutex<Vec<zephium_core::downloads::DownloadRecord>>,
}

impl zephium_core::ports::store::Store for QualificationDownloadStore {
    fn save_session(&self, _: zephium_core::session::SessionState) {}
    fn flush(&self) -> bool {
        true
    }
    fn load_session(&self) -> zephium_core::ports::store::SessionLoad {
        zephium_core::ports::store::SessionLoad::Absent
    }
    fn record_visit(&self, _: zephium_core::ids::ProfileId, _: String, _: String) {}
    fn app_setting(&self, _: &str) -> Option<String> {
        None
    }
    fn set_app_setting(&self, _: String, _: String) -> bool {
        true
    }
    fn search_history(
        &self,
        _: zephium_core::ids::ProfileId,
        _: &str,
        _: u32,
    ) -> Vec<zephium_core::ports::store::HistoryHit> {
        Vec::new()
    }
    fn history_page(
        &self,
        _: zephium_core::ids::ProfileId,
        _: &str,
        _: Option<i64>,
        _: Option<i64>,
        _: u32,
    ) -> Vec<zephium_core::ports::store::HistoryVisit> {
        Vec::new()
    }
    fn forget_history_urls(&self, _: zephium_core::ids::ProfileId, _: &[String]) -> u32 {
        0
    }
    fn clear_history(&self, _: zephium_core::ids::ProfileId, _: Option<i64>) -> u32 {
        0
    }
    fn amend_visit_title(&self, _: zephium_core::ids::ProfileId, _: String, _: String) -> bool {
        false
    }
    fn favicon_age(&self, _: zephium_core::ids::ProfileId, _: &str) -> Option<i64> {
        None
    }
    fn save_favicon(
        &self,
        _: zephium_core::ids::ProfileId,
        _: String,
        _: Option<String>,
        _: Vec<u8>,
    ) {
    }
    fn favicon_bytes(
        &self,
        _: zephium_core::ids::ProfileId,
        _: &str,
    ) -> Option<(Option<String>, Vec<u8>)> {
        None
    }
    fn download_sites(
        &self,
        _: zephium_core::ids::ProfileId,
    ) -> Option<Vec<(zephium_core::permissions::PageOrigin, bool)>> {
        Some(vec![(self.origin.clone(), true)])
    }
    fn download_call(
        &self,
        _: zephium_core::ids::ProfileId,
        call: zephium_core::downloads::DownloadStoreCall,
        done: Box<dyn FnOnce(zephium_core::downloads::DownloadStoreReply) + Send>,
    ) -> bool {
        use zephium_core::downloads::{DownloadStoreCall, DownloadStoreReply};
        let reply = match call {
            DownloadStoreCall::Preferences => {
                DownloadStoreReply::Preferences(self.preferences.clone())
            }
            DownloadStoreCall::Save(record) => {
                let mut records = self.records.lock().unwrap();
                if let Some(existing) = records.iter_mut().find(|item| item.id == record.id) {
                    *existing = *record;
                } else {
                    records.push(*record);
                }
                DownloadStoreReply::Saved
            }
            DownloadStoreCall::List { .. } => {
                let mut records = self.records.lock().unwrap().clone();
                records.sort_by_key(|right| std::cmp::Reverse(right.created_at));
                DownloadStoreReply::Page(records)
            }
            DownloadStoreCall::RecoveryPage { .. } => DownloadStoreReply::Page(Vec::new()),
            _ => DownloadStoreReply::Saved,
        };
        done(reply);
        true
    }
}

#[test]
#[ignore = "requires installed WebView2; verifies live coordinator cancel and truncation recovery"]
fn native_download_coordinator_settles_truncation_and_cancel_without_duplicates() {
    use std::cell::Cell;
    use std::sync::atomic::AtomicBool;
    use zephium_core::downloads::{
        DownloadCall, DownloadCompletion, DownloadError, DownloadPreferences, DownloadResponse,
        DownloadState, DownloadView,
    };
    use zephium_core::ids::{ItemId, ProfileId};
    use zephium_core::permissions::PageOrigin;
    use zephium_core::ports::engine::Partition;

    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok().unwrap();
    }
    let profile_dir = tempfile::tempdir().unwrap();
    let download_dir = tempfile::tempdir().unwrap();
    let (directory, identity) =
        super::download_files::select_directory(download_dir.path().to_path_buf()).unwrap();
    let host = Host::new();
    unsafe {
        let _ = ShowWindow(host.0, SW_SHOWNOACTIVATE);
    }

    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();
    let base_url = format!("http://127.0.0.1:{port}/");
    let origin = PageOrigin::from_url(&url::Url::parse(&base_url).unwrap()).unwrap();
    let stop_server = Arc::new(AtomicBool::new(false));
    let stop_flag = stop_server.clone();
    let server = std::thread::spawn(move || {
        let mut workers = Vec::new();
        while !stop_flag.load(std::sync::atomic::Ordering::Acquire) {
            let Ok((mut socket, _)) = listener.accept() else {
                std::thread::sleep(Duration::from_millis(5));
                continue;
            };
            let worker = std::thread::spawn(move || {
                socket.set_nonblocking(false).unwrap();
                let _ = socket.set_read_timeout(Some(Duration::from_secs(2)));
                let _ = socket.set_write_timeout(Some(Duration::from_secs(2)));
                let mut req = [0u8; 4096];
                let Ok(read) = socket.read(&mut req) else {
                    return;
                };
                let head = String::from_utf8_lossy(&req[..read]);
                if head.starts_with("GET / HTTP") {
                    let body = b"<!doctype html><a id='t' href='/trunc.zip' download='trunc.zip'>t</a><a id='s' href='/slow.zip' download='slow.zip'>s</a>";
                    let header = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = socket.write_all(header.as_bytes());
                    let _ = socket.write_all(body);
                } else if head.starts_with("GET /trunc.zip HTTP") {
                    let header = "HTTP/1.1 200 OK\r\nContent-Type: application/zip\r\nContent-Disposition: attachment; filename=\"trunc.zip\"\r\nContent-Length: 524288\r\nConnection: close\r\n\r\n";
                    let _ = socket.write_all(header.as_bytes());
                    let _ = socket.write_all(&vec![b'T'; 65536]);
                } else if head.starts_with("GET /slow.zip HTTP") {
                    let header = "HTTP/1.1 200 OK\r\nContent-Type: application/zip\r\nContent-Disposition: attachment; filename=\"slow.zip\"\r\nContent-Length: 524288\r\nConnection: close\r\n\r\n";
                    let _ = socket.write_all(header.as_bytes());
                    if socket.write_all(&vec![b'S'; 65536]).is_err() {
                        return;
                    }
                    let chunk = vec![b'S'; 512];
                    for _ in 0..200 {
                        std::thread::sleep(Duration::from_millis(50));
                        if socket.write_all(&chunk).is_err() {
                            break;
                        }
                    }
                } else {
                    let _ = socket.write_all(
                        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    );
                }
            });
            workers.push(worker);
        }
        for worker in workers {
            let _ = worker.join();
        }
    });

    let profile = ProfileId::generate();
    let partition = Partition::Persistent(profile);
    let store = Arc::new(QualificationDownloadStore {
        origin: origin.clone(),
        preferences: DownloadPreferences {
            directory: Some(directory),
            directory_identity: Some(identity),
            ask_destination: false,
        },
        records: Mutex::new(Vec::new()),
    });
    let changed = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let changed_hits = changed.clone();
    let downloads = super::downloads::Downloads::new(
        store.clone(),
        Arc::new(move |_| {
            changed_hits.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        }),
    );
    downloads.set_sites(profile, vec![(origin.clone(), true)]);

    let list_downloads = |manager: &Rc<super::downloads::Downloads>| -> Vec<DownloadView> {
        let reply = Arc::new(Mutex::new(None));
        let slot = reply.clone();
        manager.call(
            partition,
            DownloadCall::List {
                before: None,
                limit: 20,
            },
            DownloadCompletion::new(Box::new(move |response| {
                if let DownloadResponse::Page { entries, .. } = response {
                    *slot.lock().unwrap() = Some(entries);
                } else {
                    *slot.lock().unwrap() = Some(Vec::new());
                }
            })),
        );
        until(|| reply.lock().unwrap().is_some());
        let entries = reply.lock().unwrap().take().unwrap();
        entries
    };

    let mut context = wry::WebContext::new(Some(profile_dir.path().to_owned()));
    let permit_token = Arc::new(AtomicBool::new(true));
    let permit = super::permits::EventPermit::bound(&permit_token);
    let surface_intent = Arc::new(AtomicBool::new(true));
    let sink = super::permits::Sink::new(Arc::new(|_| {}));
    let item = Rc::new(Cell::new(ItemId::generate()));

    let build_loaded_view = |context: &mut wry::WebContext| {
        let navigation = crate::navigation_epoch::NavigationEpochTracker::new();
        let _ = navigation.begin(&base_url);
        let loaded = Arc::new(Mutex::new(false));
        let page_loaded = loaded.clone();
        let admit_downloads = Rc::downgrade(&downloads);
        let admit_permit = permit.clone();
        let admit_intent = surface_intent.clone();
        let admit_origin = origin.clone();
        let view_item = item.clone();
        let view_sink = sink.clone();
        let view = WebViewBuilder::new_with_web_context(context)
            .with_visible(true)
            .with_focused(false)
            .with_devtools(false)
            .with_on_page_load_handler(move |event, url| {
                if matches!(event, wry::PageLoadEvent::Finished)
                    && url.starts_with("http://127.0.0.1:")
                {
                    *page_loaded.lock().unwrap() = true;
                }
            })
            .with_download_policy(wry::DownloadPolicy::UseHandlers)
            .with_native_download_handler(move |controller, args| {
                if let Some(manager) = admit_downloads.upgrade() {
                    manager.admit(
                        partition,
                        admit_permit.clone(),
                        admit_intent.clone(),
                        navigation.clone(),
                        (
                            controller,
                            args,
                            Some(super::downloads::SiteAsk {
                                item: view_item.clone(),
                                permit: admit_permit.clone(),
                                sink: view_sink.clone(),
                                site: Some(admit_origin.clone()),
                                opened_by_click: false,
                            }),
                        ),
                        None,
                    );
                }
            })
            .build_as_child(&host)
            .unwrap();
        view.load_url(&base_url).unwrap();
        until(|| *loaded.lock().unwrap());
        view
    };

    let view = build_loaded_view(&mut context);

    // 1) Server truncation / Content-Length mismatch must settle cleanly as one
    // Failed (Integrity) entry without spawning duplicate auto-restart rows.
    view.evaluate_script("location.href='/trunc.zip'").unwrap();
    until(|| {
        list_downloads(&downloads)
            .iter()
            .any(|entry| entry.filename == "trunc.zip" && entry.state.terminal())
    });
    // Wait briefly to confirm WebView2 auto-restart duplicates were suppressed.
    let settle_until = Instant::now() + Duration::from_millis(700);
    until(|| Instant::now() >= settle_until);
    let entries = list_downloads(&downloads);
    assert_eq!(entries.len(), 1, "unexpected duplicate rows: {entries:?}");
    assert_eq!(entries[0].filename, "trunc.zip");
    assert_eq!(entries[0].state, DownloadState::Failed);
    assert_eq!(entries[0].error, Some(DownloadError::Integrity));

    let view2 = build_loaded_view(&mut context);

    // 2) Cancelling an active receiving transfer must settle cleanly to Cancelled
    // rather than stalling indefinitely in Cancelling.
    view2.evaluate_script("location.href='/slow.zip'").unwrap();
    until(|| {
        list_downloads(&downloads).iter().any(|entry| {
            entry.filename == "slow.zip"
                && entry.state == DownloadState::Receiving
                && entry
                    .received
                    .bytes()
                    .any(|byte| matches!(byte, b'1'..=b'9'))
        })
    });
    let slow_id = list_downloads(&downloads)
        .into_iter()
        .find(|entry| entry.filename == "slow.zip")
        .unwrap()
        .id;
    let cancelled_ack = Arc::new(Mutex::new(None));
    let cancel_slot = cancelled_ack.clone();
    downloads.call(
        partition,
        DownloadCall::Cancel { id: slow_id },
        DownloadCompletion::new(Box::new(move |response| {
            *cancel_slot.lock().unwrap() = Some(response);
        })),
    );
    until(|| cancelled_ack.lock().unwrap().is_some());
    until(|| {
        list_downloads(&downloads)
            .iter()
            .any(|entry| entry.filename == "slow.zip" && entry.state == DownloadState::Cancelled)
    });

    let final_entries = list_downloads(&downloads);
    assert_eq!(final_entries.len(), 2, "unexpected rows: {final_entries:?}");
    let slow = final_entries
        .iter()
        .find(|entry| entry.filename == "slow.zip")
        .unwrap();
    assert_eq!(slow.state, DownloadState::Cancelled);
    until(|| {
        std::fs::read_dir(download_dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .all(|entry| {
                !entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".zephium-download-")
            })
    });

    stop_server.store(true, std::sync::atomic::Ordering::Release);
    drop(view2);
    drop(view);
    drop(context);
    server.join().unwrap();
    unsafe {
        windows::Win32::System::Com::CoUninitialize();
    }
}
