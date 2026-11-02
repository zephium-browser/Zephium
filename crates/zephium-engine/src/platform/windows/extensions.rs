//! Small, apartment-bound WebView2 extension operations. No extension API emulation.
use std::cell::Cell;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use webview2_com::Microsoft::Web::WebView2::Win32::*;
use webview2_com::{
    take_pwstr, BrowserExtensionEnableCompletedHandler, BrowserExtensionRemoveCompletedHandler,
    CallDevToolsProtocolMethodCompletedHandler, ProfileAddBrowserExtensionCompletedHandler,
    ProfileGetBrowserExtensionsCompletedHandler,
};
use windows::core::{Interface, HSTRING, PWSTR};
use windows::Win32::Foundation::E_FAIL;
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, PeekMessageW, PostQuitMessage, TranslateMessage, MSG, PM_REMOVE, WM_QUIT,
};

pub(crate) type Result<T> = std::result::Result<T, String>;

const TIMED_OUT: &str = "Native extension operation timed out.";

// A DevTools call that timed out means the browser process is not answering
// (an extension's prompt, for one). Every further call would hold this UI
// thread for the whole deadline again, so they are refused for a while.
const CDP_PAUSE: Duration = Duration::from_secs(30);

thread_local! {
    static CDP_PAUSED_UNTIL: Cell<Option<Instant>> = const { Cell::new(None) };
}

// WebView2 completion callbacks run on this STA. The engine's dispatch owner
// defers reentrant host work; the deadline never authorizes a late operation.
fn wait<T>(rx: mpsc::Receiver<windows::core::Result<T>>) -> Result<T> {
    wait_within(rx, Duration::from_secs(5))
}

fn wait_within<T>(rx: mpsc::Receiver<windows::core::Result<T>>, budget: Duration) -> Result<T> {
    let deadline = Instant::now() + budget;
    loop {
        match rx.try_recv() {
            Ok(result) => return result.map_err(|e| e.to_string()),
            Err(mpsc::TryRecvError::Disconnected) => {
                return Err("Native extension callback was lost.".into());
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }
        if Instant::now() >= deadline {
            return Err(TIMED_OUT.into());
        }
        // SAFETY: local message storage and the controller's owning UI thread.
        unsafe {
            let mut message = MSG::default();
            while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                if message.message == WM_QUIT {
                    PostQuitMessage(message.wParam.0 as i32);
                    return Err("The browser is shutting down.".into());
                }
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
                if Instant::now() >= deadline {
                    break;
                }
            }
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

pub(crate) fn profile(core: &ICoreWebView2) -> windows::core::Result<ICoreWebView2Profile7> {
    // SAFETY: all COM objects are confined to their owning apartment.
    unsafe { core.cast::<ICoreWebView2_13>()?.Profile()?.cast() }
}

pub(crate) fn extension_id(item: &ICoreWebView2BrowserExtension) -> windows::core::Result<String> {
    let mut id = PWSTR::null();
    // SAFETY: valid output storage; take_pwstr releases COM's allocation.
    unsafe {
        item.Id(&mut id)?;
    }
    Ok(take_pwstr(id))
}

pub(crate) fn list(profile: &ICoreWebView2Profile7) -> Result<Vec<ICoreWebView2BrowserExtension>> {
    let (tx, rx) = mpsc::channel();
    let handler =
        ProfileGetBrowserExtensionsCompletedHandler::create(Box::new(move |status, list| {
            let result = (|| {
                status?;
                let list = list.ok_or_else(|| windows::core::Error::from(E_FAIL))?;
                let mut count = 0;
                // SAFETY: list owns its entries; bound the native inventory before allocation.
                unsafe {
                    list.Count(&mut count)?;
                    if count > 64 {
                        return Err(E_FAIL.into());
                    }
                    (0..count)
                        .map(|index| list.GetValueAtIndex(index))
                        .collect()
                }
            })();
            let _ = tx.send(result);
            Ok(())
        }));
    // SAFETY: WebView2 retains the completion handler.
    unsafe { profile.GetBrowserExtensions(&handler) }.map_err(|e| e.to_string())?;
    wait(rx)
}

pub(crate) fn enable(item: &ICoreWebView2BrowserExtension, enabled: bool) -> Result<()> {
    let (tx, rx) = mpsc::channel();
    let late_item = item.clone();
    let handler = BrowserExtensionEnableCompletedHandler::create(Box::new(move |status| {
        if tx.send(status).is_err() && enabled {
            let handler = BrowserExtensionEnableCompletedHandler::create(Box::new(|_| Ok(())));
            // SAFETY: a timed-out enable has no live owner; revoke it on this STA.
            let _ = unsafe { late_item.Enable(false, &handler) };
        }
        Ok(())
    }));
    // SAFETY: retained native extension on its apartment.
    unsafe { item.Enable(enabled, &handler) }.map_err(|e| e.to_string())?;
    wait(rx)
}

pub(crate) fn remove(item: &ICoreWebView2BrowserExtension) -> Result<()> {
    let (tx, rx) = mpsc::channel();
    let handler = BrowserExtensionRemoveCompletedHandler::create(Box::new(move |status| {
        let _ = tx.send(status);
        Ok(())
    }));
    // SAFETY: retained native extension on its apartment.
    unsafe { item.Remove(&handler) }.map_err(|e| e.to_string())?;
    wait(rx)
}

pub(crate) fn add(
    profile: &ICoreWebView2Profile7,
    root: &std::path::Path,
) -> Result<ICoreWebView2BrowserExtension> {
    let (tx, rx) = mpsc::channel();
    let handler =
        ProfileAddBrowserExtensionCompletedHandler::create(Box::new(move |status, item| {
            let result =
                status.and_then(|()| item.ok_or_else(|| windows::core::Error::from(E_FAIL)));
            if let Err(mpsc::SendError(Ok(item))) = tx.send(result) {
                // A late successful install must not execute after the owner gave up.
                let handler = BrowserExtensionEnableCompletedHandler::create(Box::new(|_| Ok(())));
                // SAFETY: late callback still runs on the owning apartment.
                let _ = unsafe { item.Enable(false, &handler) };
            }
            Ok(())
        }));
    // SAFETY: the immutable package remains on disk while installed.
    unsafe {
        profile.AddBrowserExtension(&HSTRING::from(root.to_string_lossy().as_ref()), &handler)
    }
    .map_err(|e| e.to_string())?;
    wait(rx)
}

pub(crate) fn cdp(core: &ICoreWebView2, method: &str, parameters: Value) -> Result<Value> {
    if CDP_PAUSED_UNTIL
        .get()
        .is_some_and(|until| Instant::now() < until)
    {
        return Err("The browser process is not answering.".into());
    }
    let (tx, rx) = mpsc::channel();
    let handler =
        CallDevToolsProtocolMethodCompletedHandler::create(Box::new(move |status, result| {
            let _ = tx.send(status.map(|()| result));
            Ok(())
        }));
    // SAFETY: host-selected fixed methods only; no page-provided CDP dispatcher.
    unsafe {
        core.CallDevToolsProtocolMethod(
            &HSTRING::from(method),
            &HSTRING::from(parameters.to_string()),
            &handler,
        )
    }
    .map_err(|e| e.to_string())?;
    let reply = wait_within(rx, Duration::from_secs(2)).inspect_err(|error| {
        if error == TIMED_OUT {
            CDP_PAUSED_UNTIL.set(Some(Instant::now() + CDP_PAUSE));
        }
    })?;
    CDP_PAUSED_UNTIL.set(None);
    serde_json::from_str(&reply).map_err(|e| e.to_string())
}

pub(crate) fn window_id(core: &ICoreWebView2) -> Result<i64> {
    cdp(core, "Browser.getWindowForTarget", json!({}))?["windowId"]
        .as_i64()
        .ok_or_else(|| "Native tab identity is unavailable.".into())
}

pub(crate) fn is_runtime_component(id: &str) -> bool {
    matches!(
        id,
        "dgiklkfkllikcanfonkcabmbdfmgleag" | "mhjfbmdgcfjbbpaeojofohoefgiehjai"
    )
}
