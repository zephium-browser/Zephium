//! One host-initiated asynchronous evaluation. No exposed message handler,
//! host object, page timer, or page-callable native capability. Linux favicon
//! calls have a native deadline; generic pulls wait for document changes.
const MAX_RESULT_BYTES: usize = 256 * 1024;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub(crate) use linux::{cancel, evaluate, evaluate_until_navigation};

#[cfg(target_os = "macos")]
pub(crate) fn evaluate(
    view: &wry::WebView,
    expression: &str,
    done: impl FnOnce(Option<String>) + Send + 'static,
) -> bool {
    use objc2::runtime::AnyObject;
    use objc2::{msg_send, ClassType};
    use objc2_foundation::{MainThreadMarker, NSError, NSString};
    use objc2_web_kit::WKContentWorld;
    use std::cell::RefCell;
    use wry::WebViewExtMacOS;
    let Some(mtm) = MainThreadMarker::new() else {
        return false;
    };
    let pending = RefCell::new(Some(done));
    let callback = block2::RcBlock::new(move |value: *mut AnyObject, error: *mut NSError| {
        let Some(done) = pending.borrow_mut().take() else {
            return;
        };
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let text = if value.is_null() || !error.is_null() {
                None
            } else {
                // SAFETY: WebKit owns this callback-scoped object. A class
                // check precedes the cast; size is bounded before copying.
                unsafe {
                    let string: bool = msg_send![value, isKindOfClass: NSString::class()];
                    if string {
                        let value = &*value.cast::<NSString>();
                        (value.length() <= MAX_RESULT_BYTES)
                            .then(|| value.to_string())
                            .filter(|value| value.len() <= MAX_RESULT_BYTES)
                    } else {
                        None
                    }
                }
            };
            done(text);
        }));
    });
    let page = view.webview();
    // SAFETY: fixed host-selected evaluation in the existing page world;
    // WebKit retains the completion until the promise settles or navigation.
    unsafe {
        page.callAsyncJavaScript_arguments_inFrame_inContentWorld_completionHandler(
            &NSString::from_str(&format!("return await ({expression});")),
            None,
            None,
            &WKContentWorld::pageWorld(mtm),
            Some(&callback),
        );
    }
    true
}

#[cfg(target_os = "windows")]
mod windows {
    use super::MAX_RESULT_BYTES;
    use std::cell::RefCell;
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        ICoreWebView2CallDevToolsProtocolMethodCompletedHandler,
        ICoreWebView2CallDevToolsProtocolMethodCompletedHandler_Impl,
    };
    use windows_core::{HRESULT, HSTRING, PCWSTR};
    use wry::WebViewExtWindows;
    type Completion = Box<dyn FnOnce(Option<String>) + Send>;
    #[windows_core::implement(ICoreWebView2CallDevToolsProtocolMethodCompletedHandler)]
    struct Reply {
        done: RefCell<Option<Completion>>,
    }
    impl ICoreWebView2CallDevToolsProtocolMethodCompletedHandler_Impl for Reply_Impl {
        fn Invoke(&self, status: HRESULT, response: &PCWSTR) -> windows_core::Result<()> {
            let Some(done) = self.done.borrow_mut().take() else {
                return Ok(());
            };
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let text = status
                    .is_ok()
                    .then(|| bounded_string(response))
                    .flatten()
                    .and_then(|text| {
                        let value: serde_json::Value = serde_json::from_str(&text).ok()?;
                        if value.get("exceptionDetails").is_some() {
                            return None;
                        }
                        let result = value.get("result")?;
                        (result.get("type")?.as_str()? == "string").then_some(())?;
                        result.get("value")?.as_str().map(str::to_owned)
                    });
                done(text);
            }));
            Ok(())
        }
    }
    fn bounded_string(value: &PCWSTR) -> Option<String> {
        let pointer = value.as_ptr();
        if pointer.is_null() {
            return None;
        }
        for length in 0..=MAX_RESULT_BYTES {
            // SAFETY: WebView2 supplies a callback-scoped NUL-terminated
            // UTF-16 string. Both scan and resulting allocation are bounded.
            if unsafe { pointer.add(length).read() } == 0 {
                let units = unsafe { std::slice::from_raw_parts(pointer, length) };
                let value = String::from_utf16(units).ok()?;
                return (value.len() <= MAX_RESULT_BYTES).then_some(value);
            }
        }
        None
    }
    pub(crate) fn evaluate(
        view: &wry::WebView,
        expression: &str,
        done: impl FnOnce(Option<String>) + Send + 'static,
    ) -> bool {
        // SAFETY: the controller and core stay on their creating STA. This
        // fixed CDP method only returns a value from host-initiated script;
        // it exposes no CDP entrypoint or new bridge to the page.
        let Ok(core) = (unsafe { view.controller().CoreWebView2() }) else {
            return false;
        };
        let args =
            serde_json::json!({"expression":expression,"awaitPromise":true,"returnByValue":true,
            "silent":true,"includeCommandLineAPI":false,"userGesture":false})
            .to_string();
        let handler: ICoreWebView2CallDevToolsProtocolMethodCompletedHandler = Reply {
            done: RefCell::new(Some(Box::new(done))),
        }
        .into();
        unsafe {
            core.CallDevToolsProtocolMethod(
                &HSTRING::from("Runtime.evaluate"),
                &HSTRING::from(args),
                &handler,
            )
        }
        .is_ok()
    }
}
#[cfg(target_os = "windows")]
pub(crate) use windows::evaluate;
