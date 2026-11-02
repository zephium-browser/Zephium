//! A refused server certificate on a tab a person reads, and the exceptions
//! that person accepted for hosts on their own network.
//!
//! Exceptions live in memory per profile and are read by that profile's
//! human tabs only; agent and extension views are built without them. The
//! leaf digest comes from the engine's own failure, never from chrome.

use std::cell::RefCell;
use std::rc::Rc;

use url::Url;
use zephium_core::certificates::{may_continue, CertificateException, CertificateExceptions};
use zephium_core::http_auth::AuthOrigin;
use zephium_core::ids::{ItemId, ProfileId};

use super::EngineHost;

pub(super) type SharedExceptions = Rc<RefCell<CertificateExceptions>>;

/// The view's main-frame address in flight, and the last certificate it
/// refused there.
#[derive(Default)]
pub(super) struct CertificateWatch {
    in_flight: RefCell<Option<AuthOrigin>>,
    refused: RefCell<Option<(Url, Option<[u8; 32]>)>>,
}

impl CertificateWatch {
    pub(super) fn navigating(&self, url: &str) {
        if let Ok(mut in_flight) = self.in_flight.try_borrow_mut() {
            *in_flight = Url::parse(url).ok().as_ref().and_then(AuthOrigin::of_url);
        }
    }

    pub(super) fn committed(&self) {
        if let (Ok(mut in_flight), Ok(mut refused)) = (
            self.in_flight.try_borrow_mut(),
            self.refused.try_borrow_mut(),
        ) {
            *in_flight = None;
            *refused = None;
        }
    }

    /// The main frame's own load was refused for its certificate.
    pub(super) fn refused(&self, url: &str, leaf_sha256: Option<[u8; 32]>) {
        let Ok(url) = Url::parse(url) else {
            return;
        };
        if let Ok(mut refused) = self.refused.try_borrow_mut() {
            *refused = Some((url, leaf_sha256));
        }
    }

    /// A refusal from WebView2, which names any request: kept only when it
    /// is the main frame's own origin in flight.
    #[cfg(target_os = "windows")]
    pub(super) fn refused_request(&self, url: &str, leaf_sha256: Option<[u8; 32]>) {
        let origin = Url::parse(url).ok().as_ref().and_then(AuthOrigin::of_url);
        if origin.is_some()
            && self
                .in_flight
                .try_borrow()
                .is_ok_and(|in_flight| *in_flight == origin)
        {
            self.refused(url, leaf_sha256);
        }
    }

    pub(super) fn refused_url(&self) -> Option<String> {
        self.refused
            .try_borrow()
            .ok()?
            .as_ref()
            .map(|(url, _)| url.to_string())
    }

    /// The exception the person may accept for `url`: the same origin the
    /// engine refused, with the leaf it saw.
    fn exception_for(&self, url: &Url) -> Option<CertificateException> {
        let refused = self.refused.try_borrow().ok()?;
        let (seen, leaf) = refused.as_ref()?;
        if AuthOrigin::of_url(seen)? != AuthOrigin::of_url(url)? {
            return None;
        }
        CertificateException::new(seen, (*leaf)?)
    }
}

impl EngineHost {
    pub(super) fn certificate_exceptions_for(&mut self, profile: ProfileId) -> SharedExceptions {
        self.certificate_exceptions
            .entry(profile)
            .or_default()
            .clone()
    }

    /// The person chose to continue past the certificate the tab refused at
    /// `url`. Only a local https host, and only the exact leaf the engine
    /// saw, is accepted.
    pub(crate) fn allow_certificate_exception(&mut self, item: ItemId, url: &str) {
        let Ok(url) = Url::parse(url) else {
            return;
        };
        if !may_continue(&url) {
            return;
        }
        let Some(profile) = self
            .partitions
            .get(&item)
            .map(|partition| partition.profile())
        else {
            return;
        };
        let Some(exception) = self
            .views
            .get(&item)
            .and_then(|view| view.certificates.exception_for(&url))
        else {
            return;
        };
        if let Ok(mut exceptions) = self.certificate_exceptions_for(profile).try_borrow_mut() {
            exceptions.insert(exception);
        }
    }

    /// The profile's session ended or its data is being erased.
    pub(super) fn forget_certificate_exceptions(&mut self, profile: ProfileId) {
        let Some(exceptions) = self.certificate_exceptions.remove(&profile) else {
            return;
        };
        // Views still open read the same set, so empty it in place.
        if let Ok(mut emptied) = exceptions.try_borrow_mut() {
            emptied.clear();
        };
        #[cfg(target_os = "windows")]
        for (item, view) in &self.views {
            if self
                .partitions
                .get(item)
                .is_some_and(|partition| partition.profile() == profile)
            {
                // WebView2 remembers an AlwaysAllow for the host and
                // certificate per profile; one view clears it for all.
                crate::platform::imp::clear_certificate_decisions(&view.view);
                break;
            }
        }
    }
}
