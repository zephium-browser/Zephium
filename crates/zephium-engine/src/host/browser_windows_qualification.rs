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
