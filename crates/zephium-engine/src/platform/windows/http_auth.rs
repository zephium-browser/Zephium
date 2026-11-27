//! WebView2's HTTP sign-in challenge, held open for a person's answer.

use webview2_com::BasicAuthenticationRequestedEventHandler;
use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2BasicAuthenticationRequestedEventArgs, ICoreWebView2Deferral,
};
use windows_core::{PCWSTR, PWSTR};
use zeroize::Zeroizing;

use super::{take_pwstr_bounded, SecurityPolicy, PAGE_URL_UTF16_LIMIT, PAGE_URL_UTF8_LIMIT};

/// Single-use completion of one challenge. Dropping it cancels. Cancel stays
/// set unless both the username and password were handed to WebView2, so a
/// partial failure never lets WebView2 fall back to its own dialog.
pub(crate) struct BasicAuthResponder {
    args: Option<ICoreWebView2BasicAuthenticationRequestedEventArgs>,
    deferral: Option<ICoreWebView2Deferral>,
}

impl BasicAuthResponder {
    pub(crate) fn respond(mut self, credentials: Option<(&str, &str)>) {
        self.finish(credentials);
    }

    fn finish(&mut self, credentials: Option<(&str, &str)>) {
        let (Some(args), Some(deferral)) = (self.args.take(), self.deferral.take()) else {
            return;
        };
        let signed_in = credentials.is_some_and(|(user, password)| {
            let user = wide(user);
            let password = wide(password);
            // SAFETY: both buffers are NUL-terminated and outlive the calls;
            // WebView2 copies the strings.
            unsafe {
                args.Response().and_then(|response| {
                    response.SetUserName(PCWSTR(user.as_ptr()))?;
                    response.SetPassword(PCWSTR(password.as_ptr()))?;
                    args.SetCancel(false)
                })
            }
            .is_ok()
        });
        if !signed_in {
            // SAFETY: the args stay alive until the deferral completes.
            let _ = unsafe { args.SetCancel(true) };
        }
        // SAFETY: completing a deferral taken from these exact args.
        let _ = unsafe { deferral.Complete() };
    }
}

impl Drop for BasicAuthResponder {
    fn drop(&mut self) {
        self.finish(None);
    }
}

fn wide(value: &str) -> Zeroizing<Vec<u16>> {
    Zeroizing::new(value.encode_utf16().chain(std::iter::once(0)).collect())
}

impl SecurityPolicy {
    /// Human tabs: a sign-in challenge goes to `request` with the native
    /// request deferred. Cancel is set first, so any failure before the
    /// answer leaves it cancelled.
    pub fn route_basic_authentication(
        &mut self,
        request: impl Fn(&str, BasicAuthResponder) + 'static,
    ) -> windows_core::Result<()> {
        let handler = BasicAuthenticationRequestedEventHandler::create(Box::new(move |_, args| {
            let Some(args) = args else {
                return Ok(());
            };
            // SAFETY: WebView2 supplied live args for this callback.
            unsafe {
                args.SetCancel(true)?;
                let mut uri = PWSTR::null();
                args.Uri(&mut uri)?;
                let Some(uri) = take_pwstr_bounded(uri, PAGE_URL_UTF16_LIMIT, PAGE_URL_UTF8_LIMIT)
                else {
                    return Ok(());
                };
                let deferral = args.GetDeferral()?;
                let responder = BasicAuthResponder {
                    args: Some(args),
                    deferral: Some(deferral),
                };
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    request(&uri, responder)
                }));
            }
            Ok(())
        }));
        let mut token = 0_i64;
        // The routing handler is in place before the cancel-only one goes.
        unsafe {
            self.core10
                .add_BasicAuthenticationRequested(&handler, &mut token)?
        };
        if let Some(previous) = self.basic_auth_token.replace(token) {
            let _ = unsafe { self.core10.remove_BasicAuthenticationRequested(previous) };
        }
        Ok(())
    }
}
