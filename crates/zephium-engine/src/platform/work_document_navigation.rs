//! Resource-owned document gate. Construction and explicit policy-bound
//! successor loads share one gate; unsolicited transitions, redirects and
//! query updates use an explicit public interaction policy within the same native document.

use std::sync::{Arc, Mutex, Weak};
use std::time::Instant;
use zephium_agentic::{
    ContextJoin, ContextNavigationRequest, ContextNavigationTarget, ContextOperationJoin,
    ContextPortFailure,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Phase {
    Bootstrap,
    Armed,
    Loading,
    Committed,
    Finalizing,
    Sampling,
    Ready,
    Human,
    Refused,
    Retired,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AdmissionKind {
    ProgrammaticGet,
    HistoryBackGet,
}
struct State {
    #[cfg(all(target_os = "macos", feature = "native-agentic-semantic-probe"))]
    form_audit: super::form_qualification::NavigationAudit,
    phase: Phase,
    bootstrap_available: bool,
    bootstrap_id: Option<wry::NavigationId>,
    bootstrap_committed: bool,
    bootstrap_finished: bool,
    target: Option<ContextNavigationTarget>,
    effective: Option<ContextNavigationTarget>,
    policy: zephium_agentic::WorkBrowserDocumentPolicy,
    finalization_generation: u64,
    location_revision: u64,
    native_id: Option<wry::NavigationId>,
    requested: bool,
    admission_kind: Option<AdmissionKind>,
    navigation_epoch: u64,
    operation: Option<ContextOperationJoin>,
    human: Option<human::HumanNavigation>,
    /// Script redirects a site session followed before its document settled.
    site_loads: u8,
    /// A site-session load a script redirect replaced; its late events are noise.
    superseded: Option<wry::NavigationId>,
    /// The gate let an action's own load replace the ready document.
    handed_on: bool,
    /// No action has run on the current document yet: a document the run
    /// opened may still load itself again soon after it commits (a bot check
    /// that clears itself), wherever it was opened from.
    first_document: bool,
    /// That load still needs the page's semantic runtime prepared for it.
    hand_on_pending: bool,
    /// Loads of the page's own the gate admitted that have not started yet.
    page_loads_unstarted: u8,
    /// When the current load committed.
    committed_at: Option<Instant>,
    /// The committed main document's address, before any route fragment.
    document: Option<ContextNavigationTarget>,
    /// A public first load's refused redirect, safe to admit as a new read.
    redirect: Option<ContextNavigationTarget>,
    /// The redirect's failure was already handed to the resource owner.
    redirect_reported: bool,
    /// Open while an admitted action runs on a ready site-session page.
    follow: Option<Follow>,
    /// Where a ready site-session page's own same-site loads are kept, with
    /// no action of the model's in flight, for the controller to follow as
    /// its own navigation; handed over with the run's first action.
    page_loads: Option<zephium_agentic::SemanticActionFollow>,
    /// A site-session page Back is loading exactly: once it is ready it is
    /// a site-session page again, routes and actions' loads included.
    back_policy: Option<zephium_agentic::WorkBrowserDocumentPolicy>,
    #[cfg(target_os = "windows")]
    write_dispatched: bool,
    #[cfg(target_os = "windows")]
    write_navigation: Option<(wry::NavigationId, u64, ContextNavigationTarget)>,
    #[cfg(feature = "native-agentic-work-lifetime-diagnostic")]
    url_observation_failure: Option<crate::WorkUrlObservationFailure>,
    #[cfg(feature = "native-agentic-work-resource-probe")]
    evidence: NavigationEvidence,
}

/// Content-free milestones only; descriptive, never navigation authority.
#[cfg(feature = "native-agentic-work-resource-probe")]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct NavigationEvidence {
    pub bootstrap_started: bool,
    pub bootstrap_committed: bool,
    pub bootstrap_finished: bool,
    pub requested: bool,
    pub started: bool,
    pub committed: bool,
    pub finished: bool,
    pub refused: bool,
    pub last_event: Option<wry::NavigationEventPhase>,
    pub location_callback_after_commit_before_ready: bool,
}

/// Component relations only: no URL, digest, query key/value, or native handle.
/// Equality here is diagnostic; only the original policy-bound gate admits a
/// document.
#[cfg(feature = "native-agentic-work-resource-probe")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CurrentDocumentEvidence {
    MissingNativeUrl,
    MissingAbsoluteString,
    Utf16Limit,
    Utf8Limit,
    InvalidUrl,
    Compared {
        raw_equal: bool,
        canonical_equal: bool,
        scheme_equal: bool,
        host_equal: bool,
        port_equal: bool,
        path_equal: bool,
        query_equal: bool,
        fragment_equal: bool,
        credentials_equal: bool,
        query_present: bool,
        query_empty: bool,
        fragment_present: bool,
        credentials_present: bool,
    },
}
#[cfg(feature = "native-agentic-work-resource-probe")]
impl CurrentDocumentEvidence {
    pub(crate) fn compare(expected: &ContextNavigationTarget, current: &str) -> Self {
        let Ok(actual) = url::Url::parse(current) else {
            return Self::InvalidUrl;
        };
        let expected = expected.as_url();
        Self::Compared {
            raw_equal: expected.as_str() == current,
            canonical_equal: expected == &actual,
            scheme_equal: expected.scheme() == actual.scheme(),
            host_equal: expected.host() == actual.host(),
            port_equal: expected.port_or_known_default() == actual.port_or_known_default(),
            path_equal: expected.path() == actual.path(),
            query_equal: expected.query() == actual.query(),
            fragment_equal: expected.fragment() == actual.fragment(),
            credentials_equal: expected.username() == actual.username()
                && expected.password() == actual.password(),
            query_present: actual.query().is_some(),
            query_empty: actual.query() == Some(""),
            fragment_present: actual.fragment().is_some(),
            credentials_present: !actual.username().is_empty() || actual.password().is_some(),
        }
    }
}

/// One resource's document authority. Construction fixes the initial source;
/// only an admitted execution-lease operation can retire it for a successor.
#[derive(Clone)]
pub(crate) struct WorkDocumentNavigation(Arc<Mutex<State>>);
/// Private, descriptive stamp of the exact committed native document.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct WorkDocumentStamp {
    native_id: wry::NavigationId,
    epoch: u64,
}
/// Visual projection of an admitted native document, including its loading
/// paint. This stamp cannot authorize semantic observations or input.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct WorkPreviewDocumentStamp {
    native_id: wry::NavigationId,
    epoch: u64,
    location_revision: u64,
}
/// Content-free revision captured before one bounded native URL quiet period.
/// It is not document authority and cannot itself authorize a sample.
#[derive(Clone, Debug)]
pub(crate) struct WorkDocumentFinalizationTicket {
    gate: Weak<Mutex<State>>,
    generation: u64,
    location_revision: u64,
}
impl PartialEq for WorkDocumentFinalizationTicket {
    fn eq(&self, other: &Self) -> bool {
        Weak::ptr_eq(&self.gate, &other.gate)
            && self.generation == other.generation
            && self.location_revision == other.location_revision
    }
}
impl Eq for WorkDocumentFinalizationTicket {}
#[cfg(test)]
impl WorkDocumentStamp {
    pub(crate) fn for_test(epoch: u64) -> Self {
        Self {
            native_id: wry::NavigationId::from_raw(1),
            epoch,
        }
    }
}
impl Default for WorkDocumentNavigation {
    fn default() -> Self {
        Self(Arc::new(Mutex::new(State {
            #[cfg(all(target_os = "macos", feature = "native-agentic-semantic-probe"))]
            form_audit: super::form_qualification::NavigationAudit::default(),
            phase: Phase::Bootstrap,
            bootstrap_available: true,
            bootstrap_id: None,
            bootstrap_committed: false,
            bootstrap_finished: false,
            target: None,
            effective: None,
            policy: zephium_agentic::WorkBrowserDocumentPolicy::Exact,
            finalization_generation: 0,
            location_revision: 0,
            native_id: None,
            requested: false,
            admission_kind: None,
            navigation_epoch: 1,
            operation: None,
            human: None,
            site_loads: 0,
            superseded: None,
            handed_on: false,
            first_document: true,
            hand_on_pending: false,
            page_loads_unstarted: 0,
            committed_at: None,
            document: None,
            redirect: None,
            redirect_reported: false,
            follow: None,
            page_loads: None,
            back_policy: None,
            #[cfg(target_os = "windows")]
            write_dispatched: false,
            #[cfg(target_os = "windows")]
            write_navigation: None,
            #[cfg(feature = "native-agentic-work-lifetime-diagnostic")]
            url_observation_failure: None,
            #[cfg(feature = "native-agentic-work-resource-probe")]
            evidence: NavigationEvidence::default(),
        })))
    }
}

/// A site-session load that settled on another of its brand's country
/// domains (kayak.com to kayak.pl): a closed fact, the brand only.
fn log_brand_move(requested: Option<&ContextNavigationTarget>, current: &str) {
    let Some(requested) = requested else {
        return;
    };
    let Ok(current) = ContextNavigationTarget::parse(current) else {
        return;
    };
    let (Some(from), Some(to)) = (
        zephium_agentic::registrable_site(requested),
        zephium_agentic::registrable_site(&current),
    ) else {
        return;
    };
    if from != to {
        if let Some(brand) = zephium_agentic::brand_family(from.as_bytes(), to.as_bytes()) {
            #[cfg(all(target_os = "macos", feature = "native-agentic-work-resource-probe"))]
            crate::diagnostic!("work: site_session moved within brand={brand}");
            #[cfg(not(all(target_os = "macos", feature = "native-agentic-work-resource-probe")))]
            let _ = brand;
        }
    }
}

/// The same page again, its query aside: what a bot check reloads, or what
/// a brand's page loads on its country domain (yelp.com to yelp.pl).
fn reloads(effective: Option<&ContextNavigationTarget>, raw: &str) -> bool {
    let Ok(target) = ContextNavigationTarget::parse(raw) else {
        return false;
    };
    effective.is_some_and(|effective| {
        let (a, b) = (effective.as_url(), target.as_url());
        let same_host = a.host() == b.host()
            || match (
                zephium_agentic::registrable_site(effective),
                zephium_agentic::registrable_site(&target),
            ) {
                (Some(from), Some(to)) => {
                    zephium_agentic::brand_family(from.as_bytes(), to.as_bytes()).is_some()
                }
                _ => false,
            };
        a.scheme() == b.scheme() && same_host && a.path() == b.path()
    })
}

/// How soon after its document committed a ready site-session page may load
/// itself again and be followed.
const SELF_LOAD_WINDOW: std::time::Duration = std::time::Duration::from_secs(20);

/// Script redirects one site-session load may follow before it settles.
const MAX_SITE_LOADS: u8 = 6;

/// What one admitted action may lead the page to load. A same-site GET is
/// cancelled and kept for the controller to follow as its own navigation; a
/// same-site form POST passes once, and only for a confirmed commit.
#[derive(Debug)]
struct Follow {
    post: bool,
    posted: bool,
    /// Rust's own press of a cookie banner's refusal: saving the choice is a
    /// read, so its same-site POST passes once and its loads are followed.
    consent: bool,
    slot: zephium_agentic::SemanticActionFollow,
}

impl State {
    fn clear_write_navigation(&mut self) {
        #[cfg(target_os = "windows")]
        {
            self.write_navigation = None;
        }
    }
    fn initial_public_reload(&self, raw: &str) -> bool {
        use zephium_agentic::WorkBrowserDocumentPolicy as P;
        matches!(
            self.policy,
            P::PublicSameDocumentQuery | P::PublicResultsPage
        ) && self.first_document
            && self.follow.is_none()
            && self.human.is_none()
            && self.site_loads < MAX_SITE_LOADS
            && self
                .committed_at
                .is_some_and(|at| at.elapsed() <= SELF_LOAD_WINDOW)
            && self.target.as_ref().is_some_and(|target| {
                self.effective
                    .as_ref()
                    .is_none_or(|effective| effective == target)
                    && self.policy.admits_request(target)
                    && ContextNavigationTarget::parse(raw).as_ref() == Ok(target)
            })
    }

    /// A site-session GET the gate follows: the tracked load's own server
    /// redirect, a script redirect before the document settles, or a first
    /// page that reloads itself moments after it settled (a bot check that
    /// clears itself). Other page-initiated loads are cancelled and the page
    /// stays.
    fn site_follows(&mut self, raw: &str) -> bool {
        let Some(requested) = self.target.as_ref() else {
            return false;
        };
        let policy = self.policy;
        if !ContextNavigationTarget::parse(raw).is_ok_and(|target| {
            zephium_agentic::same_work_site(requested, &target)
                && (!policy.is_public() || policy.admits_request(&target))
        }) {
            return false;
        }
        match self.phase {
            Phase::Loading => self.requested && self.native_id.is_some(),
            Phase::Committed | Phase::Finalizing | Phase::Sampling
                if self.site_loads < MAX_SITE_LOADS =>
            {
                let (Some(generation), Some(loads)) = (
                    self.finalization_generation.checked_add(1),
                    self.site_loads.checked_add(1),
                ) else {
                    return false;
                };
                self.site_loads = loads;
                self.superseded = self.native_id.take();
                self.finalization_generation = generation;
                self.clear_write_navigation();
                self.location_revision = 0;
                self.requested = true;
                self.phase = Phase::Armed;
                // The committed document's semantic runtime ends with it; the
                // policy callback prepares the runtime for the new one.
                self.hand_on_pending = true;
                self.page_loads_unstarted = 1;
                true
            }
            // The page asked for another load of its own before the one the
            // gate followed started (a bot check that reloads its page more
            // than once): WebKit drops the earlier request for the later one,
            // so refusing the later one leaves the page loading nothing.
            Phase::Armed
                if self.page_loads_unstarted > 0
                    && self.requested
                    && self.operation.is_none()
                    && self.follow.is_none()
                    && self.human.is_none()
                    && self.site_loads < MAX_SITE_LOADS =>
            {
                let (Some(loads), Some(unstarted)) = (
                    self.site_loads.checked_add(1),
                    self.page_loads_unstarted.checked_add(1),
                ) else {
                    return false;
                };
                self.site_loads = loads;
                self.page_loads_unstarted = unstarted;
                self.hand_on_pending = true;
                true
            }
            Phase::Ready
                if (policy == zephium_agentic::WorkBrowserDocumentPolicy::SiteSession
                    || self.initial_public_reload(raw))
                    && self.site_loads < MAX_SITE_LOADS
                    && self.first_document
                    && reloads(self.effective.as_ref(), raw)
                    && self.operation.is_none()
                    && self.follow.is_none()
                    && self.human.is_none()
                    && self
                        .committed_at
                        .is_some_and(|at| at.elapsed() <= SELF_LOAD_WINDOW) =>
            {
                let (Some(generation), Some(loads)) = (
                    self.finalization_generation.checked_add(1),
                    self.site_loads.checked_add(1),
                ) else {
                    return false;
                };
                self.site_loads = loads;
                self.superseded = self.native_id.take();
                self.effective = None;
                self.finalization_generation = generation;
                self.clear_write_navigation();
                self.location_revision = 0;
                self.requested = true;
                self.phase = Phase::Armed;
                // Settled like an action's own load: reads wait for the new
                // document, which the host finalizes as it does a hand-on.
                self.handed_on = true;
                self.hand_on_pending = true;
                self.page_loads_unstarted = 1;
                true
            }
            _ => false,
        }
    }
}

impl State {
    /// A move to a fragment of the committed document itself (a script's
    /// `#inbox`, an app's hash route) loads nothing: it is admitted in place
    /// and `location_changed` records the new address. The document an
    /// armed load committed routes itself the same way before it settles.
    fn route_move(&self, raw: &str) -> bool {
        self.policy == zephium_agentic::WorkBrowserDocumentPolicy::SiteSession
            && matches!(
                self.phase,
                Phase::Committed | Phase::Finalizing | Phase::Sampling | Phase::Ready
            )
            && self
                .document
                .as_ref()
                .is_some_and(|document| human::same_document_move(document, raw))
    }
    /// A same-site GET a ready site-session page starts by itself (a bot
    /// check that cleared, a timed refresh, a script redirect) while no
    /// action of the model's runs: cancelled, and kept for the controller,
    /// which loads it as its own navigation. Whether it was kept.
    fn keep_page_load(&mut self, raw: &str) -> bool {
        if self.policy != zephium_agentic::WorkBrowserDocumentPolicy::SiteSession
            || self.phase != Phase::Ready
            || self.follow.is_some()
            || self.operation.is_some()
            || self.human.is_some()
        {
            return false;
        }
        let Some(slot) = self.page_loads.as_ref() else {
            return false;
        };
        let Ok(target) = ContextNavigationTarget::parse(raw) else {
            return false;
        };
        if !self
            .target
            .as_ref()
            .is_some_and(|requested| zephium_agentic::same_work_site(requested, &target))
            || !self.policy.admits_request(&target)
        {
            return false;
        }
        slot.record(target);
        true
    }

    /// A public first load sent elsewhere before its document is ready: a
    /// server redirect, or the page's own script replacing it. The load is
    /// refused like any unsolicited one, and the document with it; the
    /// destination is kept only when a new public read could be admitted
    /// there. A refusal at any other time is not a redirect.
    fn refuse_redirect(&mut self, raw: &str) -> bool {
        let initial = self.policy.is_public()
            && self.operation.is_none()
            && self.human.is_none()
            && self.admission_kind == Some(AdmissionKind::ProgrammaticGet)
            && self.requested
            && match self.phase {
                Phase::Loading => self.native_id.is_some(),
                Phase::Committed | Phase::Finalizing | Phase::Sampling => true,
                _ => false,
            };
        if !initial {
            return false;
        }
        let Some(destination) = ContextNavigationTarget::parse(raw)
            .ok()
            .filter(|destination| {
                self.target.as_ref() != Some(destination)
                    && self.policy.admits_request(destination)
                    && zephium_agentic::AgentNavigationDiscovery::try_new_public_page(
                        destination.clone(),
                    )
                    .is_ok()
            })
        else {
            return false;
        };
        self.redirect = Some(destination);
        self.clear_write_navigation();
        self.phase = Phase::Refused;
        true
    }
}

impl State {
    /// A main-frame load a page starts while an admitted action runs. Only
    /// the confirmed commit's same-site POST passes, once, and the gate
    /// follows it like a script redirect; a same-site GET is kept for the
    /// controller and cancelled; anything else is cancelled.
    fn follow_from_action(&mut self, raw: &str, get: bool) -> bool {
        let same_site = self.target.as_ref().is_some_and(|requested| {
            ContextNavigationTarget::parse(raw)
                .is_ok_and(|target| zephium_agentic::same_work_site(requested, &target))
        });
        let Some(follow) = self.follow.as_mut() else {
            return false;
        };
        if !same_site {
            return false;
        }
        // A saved commit's own page may hand on with a form GET (a consent
        // host returning to the site): it is followed as the POST's redirect.
        // A consent press's own reload or hand-back is followed the same way,
        // and so is any action's reload of the page it is on: it opens no new
        // destination, and the controller cannot follow it as one.
        let reload = self.effective.as_ref().is_some_and(|effective| {
            ContextNavigationTarget::parse(raw).is_ok_and(|target| target == *effective)
        });
        if get && (follow.posted || follow.consent || reload) {
            let (Some(generation), Some(loads)) = (
                self.finalization_generation.checked_add(1),
                self.site_loads
                    .checked_add(1)
                    .filter(|loads| *loads <= MAX_SITE_LOADS),
            ) else {
                return false;
            };
            self.superseded = self.native_id.take();
            self.effective = None;
            self.finalization_generation = generation;
            self.clear_write_navigation();
            self.location_revision = 0;
            self.site_loads = loads;
            self.requested = true;
            self.phase = Phase::Armed;
            self.handed_on = true;
            self.hand_on_pending = true;
            return true;
        }
        if get {
            if let Ok(target) = ContextNavigationTarget::parse(raw) {
                follow.slot.record(target);
            }
            return false;
        }
        // A site's own consent host saves the person's cookie choice with a
        // POST; that choice is a read, so it passes once like a commit.
        let consent = self.effective.as_ref().is_some_and(|document| {
            document
                .as_url()
                .host_str()
                .is_some_and(|host| host.starts_with("consent."))
        });
        if !(follow.post || follow.consent || consent) || follow.posted {
            return false;
        }
        let Some(generation) = self.finalization_generation.checked_add(1) else {
            return false;
        };
        follow.posted = true;
        self.superseded = self.native_id.take();
        self.effective = None;
        self.finalization_generation = generation;
        self.clear_write_navigation();
        self.location_revision = 0;
        self.site_loads = 0;
        self.requested = true;
        self.phase = Phase::Armed;
        self.handed_on = true;
        self.hand_on_pending = true;
        true
    }
}

impl WorkDocumentNavigation {
    /// Opens the follow window for one dispatched action on a ready
    /// site-session page; it lasts until the next action, document change
    /// or cancellation, since a page's script often navigates only after
    /// the click returns. `post` admits one same-site form POST.
    pub(crate) fn open_follow(
        &self,
        post: bool,
        consent: bool,
        slot: zephium_agentic::SemanticActionFollow,
    ) {
        if let Ok(mut state) = self.0.lock() {
            state.handed_on = false;
            state.first_document = false;
            state.clear_write_navigation();
            #[cfg(target_os = "windows")]
            {
                state.write_dispatched = false;
            }
            if state.policy == zephium_agentic::WorkBrowserDocumentPolicy::SiteSession {
                state.page_loads = Some(slot.clone());
            }
            state.follow = (state.policy
                == zephium_agentic::WorkBrowserDocumentPolicy::SiteSession
                && state.phase == Phase::Ready)
                .then_some(Follow {
                    post,
                    posted: false,
                    consent,
                    slot,
                });
        }
    }
    /// Re-admits the unchanged ready document under the next epoch after an
    /// unpresented handover; a moved or loading page is refused.
    pub(crate) fn hand_back(&self, current: &str) -> Result<ContextNavigationTarget, ()> {
        let mut state = self.0.lock().map_err(|_| ())?;
        let effective = state.effective.clone().ok_or(())?;
        if state.phase != Phase::Ready
            || state.operation.is_some()
            || state.native_id.is_none()
            || effective.as_url().as_str() != current
        {
            return Err(());
        }
        state.navigation_epoch = state.navigation_epoch.checked_add(1).ok_or(())?;
        state.follow = None;
        // The page the run resumes on is the document, not the route the app
        // keeps in its fragment: the successor is admitted as a request.
        ContextNavigationTarget::parse(human::without_any_fragment(&effective).as_str())
            .map_err(|_| ())
    }
    /// The document is being replaced by a load the gate let an action's
    /// page start (a saved consent, a confirmed commit): reads and actions
    /// of the old document end without leaving the resource uncertain.
    /// The settled document's own address, whatever is armed after it.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub(crate) fn settled_target(&self) -> Option<ContextNavigationTarget> {
        let state = self.0.lock().ok()?;
        state.effective.clone()
    }
    /// The settled site-session document a load would depart from, by its
    /// address without the route an app keeps in its fragment.
    pub(crate) fn departure(&self) -> Option<ContextNavigationTarget> {
        let state = self.0.lock().ok()?;
        if state.phase != Phase::Ready
            || state.policy != zephium_agentic::WorkBrowserDocumentPolicy::SiteSession
        {
            return None;
        }
        ContextNavigationTarget::parse(
            human::without_any_fragment(state.effective.as_ref()?).as_str(),
        )
        .ok()
    }
    pub(crate) fn handed_on(&self) -> bool {
        self.0.lock().is_ok_and(|state| state.handed_on)
    }
    /// When a site-session document committed without reporting its load
    /// finished: a page that keeps loading while it is already usable.
    pub(crate) fn committed_unfinished_since(&self) -> Option<Instant> {
        let state = self.0.lock().ok()?;
        (state.policy == zephium_agentic::WorkBrowserDocumentPolicy::SiteSession
            && state.phase == Phase::Committed
            && state.native_id.is_some())
        .then_some(state.committed_at)
        .flatten()
    }
    /// When the document an admitted hand-on load committed did so, while it
    /// has not reported its load finished.
    pub(crate) fn hand_on_committed_since(&self) -> Option<Instant> {
        let state = self.0.lock().ok()?;
        (state.handed_on && state.phase == Phase::Committed)
            .then_some(state.committed_at)
            .flatten()
    }
    /// Takes the one preparation the last admitted hand-on load needs.
    pub(crate) fn take_hand_on(&self) -> bool {
        self.0
            .lock()
            .is_ok_and(|mut state| std::mem::take(&mut state.hand_on_pending))
    }
    pub(crate) fn close_follow(&self) {
        if let Ok(mut state) = self.0.lock() {
            state.follow = None;
        }
    }
    /// The action's confirmed POST is still loading its response.
    pub(crate) fn posting(&self) -> bool {
        self.0.lock().is_ok_and(|state| {
            state.follow.as_ref().is_some_and(|follow| follow.posted)
                && !matches!(state.phase, Phase::Ready | Phase::Refused | Phase::Retired)
        })
    }
    pub(crate) fn ready_target(&self) -> Option<ContextNavigationTarget> {
        let state = self.0.lock().ok()?;
        (state.phase == Phase::Ready && state.operation.is_none())
            .then(|| state.effective.clone())
            .flatten()
    }
    /// Content-free control stage for release-excluded lifetime diagnostics.
    /// This never samples the current URL or exposes native navigation IDs.
    #[cfg(feature = "native-agentic-work-lifetime-diagnostic")]
    pub(crate) fn deadline_stage(&self) -> crate::WorkResourceDeadlineStage {
        use crate::WorkResourceDeadlineStage as Stage;

        self.0
            .lock()
            .map_or(Stage::Unattributed, |state| match state.phase {
                Phase::Bootstrap => Stage::ConstructionBootstrap,
                Phase::Armed => Stage::ConstructionTargetArmed,
                Phase::Loading => Stage::ConstructionTargetProvisional,
                Phase::Committed => Stage::ConstructionTargetCommitted,
                Phase::Finalizing => Stage::ConstructionTargetFinalizing,
                Phase::Sampling => Stage::ConstructionTargetSampling,
                Phase::Ready => Stage::ConstructionTargetReady,
                Phase::Human => Stage::Unattributed,
                Phase::Refused => Stage::ConstructionRefused,
                Phase::Retired => Stage::ConstructionRetired,
            })
    }

    #[cfg(any(
        test,
        all(target_os = "macos", feature = "native-agentic-semantic-probe")
    ))]
    pub(crate) fn document_stamp(&self) -> Option<WorkDocumentStamp> {
        let state = self.0.lock().ok()?;
        if state.phase != Phase::Ready {
            return None;
        }
        Some(WorkDocumentStamp {
            native_id: state.native_id?,
            epoch: state.navigation_epoch,
        })
    }
    pub(crate) fn preview_document_stamp(&self) -> Option<WorkPreviewDocumentStamp> {
        let state = self.0.lock().ok()?;
        if !matches!(
            state.phase,
            Phase::Committed | Phase::Finalizing | Phase::Sampling | Phase::Ready
        ) {
            return None;
        }
        Some(WorkPreviewDocumentStamp {
            native_id: state.native_id?,
            epoch: state.navigation_epoch,
            location_revision: state.location_revision,
        })
    }
    pub(crate) fn observation_stamp(&self, context: ContextJoin) -> Option<WorkDocumentStamp> {
        let state = self.0.lock().ok()?;
        (state.phase == Phase::Ready
            && state.operation.is_none()
            && state.navigation_epoch == context.navigation_epoch().get()
            && state.navigation_epoch == context.frame_generation().get())
        .then_some(WorkDocumentStamp {
            native_id: state.native_id?,
            epoch: state.navigation_epoch,
        })
    }
    /// Reuse the exact resource gate only under an admitted successor operation.
    /// The source was freshly observed under the same execution lease; the host
    /// additionally checks the current native URL and exact ingress reservation.
    pub(crate) fn arm_successor(
        &self,
        source: ContextJoin,
        request: &ContextNavigationRequest,
    ) -> Result<(), ()> {
        let mut state = self.0.lock().map_err(|_| ())?;
        let next = request.operation().context();
        if state.phase != Phase::Ready
            || state.operation.is_some()
            || state.native_id.is_none()
            || state.navigation_epoch != source.navigation_epoch().get()
            || source.identity() != next.identity()
            || source.context_generation() != next.context_generation()
            || source.cancellation_generation() != next.cancellation_generation()
            || source.frame() != zephium_agentic::FrameId::MAIN
            || next.frame() != zephium_agentic::FrameId::MAIN
            || source.navigation_epoch().get().checked_add(1) != Some(next.navigation_epoch().get())
            || source.frame_generation().get().checked_add(1) != Some(next.frame_generation().get())
            || request.redirect_policy().is_some()
            || request.document_policy()
                == zephium_agentic::WorkBrowserDocumentPolicy::InitialQueryFinalization
            || !request.document_policy().admits_request(request.target())
        {
            return Err(());
        }
        state.follow = None;
        state.back_policy = None;
        state.operation = Some(request.operation());
        state.first_document = true;
        state.page_loads_unstarted = 0;
        state.target = Some(request.target().clone());
        state.effective = None;
        state.policy = request.document_policy();
        state.native_id = None;
        state.requested = false;
        state.admission_kind = Some(AdmissionKind::ProgrammaticGet);
        state.finalization_generation = state.finalization_generation.checked_add(1).ok_or(())?;
        state.clear_write_navigation();
        state.location_revision = 0;
        #[cfg(feature = "native-agentic-work-lifetime-diagnostic")]
        {
            state.url_observation_failure = None;
        }
        state.phase = Phase::Armed;
        Ok(())
    }

    /// Arms a load of the document a failed site-session load departed
    /// from, under that load's own operation, whatever phase the failure
    /// left. The failed load's late events are its own, never this load's.
    pub(crate) fn arm_departure(
        &self,
        source: ContextJoin,
        operation: ContextOperationJoin,
        target: ContextNavigationTarget,
    ) -> Result<(), ()> {
        let mut state = self.0.lock().map_err(|_| ())?;
        if matches!(
            state.phase,
            Phase::Bootstrap | Phase::Human | Phase::Retired | Phase::Ready
        ) || state.operation.is_some_and(|armed| armed != operation)
            || state.navigation_epoch != source.navigation_epoch().get()
            || state.policy != zephium_agentic::WorkBrowserDocumentPolicy::SiteSession
            || !state.policy.admits_request(&target)
        {
            return Err(());
        }
        state.superseded = state.native_id.take();
        state.follow = None;
        state.back_policy = None;
        state.operation = Some(operation);
        state.first_document = true;
        state.page_loads_unstarted = 0;
        state.target = Some(target);
        state.effective = None;
        state.requested = false;
        state.admission_kind = Some(AdmissionKind::ProgrammaticGet);
        state.finalization_generation = state.finalization_generation.checked_add(1).ok_or(())?;
        state.clear_write_navigation();
        state.location_revision = 0;
        state.phase = Phase::Armed;
        Ok(())
    }

    pub(crate) fn arm_history_back(
        &self,
        source: ContextJoin,
        operation: ContextOperationJoin,
        target: ContextNavigationTarget,
    ) -> Result<(), ()> {
        self.arm_back(source, operation, target, AdmissionKind::HistoryBackGet)
    }

    /// Back as a fresh load of the predecessor's address, for a page whose
    /// own history no longer joins the run's ledger.
    #[cfg(target_os = "macos")]
    pub(crate) fn arm_history_reload(
        &self,
        source: ContextJoin,
        operation: ContextOperationJoin,
        target: ContextNavigationTarget,
    ) -> Result<(), ()> {
        self.arm_back(source, operation, target, AdmissionKind::ProgrammaticGet)
    }

    fn arm_back(
        &self,
        source: ContextJoin,
        operation: ContextOperationJoin,
        target: ContextNavigationTarget,
        admission: AdmissionKind,
    ) -> Result<(), ()> {
        let mut state = self.0.lock().map_err(|_| ())?;
        let next = operation.context();
        if state.phase != Phase::Ready
            || state.operation.is_some()
            || state.native_id.is_none()
            || state.navigation_epoch != source.navigation_epoch().get()
            || source.identity() != next.identity()
            || source.context_generation() != next.context_generation()
            || source.cancellation_generation() != next.cancellation_generation()
            || source.frame() != zephium_agentic::FrameId::MAIN
            || next.frame() != zephium_agentic::FrameId::MAIN
            || source.navigation_epoch().get().checked_add(1) != Some(next.navigation_epoch().get())
            || source.frame_generation().get().checked_add(1) != Some(next.frame_generation().get())
        {
            return Err(());
        }
        state.follow = None;
        state.back_policy = (state.policy
            == zephium_agentic::WorkBrowserDocumentPolicy::SiteSession)
            .then_some(state.policy);
        state.operation = Some(operation);
        state.first_document = true;
        state.page_loads_unstarted = 0;
        state.target = Some(target);
        state.effective = None;
        state.policy = zephium_agentic::WorkBrowserDocumentPolicy::Exact;
        state.native_id = None;
        state.requested = false;
        state.admission_kind = Some(admission);
        state.finalization_generation = state.finalization_generation.checked_add(1).ok_or(())?;
        state.clear_write_navigation();
        state.location_revision = 0;
        state.phase = Phase::Armed;
        Ok(())
    }
    /// One exact terminal after original native event callbacks have returned.
    pub(crate) fn take_successor_terminal(
        &self,
    ) -> Option<(
        ContextOperationJoin,
        Result<ContextNavigationTarget, ContextPortFailure>,
    )> {
        let mut state = self.0.lock().ok()?;
        let outcome = match state.phase {
            Phase::Ready => Ok(state.effective.clone()?),
            Phase::Refused | Phase::Retired => Err(ContextPortFailure::NativeRefused),
            _ => return None,
        };
        let operation = state.operation.take()?;
        if outcome.is_ok() {
            state.navigation_epoch = operation.context().navigation_epoch().get();
        }
        Some((operation, outcome))
    }
    #[cfg(feature = "native-agentic-work-resource-probe")]
    pub(crate) fn construction_evidence(&self) -> Option<NavigationEvidence> {
        let state = self.0.lock().ok()?;
        Some(NavigationEvidence {
            bootstrap_started: state.bootstrap_id.is_some() || state.bootstrap_finished,
            bootstrap_committed: state.bootstrap_committed,
            bootstrap_finished: state.bootstrap_finished,
            requested: state.requested,
            started: state.native_id.is_some(),
            refused: state.phase == Phase::Refused,
            ..state.evidence
        })
    }
    #[cfg(feature = "native-agentic-work-resource-probe")]
    pub(crate) fn witness_document(&self) -> Option<wry::NavigationId> {
        let state = self.0.lock().ok()?;
        (state.phase == Phase::Ready)
            .then_some(state.native_id)
            .flatten()
    }
    /// Called once after selected-profile policy and native owner publication.
    #[cfg(test)]
    pub(crate) fn arm(&self, target: ContextNavigationTarget) -> Result<(), ()> {
        self.arm_with_policy(target, zephium_agentic::WorkBrowserDocumentPolicy::Exact)
    }
    pub(crate) fn arm_with_policy(
        &self,
        target: ContextNavigationTarget,
        policy: zephium_agentic::WorkBrowserDocumentPolicy,
    ) -> Result<(), ()> {
        let mut state = self.0.lock().map_err(|_| ())?;
        if state.phase != Phase::Bootstrap
            || !state.bootstrap_finished
            || state.target.is_some()
            || !policy.admits_request(&target)
        {
            return Err(());
        }
        state.bootstrap_available = false;
        state.bootstrap_id = None;
        state.target = Some(target);
        state.admission_kind = Some(AdmissionKind::ProgrammaticGet);
        state.policy = policy;
        state.finalization_generation = state.finalization_generation.checked_add(1).ok_or(())?;
        state.clear_write_navigation();
        state.location_revision = 0;
        #[cfg(feature = "native-agentic-work-lifetime-diagnostic")]
        {
            state.url_observation_failure = None;
        }
        state.phase = Phase::Armed;
        Ok(())
    }
    #[cfg(any(test, target_os = "windows"))]
    pub(crate) fn allows(&self, target: &str) -> bool {
        let Ok(mut state) = self.0.lock() else {
            return false;
        };
        #[cfg(target_os = "windows")]
        if state.phase == Phase::Human {
            return state
                .human
                .as_mut()
                .is_some_and(|human| human.allows(target, Some(true)));
        }
        if state.phase == Phase::Bootstrap && target == "about:blank" && state.bootstrap_available {
            state.bootstrap_available = false;
            return true;
        }
        if state.phase == Phase::Armed
            && !state.requested
            && state
                .target
                .as_ref()
                .is_some_and(|expected| expected.as_url().as_str() == target)
        {
            state.requested = true;
            return true;
        }
        if state.route_move(target) {
            return true;
        }
        #[cfg(target_os = "windows")]
        if state.policy == zephium_agentic::WorkBrowserDocumentPolicy::SiteSession
            && state.phase == Phase::Ready
            && state.follow.is_some()
        {
            let confirmed = state
                .follow
                .as_ref()
                .is_some_and(|follow| follow.post || follow.consent);
            return state.follow_from_action(target, !confirmed);
        }
        if (state.policy.follows_site_loads() || state.initial_public_reload(target))
            && state.site_follows(target)
        {
            return true;
        }
        // A refused unsolicited navigation grants no replacement document.
        // WebView2 asks again for each server redirect of the main frame.
        if !state.keep_page_load(target) {
            state.refuse_redirect(target);
        }
        false
    }
    #[cfg(target_os = "windows")]
    pub(crate) fn allows_windows_request(
        &self,
        raw: &str,
        method: &str,
        main_document: Option<bool>,
    ) -> bool {
        let Ok(mut state) = self.0.lock() else {
            return false;
        };
        if matches!(state.phase, Phase::Retired | Phase::Refused) {
            return false;
        }
        if main_document == Some(false) {
            return true;
        }
        if matches!(method, "GET" | "HEAD" | "OPTIONS") {
            return true;
        }
        if state.phase == Phase::Human {
            return state
                .human
                .as_mut()
                .is_some_and(|human| human.allows_windows_request(raw, main_document));
        }
        // WebView2 may expose the request before browser-generated Fetch
        // Metadata headers exist. Their absence supplies no frame identity.
        // Instead correlate only an already-admitted main NavigationStarting
        // with this exact URI, native ID and generation while it is Loading.
        // A same-URI subframe can consume this one-shot token (availability),
        // but cannot grant a new destination, action or cross-origin write.
        let joined_navigation = main_document.is_none()
            && state.phase == Phase::Loading
            && state.handed_on
            && state
                .write_navigation
                .as_ref()
                .is_some_and(|(id, generation, target)| {
                    state.native_id == Some(*id)
                        && state.finalization_generation == *generation
                        && target.as_url().as_str() == raw
                });
        if main_document != Some(true) && !joined_navigation {
            return false;
        }
        let same_site = state.target.as_ref().is_some_and(|target| {
            ContextNavigationTarget::parse(raw)
                .is_ok_and(|request| zephium_agentic::same_work_site(target, &request))
        });
        let confirmed = state
            .follow
            .as_ref()
            .is_some_and(|follow| follow.post || follow.consent);
        if same_site && confirmed && !state.write_dispatched {
            state.write_dispatched = true;
            state.clear_write_navigation();
            true
        } else {
            false
        }
    }
    /// Apple policy admission joins the exact armed transition class with the
    /// native request cause, method and frame. URL equality alone cannot turn
    /// a page-driven form or history action into host authority.
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    pub(crate) fn allows_apple_action(
        &self,
        target: &str,
        action: wry::AppleNavigationAction,
    ) -> bool {
        use wry::{AppleNavigationAction, AppleNavigationType};
        let Ok(mut state) = self.0.lock() else {
            return false;
        };
        if state.phase == Phase::Human {
            return state
                .human
                .as_mut()
                .is_some_and(|human| human.allows(target, action.target_is_main_frame));
        }
        if state.phase == Phase::Bootstrap
            && target == "about:blank"
            && state.bootstrap_available
            && action
                == (AppleNavigationAction {
                    navigation_type: AppleNavigationType::Other,
                    is_get: true,
                    target_is_main_frame: Some(true),
                    source: None,
                })
        {
            state.bootstrap_available = false;
            return true;
        }
        // Frames inside a loading or ready document are page content, not a
        // new document: embeds and bot-check widgets load through them.
        if action.target_is_main_frame == Some(false)
            && matches!(
                state.phase,
                Phase::Armed
                    | Phase::Loading
                    | Phase::Committed
                    | Phase::Finalizing
                    | Phase::Sampling
                    | Phase::Ready
            )
        {
            return true;
        }
        if action.target_is_main_frame == Some(true) && action.is_get && state.route_move(target) {
            return true;
        }
        if state.policy == zephium_agentic::WorkBrowserDocumentPolicy::SiteSession
            && state.phase == Phase::Ready
            && action.target_is_main_frame == Some(true)
            && state.follow.is_some()
        {
            return state.follow_from_action(target, action.is_get);
        }
        let site_load = state.policy.follows_site_loads()
            && (state.phase != Phase::Ready
                || state.policy == zephium_agentic::WorkBrowserDocumentPolicy::SiteSession);
        let public_reload = state.initial_public_reload(target)
            && matches!(
                action.navigation_type,
                AppleNavigationType::Reload | AppleNavigationType::Other
            );
        if (site_load || public_reload)
            && action.target_is_main_frame == Some(true)
            && action.is_get
            && state.site_follows(target)
        {
            return true;
        }
        // The page replaced its admitted POST, before it started, with a
        // same-site GET (a consent host handing back to the site).
        if state.policy == zephium_agentic::WorkBrowserDocumentPolicy::SiteSession
            && action.target_is_main_frame == Some(true)
            && action.is_get
            && state.phase == Phase::Armed
            && state.requested
            && state.native_id.is_none()
            && state.follow.as_ref().is_some_and(|follow| follow.posted)
            && state.target.as_ref().is_some_and(|requested| {
                ContextNavigationTarget::parse(target)
                    .is_ok_and(|target| zephium_agentic::same_work_site(requested, &target))
            })
        {
            return true;
        }
        let cause_matches = match state.admission_kind {
            Some(AdmissionKind::ProgrammaticGet) => {
                action.navigation_type == AppleNavigationType::Other
            }
            Some(AdmissionKind::HistoryBackGet) => {
                action.navigation_type == AppleNavigationType::BackForward
            }
            None => false,
        };
        if state.phase == Phase::Armed
            && !state.requested
            && cause_matches
            && action.is_get
            && action.target_is_main_frame == Some(true)
            && state
                .target
                .as_ref()
                .is_some_and(|expected| expected.as_url().as_str() == target)
        {
            state.requested = true;
            return true;
        }
        if action.target_is_main_frame == Some(true)
            && action.is_get
            && !state.keep_page_load(target)
        {
            state.refuse_redirect(target);
        }
        false
    }
    /// The refused redirect that failed this public document, if one did.
    pub(crate) fn redirected(&self) -> Option<ContextNavigationTarget> {
        let state = self.0.lock().ok()?;
        (state.phase == Phase::Refused)
            .then(|| state.redirect.clone())
            .flatten()
    }
    /// The redirect failure, the first time it is asked for after the gate
    /// refused the load; the policy callback reports it at once.
    pub(crate) fn take_redirect_report(&self) -> Option<ContextNavigationTarget> {
        let mut state = self.0.lock().ok()?;
        if state.phase != Phase::Refused || state.redirect_reported {
            return None;
        }
        let redirect = state.redirect.clone()?;
        state.redirect_reported = true;
        Some(redirect)
    }
    /// Returns (actual document committed, resource owner must reconcile).
    pub(crate) fn observe(&self, event: wry::NavigationEvent) -> Result<(bool, bool), ()> {
        use wry::NavigationEventPhase as E;
        let mut state = self.0.lock().map_err(|_| ())?;
        #[cfg(target_os = "windows")]
        if state
            .write_navigation
            .as_ref()
            .is_some_and(|(id, _, _)| *id == event.id)
            && event.phase != E::Started
        {
            state.clear_write_navigation();
        }
        if matches!(state.phase, Phase::Refused | Phase::Retired) {
            return Ok((false, false));
        }
        if state.phase == Phase::Human {
            return match state.human.as_mut().ok_or(())?.observe(event) {
                Ok(facts) => Ok(facts),
                Err(()) => {
                    state.phase = Phase::Refused;
                    Ok((false, true))
                }
            };
        }
        #[cfg(feature = "native-agentic-work-resource-probe")]
        {
            state.evidence.last_event = Some(event.phase);
        }
        if state.phase == Phase::Bootstrap {
            if event.url != "about:blank" {
                state.phase = Phase::Refused;
                return Ok((false, true));
            }
            return match event.phase {
                E::Started if !state.bootstrap_available && state.bootstrap_id.is_none() => {
                    state.bootstrap_id = Some(event.id);
                    Ok((false, false))
                }
                E::Committed
                    if state.bootstrap_id == Some(event.id) && !state.bootstrap_committed =>
                {
                    state.bootstrap_committed = true;
                    Ok((true, false))
                }
                E::Finished
                    if state.bootstrap_id == Some(event.id)
                        && state.bootstrap_committed
                        && !state.bootstrap_finished =>
                {
                    state.bootstrap_finished = true;
                    Ok((false, true))
                }
                _ => {
                    state.phase = Phase::Refused;
                    Ok((false, true))
                }
            };
        }
        let site = state.policy.follows_site_loads();
        let replaced = site
            || (state.policy
                == zephium_agentic::WorkBrowserDocumentPolicy::PublicSameDocumentQuery
                && state.site_loads > 0);
        // Navigation IDs are WebKit object addresses, which a later load can
        // reuse: a superseded load has already started, so a start while the
        // gate waits for its armed load is that load, whatever its ID.
        if replaced
            && state.superseded == Some(event.id)
            && state.phase == Phase::Armed
            && state.requested
            && state.native_id.is_none()
            && event.phase == E::Started
        {
            state.superseded = None;
        }
        if replaced && state.superseded == Some(event.id) {
            return Ok((false, false));
        }
        if replaced
            && state.phase == Phase::Ready
            && state.native_id != Some(event.id)
            && matches!(event.phase, E::Failed | E::Cancelled)
        {
            return Ok((false, false));
        }
        let exact = state.target.as_ref().is_some_and(|target| {
            if site {
                ContextNavigationTarget::parse(&event.url)
                    .is_ok_and(|url| zephium_agentic::same_work_site(target, &url))
            } else {
                target.as_url().as_str() == event.url
            }
        });
        match (state.phase, event.phase) {
            (Phase::Loading, E::Redirected) if site && state.native_id == Some(event.id) => {
                Ok((false, false))
            }
            (Phase::Armed, E::Started) if exact && state.requested && state.native_id.is_none() => {
                state.native_id = Some(event.id);
                state.phase = Phase::Loading;
                state.page_loads_unstarted = state.page_loads_unstarted.saturating_sub(1);
                #[cfg(target_os = "windows")]
                if state.handed_on
                    && state
                        .follow
                        .as_ref()
                        .is_some_and(|follow| follow.post || follow.consent)
                {
                    state.write_navigation = ContextNavigationTarget::parse(&event.url)
                        .ok()
                        .filter(|target| target.as_url().as_str() == event.url)
                        .map(|target| (event.id, state.finalization_generation, target));
                }
                Ok((false, false))
            }
            // A later request of the page's own replaced the load the gate
            // was following before it committed.
            (Phase::Loading, E::Started)
                if replaced
                    && exact
                    && state.page_loads_unstarted > 0
                    && state.native_id.is_some_and(|id| id != event.id) =>
            {
                state.superseded = state.native_id.replace(event.id);
                state.page_loads_unstarted = state.page_loads_unstarted.saturating_sub(1);
                Ok((false, false))
            }
            (Phase::Loading, E::Committed) if exact && state.native_id == Some(event.id) => {
                if state.policy == zephium_agentic::WorkBrowserDocumentPolicy::SiteSession {
                    log_brand_move(state.target.as_ref(), &event.url);
                }
                #[cfg(feature = "native-agentic-work-resource-probe")]
                {
                    state.evidence.committed = true;
                }
                state.phase = Phase::Committed;
                state.committed_at = Some(Instant::now());
                state.document = ContextNavigationTarget::parse(&event.url).ok();
                // The host schedules the wait for a load that never finishes.
                Ok((
                    true,
                    state.policy == zephium_agentic::WorkBrowserDocumentPolicy::SiteSession,
                ))
            }
            (Phase::Committed, E::Finished) if exact && state.native_id == Some(event.id) => {
                #[cfg(all(target_os = "macos", feature = "native-agentic-semantic-probe"))]
                if super::form_qualification::selection().is_some() {
                    state.form_audit.initial_finished = true;
                }
                #[cfg(feature = "native-agentic-work-resource-probe")]
                {
                    state.evidence.finished = true;
                }
                state.phase = if state.policy == zephium_agentic::WorkBrowserDocumentPolicy::Exact {
                    state.effective = state.target.clone();
                    if let Some(policy) = state.back_policy.take() {
                        state.policy = policy;
                    }
                    Phase::Ready
                } else {
                    Phase::Finalizing
                };
                Ok((false, true))
            }
            _ => {
                state.clear_write_navigation();
                state.phase = Phase::Refused;
                Ok((false, true))
            }
        }
    }
    /// Initial-load KVO does not establish readiness. Once exact native
    /// navigation and URL sampling have sealed the document, a delayed or
    /// duplicate URL notification must match the sealed URL. An explicit public
    /// interaction policy also admits safe query updates in that same document.
    /// Missing, oversized or noncanonical values always revoke authority.
    pub(crate) fn location_changed(&self, current: Option<&str>) -> Result<bool, ()> {
        let mut state = self.0.lock().map_err(|_| ())?;
        if state.phase == Phase::Human {
            return match state.human.as_mut().ok_or(())?.location_changed(current) {
                Ok(changed) => Ok(changed),
                Err(()) => {
                    state.phase = Phase::Refused;
                    Ok(true)
                }
            };
        }
        #[cfg(feature = "native-agentic-work-resource-probe")]
        if state.phase == Phase::Committed {
            // This is now a classified URL notification rather than erased
            // back/forward availability. It remains evidence only: readiness
            // still requires the native navigation terminal and URL sample.
            state.evidence.location_callback_after_commit_before_ready = true;
        }
        if state.phase == Phase::Ready {
            if current.is_some_and(|current| {
                state
                    .effective
                    .as_ref()
                    .is_some_and(|effective| effective.as_url().as_str() == current)
            }) {
                return Ok(false);
            }
            // Ready retains the exact committed native ID. Every unarmed load
            // is still refused by the navigation delegate and event state machine.
            if matches!(
                state.policy,
                zephium_agentic::WorkBrowserDocumentPolicy::PublicSameDocumentQuery
                    | zephium_agentic::WorkBrowserDocumentPolicy::PublicResultsPage
                    | zephium_agentic::WorkBrowserDocumentPolicy::SiteSession
            ) && state.native_id.is_some()
                // A site page that arrived routes itself before the host has
                // taken its arrival; the arrival is still that document.
                && (state.operation.is_none()
                    || state.policy == zephium_agentic::WorkBrowserDocumentPolicy::SiteSession)
            {
                let observed = current.and_then(|raw| {
                    ContextNavigationTarget::parse(raw)
                        .ok()
                        .filter(|target| target.as_url().as_str() == raw)
                });
                if let Some(observed) = observed.filter(|observed| {
                    state.target.as_ref().is_some_and(|requested| {
                        state.policy.admits_final_document(requested, observed)
                    })
                }) {
                    let Some(revision) = state.location_revision.checked_add(1) else {
                        state.phase = Phase::Refused;
                        return Err(());
                    };
                    state.location_revision = revision;
                    state.effective = Some(observed);
                    return Ok(false);
                }
            }
            #[cfg(feature = "native-agentic-work-lifetime-diagnostic")]
            {
                state.url_observation_failure = Some(crate::WorkUrlObservationFailure::compare(
                    state.effective.as_ref(),
                    current,
                ));
            }
            state.phase = Phase::Refused;
            return Ok(true);
        }
        if matches!(
            state.phase,
            Phase::Committed | Phase::Finalizing | Phase::Sampling
        ) && state.policy != zephium_agentic::WorkBrowserDocumentPolicy::Exact
        {
            let observed = current.and_then(|raw| {
                ContextNavigationTarget::parse(raw)
                    .ok()
                    .filter(|target| target.as_url().as_str() == raw)
            });
            if !observed
                .as_ref()
                .zip(state.target.as_ref())
                .is_some_and(|(observed, requested)| {
                    state.policy.admits_final_document(requested, observed)
                })
            {
                #[cfg(feature = "native-agentic-work-lifetime-diagnostic")]
                {
                    state.url_observation_failure = Some(
                        crate::WorkUrlObservationFailure::compare(state.target.as_ref(), current),
                    );
                }
                state.phase = Phase::Refused;
                return Ok(true);
            }
        }
        if matches!(state.phase, Phase::Finalizing | Phase::Sampling) {
            let Some(revision) = state.location_revision.checked_add(1) else {
                state.phase = Phase::Refused;
                return Err(());
            };
            state.location_revision = revision;
        }
        Ok(false)
    }
    /// Back/forward availability is browser-chrome state, not document
    /// authority. Work deliberately ignores it in every phase, including the
    /// revision-fenced finalization window.
    #[cfg(any(target_os = "macos", test))]
    pub(crate) const fn history_availability_changed(&self) -> bool {
        false
    }
    pub(crate) fn finalization_pending(&self) -> bool {
        self.0
            .lock()
            .is_ok_and(|state| state.phase == Phase::Finalizing)
    }
    /// Captures the current URL-observation revision before a host-owned quiet
    /// period. A later KVO notification invalidates only this ticket; it does
    /// not consume the operation's sole authoritative native URL sample.
    pub(crate) fn finalization_ticket(&self) -> Option<WorkDocumentFinalizationTicket> {
        let state = self.0.lock().ok()?;
        (state.phase == Phase::Finalizing).then(|| WorkDocumentFinalizationTicket {
            gate: Arc::downgrade(&self.0),
            generation: state.finalization_generation,
            location_revision: state.location_revision,
        })
    }
    /// One native sample outside the lock. A callback racing the getter closes
    /// this attempt. A stale pre-sample quiet-period ticket consumes no sample
    /// and asks the host to establish a fresh bounded quiet period.
    pub(crate) fn finalize_after_quiet_period(
        &self,
        ticket: WorkDocumentFinalizationTicket,
        sample: impl FnOnce() -> Option<String>,
    ) -> Result<Option<ContextNavigationTarget>, ()> {
        let revision = {
            let mut state = self.0.lock().map_err(|_| ())?;
            if state.phase != Phase::Finalizing {
                return Err(());
            }
            if !Weak::ptr_eq(&ticket.gate, &Arc::downgrade(&self.0))
                || state.finalization_generation != ticket.generation
                || state.location_revision != ticket.location_revision
            {
                return Ok(None);
            }
            state.phase = Phase::Sampling;
            state.location_revision
        };
        let sampled = sample();
        let mut state = self.0.lock().map_err(|_| ())?;
        let effective = sampled.as_deref().and_then(|raw| {
            ContextNavigationTarget::parse(raw)
                .ok()
                .filter(|target| target.as_url().as_str() == raw)
        });
        if state.phase != Phase::Sampling
            || state.location_revision != revision
            || !effective.as_ref().zip(state.target.as_ref()).is_some_and(
                |(effective, requested)| state.policy.admits_final_document(requested, effective),
            )
        {
            state.phase = Phase::Refused;
            return Err(());
        }
        let effective = effective.ok_or(())?;
        state.effective = Some(effective.clone());
        state.phase = Phase::Ready;
        Ok(Some(effective))
    }
    #[cfg(test)]
    fn finalize(
        &self,
        sample: impl FnOnce() -> Option<String>,
    ) -> Result<ContextNavigationTarget, ()> {
        let ticket = self.finalization_ticket().ok_or(())?;
        self.finalize_after_quiet_period(ticket, sample)?.ok_or(())
    }
    /// A site-session page that committed its document but never reported
    /// its load finished (a page that keeps loading in a hidden view) is
    /// taken as loaded once its navigation's time is up; the ordinary quiet
    /// period and location sample still decide when it is ready.
    pub(crate) fn accept_committed_load(&self) -> bool {
        let Ok(mut state) = self.0.lock() else {
            return false;
        };
        if state.policy == zephium_agentic::WorkBrowserDocumentPolicy::SiteSession
            && state.phase == Phase::Committed
            && state.native_id.is_some()
        {
            state.phase = Phase::Finalizing;
            return true;
        }
        false
    }
    pub(crate) fn refuse(&self) {
        if let Ok(mut state) = self.0.lock() {
            state.clear_write_navigation();
            state.phase = Phase::Refused;
        }
    }
    /// The page is still on its document. An app may have moved to a
    /// fragment of it (Gmail's `#inbox`); that loads nothing.
    pub(crate) fn ready(&self, current: Option<&str>) -> bool {
        self.0.lock().is_ok_and(|state| {
            state.phase == Phase::Ready
                && state.effective.as_ref().is_some_and(|target| {
                    Some(target.as_url().as_str()) == current
                        || current
                            .and_then(|current| ContextNavigationTarget::parse(current).ok())
                            .is_some_and(|current| {
                                target.as_url().fragment().is_none()
                                    && *target.as_url() == human::without_any_fragment(&current)
                            })
                })
        })
    }
    /// A document load is under way or unsettled; the page is not ready to read.
    pub(crate) fn loading(&self) -> bool {
        self.0
            .lock()
            .map_or(true, |state| state.phase != Phase::Ready)
    }
    /// A refused load that an admitted site-session navigation still owns:
    /// that navigation decides how the page goes on, not the event that
    /// refused it.
    pub(crate) fn owned_refusal(&self) -> bool {
        self.0.lock().is_ok_and(|state| {
            state.phase == Phase::Refused
                && state.operation.is_some()
                && state.policy == zephium_agentic::WorkBrowserDocumentPolicy::SiteSession
        })
    }
    pub(crate) fn failed(&self) -> bool {
        self.0.lock().map_or(true, |state| {
            matches!(state.phase, Phase::Refused | Phase::Retired)
        })
    }
    #[cfg(feature = "native-agentic-work-lifetime-diagnostic")]
    pub(crate) fn url_observation_failure(&self) -> Option<crate::WorkUrlObservationFailure> {
        self.0
            .lock()
            .ok()
            .and_then(|state| state.url_observation_failure)
    }
    pub(crate) fn bootstrap_ready(&self) -> bool {
        self.0
            .lock()
            .is_ok_and(|state| state.phase == Phase::Bootstrap && state.bootstrap_finished)
    }
    pub(crate) fn retire(&self) -> bool {
        match self.0.lock() {
            Ok(mut state) => {
                state.clear_write_navigation();
                state.phase = Phase::Retired;
                state.human = None;
                true
            }
            Err(_) => false,
        }
    }
}

#[path = "work_human_navigation.rs"]
mod human;

#[cfg(test)]
mod tests {
    use super::*;
    use wry::NavigationEventPhase as E;
    pub(super) const URL: &str = "https://example.test/frozen";
    fn next_request() -> (ContextJoin, ContextNavigationRequest) {
        next_request_with_policy(zephium_agentic::WorkBrowserDocumentPolicy::Exact)
    }
    fn next_request_with_policy(
        policy: zephium_agentic::WorkBrowserDocumentPolicy,
    ) -> (ContextJoin, ContextNavigationRequest) {
        use zephium_agentic::*;
        let identity = ContextIdentity::new(
            ContextId::generate(),
            ContextRunId::generate(),
            zephium_core::ids::ProfileId::generate(),
            ContextKind::Owned,
        );
        let mut registry = ContextRegistry::new();
        registry
            .reserve(
                identity,
                ContextCapabilities::try_new(
                    ContextKind::Owned,
                    &[ContextCapability::Navigate, ContextCapability::Observe],
                )
                .unwrap(),
            )
            .unwrap();
        let construction = registry
            .begin_context(identity.id(), ContextOperationId::new(1).unwrap())
            .unwrap();
        registry
            .settle_construction(identity.id(), construction, ContextSettlement::Applied)
            .unwrap();
        let source = registry.join(identity.id()).unwrap();
        registry
            .acknowledge_observation(identity.id(), source)
            .unwrap();
        let operation = registry
            .begin_navigation(identity.id(), ContextOperationId::new(2).unwrap())
            .unwrap();
        (
            source,
            ContextNavigationRequest::try_new_with_document_policy(
                operation,
                ContextNavigationTarget::parse("https://example.test/next").unwrap(),
                policy,
            )
            .unwrap(),
        )
    }
    pub(super) fn ready_gate() -> WorkDocumentNavigation {
        let gate = armed();
        for phase in [E::Started, E::Committed, E::Finished] {
            gate.observe(event(1, phase, URL)).unwrap();
        }
        gate
    }
    #[test]
    fn hidden_approval_handback_keeps_native_document_but_changes_virtual_stamp() {
        let gate = ready_gate();
        let before = gate.document_stamp().unwrap();
        assert_eq!(gate.hand_back(URL).unwrap().as_url().as_str(), URL);
        let after = gate.document_stamp().unwrap();
        assert_eq!(before.native_id, after.native_id);
        assert_ne!(before.epoch, after.epoch);
    }

    #[test]
    fn preview_stamp_requires_committed_identity_without_authorizing_semantics() {
        let gate = armed();
        assert!(gate.preview_document_stamp().is_none());
        gate.observe(event(1, E::Started, URL)).unwrap();
        assert!(gate.preview_document_stamp().is_none());
        gate.observe(event(1, E::Committed, URL)).unwrap();
        let committed = gate.preview_document_stamp().unwrap();
        assert!(gate.document_stamp().is_none());
        assert!(!gate.ready(Some(URL)));
        gate.observe(event(1, E::Finished, URL)).unwrap();
        assert_eq!(gate.preview_document_stamp(), Some(committed));
        assert!(gate.document_stamp().is_some());
        let (source, request) = next_request();
        gate.arm_successor(source, &request).unwrap();
        assert!(gate.preview_document_stamp().is_none());
        let next = request.target().as_url().as_str();
        assert!(gate.allows(next));
        gate.observe(event(2, E::Started, next)).unwrap();
        gate.observe(event(2, E::Committed, next)).unwrap();
        assert_ne!(gate.preview_document_stamp(), Some(committed));
        gate.retire();
        assert!(gate.preview_document_stamp().is_none());
    }

    #[test]
    fn preview_stamp_rejects_same_document_query_race() {
        let gate =
            armed_policy(zephium_agentic::WorkBrowserDocumentPolicy::PublicSameDocumentQuery);
        for phase in [E::Started, E::Committed, E::Finished] {
            gate.observe(event(1, phase, URL)).unwrap();
        }
        // Public query policy finalizes separately before becoming Ready.
        if gate.finalization_pending() {
            gate.finalize(|| Some(URL.into())).unwrap();
        }
        let prior = gate.preview_document_stamp().unwrap();
        assert_eq!(
            gate.location_changed(Some("https://example.test/frozen?q=changed")),
            Ok(false)
        );
        assert_ne!(gate.preview_document_stamp(), Some(prior));
    }
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    pub(super) fn apple_action(
        navigation_type: wry::AppleNavigationType,
        is_get: bool,
    ) -> wry::AppleNavigationAction {
        wry::AppleNavigationAction {
            navigation_type,
            is_get,
            target_is_main_frame: Some(true),
            source: None,
        }
    }
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    #[test]
    fn apple_admission_stays_closed_until_dispatch_and_binds_exact_native_cause() {
        use wry::AppleNavigationType as T;

        let gate = ready_gate();
        let (source, request) = next_request();
        let target = request.target().as_url().as_str();
        assert!(
            !gate.allows_apple_action(target, apple_action(T::BackForward, true)),
            "page-driven history during PARK cannot consume authority"
        );
        assert!(
            !gate.allows_apple_action(target, apple_action(T::FormSubmitted, false)),
            "a same-URL POST during PARK cannot consume authority"
        );
        gate.arm_history_back(source, request.operation(), request.target().clone())
            .unwrap();
        for (kind, is_get) in [
            (T::LinkActivated, true),
            (T::FormSubmitted, false),
            (T::Reload, true),
            (T::FormResubmitted, false),
            (T::Other, true),
        ] {
            assert!(!gate.allows_apple_action(target, apple_action(kind, is_get)));
        }
        assert!(!gate.allows_apple_action(target, apple_action(T::BackForward, false)));
        let mut child = apple_action(T::BackForward, true);
        child.target_is_main_frame = Some(false);
        assert!(
            gate.allows_apple_action("https://embed.example/widget", child),
            "a frame inside the ready document is page content"
        );
        assert!(gate.allows_apple_action(target, apple_action(T::BackForward, true)));
        assert!(!gate.allows_apple_action(target, apple_action(T::BackForward, true)));

        let gate = ready_gate();
        gate.arm_successor(source, &request).unwrap();
        for (kind, is_get) in [
            (T::LinkActivated, true),
            (T::FormSubmitted, false),
            (T::BackForward, true),
            (T::Reload, true),
            (T::FormResubmitted, false),
        ] {
            assert!(!gate.allows_apple_action(target, apple_action(kind, is_get)));
        }
        assert!(gate.allows_apple_action(target, apple_action(T::Other, true)));

        #[cfg(target_os = "macos")]
        {
            let gate = ready_gate();
            gate.arm_history_reload(source, request.operation(), request.target().clone())
                .unwrap();
            assert!(!gate.allows_apple_action(target, apple_action(T::BackForward, true)));
            assert!(gate.allows_apple_action(target, apple_action(T::Other, true)));
            assert!(!gate.allows_apple_action(target, apple_action(T::Other, true)));
        }
    }
    #[test]
    fn a_site_page_back_reached_is_a_site_page_again_once_ready() {
        let gate = armed_policy(zephium_agentic::WorkBrowserDocumentPolicy::SiteSession);
        for phase in [E::Started, E::Committed, E::Finished] {
            gate.observe(event(1, phase, URL)).unwrap();
        }
        settle(&gate, URL);
        let (source, request) = next_request();
        let back = request.target().as_url().as_str();
        gate.arm_history_back(source, request.operation(), request.target().clone())
            .unwrap();
        assert!(gate.allows(back));
        for phase in [E::Started, E::Committed, E::Finished] {
            gate.observe(event(2, phase, back)).unwrap();
        }
        assert_eq!(
            gate.location_changed(Some("https://example.test/next?sort=price")),
            Ok(false)
        );
        assert!(gate.ready(Some("https://example.test/next?sort=price")));
        assert!(!gate.failed());
    }
    #[test]
    fn a_site_load_that_fails_goes_back_to_the_page_it_left() {
        let gate = armed_policy(zephium_agentic::WorkBrowserDocumentPolicy::SiteSession);
        for phase in [E::Started, E::Committed, E::Finished] {
            gate.observe(event(1, phase, URL)).unwrap();
        }
        settle(&gate, URL);
        let departure = gate.departure().unwrap();
        assert_eq!(departure.as_url().as_str(), URL);
        let (source, request) =
            next_request_with_policy(zephium_agentic::WorkBrowserDocumentPolicy::SiteSession);
        gate.arm_successor(source, &request).unwrap();
        assert!(gate.departure().is_none());
        let next = request.target().as_url().as_str();
        assert!(gate.allows(next));
        gate.observe(event(2, E::Started, next)).unwrap();
        // The site sends the address to another site, which the session refuses.
        assert!(!gate.allows("https://accounts.other.test/login"));
        gate.observe(event(2, E::Failed, next)).unwrap();
        assert!(gate.failed());
        gate.arm_departure(source, request.operation(), departure.clone())
            .unwrap();
        assert!(gate.allows(URL));
        // The failed load's own late events are not the page going back.
        gate.observe(event(2, E::Cancelled, next)).unwrap();
        for phase in [E::Started, E::Committed, E::Finished] {
            gate.observe(event(3, phase, URL)).unwrap();
        }
        settle(&gate, URL);
        assert!(gate.ready(Some(URL)));
        assert!(!gate.failed());
        let (operation, outcome) = gate.take_successor_terminal().unwrap();
        assert_eq!(operation, request.operation());
        assert_eq!(outcome, Ok(departure));
    }
    #[test]
    fn the_document_a_model_load_committed_may_route_itself_before_it_settles() {
        let gate = armed_policy(zephium_agentic::WorkBrowserDocumentPolicy::SiteSession);
        for phase in [E::Started, E::Committed, E::Finished] {
            gate.observe(event(1, phase, URL)).unwrap();
        }
        settle(&gate, URL);
        let (source, request) =
            next_request_with_policy(zephium_agentic::WorkBrowserDocumentPolicy::SiteSession);
        gate.arm_successor(source, &request).unwrap();
        let next = request.target().as_url().as_str();
        let routed = "https://example.test/next#inbox";
        assert!(gate.allows(next));
        gate.observe(event(2, E::Started, next)).unwrap();
        gate.observe(event(2, E::Committed, next)).unwrap();
        assert!(gate.allows(routed));
        assert_eq!(gate.location_changed(Some(routed)), Ok(false));
        gate.observe(event(2, E::Finished, next)).unwrap();
        settle(&gate, routed);
        assert!(gate.ready(Some(routed)) && !gate.failed());
        let (operation, outcome) = gate.take_successor_terminal().unwrap();
        assert_eq!(operation, request.operation());
        assert_eq!(outcome.unwrap().as_url().as_str(), routed);
    }
    fn site_gate(url: &str) -> WorkDocumentNavigation {
        let gate = WorkDocumentNavigation::default();
        assert!(gate.allows("about:blank"));
        for phase in [E::Started, E::Committed, E::Finished] {
            gate.observe(event(9, phase, "about:blank")).unwrap();
        }
        gate.arm_with_policy(
            ContextNavigationTarget::parse(url).unwrap(),
            zephium_agentic::WorkBrowserDocumentPolicy::SiteSession,
        )
        .unwrap();
        assert!(gate.allows(url));
        gate
    }
    fn settle(gate: &WorkDocumentNavigation, current: &str) {
        let ticket = gate.finalization_ticket().unwrap();
        gate.finalize_after_quiet_period(ticket, || Some(current.to_owned()))
            .unwrap()
            .unwrap();
    }
    #[test]
    fn site_session_follows_same_site_redirects_and_script_loads_until_ready() {
        let start = "https://app.slack.com/client";
        let gate = site_gate(start);
        gate.observe(event(1, E::Started, start)).unwrap();
        assert!(!gate.allows("https://accounts.google.com/login"));
        assert!(gate.allows("https://slack.com/signin"));
        gate.observe(event(1, E::Redirected, start)).unwrap();
        gate.observe(event(1, E::Committed, "https://slack.com/signin"))
            .unwrap();
        // A script redirect before the document settles continues the load.
        assert!(gate.allows("https://app.slack.com/client/T1"));
        gate.observe(event(1, E::Finished, "https://slack.com/signin"))
            .unwrap();
        gate.observe(event(2, E::Started, "https://app.slack.com/client/T1"))
            .unwrap();
        gate.observe(event(2, E::Committed, "https://app.slack.com/client/T1"))
            .unwrap();
        gate.observe(event(2, E::Finished, "https://app.slack.com/client/T1"))
            .unwrap();
        settle(&gate, "https://app.slack.com/client/T1");
        assert!(gate.ready(Some("https://app.slack.com/client/T1")));
        assert!(!gate.failed());
    }
    #[test]
    fn a_first_page_that_loads_itself_again_soon_after_it_settled_is_followed() {
        let start = "https://www.example.com/";
        let gate = site_gate(start);
        for phase in [E::Started, E::Committed, E::Finished] {
            gate.observe(event(1, phase, start)).unwrap();
        }
        settle(&gate, start);
        assert!(gate.ready(Some(start)));
        // A bot check that cleared itself reloads the page.
        assert!(gate.allows("https://www.example.com/?checked=1"));
        assert!(gate.handed_on());
        for phase in [E::Started, E::Committed, E::Finished] {
            gate.observe(event(2, phase, "https://www.example.com/?checked=1"))
                .unwrap();
        }
        settle(&gate, "https://www.example.com/?checked=1");
        assert!(gate.ready(Some("https://www.example.com/?checked=1")));
        // Another site, or another page of it, is never followed.
        assert!(!gate.allows("https://elsewhere.example.org/"));
        assert!(!gate.allows("https://www.example.com/account"));
    }

    #[test]
    fn a_page_the_run_opened_that_reloads_itself_behind_a_bot_check_is_followed() {
        let start = "https://www.yelp.pl/";
        let gate = site_gate(start);
        for phase in [E::Started, E::Committed, E::Finished] {
            gate.observe(event(1, phase, start)).unwrap();
        }
        settle(&gate, start);
        // The run opened the search, which DataDome's device check covers
        // and reloads once it passes.
        let search = "https://www.yelp.pl/search?find_desc=pizza";
        {
            let mut state = gate.0.lock().unwrap();
            state.first_document = true;
            state.target = Some(ContextNavigationTarget::parse(search).unwrap());
            state.effective = state.target.clone();
            state.committed_at = Some(Instant::now());
        }
        assert!(gate.allows(search));
        for phase in [E::Started, E::Committed, E::Finished] {
            gate.observe(event(2, phase, search)).unwrap();
        }
        settle(&gate, search);
        assert!(gate.ready(Some(search)));
        // Once an action ran on the page, its own loads are the action's.
        gate.open_follow(
            false,
            false,
            zephium_agentic::SemanticActionFollow::default(),
        );
        gate.close_follow();
        assert!(!gate.allows(search));
    }

    #[test]
    fn a_page_that_asks_for_its_load_again_before_it_starts_keeps_loading() {
        let start = "https://allegro.pl/listing?string=gpu";
        let gate = site_gate(start);
        for phase in [E::Started, E::Committed, E::Finished] {
            gate.observe(event(1, phase, start)).unwrap();
        }
        settle(&gate, start);
        // A bot check reloads its page three times in a row; WebKit starts
        // only the last request.
        assert!(gate.allows(start));
        assert!(gate.allows(start));
        assert!(gate.allows(start));
        for phase in [E::Started, E::Committed, E::Finished] {
            gate.observe(event(2, phase, start)).unwrap();
        }
        settle(&gate, start);
        assert!(gate.ready(Some(start)));

        // When WebKit does start an earlier request, the later one replaces
        // it before it commits.
        let gate = site_gate(start);
        for phase in [E::Started, E::Committed, E::Finished] {
            gate.observe(event(1, phase, start)).unwrap();
        }
        settle(&gate, start);
        assert!(gate.allows(start));
        assert!(gate.allows(start));
        gate.observe(event(2, E::Started, start)).unwrap();
        gate.observe(event(3, E::Started, start)).unwrap();
        gate.observe(event(2, E::Cancelled, start)).unwrap();
        for phase in [E::Committed, E::Finished] {
            gate.observe(event(3, phase, start)).unwrap();
        }
        settle(&gate, start);
        assert!(gate.ready(Some(start)));
        assert!(!gate.failed());
        // Once the followed load started, the page's other loads are its own
        // again: another page of the site is not followed.
        assert!(!gate.allows("https://allegro.pl/konto"));
    }

    #[test]
    fn a_first_page_its_script_moves_to_the_brand_s_country_domain_is_followed() {
        let start = "https://www.yelp.com/";
        let gate = site_gate(start);
        for phase in [E::Started, E::Committed, E::Finished] {
            gate.observe(event(1, phase, start)).unwrap();
        }
        settle(&gate, start);
        assert!(gate.allows("https://www.yelp.pl/"));
        for phase in [E::Started, E::Committed, E::Finished] {
            gate.observe(event(2, phase, "https://www.yelp.pl/"))
                .unwrap();
        }
        settle(&gate, "https://www.yelp.pl/");
        assert!(gate.ready(Some("https://www.yelp.pl/")));
        assert!(!gate.allows("https://www.yelpish.pl/"));
    }

    #[test]
    fn a_site_load_that_reuses_a_superseded_load_id_is_still_followed() {
        let start = "https://www.example.com/";
        let gate = site_gate(start);
        gate.observe(event(1, E::Started, start)).unwrap();
        gate.observe(event(1, E::Committed, start)).unwrap();
        assert!(gate.allows("https://www.example.com/?reloaded=1"));
        // WebKit hands the next load the address the superseded one had.
        for phase in [E::Started, E::Committed, E::Finished] {
            gate.observe(event(1, phase, "https://www.example.com/?reloaded=1"))
                .unwrap();
        }
        settle(&gate, "https://www.example.com/?reloaded=1");
        assert!(gate.ready(Some("https://www.example.com/?reloaded=1")));
        assert!(!gate.failed());
    }
    fn public_gate(
        url: &str,
        policy: zephium_agentic::WorkBrowserDocumentPolicy,
    ) -> WorkDocumentNavigation {
        let gate = WorkDocumentNavigation::default();
        assert!(gate.allows("about:blank"));
        for phase in [E::Started, E::Committed, E::Finished] {
            gate.observe(event(9, phase, "about:blank")).unwrap();
        }
        gate.arm_with_policy(ContextNavigationTarget::parse(url).unwrap(), policy)
            .unwrap();
        assert!(gate.allows(url));
        gate
    }
    #[test]
    fn a_public_first_load_redirect_is_kept_and_refused_without_an_invariant() {
        use zephium_agentic::WorkBrowserDocumentPolicy as P;
        let start = "https://hetzner.com/cloud";
        for (policy, to) in [
            (P::PublicSameDocumentQuery, "https://www.hetzner.com/cloud"),
            (P::PublicQueryFinalization, "https://www.hetzner.com/cloud"),
            // A cross-site destination is kept too; its owner decides.
            (
                P::PublicSameDocumentQuery,
                "https://www.other-site.com/cloud?x=1",
            ),
        ] {
            let gate = public_gate(start, policy);
            gate.observe(event(1, E::Started, start)).unwrap();
            assert_eq!(gate.redirected(), None);
            // WebKit and WebView2 ask policy again for the server redirect.
            assert!(!gate.allows(to));
            assert!(gate.failed());
            let to = ContextNavigationTarget::parse(to).unwrap();
            assert_eq!(gate.take_redirect_report(), Some(to.clone()));
            assert_eq!(gate.take_redirect_report(), None);
            // The cancelled provisional load changes nothing after that.
            assert_eq!(gate.observe(event(1, E::Failed, start)), Ok((false, false)));
            assert_eq!(gate.redirected(), Some(to));
            assert!(!gate.allows(start));
        }
    }
    #[test]
    fn a_public_first_document_can_finish_its_exact_address_self_reload() {
        use zephium_agentic::WorkBrowserDocumentPolicy as P;
        let start = "https://www.example.com/reference";
        for ready in [false, true] {
            let gate = public_gate(start, P::PublicSameDocumentQuery);
            for phase in [E::Started, E::Committed, E::Finished] {
                gate.observe(event(1, phase, start)).unwrap();
            }
            if ready {
                settle(&gate, start);
            }
            assert!(
                gate.allows(start),
                "a first-page verification reload must not strand readiness"
            );
            assert!(gate.take_hand_on());
            assert_eq!(gate.observe(event(1, E::Failed, start)), Ok((false, false)));
            assert!(!gate.failed());
            for phase in [E::Started, E::Committed, E::Finished] {
                gate.observe(event(2, phase, start)).unwrap();
            }
            settle(&gate, start);
            assert!(gate.ready(Some(start)));
        }
    }

    #[test]
    fn public_self_reload_does_not_authorize_new_destinations_or_post_action_replays() {
        use zephium_agentic::WorkBrowserDocumentPolicy as P;
        let start = "https://www.example.com/reference";
        let ready = || {
            let gate = public_gate(start, P::PublicSameDocumentQuery);
            for phase in [E::Started, E::Committed, E::Finished] {
                gate.observe(event(1, phase, start)).unwrap();
            }
            settle(&gate, start);
            gate
        };
        for target in [
            "https://www.example.com/reference?changed=1",
            "https://www.example.com/other",
            "https://elsewhere.example/reference",
            "https://www.example.com/reference#fragment",
        ] {
            let gate = ready();
            assert!(!gate.allows(target));
            assert!(gate.ready(Some(start)));
        }
        for limit in 0..3 {
            let gate = ready();
            {
                let mut state = gate.0.lock().unwrap();
                match limit {
                    0 => state.first_document = false,
                    1 => state.site_loads = MAX_SITE_LOADS,
                    _ => {
                        state.committed_at = Some(
                            Instant::now() - SELF_LOAD_WINDOW - std::time::Duration::from_secs(1),
                        )
                    }
                }
            }
            assert!(!gate.allows(start));
            assert!(gate.ready(Some(start)));
        }
        let gate = ready();
        gate.retire();
        assert!(!gate.allows(start));
    }

    #[cfg(any(target_os = "macos", target_os = "ios"))]
    #[test]
    fn apple_public_self_reload_requires_a_main_frame_get() {
        use wry::AppleNavigationType as T;
        use zephium_agentic::WorkBrowserDocumentPolicy as P;
        let start = "https://www.example.com/reference";
        for (kind, get, expected) in [
            (T::Reload, true, true),
            (T::Other, true, true),
            (T::FormSubmitted, true, false),
            (T::FormSubmitted, false, false),
            (T::LinkActivated, true, false),
            (T::BackForward, true, false),
            (T::FormResubmitted, false, false),
        ] {
            let gate = public_gate(start, P::PublicSameDocumentQuery);
            for phase in [E::Started, E::Committed, E::Finished] {
                gate.observe(event(1, phase, start)).unwrap();
            }
            settle(&gate, start);
            assert_eq!(
                gate.allows_apple_action(start, apple_action(kind, get)),
                expected
            );
        }
    }

    #[test]
    fn a_public_script_redirect_before_ready_is_kept_but_never_after() {
        use zephium_agentic::WorkBrowserDocumentPolicy as P;
        let start = "https://www.example.com/plans";
        let to = "https://www.example.com/plans/";
        let gate = public_gate(start, P::PublicSameDocumentQuery);
        gate.observe(event(1, E::Started, start)).unwrap();
        gate.observe(event(1, E::Committed, start)).unwrap();
        assert!(!gate.allows(to));
        assert_eq!(gate.redirected(), ContextNavigationTarget::parse(to).ok());
        // Once ready, a page-initiated load is refused and the page stays.
        let gate = public_gate(start, P::PublicSameDocumentQuery);
        for phase in [E::Started, E::Committed, E::Finished] {
            gate.observe(event(1, phase, start)).unwrap();
        }
        settle(&gate, start);
        assert!(!gate.allows(to));
        assert!(!gate.failed() && gate.ready(Some(start)));
        assert_eq!(gate.redirected(), None);
        assert_eq!(gate.take_redirect_report(), None);
    }
    #[test]
    fn an_unsafe_or_non_public_refusal_keeps_today_s_failure() {
        use zephium_agentic::WorkBrowserDocumentPolicy as P;
        let start = "https://www.example.com/plans";
        for to in [
            "http://www.example.com/plans",
            "https://127.0.0.1/plans",
            "https://intranet.local/plans",
            "https://www.example.com/plans?access_token=private-value",
            "https://www.example.com/plans#section",
            start,
        ] {
            let gate = public_gate(start, P::PublicSameDocumentQuery);
            gate.observe(event(1, E::Started, start)).unwrap();
            assert!(!gate.allows(to), "{to}");
            assert!(!gate.failed(), "{to}");
            assert_eq!(gate.observe(event(1, E::Failed, start)), Ok((false, true)));
            assert!(gate.failed() && gate.redirected().is_none(), "{to}");
        }
        // Exact and site-session documents never keep a redirect.
        for policy in [P::Exact, P::SiteSession] {
            let gate = public_gate(start, policy);
            gate.observe(event(1, E::Started, start)).unwrap();
            assert!(!gate.allows("https://www.other-site.com/"));
            assert_eq!(gate.redirected(), None);
        }
    }
    #[test]
    fn a_results_page_settles_on_same_site_redirects_up_to_its_bound() {
        use zephium_agentic::WorkBrowserDocumentPolicy as P;
        let start = "https://www.google.com/search?q=rust";
        let gate = public_gate(start, P::PublicResultsPage);
        gate.observe(event(1, E::Started, start)).unwrap();
        assert!(gate.allows("https://google.com/search?q=rust"));
        gate.observe(event(1, E::Redirected, start)).unwrap();
        gate.observe(event(1, E::Committed, "https://google.com/search?q=rust"))
            .unwrap();
        let mut id = 1;
        for hop in 0..MAX_SITE_LOADS {
            let next = format!("https://www.google.com/search?q=rust&sei={hop}");
            assert!(gate.allows(&next), "{hop}");
            id += 1;
            gate.observe(event(id, E::Started, &next)).unwrap();
            gate.observe(event(id, E::Committed, &next)).unwrap();
        }
        let last = format!(
            "https://www.google.com/search?q=rust&sei={}",
            MAX_SITE_LOADS - 1
        );
        // Past the bound, a load is a redirect the gate refuses and keeps.
        assert!(!gate.allows("https://www.google.com/search?q=rust&sei=again"));
        assert!(gate.failed());
        assert!(gate.redirected().is_some());
        let gate = public_gate(start, P::PublicResultsPage);
        gate.observe(event(1, E::Started, start)).unwrap();
        for unsafe_load in [
            "https://www.bing.com/search?q=rust",
            "https://www.google.com/search?q=rust#x",
            "http://www.google.com/search?q=rust",
        ] {
            assert!(!gate.allows(unsafe_load), "{unsafe_load}");
        }
        let gate = public_gate(start, P::PublicResultsPage);
        gate.observe(event(1, E::Started, start)).unwrap();
        gate.observe(event(1, E::Committed, start)).unwrap();
        assert!(gate.allows(&last));
        gate.observe(event(1, E::Finished, start)).unwrap();
        gate.observe(event(2, E::Started, &last)).unwrap();
        gate.observe(event(2, E::Committed, &last)).unwrap();
        gate.observe(event(2, E::Finished, &last)).unwrap();
        settle(&gate, &last);
        assert!(gate.ready(Some(&last)) && !gate.failed());
        // A ready results page keeps its document.
        assert!(!gate.allows(start));
        assert!(gate.ready(Some(&last)));
    }
    #[test]
    fn site_session_routes_in_place_and_cancels_page_loads_without_losing_the_page() {
        let start = "https://app.slack.com/client";
        let gate = site_gate(start);
        for phase in [E::Started, E::Committed, E::Finished] {
            gate.observe(event(1, phase, start)).unwrap();
        }
        settle(&gate, start);
        let route = "https://app.slack.com/client/T1/C2";
        assert_eq!(gate.location_changed(Some(route)), Ok(false));
        assert!(gate.ready(Some(route)));
        for foreign in [
            "https://evil.test/",
            "https://app.slack.com/other",
            "https://files.slack.com/x",
        ] {
            assert!(!gate.allows(foreign), "{foreign}");
        }
        gate.observe(event(7, E::Cancelled, "https://evil.test/"))
            .unwrap();
        assert!(gate.ready(Some(route)));
        assert!(!gate.failed());
        assert_eq!(gate.location_changed(Some("https://evil.test/x")), Ok(true));
        assert!(gate.failed());
    }
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    #[test]
    fn site_session_never_follows_posts_or_cross_site_redirects() {
        use wry::AppleNavigationType as T;
        let start = "https://app.slack.com/client";
        let gate = site_gate(start);
        gate.observe(event(1, E::Started, start)).unwrap();
        assert!(!gate.allows_apple_action(
            "https://app.slack.com/login",
            apple_action(T::FormSubmitted, false)
        ));
        assert!(
            !gate.allows_apple_action("https://accounts.google.com/", apple_action(T::Other, true))
        );
        assert!(gate.allows_apple_action("https://slack.com/signin", apple_action(T::Other, true)));
        let mut popup = apple_action(T::LinkActivated, true);
        popup.target_is_main_frame = None;
        assert!(!gate.allows_apple_action("https://app.slack.com/x", popup));
    }
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    #[test]
    fn an_action_diverts_same_site_gets_and_admits_one_post_only_for_a_commit() {
        use wry::AppleNavigationType as T;
        let start = "https://app.slack.com/client";
        let ready = || {
            let gate = site_gate(start);
            for phase in [E::Started, E::Committed, E::Finished] {
                gate.observe(event(1, phase, start)).unwrap();
            }
            settle(&gate, start);
            gate
        };
        let gate = ready();
        let slot = zephium_agentic::SemanticActionFollow::default();
        let search = "https://app.slack.com/search?q=design";
        // Without an action in flight nothing is kept.
        assert!(!gate.allows_apple_action(search, apple_action(T::Other, true)));
        gate.open_follow(false, false, slot.clone());
        assert!(!gate.allows_apple_action(search, apple_action(T::FormSubmitted, true)));
        assert!(!gate.allows_apple_action(search, apple_action(T::FormSubmitted, false)));
        assert!(!gate.allows_apple_action("https://evil.test/", apple_action(T::Other, true)));
        assert!(gate.ready(Some(start)));
        assert_eq!(
            slot.take().map(|t| t.as_url().to_string()),
            Some(search.to_owned())
        );
        assert_eq!(slot.take(), None);
        gate.close_follow();
        // With no action in flight, the page's own same-site load is still
        // cancelled, and kept for the controller to follow; another site's
        // is not.
        assert!(!gate.allows_apple_action(search, apple_action(T::Other, true)));
        assert_eq!(
            slot.take().map(|t| t.as_url().to_string()),
            Some(search.to_owned())
        );
        assert!(!gate.allows_apple_action("https://evil.test/", apple_action(T::Other, true)));
        assert_eq!(slot.take(), None);

        let gate = ready();
        gate.open_follow(true, false, slot.clone());
        let book = "https://app.slack.com/book";
        assert!(!gate.allows_apple_action(
            "https://evil.test/pay",
            apple_action(T::FormSubmitted, false)
        ));
        assert!(gate.allows_apple_action(book, apple_action(T::FormSubmitted, false)));
        assert!(gate.posting());
        assert!(!gate.allows_apple_action(book, apple_action(T::FormSubmitted, false)));
        for phase in [E::Started, E::Committed, E::Finished] {
            gate.observe(event(2, phase, book)).unwrap();
        }
        settle(&gate, book);
        assert!(!gate.posting());
        assert!(gate.ready(Some(book)));
        assert!(!gate.failed());
    }
    #[test]
    fn a_committed_site_page_that_never_finishes_is_taken_as_loaded_once() {
        let start = "https://www.airbnb.com/rooms/1";
        let gate = site_gate(start);
        assert!(!gate.accept_committed_load());
        gate.observe(event(1, E::Started, start)).unwrap();
        assert!(!gate.accept_committed_load());
        gate.observe(event(1, E::Committed, start)).unwrap();
        assert!(gate.accept_committed_load());
        assert!(gate.finalization_pending());
        assert!(!gate.accept_committed_load());
        settle(&gate, start);
        assert!(gate.ready(Some(start)) && !gate.failed());
    }
    #[test]
    fn a_route_of_the_committed_document_is_no_new_load_and_a_load_that_never_ends_still_settles() {
        let start = "https://mail.example.test/mail/u/0/";
        let routed = "https://mail.example.test/mail/u/0/#inbox";
        let gate = site_gate(start);
        gate.observe(event(1, E::Started, start)).unwrap();
        assert!(gate.committed_unfinished_since().is_none());
        gate.observe(event(1, E::Committed, start)).unwrap();
        assert!(gate.committed_unfinished_since().is_some());
        assert!(gate.allows(routed));
        assert!(!gate.failed());
        assert_eq!(gate.location_changed(Some(routed)), Ok(false));
        assert!(gate.accept_committed_load());
        settle(&gate, routed);
        assert!(gate.ready(Some(routed)) && !gate.failed());
        assert!(gate.allows("https://mail.example.test/mail/u/0/#sent"));
        assert!(!gate.allows("https://mail.example.test/mail/u/1/"));
    }
    #[test]
    fn a_route_that_holds_state_is_still_no_new_load() {
        let start = "https://mail.example.test/mail/u/0/";
        let gate = site_gate(start);
        for phase in [E::Started, E::Committed, E::Finished] {
            gate.observe(event(1, phase, start)).unwrap();
        }
        settle(&gate, start);
        let compose = "https://mail.example.test/mail/u/0/#inbox?compose=new";
        assert!(gate.allows(compose));
        assert_eq!(gate.location_changed(Some(compose)), Ok(false));
        assert!(gate.ready(Some(compose)) && !gate.failed());
        assert!(gate.allows("https://mail.example.test/mail/u/0/#inbox"));
        assert!(!gate.allows("https://mail.example.test/mail/u/1/#inbox?compose=new"));
        // Handed back after a person's decision, the page resumes as a
        // document: the app's state in the fragment is not a request.
        assert_eq!(gate.hand_back(compose).unwrap().as_url().as_str(), start);
    }
    #[cfg(target_os = "windows")]
    #[test]
    fn windows_missing_destination_requires_exact_loading_action_navigation_once() {
        let start = "http://127.0.0.1:4242/gconsent";
        let save = "http://127.0.0.1:4242/consent/save";
        for consent in [false, true] {
            let gate = site_gate(start);
            for phase in [E::Started, E::Committed, E::Finished] {
                gate.observe(event(1, phase, start)).unwrap();
            }
            settle(&gate, start);
            gate.open_follow(
                !consent,
                consent,
                zephium_agentic::SemanticActionFollow::default(),
            );
            assert!(!gate.allows_windows_request(save, "POST", None));
            assert!(gate.allows(save));
            assert!(!gate.allows_windows_request(save, "POST", None));
            gate.observe(event(2, E::Started, save)).unwrap();
            assert!(!gate.allows_windows_request("http://127.0.0.1:4242/unrelated", "POST", None));
            assert!(!gate.allows_windows_request(
                "http://127.0.0.1:4243/consent/save",
                "POST",
                None
            ));
            assert!(gate.allows_windows_request(save, "POST", None));
            assert!(!gate.allows_windows_request(save, "POST", None));
            assert!(!gate.allows_windows_request(save, "POST", Some(true)));
            for phase in [E::Committed, E::Finished] {
                gate.observe(event(2, phase, save)).unwrap();
            }
            settle(&gate, save);
            assert!(!gate.allows_windows_request(save, "POST", None));
        }
    }
    #[cfg(target_os = "windows")]
    #[test]
    fn windows_missing_destination_rejects_stale_cancelled_or_unconfirmed_join() {
        let start = "https://example.test/consent";
        let save = "https://example.test/save";
        let ready = || {
            let gate = site_gate(start);
            for phase in [E::Started, E::Committed, E::Finished] {
                gate.observe(event(1, phase, start)).unwrap();
            }
            settle(&gate, start);
            gate
        };
        let gate = ready();
        gate.open_follow(
            false,
            false,
            zephium_agentic::SemanticActionFollow::default(),
        );
        assert!(!gate.allows(save));
        assert!(!gate.allows_windows_request(save, "POST", None));
        for terminal in [E::Cancelled, E::Committed, E::Finished] {
            let gate = ready();
            gate.open_follow(
                false,
                true,
                zephium_agentic::SemanticActionFollow::default(),
            );
            assert!(gate.allows(save));
            gate.observe(event(2, E::Started, save)).unwrap();
            gate.observe(event(2, terminal, save)).unwrap();
            assert!(!gate.allows_windows_request(save, "POST", None));
            assert!(gate.0.lock().unwrap().write_navigation.is_none());
        }
        let gate = ready();
        gate.open_follow(
            false,
            true,
            zephium_agentic::SemanticActionFollow::default(),
        );
        assert!(gate.allows(save));
        gate.observe(event(2, E::Started, save)).unwrap();
        gate.0.lock().unwrap().finalization_generation += 1;
        assert!(!gate.allows_windows_request(save, "POST", None));
        gate.retire();
        assert!(!gate.allows_windows_request(save, "POST", None));
        assert!(gate.0.lock().unwrap().write_navigation.is_none());
    }
    #[cfg(target_os = "windows")]
    #[test]
    fn windows_method_fence_only_admits_one_confirmed_main_document_write() {
        let start = "https://www.google.com/travel/flights";
        let gate = site_gate(start);
        for phase in [E::Started, E::Committed, E::Finished] {
            gate.observe(event(1, phase, start)).unwrap();
        }
        settle(&gate, start);
        let write = "https://www.google.com/save";
        assert!(gate.allows_windows_request(write, "POST", Some(false)));
        assert!(gate.allows_windows_request(write, "GET", Some(true)));
        assert!(!gate.allows_windows_request(write, "POST", Some(true)));
        assert!(!gate.allows_windows_request(write, "POST", None));
        gate.open_follow(
            true,
            false,
            zephium_agentic::SemanticActionFollow::default(),
        );
        assert!(!gate.allows_windows_request("https://evil.test/save", "POST", Some(true)));
        assert!(!gate.allows_windows_request(write, "POST", None));
        assert!(gate.allows_windows_request(write, "POST", Some(true)));
        assert!(!gate.allows_windows_request(write, "POST", Some(true)));
        assert!(gate.allows_windows_request(write, "POST", Some(false)));
    }
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    #[test]
    fn a_consent_host_saves_the_choice_with_one_post_on_any_action() {
        use wry::AppleNavigationType as T;
        let start = "https://www.google.com/travel/flights";
        let consent = "https://consent.google.com/m?hl=en";
        let gate = site_gate(start);
        gate.observe(event(1, E::Started, start)).unwrap();
        gate.observe(event(1, E::Redirected, start)).unwrap();
        gate.observe(event(1, E::Committed, consent)).unwrap();
        gate.observe(event(1, E::Finished, consent)).unwrap();
        settle(&gate, consent);
        let slot = zephium_agentic::SemanticActionFollow::default();
        gate.open_follow(false, false, slot);
        let save = "https://consent.google.com/save";
        assert!(!gate.allows_apple_action(
            "https://evil.test/save",
            apple_action(T::FormSubmitted, false)
        ));
        assert!(gate.allows_apple_action(save, apple_action(T::FormSubmitted, false)));
        assert!(!gate.allows_apple_action(save, apple_action(T::FormSubmitted, false)));
        // Replaced before it starts by a same-site GET, never another site.
        assert!(
            !gate.allows_apple_action("https://evil.test/", apple_action(T::FormSubmitted, true))
        );
        assert!(gate.allows_apple_action(start, apple_action(T::FormSubmitted, true)));
        for phase in [E::Started, E::Committed, E::Finished] {
            gate.observe(event(2, phase, save)).unwrap();
        }
        settle(&gate, save);
        // Its own hand-on back to the site follows as a redirect.
        assert!(gate.allows_apple_action(start, apple_action(T::FormSubmitted, true)));
        for phase in [E::Started, E::Committed, E::Finished] {
            gate.observe(event(3, phase, start)).unwrap();
        }
        settle(&gate, start);
        assert!(gate.ready(Some(start)) && !gate.failed());

        // An ordinary host still posts only for a commit.
        let gate = site_gate(start);
        for phase in [E::Started, E::Committed, E::Finished] {
            gate.observe(event(1, phase, start)).unwrap();
        }
        settle(&gate, start);
        gate.open_follow(
            false,
            false,
            zephium_agentic::SemanticActionFollow::default(),
        );
        assert!(!gate.allows_apple_action(
            "https://www.google.com/save",
            apple_action(T::FormSubmitted, false)
        ));
    }
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    #[test]
    fn a_consent_press_saves_its_choice_once_and_follows_its_own_loads() {
        use wry::AppleNavigationType as T;
        let start = "https://www.google.com/travel/flights";
        let ready = || {
            let gate = site_gate(start);
            for phase in [E::Started, E::Committed, E::Finished] {
                gate.observe(event(1, phase, start)).unwrap();
            }
            settle(&gate, start);
            gate
        };
        let slot = zephium_agentic::SemanticActionFollow::default();
        // A form saves the choice on the site's consent host, once, then
        // hands back to the page.
        let gate = ready();
        gate.open_follow(false, true, slot.clone());
        let save = "https://consent.google.com/save";
        assert!(!gate.allows_apple_action(
            "https://evil.test/save",
            apple_action(T::FormSubmitted, false)
        ));
        assert!(gate.allows_apple_action(save, apple_action(T::FormSubmitted, false)));
        assert!(!gate.allows_apple_action(save, apple_action(T::FormSubmitted, false)));
        for phase in [E::Started, E::Committed, E::Finished] {
            gate.observe(event(2, phase, save)).unwrap();
        }
        settle(&gate, save);
        assert!(gate.allows_apple_action(start, apple_action(T::Other, true)));
        for phase in [E::Started, E::Committed, E::Finished] {
            gate.observe(event(3, phase, start)).unwrap();
        }
        settle(&gate, start);
        assert!(gate.ready(Some(start)) && !gate.failed());
        assert_eq!(slot.take(), None);
        // An ordinary action's reload of its own page is followed natively;
        // a load of another page on the site stays the controller's to follow.
        let gate = ready();
        gate.open_follow(false, false, slot.clone());
        assert!(!gate.allows_apple_action(
            "https://www.google.com/travel/hotels",
            apple_action(T::LinkActivated, true)
        ));
        assert!(gate.ready(Some(start)));
        assert!(slot.take().is_some());
        assert!(gate.allows_apple_action(start, apple_action(T::Reload, true)));
        assert!(gate.handed_on());
        // A script that saved the choice and reloads the page is followed,
        // not diverted; another site never is.
        let gate = ready();
        gate.open_follow(false, true, slot.clone());
        assert!(!gate.allows_apple_action("https://evil.test/", apple_action(T::Other, true)));
        assert!(gate.allows_apple_action(start, apple_action(T::Reload, true)));
        for phase in [E::Started, E::Committed, E::Finished] {
            gate.observe(event(2, phase, start)).unwrap();
        }
        settle(&gate, start);
        assert!(gate.ready(Some(start)) && !gate.failed());
        assert_eq!(slot.take(), None);
    }
    #[test]
    fn successor_uses_same_gate_exact_lineage_and_one_terminal_without_bootstrap() {
        let gate = ready_gate();
        let (source, request) = next_request();
        let prior_stamp = gate.observation_stamp(source).unwrap();
        let next = request.target().as_url().as_str();
        gate.arm_successor(source, &request).unwrap();
        assert!(gate.observation_stamp(source).is_none());
        assert!(!gate.ready(Some(URL)));
        assert!(!gate.allows("about:blank"));
        assert!(!gate.allows(URL));
        assert!(gate.allows(next));
        assert!(!gate.allows(next));
        assert!(gate.take_successor_terminal().is_none());
        gate.observe(event(2, E::Started, next)).unwrap();
        gate.location_changed(Some(next)).unwrap();
        assert_eq!(
            gate.observe(event(2, E::Committed, next)),
            Ok((true, false))
        );
        assert!(gate.take_successor_terminal().is_none());
        gate.observe(event(2, E::Finished, next)).unwrap();
        assert!(gate.ready(Some(next)));
        assert!(gate.arm_successor(source, &request).is_err());
        let (operation, outcome) = gate.take_successor_terminal().unwrap();
        assert_eq!(operation, request.operation());
        assert_eq!(outcome.as_ref(), Ok(request.target()));
        assert_eq!(gate.0.lock().unwrap().navigation_epoch, 2);
        assert!(gate.observation_stamp(source).is_none());
        assert_ne!(
            gate.observation_stamp(operation.context()),
            Some(prior_stamp)
        );
        assert!(gate.observation_stamp(operation.context()).is_some());
        assert!(gate.take_successor_terminal().is_none());
        assert!(gate.arm_successor(source, &request).is_err());
        let successor_stamp = gate.observation_stamp(operation.context()).unwrap();
        assert_eq!(gate.location_changed(Some(next)), Ok(false));
        assert_eq!(
            gate.observation_stamp(operation.context()),
            Some(successor_stamp)
        );
        assert!(gate.ready(Some(next)));
        assert_eq!(
            gate.location_changed(Some("https://example.test/next#drift")),
            Ok(true)
        );
        assert!(!gate.ready(Some(next)));
        assert!(gate.observation_stamp(operation.context()).is_none());
    }
    #[test]
    fn successor_rejects_old_document_events_redirects_wrong_ids_and_post_ready_drift() {
        for fault in 0..6 {
            let gate = ready_gate();
            let (source, request) = next_request();
            let next = request.target().as_url().as_str();
            gate.arm_successor(source, &request).unwrap();
            assert!(gate.allows(next));
            gate.observe(event(2, E::Started, next)).unwrap();
            let hostile = match fault {
                0 => event(1, E::Finished, URL),
                1 => event(2, E::Redirected, next),
                2 => event(3, E::Committed, next),
                3 => event(2, E::Committed, URL),
                4 => event(2, E::Finished, next),
                _ => event(2, E::Failed, next),
            };
            gate.observe(hostile).unwrap();
            assert!(gate.failed());
            let (operation, outcome) = gate.take_successor_terminal().unwrap();
            assert_eq!(operation, request.operation());
            assert!(outcome.is_err());
            gate.observe(event(2, E::Committed, next)).unwrap();
            gate.observe(event(2, E::Finished, next)).unwrap();
            assert!(!gate.ready(Some(next)));
            assert!(!gate.ready(Some(URL)));
            assert!(gate.arm_successor(source, &request).is_err());
            assert_eq!(gate.0.lock().unwrap().navigation_epoch, 1);
        }
    }
    #[cfg(feature = "native-agentic-work-resource-probe")]
    #[test]
    fn current_components_distinguish_normalization_and_drift_without_granting_authority() {
        use CurrentDocumentEvidence as C;
        let expected = ContextNavigationTarget::parse(URL).unwrap();
        let gate = armed();
        for phase in [E::Started, E::Committed, E::Finished] {
            gate.observe(event(1, phase, URL)).unwrap();
        }
        let normalized = "https://EXAMPLE.TEST:443/frozen";
        assert!(matches!(
            C::compare(&expected, normalized),
            C::Compared {
                raw_equal: false,
                canonical_equal: true,
                ..
            }
        ));
        assert!(!gate.ready(Some(normalized)));
        assert!(gate.ready(Some(URL)));
        let query = C::compare(&expected, "https://example.test/frozen?opaque=one");
        assert!(matches!(
            query,
            C::Compared {
                raw_equal: false,
                canonical_equal: false,
                scheme_equal: true,
                host_equal: true,
                port_equal: true,
                path_equal: true,
                query_equal: false,
                fragment_equal: true,
                query_present: true,
                query_empty: false,
                credentials_present: false,
                ..
            }
        ));
        assert_eq!(
            query,
            C::compare(&expected, "https://example.test/frozen?different=two")
        );
        let safe = format!("{query:?}");
        for secret in [
            "example.test",
            "opaque",
            "one",
            "different",
            "two",
            "/frozen",
        ] {
            assert!(!safe.contains(secret));
        }
        assert!(matches!(
            C::compare(&expected, "https://example.test/frozen?"),
            C::Compared {
                query_present: true,
                query_empty: true,
                ..
            }
        ));
        assert!(matches!(
            C::compare(&expected, "https://example.test/frozen#private"),
            C::Compared {
                query_equal: true,
                fragment_equal: false,
                fragment_present: true,
                ..
            }
        ));
        assert!(matches!(
            C::compare(&expected, "https://example.test/changed"),
            C::Compared {
                path_equal: false,
                ..
            }
        ));
        assert!(matches!(
            C::compare(&expected, "https://other.test/frozen"),
            C::Compared {
                host_equal: false,
                ..
            }
        ));
        assert!(matches!(
            C::compare(&expected, "https://user:secret@example.test/frozen"),
            C::Compared {
                credentials_equal: false,
                credentials_present: true,
                ..
            }
        ));
        assert!(matches!(
            C::compare(&expected, "http://example.test/frozen"),
            C::Compared {
                scheme_equal: false,
                port_equal: false,
                ..
            }
        ));
        assert!(matches!(
            C::compare(&expected, "https://example.test:8443/frozen"),
            C::Compared {
                port_equal: false,
                ..
            }
        ));
        assert_eq!(C::compare(&expected, "not a URL"), C::InvalidUrl);
    }
    #[cfg(feature = "native-agentic-work-resource-probe")]
    #[test]
    fn finished_commit_identity_does_not_authorize_a_pre_finish_location_change() {
        let gate = armed();
        gate.observe(event(1, E::Started, URL)).unwrap();
        gate.observe(event(1, E::Committed, URL)).unwrap();
        assert_eq!(gate.location_changed(Some(URL)), Ok(false));
        gate.observe(event(1, E::Finished, URL)).unwrap();
        let evidence = gate.construction_evidence().unwrap();
        assert!(
            evidence.finished
                && evidence.location_callback_after_commit_before_ready
                && !evidence.refused
        );
        assert!(!gate.ready(Some("https://example.test/frozen?changed=1")));
        assert!(!gate.ready(None));
    }
    #[cfg(feature = "native-agentic-work-resource-probe")]
    #[test]
    fn construction_milestones_distinguish_waits_and_freeze_first_navigation_refusal() {
        let empty = WorkDocumentNavigation::default()
            .construction_evidence()
            .unwrap();
        assert_eq!(empty, NavigationEvidence::default());
        let gate = armed();
        let bootstrap = gate.construction_evidence().unwrap();
        assert!(
            bootstrap.bootstrap_started
                && bootstrap.bootstrap_committed
                && bootstrap.bootstrap_finished
                && bootstrap.requested
        );
        assert!(
            !bootstrap.started && !bootstrap.committed && !bootstrap.finished && !bootstrap.refused
        );
        gate.observe(event(1, E::Started, URL)).unwrap();
        assert!(gate.construction_evidence().unwrap().started);
        gate.observe(event(1, E::Committed, URL)).unwrap();
        let committed = gate.construction_evidence().unwrap();
        assert!(committed.committed && !committed.finished && !committed.refused);
        gate.observe(event(1, E::Redirected, URL)).unwrap();
        let refused = gate.construction_evidence().unwrap();
        assert!(refused.refused && refused.committed && !refused.finished);
        assert_eq!(refused.last_event, Some(E::Redirected));
        gate.observe(event(1, E::Finished, URL)).unwrap();
        assert_eq!(gate.construction_evidence(), Some(refused));
        assert!(!gate.ready(Some(URL)));
    }
    pub(super) fn event(id: u64, phase: E, url: &str) -> wry::NavigationEvent {
        wry::NavigationEvent {
            id: wry::NavigationId::from_raw(id),
            phase,
            url: url.into(),
        }
    }
    fn armed() -> WorkDocumentNavigation {
        armed_policy(zephium_agentic::WorkBrowserDocumentPolicy::Exact)
    }
    fn armed_policy(policy: zephium_agentic::WorkBrowserDocumentPolicy) -> WorkDocumentNavigation {
        let gate = WorkDocumentNavigation::default();
        assert!(gate.allows("about:blank"));
        for phase in [E::Started, E::Committed, E::Finished] {
            gate.observe(event(99, phase, "about:blank")).unwrap();
        }
        gate.arm_with_policy(ContextNavigationTarget::parse(URL).unwrap(), policy)
            .unwrap();
        assert!(gate.allows(URL));
        gate
    }
    fn finalizing() -> WorkDocumentNavigation {
        let gate =
            armed_policy(zephium_agentic::WorkBrowserDocumentPolicy::InitialQueryFinalization);
        for phase in [E::Started, E::Committed] {
            gate.observe(event(1, phase, URL)).unwrap();
        }
        assert_eq!(gate.location_changed(Some(URL)), Ok(false));
        gate.observe(event(1, E::Finished, URL)).unwrap();
        assert!(gate.finalization_pending());
        assert!(!gate.ready(Some(URL)));
        gate
    }
    #[cfg(feature = "native-agentic-work-lifetime-diagnostic")]
    #[test]
    fn deadline_stage_tracks_only_the_closed_native_control_phase() {
        use crate::WorkResourceDeadlineStage as Stage;

        let bootstrap = WorkDocumentNavigation::default();
        assert_eq!(bootstrap.deadline_stage(), Stage::ConstructionBootstrap);
        assert!(bootstrap.allows("about:blank"));
        for phase in [E::Started, E::Committed, E::Finished] {
            bootstrap.observe(event(99, phase, "about:blank")).unwrap();
            assert_eq!(bootstrap.deadline_stage(), Stage::ConstructionBootstrap);
        }

        let exact = bootstrap;
        exact
            .arm(ContextNavigationTarget::parse(URL).unwrap())
            .unwrap();
        assert_eq!(exact.deadline_stage(), Stage::ConstructionTargetArmed);
        assert!(exact.allows(URL));
        assert_eq!(exact.deadline_stage(), Stage::ConstructionTargetArmed);
        exact.observe(event(1, E::Started, URL)).unwrap();
        assert_eq!(exact.deadline_stage(), Stage::ConstructionTargetProvisional);
        exact.observe(event(1, E::Committed, URL)).unwrap();
        assert_eq!(exact.deadline_stage(), Stage::ConstructionTargetCommitted);
        exact.observe(event(1, E::Finished, URL)).unwrap();
        assert_eq!(exact.deadline_stage(), Stage::ConstructionTargetReady);
        assert!(exact.retire());
        assert_eq!(exact.deadline_stage(), Stage::ConstructionRetired);

        let finalizing = finalizing();
        assert_eq!(
            finalizing.deadline_stage(),
            Stage::ConstructionTargetFinalizing
        );
        let effective = finalizing
            .finalize(|| {
                assert_eq!(
                    finalizing.deadline_stage(),
                    Stage::ConstructionTargetSampling
                );
                Some("https://example.test/frozen?opaque=diagnostic".into())
            })
            .unwrap();
        assert!(finalizing.ready(Some(effective.as_url().as_str())));
        assert_eq!(finalizing.deadline_stage(), Stage::ConstructionTargetReady);
        finalizing.refuse();
        assert_eq!(finalizing.deadline_stage(), Stage::ConstructionRefused);
    }
    #[test]
    fn history_availability_is_not_document_authority_when_ready_or_finalizing() {
        let ready = armed();
        for phase in [E::Started, E::Committed, E::Finished] {
            ready.observe(event(1, phase, URL)).unwrap();
        }
        assert!(ready.ready(Some(URL)));
        assert!(!ready.history_availability_changed());
        assert!(ready.ready(Some(URL)));
        assert!(!ready.failed());

        let finalizing = finalizing();
        assert!(!finalizing.history_availability_changed());
        let effective = finalizing
            .finalize(|| {
                assert!(!finalizing.history_availability_changed());
                Some("https://example.test/frozen?opaque=history".into())
            })
            .unwrap();
        assert!(finalizing.ready(Some(effective.as_url().as_str())));
        assert!(!finalizing.failed());
    }
    #[test]
    fn ready_url_observation_accepts_only_the_raw_exact_sealed_value() {
        let exact = ready_gate();
        assert!(exact.ready(Some(URL)));
        assert_eq!(exact.location_changed(Some(URL)), Ok(false));
        assert!(exact.ready(Some(URL)));
        assert!(!exact.failed());

        for current in [
            None,
            Some("https://EXAMPLE.TEST/frozen"),
            Some("https://example.test:443/frozen"),
            Some("https://example.test/other"),
            Some("https://example.test/frozen?query=1"),
            Some("https://example.test/frozen#fragment"),
            Some("https://other.test/frozen"),
            Some("https://user@example.test/frozen"),
            Some("not a URL"),
            Some(""),
        ] {
            let gate = ready_gate();
            assert!(gate.ready(Some(URL)));
            assert_eq!(gate.location_changed(current), Ok(true));
            assert!(gate.failed());
        }
    }

    #[cfg(feature = "native-agentic-work-lifetime-diagnostic")]
    #[test]
    fn ready_url_refusal_records_only_content_free_component_relations() {
        use crate::WorkUrlObservationFailure as Failure;

        let missing = ready_gate();
        assert_eq!(missing.location_changed(None), Ok(true));
        assert_eq!(
            missing.url_observation_failure(),
            Some(Failure::NativeValueUnavailable)
        );

        let changed = ready_gate();
        assert_eq!(
            changed.location_changed(Some("https://example.test/frozen?private=value")),
            Ok(true)
        );
        assert!(matches!(
            changed.url_observation_failure(),
            Some(Failure::Compared {
                raw_equal: false,
                canonical_equal: false,
                scheme_equal: true,
                host_equal: true,
                port_equal: true,
                path_equal: true,
                query_equal: false,
                fragment_equal: true,
                credentials_equal: true,
                query_present: true,
                fragment_present: false,
                credentials_present: false,
            })
        ));
        let diagnostic = format!("{:?}", changed.url_observation_failure());
        assert!(!diagnostic.contains("private"));
        assert!(!diagnostic.contains("value"));
        assert!(!diagnostic.contains("example.test"));

        let canonical_only = ready_gate();
        assert_eq!(
            canonical_only.location_changed(Some("https://EXAMPLE.test/frozen")),
            Ok(true)
        );
        assert!(matches!(
            canonical_only.url_observation_failure(),
            Some(Failure::Compared {
                raw_equal: false,
                canonical_equal: true,
                ..
            })
        ));
    }

    #[test]
    fn url_observation_is_evidence_only_before_and_after_exact_navigation() {
        let gate = armed();
        assert_eq!(gate.location_changed(Some(URL)), Ok(false));
        assert!(!gate.failed());
        gate.observe(event(1, E::Started, URL)).unwrap();
        assert_eq!(gate.location_changed(Some(URL)), Ok(false));
        assert!(!gate.failed());
        gate.observe(event(1, E::Committed, URL)).unwrap();
        assert_eq!(gate.location_changed(Some(URL)), Ok(false));
        assert!(!gate.failed());
        gate.observe(event(1, E::Finished, URL)).unwrap();
        assert!(gate.ready(Some(URL)));
        assert_eq!(gate.location_changed(Some(URL)), Ok(false));
        assert!(gate.ready(Some(URL)));
    }

    #[test]
    fn public_query_finalization_seals_one_native_document_and_then_remains_exact() {
        let requested = "https://example.test/catalog?sort=price";
        let effective = "https://example.test/catalog?sort=price&page=1";
        let gate = WorkDocumentNavigation::default();
        assert!(gate.allows("about:blank"));
        for phase in [E::Started, E::Committed, E::Finished] {
            gate.observe(event(99, phase, "about:blank")).unwrap();
        }
        gate.arm_with_policy(
            ContextNavigationTarget::parse(requested).unwrap(),
            zephium_agentic::WorkBrowserDocumentPolicy::PublicQueryFinalization,
        )
        .unwrap();
        assert!(gate.allows(requested));
        for phase in [E::Started, E::Committed] {
            gate.observe(event(1, phase, requested)).unwrap();
        }
        assert_eq!(gate.location_changed(Some(effective)), Ok(false));
        gate.observe(event(1, E::Finished, requested)).unwrap();
        assert!(!gate.ready(Some(effective)));
        let frozen = gate.finalize(|| Some(effective.into())).unwrap();
        assert_eq!(frozen.as_url().as_str(), effective);
        assert!(gate.ready(Some(effective)));
        assert!(!gate.allows(effective));
        assert_eq!(gate.location_changed(Some(requested)), Ok(true));
        assert!(gate.failed());
    }

    #[test]
    fn public_interaction_queries_preserve_native_document_but_never_admit_loads() {
        fn ready() -> WorkDocumentNavigation {
            let gate = WorkDocumentNavigation::default();
            assert!(gate.allows("about:blank"));
            for phase in [E::Started, E::Committed, E::Finished] {
                gate.observe(event(99, phase, "about:blank")).unwrap();
            }
            gate.arm_with_policy(
                ContextNavigationTarget::parse(URL).unwrap(),
                zephium_agentic::WorkBrowserDocumentPolicy::PublicSameDocumentQuery,
            )
            .unwrap();
            assert!(gate.allows(URL));
            for phase in [E::Started, E::Committed, E::Finished] {
                gate.observe(event(1, phase, URL)).unwrap();
            }
            gate.finalize(|| Some(URL.into())).unwrap();
            gate
        }
        let gate = ready();
        gate.open_follow(
            false,
            false,
            zephium_agentic::SemanticActionFollow::default(),
        );
        for current in [
            "https://example.test/frozen?entry=adult",
            "https://example.test/frozen?sort=price",
            URL,
        ] {
            assert_eq!(gate.location_changed(Some(current)), Ok(false));
            assert!(gate.ready(Some(current)));
            assert!(!gate.allows(current));
            let state = gate.0.lock().unwrap();
            assert_eq!(state.native_id, Some(wry::NavigationId::from_raw(1)));
            assert_eq!(state.navigation_epoch, 1);
            assert_eq!(state.target.as_ref().unwrap().as_url().as_str(), URL);
        }
        for current in [
            None,
            Some("https://example.test/other?sort=price"),
            Some("https://other.test/frozen?sort=price"),
            Some("http://example.test/frozen"),
            Some("https://example.test/frozen#changed"),
            Some("https://example.test/frozen?access_token=secret-value"),
            Some("https://EXAMPLE.test/frozen?sort=price"),
        ] {
            let gate = ready();
            assert_eq!(gate.location_changed(current), Ok(true));
            assert!(gate.failed());
        }
        for phase in [E::Started, E::Committed, E::Finished] {
            let gate = ready();
            assert_eq!(gate.observe(event(2, phase, URL)), Ok((false, true)));
            assert!(gate.failed());
        }
    }

    #[test]
    fn finalization_sampling_remains_revision_fenced_by_equal_url_observation() {
        let finalizing = finalizing();
        assert!(finalizing
            .finalize(|| {
                assert_eq!(finalizing.location_changed(Some(URL)), Ok(false));
                Some(URL.into())
            })
            .is_err());
        assert!(finalizing.failed());
    }
    #[test]
    fn stale_quiet_period_ticket_reschedules_without_sampling_or_widening_authority() {
        let gate = finalizing();
        let stale = gate.finalization_ticket().unwrap();
        assert_eq!(
            gate.location_changed(Some("https://example.test/frozen?opaque=late")),
            Ok(false)
        );
        assert_eq!(
            gate.finalize_after_quiet_period(stale, || panic!("stale ticket must not sample")),
            Ok(None)
        );
        assert!(gate.finalization_pending());
        let current = "https://example.test/frozen?opaque=late";
        let effective = gate
            .finalize_after_quiet_period(gate.finalization_ticket().unwrap(), || {
                Some(current.into())
            })
            .unwrap()
            .unwrap();
        assert_eq!(effective.as_url().as_str(), current);
        assert!(gate.ready(Some(current)));
    }
    #[test]
    fn quiet_period_ticket_is_bound_to_its_gate_and_navigation_episode() {
        let gate = finalizing();
        let initial_ticket = gate.finalization_ticket().unwrap();
        gate.finalize_after_quiet_period(initial_ticket.clone(), || Some(URL.into()))
            .unwrap()
            .unwrap();

        let (source, request) = next_request_with_policy(
            zephium_agentic::WorkBrowserDocumentPolicy::DocumentQueryFinalization,
        );
        let requested = request.target().as_url().as_str();
        gate.arm_successor(source, &request).unwrap();
        assert!(gate.allows(requested));
        // Deliberately reuse the native test ID and keep the URL-observation
        // revision at zero. The episode generation must still reject the old
        // initial-document ticket without sampling.
        for phase in [E::Started, E::Committed, E::Finished] {
            gate.observe(event(1, phase, requested)).unwrap();
        }
        assert_eq!(
            gate.finalize_after_quiet_period(initial_ticket, || {
                panic!("an earlier navigation ticket must not sample")
            }),
            Ok(None)
        );

        let foreign = finalizing();
        let foreign_ticket = foreign.finalization_ticket().unwrap();
        assert_eq!(
            gate.finalize_after_quiet_period(foreign_ticket, || {
                panic!("a different gate's ticket must not sample")
            }),
            Ok(None)
        );

        let effective = "https://example.test/next?opaque=native";
        gate.finalize_after_quiet_period(gate.finalization_ticket().unwrap(), || {
            Some(effective.into())
        })
        .unwrap()
        .unwrap();
        assert!(gate.ready(Some(effective)));
    }
    #[test]
    fn successor_query_finalization_uses_its_exact_operation_and_effective_document() {
        let gate = ready_gate();
        let (source, request) = next_request_with_policy(
            zephium_agentic::WorkBrowserDocumentPolicy::DocumentQueryFinalization,
        );
        let requested = request.target().as_url().as_str();
        gate.arm_successor(source, &request).unwrap();
        assert!(gate.allows(requested));
        for phase in [E::Started, E::Committed, E::Finished] {
            gate.observe(event(2, phase, requested)).unwrap();
        }
        assert!(gate.finalization_pending());
        assert!(gate.take_successor_terminal().is_none());
        let current = "https://example.test/next?opaque=native";
        gate.finalize_after_quiet_period(gate.finalization_ticket().unwrap(), || {
            Some(current.into())
        })
        .unwrap()
        .unwrap();
        let (operation, outcome) = gate.take_successor_terminal().unwrap();
        assert_eq!(operation, request.operation());
        assert_eq!(outcome.unwrap().as_url().as_str(), current);
        assert!(gate.ready(Some(current)));
        assert_eq!(gate.location_changed(Some(requested)), Ok(true));
        assert!(gate.failed());
    }
    #[test]
    fn startup_finalization_freezes_original_navigation_before_any_dispatch() {
        let current = "https://example.test/frozen?opaque=one";
        let gate = finalizing();
        assert!(!gate.allows(current));
        let effective = gate.finalize(|| Some(current.into())).unwrap();
        assert_eq!(effective.as_url().as_str(), current);
        assert!(gate.ready(Some(current)));
        assert!(!gate.ready(Some(URL)));
        assert!(!gate.allows(current));
        assert!(!gate.finalization_pending());
        assert!(gate
            .finalize(|| panic!("never resample a sealed document"))
            .is_err());
        // A duplicate native notification cannot mint new authority or revive
        // refs, but raw-exact equality also cannot erase the already sealed
        // document merely because KVO delivery was delayed.
        assert_eq!(gate.location_changed(Some(current)), Ok(false));
        assert!(gate.ready(Some(current)));
        assert_eq!(gate.location_changed(Some(URL)), Ok(true));
        assert!(!gate.ready(Some(current)));
        assert!(gate.failed());
        gate.observe(event(1, E::Finished, URL)).unwrap();
        assert!(!gate.ready(Some(current)));
    }
    #[test]
    fn startup_sampling_is_first_attempt_only_and_revision_fenced() {
        let gate = finalizing();
        let count = std::cell::Cell::new(0);
        assert!(gate
            .finalize(|| {
                count.set(count.get() + 1);
                assert!(!gate.ready(Some(URL)));
                assert!(gate.finalize(|| panic!("reentrant sample")).is_err());
                gate.location_changed(Some(URL)).unwrap();
                Some(URL.into())
            })
            .is_err());
        assert_eq!(count.get(), 1);
        assert!(gate.failed());
        assert!(gate.finalize(|| panic!("retry sample")).is_err());
        assert!(!gate.ready(Some(URL)));
    }
    #[test]
    fn startup_refuses_missing_noncanonical_and_changed_documents() {
        for current in [
            None,
            Some("https://EXAMPLE.TEST/frozen?x=1"),
            Some("https://example.test/other?x=1"),
            Some("https://else.test/frozen?x=1"),
            Some("https://example.test/frozen?"),
            Some("https://example.test/frozen?x=1#fragment"),
        ] {
            let gate = finalizing();
            assert!(gate.finalize(|| current.map(str::to_owned)).is_err());
            assert!(gate.failed());
        }
    }
    #[test]
    fn query_finalization_refuses_transient_forbidden_kvo_before_any_sample() {
        for current in [
            None,
            Some("https://example.test/other?x=1"),
            Some("https://else.test/frozen?x=1"),
            Some("https://example.test/frozen?x=1#fragment"),
            Some("not a URL"),
        ] {
            let gate = finalizing();
            let ticket = gate.finalization_ticket().unwrap();
            assert_eq!(gate.location_changed(current), Ok(true));
            assert!(gate.failed());
            assert!(gate
                .finalize_after_quiet_period(ticket, || panic!(
                    "forbidden KVO cannot be hidden by a later sample"
                ),)
                .is_err());
        }

        let committed =
            armed_policy(zephium_agentic::WorkBrowserDocumentPolicy::InitialQueryFinalization);
        assert_eq!(
            committed.observe(event(1, E::Started, URL)),
            Ok((false, false))
        );
        assert_eq!(
            committed.observe(event(1, E::Committed, URL)),
            Ok((true, false))
        );
        assert_eq!(
            committed.location_changed(Some("https://example.test/other?x=1")),
            Ok(true)
        );
        assert!(committed.failed());
    }
    #[test]
    fn startup_never_finalizes_missing_foreign_or_redirected_native_lineage() {
        for events in [
            vec![event(1, E::Finished, URL)],
            vec![event(1, E::Started, URL), event(2, E::Committed, URL)],
            vec![event(1, E::Started, URL), event(1, E::Redirected, URL)],
            vec![
                event(1, E::Started, URL),
                event(1, E::Committed, URL),
                event(2, E::Finished, URL),
            ],
        ] {
            let gate =
                armed_policy(zephium_agentic::WorkBrowserDocumentPolicy::InitialQueryFinalization);
            for event in events {
                gate.observe(event).unwrap();
            }
            assert!(gate.failed());
            assert!(gate
                .finalize(|| panic!("unproved lineage cannot sample"))
                .is_err());
        }
    }
    #[test]
    fn exact_frozen_source_requires_start_commit_finish_and_never_rearms() {
        let gate = armed();
        assert!(!gate.allows(URL));
        assert!(!gate.ready(Some(URL)));
        assert_eq!(gate.observe(event(1, E::Started, URL)), Ok((false, false)));
        assert_eq!(gate.observe(event(1, E::Committed, URL)), Ok((true, false)));
        assert!(!gate.ready(Some(URL)));
        assert_eq!(gate.observe(event(1, E::Finished, URL)), Ok((false, true)));
        assert!(gate.ready(Some(URL)));
        assert!(!gate.ready(Some("https://example.test/other")));
        assert!(gate
            .arm(ContextNavigationTarget::parse(URL).unwrap())
            .is_err());
        assert!(!gate.allows(URL));
    }
    #[test]
    fn redirects_substitution_foreign_events_reloads_and_same_document_reveal_refuse() {
        for hostile in [
            event(1, E::Redirected, URL),
            event(1, E::Committed, "https://example.test/other"),
            event(2, E::Committed, URL),
            event(1, E::Finished, URL),
            event(1, E::Failed, URL),
        ] {
            let gate = armed();
            gate.observe(event(1, E::Started, URL)).unwrap();
            assert_eq!(gate.observe(hostile), Ok((false, true)));
            assert!(gate.failed());
            assert!(!gate.ready(Some(URL)));
        }
        for reload in [false, true] {
            let gate = armed();
            for phase in [E::Started, E::Committed, E::Finished] {
                gate.observe(event(1, phase, URL)).unwrap();
            }
            if reload {
                gate.observe(event(2, E::Started, URL)).unwrap();
            } else {
                assert_eq!(
                    gate.location_changed(Some("https://example.test/frozen#drift")),
                    Ok(true)
                );
            }
            assert!(gate.failed());
        }
    }
    #[test]
    fn about_blank_is_construction_only_and_retirement_never_reopens_it() {
        let gate = WorkDocumentNavigation::default();
        assert!(!gate.allows(URL));
        assert!(gate.allows("about:blank"));
        assert!(!gate.allows("about:blank"));
        gate.observe(event(1, E::Started, "about:blank")).unwrap();
        assert_eq!(
            gate.observe(event(1, E::Committed, "about:blank")),
            Ok((true, false))
        );
        gate.observe(event(1, E::Finished, "about:blank")).unwrap();
        gate.arm(ContextNavigationTarget::parse(URL).unwrap())
            .unwrap();
        assert!(!gate.allows("about:blank"));
        assert!(gate.retire());
        assert!(!gate.allows(URL));
        assert!(gate.failed());
    }
}

#[cfg(all(target_os = "macos", feature = "native-agentic-semantic-probe"))]
impl WorkDocumentNavigation {
    pub(crate) fn record_form_probe_navigation(
        &self,
        action: wry::AppleNavigationAction,
        allowed: bool,
    ) {
        if super::form_qualification::selection().is_none() {
            return;
        }
        if let Ok(mut state) = self.0.lock() {
            let consent = state.follow.as_ref().is_some_and(|follow| follow.consent);
            state.form_audit.navigation(action, allowed, consent);
        }
    }
    pub(crate) fn form_probe_evidence(&self) -> Option<super::form_qualification::NavigationAudit> {
        Some(self.0.lock().ok()?.form_audit)
    }
}
