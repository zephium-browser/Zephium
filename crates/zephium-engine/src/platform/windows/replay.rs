//! Read-only native provenance for the current, safely reloadable document.
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2, COREWEBVIEW2_WEB_RESOURCE_CONTEXT_DOCUMENT,
};
use webview2_com::{NavigationStartingEventHandler, WebResourceRequestedEventHandler};
use windows_core::{w, PWSTR};

use crate::replay_witness::NavigationWitness;

#[derive(Default)]
struct Witness {
    navigation: Mutex<NavigationWitness>,
    unsafe_history: AtomicBool,
    retired: AtomicBool,
}

/// Installed before the first content load. The owner additionally requires
/// engine-initiated GET provenance, fresh activity checks, zero scroll and no
/// meaningful back/forward history. Native history guards remain separate; an unobserved POST never grants
/// URL replay provenance.
pub(crate) struct RequestWitness {
    core: ICoreWebView2,
    resource_token: i64,
    navigation_token: Option<i64>,
    state: Arc<Witness>,
}

impl RequestWitness {
    pub(crate) fn install(
        core: ICoreWebView2,
        allow_owned_bootstrap: bool,
    ) -> windows_core::Result<Self> {
        let state = Arc::new(Witness::default());
        let observed = state.clone();
        let handler = WebResourceRequestedEventHandler::create(Box::new(move |_, args| {
            if observed.retired.load(Ordering::Acquire) {
                return Ok(());
            }
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let args = args?;
                let mut context = Default::default();
                // SAFETY: native event arguments remain valid for this callback.
                unsafe { args.ResourceContext(&mut context) }.ok()?;
                if context != COREWEBVIEW2_WEB_RESOURCE_CONTEXT_DOCUMENT {
                    return Some(());
                }
                let request = unsafe { args.Request() }.ok()?;
                let mut uri = PWSTR::null();
                let received = unsafe { request.Uri(&mut uri) };
                let uri = super::take_pwstr_bounded(uri, 8192, 8192);
                received.ok()?;
                let uri = uri?;
                let mut method = PWSTR::null();
                let received = unsafe { request.Method(&mut method) };
                let method = super::take_pwstr_bounded(method, 32, 32);
                received.ok()?;
                let method = method?;
                let mut navigation = observed.navigation.lock().ok()?;
                navigation.requested(uri, &method);
                Some(())
            }));
            if !matches!(result, Ok(Some(()))) {
                observed.unsafe_history.store(true, Ordering::Release);
            }
            // Never modify a request, install a response, or defer navigation.
            Ok(())
        }));
        let mut resource_token = 0;
        // SAFETY: COM copies the handler; callbacks hold no controller reference.
        unsafe {
            core.add_WebResourceRequested(&handler, &mut resource_token)?;
        }
        let mut registration = Self {
            core,
            resource_token,
            navigation_token: None,
            state,
        };
        let observed = registration.state.clone();
        let navigation = NavigationStartingEventHandler::create(Box::new(move |_, args| {
            if observed.retired.load(Ordering::Acquire) {
                return Ok(());
            }
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let args = args?;
                let mut id = 0;
                // SAFETY: request metadata is read only and callback-scoped.
                unsafe { args.NavigationId(&mut id) }.ok()?;
                let mut uri = PWSTR::null();
                let received = unsafe { args.Uri(&mut uri) };
                let uri = super::take_pwstr_bounded(uri, 8192, 8192);
                received.ok()?;
                let uri = uri?;
                let mut navigation = observed.navigation.lock().ok()?;
                navigation
                    .started(id, uri, allow_owned_bootstrap)
                    .then_some(())
            }));
            if !matches!(result, Ok(Some(()))) {
                observed.unsafe_history.store(true, Ordering::Release);
            }
            Ok(())
        }));
        let mut navigation_token = 0;
        // SAFETY: registration owns both exact event tokens until retirement.
        unsafe {
            registration
                .core
                .add_NavigationStarting(&navigation, &mut navigation_token)?;
        }
        registration.navigation_token = Some(navigation_token);
        // Leave this filter until controller destruction: removing it could
        // remove an equivalent filter another owner depends upon.
        unsafe {
            registration.core.AddWebResourceRequestedFilter(
                w!("http*://*"),
                COREWEBVIEW2_WEB_RESOURCE_CONTEXT_DOCUMENT,
            )?;
        }
        Ok(registration)
    }

    pub(crate) fn allows_replay(&self, current_url: &str) -> bool {
        !self.state.retired.load(Ordering::Acquire)
            && !self.state.unsafe_history.load(Ordering::Acquire)
            && self
                .state
                .navigation
                .lock()
                .is_ok_and(|navigation| navigation.allows(current_url))
    }

    pub(crate) fn has_owned_blank_bootstrap(&self) -> bool {
        self.state
            .navigation
            .lock()
            .is_ok_and(|navigation| navigation.bootstrap.is_some())
    }

    pub(crate) fn taint(&self) {
        self.state.unsafe_history.store(true, Ordering::Release);
    }

    pub(crate) fn tainted(&self) -> bool {
        self.state.unsafe_history.load(Ordering::Acquire)
    }
}

impl Drop for RequestWitness {
    fn drop(&mut self) {
        self.state.retired.store(true, Ordering::Release);
        // SAFETY: tokens belong to this exact controller. On native refusal,
        // callbacks stay inert and own only bounded metadata, not the view.
        if let Some(token) = self.navigation_token.take() {
            let _ = unsafe { self.core.remove_NavigationStarting(token) };
        }
        let _ = unsafe { self.core.remove_WebResourceRequested(self.resource_token) };
    }
}
