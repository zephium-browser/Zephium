//! Controller-lifetime gates for extension bridge and popup native navigation.
use webview2_com::Microsoft::Web::WebView2::Win32::{ICoreWebView2, ICoreWebView2_18};
use webview2_com::{LaunchingExternalUriSchemeEventHandler, NavigationStartingEventHandler};
use windows_core::{Interface, PWSTR};

pub(crate) struct ExtensionNavigationPolicy {
    core: ICoreWebView2,
    external: ICoreWebView2_18,
    frame_token: Option<i64>,
    external_token: Option<i64>,
}

impl ExtensionNavigationPolicy {
    pub(crate) fn install(core: &ICoreWebView2, extension: String) -> windows_core::Result<Self> {
        let external = core.cast::<ICoreWebView2_18>()?;
        let mut policy = Self {
            core: core.clone(),
            external,
            frame_token: None,
            external_token: None,
        };
        let frame = NavigationStartingEventHandler::create(Box::new(move |_, args| {
            let Some(args) = args else {
                return Ok(());
            };
            // Cancel before any native extraction or policy parsing can fail.
            // SAFETY: callback-scoped native event arguments on the UI apartment.
            unsafe {
                args.SetCancel(true)?;
            }
            let mut uri = PWSTR::null();
            // SAFETY: the native getter initializes its owned output pointer.
            let received = unsafe { args.Uri(&mut uri) };
            let uri = super::take_pwstr_bounded(
                uri,
                super::PAGE_URL_UTF16_LIMIT,
                super::PAGE_URL_UTF8_LIMIT,
            );
            received?;
            if uri.is_some_and(|uri| crate::extension_navigation::allows_frame(&extension, &uri)) {
                // SAFETY: the frame passed the extension-only allowlist.
                unsafe {
                    args.SetCancel(false)?;
                }
            }
            Ok(())
        }));
        let mut token = 0;
        // SAFETY: this registration is owned by the exact controller below.
        unsafe {
            policy
                .core
                .add_FrameNavigationStarting(&frame, &mut token)?;
        }
        policy.frame_token = Some(token);
        let handler = LaunchingExternalUriSchemeEventHandler::create(Box::new(|_, args| {
            if let Some(args) = args {
                // SAFETY: callback-scoped event arguments; native launch always denied.
                unsafe {
                    args.SetCancel(true)?;
                }
            }
            Ok(())
        }));
        let mut token = 0;
        // SAFETY: this registration is owned by the exact external interface.
        unsafe {
            policy
                .external
                .add_LaunchingExternalUriScheme(&handler, &mut token)?;
        }
        policy.external_token = Some(token);
        Ok(policy)
    }
}

impl Drop for ExtensionNavigationPolicy {
    fn drop(&mut self) {
        if let Some(token) = self.frame_token.take() {
            // SAFETY: token belongs to this retained controller on its apartment.
            let _ = unsafe { self.core.remove_FrameNavigationStarting(token) };
        }
        if let Some(token) = self.external_token.take() {
            // SAFETY: token belongs to this retained interface on its apartment.
            let _ = unsafe { self.external.remove_LaunchingExternalUriScheme(token) };
        }
    }
}
