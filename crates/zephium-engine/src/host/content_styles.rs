//! Document-scoped, bounded style delivery. No page-to-native bridge.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use sha2::{Digest, Sha256};
use zephium_core::blocker::{BlockerSite, ContentRuleDigest};
use zephium_core::ids::{ItemId, ProfileId};

use super::dispatch::with_document_style;
use super::permits::EventPermit;
use super::EngineHost;

pub(super) type SitePreferencesSlot =
    Rc<RefCell<Option<Arc<zephium_core::blocker::PreparedBlockerSites>>>>;

pub(super) struct ViewSiteScope {
    preferences: SitePreferencesSlot,
    target: RefCell<String>,
    pub(super) pause: crate::platform::content_pause::ContentPause,
}

impl ViewSiteScope {
    pub(super) fn new(preferences: SitePreferencesSlot, url: &str) -> Rc<Self> {
        let scope = Rc::new(Self {
            preferences,
            target: RefCell::new(url.to_owned()),
            pause: Default::default(),
        });
        scope.refresh();
        scope
    }
    pub(super) fn navigating(&self, url: &str) {
        self.target.replace(url.to_owned());
        self.refresh();
    }
    pub(super) fn refresh(&self) {
        let paused = {
            let preferences = self.preferences.borrow();
            match preferences.as_ref() {
                None => true,
                Some(preferences) => BlockerSite::from_url(&self.target.borrow())
                    .and_then(|site| preferences.get(&site))
                    .is_some_and(|entry| entry.paused),
            }
        };
        self.pause.set(paused);
    }
}
use crate::navigation_epoch::{NavigationEpoch, NavigationEpochTracker};

const INSPECT: &str = "(()=>{const a=globalThis.__zephium_content_style_v1__;return a&&a.version===1?a.inspectEncoded():null})()";
const MAX_DELIVERY_BYTES: usize = 64 * 1024 * 1024;
static DELIVERY_BYTES: AtomicUsize = AtomicUsize::new(0);

// Both initial sheets and incremental replies share one retained-script budget.
pub(super) fn charge_script(bytes: usize) -> Option<usize> {
    let charge = bytes.checked_mul(3)?;
    DELIVERY_BYTES
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
            current
                .checked_add(charge)
                .filter(|total| *total <= MAX_DELIVERY_BYTES)
        })
        .ok()?;
    Some(charge)
}
pub(super) fn release_script(charge: usize) {
    DELIVERY_BYTES.fetch_sub(charge, Ordering::Relaxed);
}

#[derive(Clone, PartialEq, Eq)]
pub(super) struct StyleKey {
    pub(super) epoch: NavigationEpoch,
    pub(super) url: String,
    pub(super) subscription: Option<ContentRuleDigest>,
    personal: Option<ContentRuleDigest>,
    paused: bool,
}

#[derive(Default)]
pub(super) struct DocumentStyleState(Mutex<StyleState>);

#[derive(Default)]
#[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
struct StyleState {
    sequence: u64,
    active: Option<u64>,
    pending_key: Option<StyleKey>,
    applied_key: Option<StyleKey>,
    dirty: bool,
    force_full: bool,
    in_flight: usize,
    generic_active: bool,
    generic_visible: bool,
    generic_visibility_revision: u64,
    generic_enabled: bool,
    generic_fingerprint: String,
    document_token: String,
}

impl DocumentStyleState {
    fn lock(&self) -> MutexGuard<'_, StyleState> {
        self.0.lock().unwrap_or_else(|p| p.into_inner())
    }

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    pub(super) fn begin_generic(&self) -> Option<(StyleKey, String, String, u64)> {
        let mut state = self.lock();
        let key = state.applied_key.clone()?;
        if state.generic_active || !state.generic_enabled || key.paused {
            return None;
        }
        state.generic_active = true;
        Some((
            key,
            state.generic_fingerprint.clone(),
            state.document_token.clone(),
            state.generic_visibility_revision,
        ))
    }
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    pub(super) fn generic_visibility(&self, visible: bool) -> u64 {
        let mut state = self.lock();
        if state.generic_visible != visible {
            state.generic_visible = visible;
            state.generic_visibility_revision = state.generic_visibility_revision.saturating_add(1);
        }
        state.generic_visibility_revision
    }
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    pub(super) fn end_generic(&self) {
        self.lock().generic_active = false;
    }
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    pub(super) fn generic_current(&self, key: &StyleKey, fingerprint: &str) -> bool {
        let state = self.lock();
        state.generic_enabled
            && state.applied_key.as_ref() == Some(key)
            && state.generic_fingerprint == fingerprint
    }

    /// Ends one delivery; true when a newer change waited on it (a URL change
    /// in place, say), which nothing else would ever deliver.
    fn settle(&self, sequence: u64) -> bool {
        let mut state = self.lock();
        state.in_flight -= 1;
        if state.active != Some(sequence) {
            return false;
        }
        state.active = None;
        state.pending_key = None;
        std::mem::take(&mut state.dirty)
    }
}

struct Delivery {
    id: ItemId,
    dispatch: crate::MainThreadDispatch,
    state: Arc<DocumentStyleState>,
    sequence: u64,
    charged_bytes: usize,
    reuse: bool,
    force_full: bool,
    generic_enabled: bool,
    generic_fingerprint: String,
    document_token: String,
    key: StyleKey,
    navigation: NavigationEpochTracker,
    permit: EventPermit,
    provider: Option<Arc<dyn zephium_core::blocker::DocumentStyleProvider>>,
    personal: Arc<str>,
}

impl Drop for Delivery {
    fn drop(&mut self) {
        release_script(self.charged_bytes);
        // The dispatch runs inline on the main thread, where the refresh takes
        // this lock again: `settle` returns with it released.
        if self.state.settle(self.sequence) {
            let id = self.id;
            let _ = (self.dispatch)(Box::new(move || {
                let _ = with_document_style(id, move |host| host.refresh_document_styles(id));
            }));
        }
    }
}

struct DocumentIdentity {
    token: String,
    url: String,
    subscription: Option<String>,
}

impl EngineHost {
    pub(crate) fn set_blocker_site_preferences(
        &mut self,
        profile: ProfileId,
        preferences: Arc<zephium_core::blocker::PreparedBlockerSites>,
    ) -> bool {
        if self.erasure_tombstones.contains(&profile)
            || (!self.blocker_sites.contains_key(&profile)
                && self.blocker_sites.len() >= zephium_core::session::MAX_SESSION_PROFILES)
        {
            return false;
        }
        let slot = self.blocker_sites.entry(profile).or_default();
        if slot.borrow().as_ref().is_some_and(|current| {
            current.revision() > preferences.revision()
                || (current.revision() == preferences.revision()
                    && !current.same_content(&preferences))
        }) {
            return false;
        }
        slot.replace(Some(preferences));
        for (id, view) in &self.views {
            if self
                .partitions
                .get(id)
                .is_some_and(|partition| partition.profile() == profile)
            {
                view.site_scope.refresh();
            }
        }
        if let Some(spare) = self
            .spare
            .as_ref()
            .filter(|spare| spare.partition.profile() == profile)
        {
            spare.view.site_scope.refresh();
        }
        self.refresh_profile_document_styles(profile);
        true
    }
    pub(super) fn refresh_profile_document_styles(&mut self, profile: ProfileId) {
        let ids: Vec<_> = self
            .partitions
            .iter()
            .filter_map(|(id, p)| (p.profile() == profile).then_some(*id))
            .collect();
        for id in ids {
            self.refresh_document_styles(id);
        }
    }

    /// Delivers the refresh a dormant view skipped, once it is awake again.
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    pub(super) fn refresh_missed_styles(&mut self, id: ItemId) {
        #[cfg(target_os = "windows")]
        let suspending = self.suspending.contains(&id);
        #[cfg(target_os = "macos")]
        let suspending = false;
        if !self.dormant.contains(&id) && !suspending && self.styles_missed.remove(&id) {
            self.refresh_document_styles(id);
        }
    }

    pub(super) fn refresh_document_styles(&mut self, id: ItemId) {
        #[cfg(not(target_os = "windows"))]
        if self.shutdown_completion.is_some() {
            return;
        }
        // A sleeping page is not woken to restyle; it is owed the refresh.
        #[cfg(target_os = "windows")]
        if self.dormant.contains(&id) || self.suspending.contains(&id) {
            self.styles_missed.insert(id);
            return;
        }
        #[cfg(target_os = "macos")]
        if self.dormant.contains(&id) {
            self.styles_missed.insert(id);
            return;
        }
        let Some(profile) = self.partitions.get(&id).map(|p| p.profile()) else {
            return;
        };
        let Some(sites) = self
            .blocker_sites
            .get(&profile)
            .and_then(|slot| slot.borrow().clone())
        else {
            return;
        };
        let Some(view) = self.views.get(&id) else {
            return;
        };
        let Some((epoch, url)) = view.navigation.committed_snapshot() else {
            return;
        };
        let Some(site) = BlockerSite::from_url(&url) else {
            return;
        };
        let site_policy = sites.get(&site);
        let paused = site_policy.is_some_and(|p| p.paused);
        let provider = self
            .content_policies
            .get(&profile)
            .and_then(|p| p.applied.as_ref())
            .and_then(|p| p.cosmetics.clone());
        let key = StyleKey {
            epoch,
            url: url.clone(),
            subscription: provider.as_ref().map(|p| p.fingerprint()),
            personal: site_policy.map(|p| p.fingerprint),
            paused,
        };
        let state = view.content_styles.clone();
        {
            let mut status = state.lock();
            if status.applied_key.as_ref() == Some(&key) {
                drop(status);
                self.refresh_generic_styles(id);
                return;
            }
            if status.active.is_some() {
                let replaced_document = status
                    .pending_key
                    .as_ref()
                    .is_some_and(|pending| pending.epoch != epoch);
                if !replaced_document || status.in_flight >= 2 {
                    if status.pending_key.as_ref() != Some(&key) {
                        status.dirty = true;
                    }
                    return;
                }
            }
        }
        let personal = site_policy
            .map(|p| p.css.clone())
            .unwrap_or_else(|| Arc::from(""));
        let sequence = {
            let status = state.lock();
            if (paused || provider.is_none()) && personal.is_empty() && status.applied_key.is_none()
            {
                return;
            }
            let Some(sequence) = status.sequence.checked_add(1) else {
                return;
            };
            sequence
        };
        // Heavy policy lookup, hashing and JSON construction run on one worker.
        if self.style_worker.is_none() {
            self.style_worker =
                super::style_worker::StyleWorker::new(self.main_dispatch.clone()).ok();
        }
        if self.style_worker.is_none() {
            return;
        }
        {
            let mut status = state.lock();
            status.sequence = sequence;
            status.active = Some(sequence);
            status.pending_key = Some(key.clone());
            status.dirty = false;
            status.in_flight += 1;
        }
        let force_full = state.lock().force_full;
        let delivery = Delivery {
            id,
            dispatch: self.main_dispatch.clone(),
            state,
            reuse: false,
            force_full,
            generic_enabled: false,
            generic_fingerprint: String::new(),
            document_token: String::new(),
            sequence,
            charged_bytes: 0,
            key,
            provider: if paused { None } else { provider },
            personal,
            navigation: view.navigation.clone(),
            permit: view.event_permit.clone(),
        };
        // Wry's callback is Send + Fn, not FnOnce. Contain duplicate callbacks
        // and retain exactly one owner without holding a lock across native work.
        let completion = Mutex::new(Some(delivery));
        let _ = view.evaluate_script_with_callback(INSPECT, move |result| {
            let Some(delivery) = completion.lock().unwrap_or_else(|p| p.into_inner()).take() else {
                return;
            };
            if result.len() > 72 * 1024
                || !delivery
                    .navigation
                    .matches_committed_snapshot(delivery.key.epoch, &delivery.key.url)
            {
                return;
            }
            let Some(identity) = serde_json::from_str::<Option<String>>(&result)
                .ok()
                .flatten()
                .filter(|s| s.len() <= 36 * 1024)
                .and_then(|s| decode_identity(&s))
            else {
                return;
            };
            if identity.url != delivery.key.url
                || identity.token.len() != 32
                || !identity.token.bytes().all(|b| b.is_ascii_hexdigit())
            {
                return;
            }
            let _ = with_document_style(id, move |host| {
                host.prepare_document_styles(id, delivery, identity)
            });
        });
    }

    fn prepare_document_styles(
        &mut self,
        id: ItemId,
        mut delivery: Delivery,
        identity: DocumentIdentity,
    ) {
        let Some(worker) = &self.style_worker else {
            return;
        };
        worker.submit(move || {
            if !delivery
                .navigation
                .matches_committed_snapshot(delivery.key.epoch, &delivery.key.url)
            {
                return None;
            }
            let plan = match delivery
                .provider
                .as_ref()
                .map(|p| p.document_plan(&delivery.key.url))
                .transpose()
            {
                Ok(Some(plan)) => plan,
                Ok(None) => zephium_core::blocker::DocumentStylePlan::empty(),
                Err(_) => return None,
            };
            let fingerprint = plan.fingerprint().as_bytes().iter().fold(
                String::with_capacity(64),
                |mut output, byte| {
                    use std::fmt::Write;
                    let _ = write!(output, "{byte:02x}");
                    output
                },
            );
            // The renderer reports only an identity. Native still computes the
            // exact URL-dependent plan, including generichide exceptions. A
            // same-document navigation or personal edit can then reuse the
            // installed subscription without encoding/copying its whole index.
            delivery.generic_enabled = plan.generic_index.as_ref() != "[]";
            delivery.generic_fingerprint = fingerprint.clone();
            delivery.document_token = identity.token.clone();
            delivery.reuse =
                !delivery.force_full && identity.subscription.as_ref() == Some(&fingerprint);
            let script = document_style_script(
                &identity,
                delivery.sequence,
                &fingerprint,
                &plan,
                &delivery.personal,
                delivery.reuse,
            );
            if script.len() > 4 * 1024 * 1024 {
                return None;
            }
            delivery.charged_bytes = charge_script(script.len())?;
            Some(Box::new(move || {
                let _ = with_document_style(id, move |host| {
                    host.deliver_document_styles(id, delivery, script)
                });
            }))
        });
    }

    fn deliver_document_styles(&mut self, id: ItemId, delivery: Delivery, script: String) {
        let restart = {
            let mut state = delivery.state.lock();
            if state.active != Some(delivery.sequence) {
                return;
            }
            std::mem::take(&mut state.dirty)
        };
        if restart {
            drop(delivery);
            self.refresh_document_styles(id);
            return;
        }
        let Some(view) = self.views.get(&id) else {
            return;
        };
        if !super::permits::navigation_callback_matches(
            &view.event_permit,
            &view.navigation,
            &delivery.permit,
            &delivery.navigation,
            delivery.key.epoch,
        ) || !delivery
            .navigation
            .matches_committed_snapshot(delivery.key.epoch, &delivery.key.url)
        {
            return;
        }
        let completion = Mutex::new(Some(delivery));
        let _ = view.evaluate_script_with_callback(&script, move |result| {
            let Some(delivery) = completion.lock().unwrap_or_else(|p| p.into_inner()).take() else {
                return;
            };
            let dirty = {
                let mut state = delivery.state.lock();
                if state.active != Some(delivery.sequence) {
                    return;
                }
                if result == "true"
                    && delivery
                        .navigation
                        .matches_committed_snapshot(delivery.key.epoch, &delivery.key.url)
                {
                    state.applied_key = Some(delivery.key.clone());
                    state.generic_enabled = delivery.generic_enabled;
                    state.generic_fingerprint = delivery.generic_fingerprint.clone();
                    state.document_token = delivery.document_token.clone();
                    state.force_full = false;
                }
                if result != "true" && delivery.reuse {
                    // The page may have removed the sheet between inspection
                    // and delivery. Retry once with bytes, never a reuse loop.
                    state.force_full = true;
                    state.applied_key = None;
                    state.dirty = true;
                }
                std::mem::take(&mut state.dirty)
            };
            drop(delivery);
            if dirty {
                let _ = with_document_style(id, move |host| host.refresh_document_styles(id));
            } else if result == "true" {
                let _ = super::dispatch::with_generic_style(id, move |host| {
                    host.refresh_generic_styles(id)
                });
            }
        });
    }
}

fn document_style_script(
    identity: &DocumentIdentity,
    sequence: u64,
    fingerprint: &str,
    plan: &zephium_core::blocker::DocumentStylePlan,
    personal: &str,
    reuse: bool,
) -> String {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    let index = serde_json::json!(!reuse && plan.generic_index.as_ref() != "[]");
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let index = serde_json::json!(if reuse {
        ""
    } else {
        plan.generic_index.as_ref()
    });
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    let exceptions = "";
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let exceptions = if reuse { "" } else { plan.exceptions.as_ref() };
    let arguments = serde_json::json!([
        identity.token,
        identity.url,
        format!("{sequence:016x}"),
        fingerprint,
        if reuse { "" } else { plan.css.as_ref() },
        index,
        exceptions,
        css_digest(personal),
        personal,
    ]);
    let subscription = if reuse {
        "a.reuseSubscription(p[0],p[1],p[2],p[3])"
    } else {
        "a.subscription(p[0],p[1],p[2],p[3],p[4],p[5],p[6])"
    };
    format!(
        "((p)=>{{const a=globalThis.__zephium_content_style_v1__;if(!a)return false;const s={subscription};const u=a.apply('personal',p[0],p[1],p[2],p[7],p[8]);return s===true&&u===true;}})({arguments})"
    )
}

fn css_digest(css: &str) -> String {
    use std::fmt::Write;
    let mut output = String::with_capacity(64);
    for byte in Sha256::digest(css.as_bytes()) {
        let _ = write!(output, "{byte:02x}");
    }
    output
}

fn decode_identity(text: &str) -> Option<DocumentIdentity> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    let object = value.as_object()?;
    if object.len() != 3 {
        return None;
    }
    Some(DocumentIdentity {
        token: object.get("token")?.as_str()?.to_owned(),
        url: object.get("url")?.as_str()?.to_owned(),
        subscription: match object.get("subscription")? {
            serde_json::Value::Null => None,
            serde_json::Value::String(value)
                if value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit()) =>
            {
                Some(value.clone())
            }
            _ => return None,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reuse_does_not_encode_the_subscription_payload() {
        let plan = zephium_core::blocker::DocumentStylePlan {
            css: Arc::from(".site-ad{display:none!important}"),
            generic_index: Arc::from("x".repeat(487_107)),
            generic_index_digest: ContentRuleDigest::from_bytes(
                Sha256::digest("x".repeat(487_107).as_bytes()).into(),
            ),
            exceptions: Arc::from("[]"),
        };
        let identity = DocumentIdentity {
            token: "a".repeat(32),
            url: "https://example.com/next".into(),
            subscription: None,
        };
        let fingerprint = "b".repeat(64);
        let full = document_style_script(&identity, 1, &fingerprint, &plan, "", false);
        let reuse = document_style_script(
            &identity,
            2,
            &fingerprint,
            &plan,
            ".personal{display:none!important}",
            true,
        );
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        assert!(full.len() < 1024);
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        assert!(full.len() > 487_107);
        assert!(reuse.len() < 1024);
        assert!(!reuse.contains(".site-ad"));
        assert!(reuse.contains(".personal"));
        assert!(reuse.contains("reuseSubscription"));
    }
    #[test]
    fn a_settled_delivery_releases_the_lock_before_redelivering() {
        let state = DocumentStyleState::default();
        {
            let mut inner = state.lock();
            inner.in_flight = 1;
            inner.active = Some(7);
            inner.dirty = true;
        }
        assert!(state.settle(7));
        let inner = state
            .0
            .try_lock()
            .expect("redelivery would deadlock on the style lock");
        assert!(inner.active.is_none() && !inner.dirty && inner.in_flight == 0);
    }
    #[test]
    fn subscription_identity_is_bounded_and_nullable() {
        assert!(decode_identity(r#"{"token":"x","url":"y","subscription":null}"#).is_some());
        assert!(decode_identity(r#"{"token":"x","url":"y","subscription":"fake"}"#).is_none());
        assert!(
            decode_identity(r#"{"token":"x","url":"y","subscription":null,"extra":1}"#).is_none()
        );
    }
    #[test]
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    fn one_generic_pull_survives_visibility_changes_without_overlapping_requests() {
        let tracker = NavigationEpochTracker::new();
        let key = StyleKey {
            epoch: tracker.begin("https://example.com/").unwrap(),
            url: "https://example.com/".into(),
            subscription: None,
            personal: None,
            paused: false,
        };
        let state = DocumentStyleState::default();
        {
            let mut inner = state.lock();
            inner.applied_key = Some(key.clone());
            inner.generic_enabled = true;
            inner.generic_fingerprint = "a".repeat(64);
            inner.document_token = "b".repeat(32);
        }
        let visible = state.generic_visibility(true);
        let first = state.begin_generic().unwrap();
        assert_eq!(first.3, visible);
        state.generic_visibility(false);
        let shown_again = state.generic_visibility(true);
        assert_ne!(shown_again, first.3);
        assert!(
            state.begin_generic().is_none(),
            "the prior native completion still owns its slot"
        );
        state.end_generic();
        assert_eq!(state.begin_generic().unwrap().3, shown_again);
        let mut replaced = key.clone();
        replaced.url = "https://example.com/next".into();
        state.lock().applied_key = Some(replaced);
        assert!(
            !state.generic_current(&key, &"a".repeat(64)),
            "a changed document cannot accept old tokens"
        );
        state.end_generic();
    }
}
