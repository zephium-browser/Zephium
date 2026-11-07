//! Platform sleep adapters. No polling, cache purges or process termination.
use tauri::WebviewWindow;

#[cfg(target_os = "windows")]
pub fn suspend(window: &WebviewWindow, done: impl FnOnce() + Send + 'static) {
    use std::sync::{Arc, Mutex};
    use webview2_com::{
        Microsoft::Web::WebView2::Win32::ICoreWebView2_3, TrySuspendCompletedHandler,
    };
    use windows::core::Interface;
    let completion = Arc::new(Mutex::new(Some(done)));
    let fallback = completion.clone();
    let result = window.with_webview(move |view| {
        let callback = completion.clone();
        let handler = TrySuspendCompletedHandler::create(Box::new(move |status, suspended| {
            // A refusal leaves the launcher awake until its next hide asks again.
            if status.is_err() || !suspended {
                crate::write_diagnostic(format_args!(
                    "launcher: suspend refused status={status:?}"
                ));
            }
            if let Some(done) = callback.lock().unwrap_or_else(|e| e.into_inner()).take() {
                done();
            }
            Ok(())
        }));
        // SAFETY: Tauri supplies the live controller on its UI thread.
        let result = unsafe {
            (|| -> windows::core::Result<()> {
                view.controller().SetIsVisible(false)?;
                view.controller()
                    .CoreWebView2()?
                    .cast::<ICoreWebView2_3>()?
                    .TrySuspend(&handler)
            })()
        };
        if result.is_err() {
            if let Some(done) = completion.lock().unwrap_or_else(|e| e.into_inner()).take() {
                done();
            }
        }
    });
    if result.is_err() {
        if let Some(done) = fallback.lock().unwrap_or_else(|e| e.into_inner()).take() {
            done();
        }
    }
}
#[cfg(target_os = "windows")]
pub fn resume(window: &WebviewWindow) {
    use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2_3;
    use windows::core::Interface;
    let _ = window.with_webview(|view| unsafe {
        if let Ok(core) = view.controller().CoreWebView2() {
            if let Ok(core) = core.cast::<ICoreWebView2_3>() {
                let _ = core.Resume();
            }
        }
        let _ = view.controller().SetIsVisible(true);
    });
}

#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use objc2::rc::Retained;
    use objc2_app_kit::NSView;
    use objc2_web_kit::WKWebView;
    use std::cell::RefCell;
    thread_local! { static PARENT: RefCell<Option<Retained<NSView>>> = const { RefCell::new(None) }; }
    pub fn suspend(window: &WebviewWindow, done: impl FnOnce() + Send + 'static) {
        let _ = window.with_webview(|view| {
            // SAFETY: a live UI-thread WKWebView; Tauri retains the detached view.
            let webview = unsafe { &*view.inner().cast::<WKWebView>() };
            if let Some(parent) = unsafe { webview.superview() } {
                PARENT.with(|slot| *slot.borrow_mut() = Some(parent));
                webview.removeFromSuperview();
            }
        });
        done();
    }
    pub fn destroyed() {
        PARENT.with(|slot| {
            slot.borrow_mut().take();
        });
    }
    pub fn resume(window: &WebviewWindow) {
        let _ = window.with_webview(|view| {
            let webview = unsafe { &*view.inner().cast::<WKWebView>() };
            PARENT.with(|slot| {
                if let Some(parent) = slot.borrow_mut().take() {
                    parent.addSubview(webview);
                }
            });
        });
    }
}
#[cfg(target_os = "macos")]
pub use macos::{destroyed, resume, suspend};
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn suspend(_: &WebviewWindow, done: impl FnOnce() + Send + 'static) {
    done();
}
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn resume(_: &WebviewWindow) {}

#[cfg(not(target_os = "macos"))]
pub fn destroyed() {}
