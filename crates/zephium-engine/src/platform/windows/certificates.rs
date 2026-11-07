//! WebView2 server-certificate errors on a person's tab.

use base64::Engine as _;
use sha2::{Digest, Sha256};
use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2Certificate, COREWEBVIEW2_SERVER_CERTIFICATE_ERROR_ACTION_ALWAYS_ALLOW,
    COREWEBVIEW2_SERVER_CERTIFICATE_ERROR_ACTION_CANCEL,
};
use webview2_com::{
    ClearServerCertificateErrorActionsCompletedHandler, ServerCertificateErrorDetectedEventHandler,
};
use windows_core::PWSTR;
use wry::WebViewExtWindows;

use super::{take_pwstr_bounded, SecurityPolicy, PAGE_URL_UTF16_LIMIT, PAGE_URL_UTF8_LIMIT};

const PEM_UTF16_LIMIT: usize = 64 * 1_024;

/// SHA-256 of the certificate's DER encoding.
fn leaf_sha256(certificate: &ICoreWebView2Certificate) -> Option<[u8; 32]> {
    let mut pem = PWSTR::null();
    // SAFETY: a live certificate; the returned string is taken and freed.
    unsafe { certificate.ToPemEncoding(&mut pem) }.ok()?;
    let pem = take_pwstr_bounded(pem, PEM_UTF16_LIMIT, PEM_UTF16_LIMIT)?;
    let body: String = pem
        .lines()
        .filter(|line| !line.starts_with("-----"))
        .flat_map(str::chars)
        .filter(|c| !c.is_whitespace())
        .collect();
    let der = base64::engine::general_purpose::STANDARD
        .decode(body)
        .ok()?;
    (!der.is_empty()).then(|| Sha256::digest(&der).into())
}

impl SecurityPolicy {
    /// Human tabs: `decide` sees each refused request's address and leaf
    /// digest, and returns true only for a certificate the person accepted.
    /// Cancel is set first, so any failure leaves the request refused.
    pub fn route_certificate_errors(
        &mut self,
        decide: impl Fn(&str, Option<[u8; 32]>) -> bool + 'static,
    ) -> windows_core::Result<()> {
        let handler =
            ServerCertificateErrorDetectedEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else {
                    return Ok(());
                };
                // SAFETY: WebView2 supplied live args for this callback.
                unsafe {
                    args.SetAction(COREWEBVIEW2_SERVER_CERTIFICATE_ERROR_ACTION_CANCEL)?;
                    let mut uri = PWSTR::null();
                    args.RequestUri(&mut uri)?;
                    let Some(uri) =
                        take_pwstr_bounded(uri, PAGE_URL_UTF16_LIMIT, PAGE_URL_UTF8_LIMIT)
                    else {
                        return Ok(());
                    };
                    let leaf = args
                        .ServerCertificate()
                        .ok()
                        .and_then(|certificate| leaf_sha256(&certificate));
                    let allowed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        decide(&uri, leaf)
                    }))
                    .unwrap_or(false);
                    if allowed {
                        args.SetAction(COREWEBVIEW2_SERVER_CERTIFICATE_ERROR_ACTION_ALWAYS_ALLOW)?;
                    }
                }
                Ok(())
            }));
        let mut token = 0_i64;
        // The routing handler is in place before the cancel-only one goes.
        unsafe {
            self.core14
                .add_ServerCertificateErrorDetected(&handler, &mut token)?
        };
        if let Some(previous) = self.certificate_error_token.replace(token) {
            let _ = unsafe { self.core14.remove_ServerCertificateErrorDetected(previous) };
        }
        Ok(())
    }
}

/// Drops the AlwaysAllow decisions WebView2 remembers for this view's
/// profile, so a forgotten exception is not still honoured by the runtime.
pub(crate) fn clear_certificate_decisions(view: &wry::WebView) {
    let Ok(core) = (unsafe { view.controller().CoreWebView2() }) else {
        return;
    };
    let Ok(core) = windows_core::Interface::cast::<
        webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2_14,
    >(&core) else {
        return;
    };
    let done = ClearServerCertificateErrorActionsCompletedHandler::create(Box::new(|_| Ok(())));
    // SAFETY: a live controller on the UI thread.
    if unsafe { core.ClearServerCertificateErrorActions(&done) }.is_err() {
        eprintln!("security: WebView2 kept remembered certificate decisions");
    }
}
