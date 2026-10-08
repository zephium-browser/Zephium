#[cfg(any(target_os = "macos", target_os = "windows"))]
use super::dispatch::with_fullscreen_observation;
#[cfg(target_os = "windows")]
use super::dispatch::with_profile_exit;
use super::dispatch::{with_renderer_exit, with_source_observation, with_title_observation};
use super::permits::{
    queue_navigation_commit, queue_navigation_completion, queue_navigation_failure, EventPermit,
};
use super::profiles::bind_profile_persistence_class;
#[cfg(target_os = "windows")]
use super::profiles::MAX_NATIVE_PROFILE_PROCESS_GROUPS;
#[cfg(target_os = "macos")]
use super::profiles::{
    profile_scoped_value, profile_value_is_isolated, MAX_PROFILE_PERSISTENCE_BINDINGS,
};
use super::resources::{NativeResourceAdmissionError, NativeResourceClass};
#[cfg(not(all(unix, not(target_os = "macos"))))]
use super::Spare;
use super::{EngineHost, ObservedView};

use std::cell::Cell;
#[cfg(target_os = "windows")]
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use wry::dpi::{LogicalPosition, LogicalSize, Position, Size};
use wry::{DownloadPolicy, WebViewBuilder};

use crate::navigation_epoch::{NavigationEpochTracker, NavigationTransition};
use zephium_core::geometry::Rect;
use zephium_core::ids::ItemId;
#[cfg(any(target_os = "macos", target_os = "windows"))]
use zephium_core::ports::engine::NavigationFailureReason;
use zephium_core::ports::engine::{EngineEvent, Partition, RunAt, UserScript, World};

#[cfg(target_os = "macos")]
use objc2::rc::Retained;
#[cfg(target_os = "windows")]
use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Environment;

pub(super) enum NativeViewPurpose {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    NativeTab(wry::NewWindowOpener),
    Tab,
    #[cfg(not(all(unix, not(target_os = "macos"))))]
    WarmSpare,
}

impl NativeViewPurpose {
    const fn resource_class(&self) -> NativeResourceClass {
        match self {
            Self::Tab => NativeResourceClass::Tab,
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            Self::NativeTab(_) => NativeResourceClass::Tab,
            #[cfg(not(all(unix, not(target_os = "macos"))))]
            Self::WarmSpare => NativeResourceClass::WarmSpare,
        }
    }

    const fn reports_failure(&self) -> bool {
        matches!(self, Self::Tab)
    }
}

#[cfg(any(target_os = "windows", test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct WindowsConstructionSettlement {
    built_view_exists: bool,
    native_cleanup_debts: usize,
    native_cleanup_overflowed: bool,
    host_cleanup_invariant_failed: bool,
    native_accounting_failed: bool,
    profile_is_quarantined: bool,
}

#[cfg(any(target_os = "windows", test))]
impl WindowsConstructionSettlement {
    /// A constructed controller is publishable only when the same native
    /// construction attempt produced no competing cleanup obligation and no
    /// host barrier became sticky while WebView2 pumped the message loop.
    const fn admits_view(self) -> bool {
        self.built_view_exists
            && self.native_cleanup_debts == 0
            && !self.native_cleanup_overflowed
            && !self.host_cleanup_invariant_failed
            && !self.native_accounting_failed
            && !self.profile_is_quarantined
    }
}

impl EngineHost {
    pub(crate) fn create_view(
        &mut self,
        id: ItemId,
        partition: Partition,
        url: &str,
        bounds: Rect,
        event_token: Arc<AtomicBool>,
    ) {
        if !event_token.load(Ordering::Acquire) {
            // This closure may have waited in the reentrant host queue after
            // the outer retirement check. Token state is the actual host-side
            // admission proof at physical execution time.
            return;
        }
        if self.erasure_tombstones.contains(&partition.profile()) {
            eprintln!("privacy: rejected view creation for tombstoned profile");
            self.sink
                .emit_for(event_token, EngineEvent::ViewCreationFailed { id });
            return;
        }
        if !self.has_applied_content_policy(partition.profile()) {
            eprintln!("content blocker: rejected view creation before explicit policy application");
            self.sink
                .emit_for(event_token, EngineEvent::ViewCreationFailed { id });
            return;
        }
        #[cfg(target_os = "windows")]
        {
            // A reentrant WebView2 close can make the process-wide cleanup
            // barrier sticky immediately before this queued create runs. Do
            // this before warm-spare adoption as well as fresh construction;
            // adoption otherwise bypasses build_view's native boundary.
            self.collect_pending_windows_cleanup_debts();
            if self.windows_view_admission_blocked(partition.profile()) {
                self.sink
                    .emit_for(event_token, EngineEvent::ViewCreationFailed { id });
                return;
            }
        }
        if !bind_profile_persistence_class(&mut self.profile_persistence_classes, partition) {
            eprintln!("privacy: rejected profile persistence-class mismatch or capacity");
            self.sink
                .emit_for(event_token, EngineEvent::ViewCreationFailed { id });
            return;
        }
        if self.views.contains_key(&id) {
            eprintln!("view-create: duplicate live tab identity");
            self.sink
                .emit_for(event_token, EngineEvent::ViewCreationFailed { id });
            return;
        }
        let event_permit = EventPermit::bound(&event_token);
        #[cfg(target_os = "windows")]
        let event_permit = event_permit.with_extensions(
            self.windows_extensions
                .navigation_grants(partition.profile()),
        );
        // Restored and explicitly opened tabs need the same live profile grant
        // as native child adoption, before either fresh or spare construction.
        if !event_permit.allows_navigation(url) {
            eprintln!("security: rejected invalid native view target");
            self.sink
                .emit_for(event_token, EngineEvent::ViewCreationFailed { id });
            return;
        }
        #[cfg(any(not(all(unix, not(target_os = "macos"))), test))]
        if let Some(mut spare) = self.spare.take_if(|s| s.partition == partition) {
            if let Err(error) = spare
                .view
                .native_resource
                .as_mut()
                .ok_or(NativeResourceAdmissionError::AccountingInvariant)
                .and_then(|lease| lease.reclassify(NativeResourceClass::Tab))
            {
                if error == NativeResourceAdmissionError::AccountingInvariant {
                    self.native_resource_accounting_failed = true;
                }
                eprintln!("engine: rejected warm-spare resource transfer: {error:?}");
                self.sink
                    .emit_for(event_token, EngineEvent::ViewCreationFailed { id });
                return;
            }
            // Update the logical id before binding. Neither this Cell write,
            // binding, nor epoch advance enters native code, so a queued
            // bootstrap callback cannot observe a half-adopted state.
            spare.id.set(id);
            if !spare.view.event_permit.bind_once(&event_token) {
                // A spare is a one-shot native generation. Rebinding it would
                // let callbacks retained by its former logical owner acquire
                // a replacement item's token.
                eprintln!("engine: rejected reuse of an already-bound spare view");
                self.sink
                    .emit_for(event_token, EngineEvent::ViewCreationFailed { id });
                return;
            }
            #[cfg(target_os = "macos")]
            super::webext::bind_auth_flow_view(partition.profile(), id, &event_token, url);
            #[cfg(target_os = "macos")]
            super::webext::bind_auth_cleanup_fence(
                partition.profile(),
                id,
                &spare.view.event_permit,
                &spare.view.navigation,
            );
            let Some(epoch) = spare.view.navigation.begin(url) else {
                eprintln!("engine: could not establish adopted-view navigation epoch");
                self.sink
                    .emit_for(event_token, EngineEvent::ViewCreationFailed { id });
                return;
            };
            // A spare may have completed its private about:blank bootstrap.
            // Adoption starts a fresh presentation obligation even though
            // the underlying native WebView generation is reused.
            spare.view.presentable = false;
            spare
                .view
                .presentation_permit
                .store(false, Ordering::Release);
            spare.view.presentation_announced = None;
            spare.view.title_ready = None;
            if !spare.view.event_permit.allows_navigation(url) {
                return;
            }
            #[cfg(target_os = "macos")]
            {
                // Prepare the real viewport while still hidden, before the first
                // navigation can lay out a document in the spare's zero-sized frame.
                if let Err(error) = spare.view.set_bounds(to_wry(bounds)) {
                    spare.view.navigation.fail_synchronous(epoch);
                    eprintln!("engine: spare viewport preparation failed: {error}");
                    self.sink
                        .emit_for(event_token, EngineEvent::ViewCreationFailed { id });
                    return;
                }
                crate::platform::imp::set_warm_spare_layout(&spare.view, false);
                if !spare.view.event_permit.matches_token(&event_token) {
                    return;
                }
            }
            #[cfg(target_os = "macos")]
            let restored = self.restore_discarded_state(
                id,
                partition,
                url,
                &spare.view.view,
                &spare.view.event_permit,
                &spare.view.navigation,
            );
            #[cfg(not(target_os = "macos"))]
            let restored: Option<bool> = None;
            if let Err(error) = match restored {
                Some(true) => Ok(()),
                #[cfg(target_os = "macos")]
                Some(false) => Err(wry::Error::NativeObjectUnavailable(
                    "native session restoration",
                )),
                _ => spare.view.load_url(url),
            } {
                spare.view.navigation.fail_synchronous(epoch);
                eprintln!("engine: spare navigation failed: {error}");
                self.sink
                    .emit_for(event_token, EngineEvent::ViewCreationFailed { id });
                return;
            }
            if !spare.view.event_permit.matches_token(&event_token) {
                // Navigation may pump native callbacks. Retirement can revoke
                // the outer token while load_url is in progress.
                return;
            }
            #[cfg(target_os = "windows")]
            {
                // Navigation is another WebView2 message-loop pump. Reimport
                // any cleanup obligation it exposed before making this
                // adopted controller reachable as a live tab.
                self.collect_pending_windows_cleanup_debts();
                if self.windows_view_admission_blocked(partition.profile()) {
                    drop(spare);
                    self.collect_pending_windows_cleanup_debts();
                    self.sink
                        .emit_for(event_token, EngineEvent::ViewCreationFailed { id });
                    return;
                }
            }
            self.partitions.insert(id, partition);
            self.views.insert(id, spare.view);
            self.finish_new_view_insertion(id, &event_token);
            return;
        }
        let cell = Rc::new(Cell::new(id));
        #[cfg(target_os = "macos")]
        super::webext::bind_auth_flow_view(partition.profile(), id, &event_token, url);
        if let Some(view) = self.build_view(
            cell,
            partition,
            url,
            bounds,
            event_permit,
            NativeViewPurpose::Tab,
        ) {
            if !event_token.load(Ordering::Acquire)
                || !view.event_permit.matches_token(&event_token)
            {
                return;
            }
            self.partitions.insert(id, partition);
            self.views.insert(id, view);
            self.finish_new_view_insertion(id, &event_token);
        } else {
            eprintln!("view-create: native build returned no view");
        }
    }

    // Rebuilt after adoption from a page-load-finished hook, when the spawn
    // cost hides behind the page render.
    #[cfg(not(all(unix, not(target_os = "macos"))))]
    pub(crate) fn ensure_spare(&mut self, partition: Partition) {
        if self.memory_pressure != zephium_core::ports::engine::MemoryPressure::Normal {
            return;
        }
        // Keep at most one warm renderer process. A load in another profile
        // must not destroy and rebuild an existing spare: profile activity
        // would otherwise churn processes, CPU and private working sets while
        // neither profile opens a tab. The matching profile eventually adopts
        // the spare; a later completed load can then replenish its partition.
        let profile = partition.profile();
        // Page-load completion can queue this optimization immediately before
        // the last real tab closes. Revalidate physical ownership on the host
        // thread so a late task cannot resurrect an otherwise idle native
        // process group solely to hold about:blank.
        if !self.has_live_profile_view(profile) || self.erasure_tombstones.contains(&profile) {
            return;
        }
        if !bind_profile_persistence_class(&mut self.profile_persistence_classes, partition) {
            eprintln!("privacy: rejected spare profile persistence-class mismatch or capacity");
            return;
        }
        if matches!(partition, Partition::Ephemeral(_)) || self.spare.is_some() {
            return;
        }
        if self.applied_content_policy(profile).is_none() {
            return;
        }
        let cell = Rc::new(Cell::new(ItemId::generate()));
        if let Some(view) = self.build_view(
            cell.clone(),
            partition,
            "about:blank",
            Rect::default(),
            EventPermit::inactive(),
            NativeViewPurpose::WarmSpare,
        ) {
            #[cfg(target_os = "macos")]
            crate::platform::imp::set_warm_spare_layout(&view, true);
            self.spare = Some(Spare {
                partition,
                view,
                id: cell,
            });
        }
    }

    // Linux's guarded first-map protocol belongs to a measured Stage. A warm
    // spare has no pane/stage on which to perform that offscreen map, so do not
    // create one until it has a measured, lifecycle-safe implementation.
    #[cfg(all(unix, not(target_os = "macos")))]
    pub(crate) fn ensure_spare(&mut self, partition: Partition) {
        if !self.erasure_tombstones.contains(&partition.profile())
            && !bind_profile_persistence_class(&mut self.profile_persistence_classes, partition)
        {
            eprintln!("privacy: rejected spare profile persistence-class mismatch or capacity");
        }
    }

    pub(super) fn build_view(
        &mut self,
        id: Rc<Cell<ItemId>>,
        partition: Partition,
        url: &str,
        bounds: Rect,
        event_permit: EventPermit,
        purpose: NativeViewPurpose,
    ) -> Option<ObservedView> {
        let report_failure = purpose.reports_failure();
        #[cfg(target_os = "windows")]
        let event_permit = event_permit.with_extensions(
            self.windows_extensions
                .navigation_grants(partition.profile()),
        );
        let target_resource_class = purpose.resource_class();
        let logical_id = id.get();
        let reservation_failure_permit = event_permit.clone();
        #[cfg(target_os = "windows")]
        {
            // The Wry fallback queue must be empty at an engine construction
            // boundary. Live ObservedViews report their profile directly; any Wry
            // fallback produced inside this call therefore belongs to this exact
            // partition, including errors after controller construction.
            let stale_debts = wry::pending_webview2_cleanup_debts();
            if !stale_debts.is_empty() {
                self.fail_windows_cleanup_invariant();
                for debt in stale_debts {
                    self.retain_unattributed_windows_cleanup_debt(partition.profile(), debt);
                }
            }
            self.collect_pending_windows_cleanup_debts();
        }

        let native_resource = match self
            .native_resources
            .try_acquire(NativeResourceClass::TransientConstruction)
        {
            Ok(resource) => resource,
            Err(NativeResourceAdmissionError::AccountingInvariant) => {
                eprintln!("view-create: native reservation accounting invariant");
                self.native_resource_accounting_failed = true;
                if report_failure {
                    event_permit.emit(
                        &self.sink,
                        EngineEvent::ViewCreationFailed { id: logical_id },
                    );
                }
                return None;
            }
            Err(
                error @ (NativeResourceAdmissionError::ClassExhausted(_)
                | NativeResourceAdmissionError::GlobalExhausted),
            ) => {
                eprintln!("view-create: native reservation refused: {error:?}");
                if report_failure {
                    event_permit.emit(
                        &self.sink,
                        EngineEvent::ViewCreationFailed { id: logical_id },
                    );
                }
                return None;
            }
        };
        if self.native_resource_accounting_failed {
            eprintln!("view-create: prior resource accounting failure");
            if report_failure {
                event_permit.emit(
                    &self.sink,
                    EngineEvent::ViewCreationFailed { id: logical_id },
                );
            }
            return None;
        }
        let mut native_resource = Some(native_resource);
        let mut built = self.build_view_inner(id, partition, url, bounds, event_permit, purpose);
        #[cfg(target_os = "windows")]
        {
            let construction_debts = wry::pending_webview2_cleanup_debts();
            let construction_debt_count = construction_debts.len();
            if !construction_debts.is_empty() {
                if built.is_some() || construction_debts.len() != 1 {
                    self.fail_windows_cleanup_invariant();
                }
                for debt in construction_debts {
                    let resource = if built.is_none() {
                        native_resource.take().or_else(|| {
                            self.native_resources
                                .try_acquire(NativeResourceClass::TeardownDebt)
                                .ok()
                        })
                    } else {
                        self.native_resources
                            .try_acquire(NativeResourceClass::TeardownDebt)
                            .ok()
                    };
                    let debt = super::OwnedWindowsCleanupDebt::new(debt, resource);
                    if !debt.accounted_as_debt() {
                        self.native_resource_accounting_failed = true;
                    }
                    self.retain_windows_cleanup_debt(partition.profile(), debt);
                }
            }
            let cleanup_overflowed = wry::webview2_cleanup_overflowed();
            if cleanup_overflowed {
                self.fail_windows_cleanup_invariant();
            }
            self.collect_pending_windows_cleanup_debts();

            let settlement = WindowsConstructionSettlement {
                built_view_exists: built.is_some(),
                native_cleanup_debts: construction_debt_count,
                native_cleanup_overflowed: cleanup_overflowed,
                host_cleanup_invariant_failed: self.windows_cleanup_invariant_failed,
                native_accounting_failed: self.native_resource_accounting_failed
                    || !self.native_resources.is_healthy(),
                profile_is_quarantined: self.windows_view_admission_blocked(partition.profile()),
            };
            if built.is_some() && !settlement.admits_view() {
                if let Some(view) = built.as_mut() {
                    // This exact transient lease already accounts for the
                    // locally constructed controller. Give it to the view
                    // before dropping so a failed close transfers the same
                    // ownership into teardown debt instead of releasing the
                    // slot around a possibly-live native object.
                    view.native_resource = native_resource.take();
                }
                drop(built.take());
                // ObservedView::drop can enqueue a new exact close debt.
                // Import it before returning so provenance remains attached
                // to this profile and the sticky barriers are visible now.
                self.collect_pending_windows_cleanup_debts();
                if report_failure {
                    reservation_failure_permit.emit(
                        &self.sink,
                        EngineEvent::ViewCreationFailed { id: logical_id },
                    );
                }
                return None;
            }
        }
        if built.is_some() {
            let Some(resource) = native_resource.as_mut() else {
                self.native_resource_accounting_failed = true;
                if report_failure {
                    reservation_failure_permit.emit(
                        &self.sink,
                        EngineEvent::ViewCreationFailed { id: logical_id },
                    );
                }
                return None;
            };
            if let Err(error) = resource.reclassify(target_resource_class) {
                if error == NativeResourceAdmissionError::AccountingInvariant {
                    self.native_resource_accounting_failed = true;
                }
                eprintln!("engine: rejected constructed native-resource transfer: {error:?}");
                if let Some(view) = built.as_mut() {
                    // The native object already exists. Hand it the unchanged
                    // construction lease before dropping it so a failed
                    // WebView2 close can transfer that exact ownership to a
                    // teardown debt instead of releasing capacity early.
                    view.native_resource = native_resource.take();
                }
                if report_failure {
                    reservation_failure_permit.emit(
                        &self.sink,
                        EngineEvent::ViewCreationFailed { id: logical_id },
                    );
                }
                return None;
            }
            if let Some(view) = built.as_mut() {
                view.native_resource = native_resource.take();
            }
        }
        built
    }

    #[cfg(target_os = "windows")]
    fn retain_unattributed_windows_cleanup_debt(
        &mut self,
        profile: zephium_core::ids::ProfileId,
        debt: wry::WebView2CleanupDebt,
    ) {
        let resource = self
            .native_resources
            .try_acquire(NativeResourceClass::TeardownDebt)
            .ok();
        let debt = super::OwnedWindowsCleanupDebt::new(debt, resource);
        if !debt.accounted_as_debt() {
            self.native_resource_accounting_failed = true;
        }
        self.retain_windows_cleanup_debt(profile, debt);
    }

    #[cfg(target_os = "windows")]
    pub(super) fn windows_view_admission_blocked(
        &self,
        profile: zephium_core::ids::ProfileId,
    ) -> bool {
        self.exiting_browser_processes.contains(&profile)
            || self.unverifiable_browser_processes.contains(&profile)
            || self.construction_unproven.contains(&profile)
            || self.windows_cleanup_debts.contains_key(&profile)
            || self.windows_cleanup_invariant_failed
            || self.native_resource_accounting_failed
            || !self.native_resources.is_healthy()
    }

    fn build_view_inner(
        &mut self,
        id: Rc<Cell<ItemId>>,
        partition: Partition,
        url: &str,
        bounds: Rect,
        event_permit: EventPermit,
        purpose: NativeViewPurpose,
    ) -> Option<ObservedView> {
        let report_failure = purpose.reports_failure();
        let popup_opener = match purpose {
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            NativeViewPurpose::NativeTab(opener) => Some(opener),
            _ => None::<wry::NewWindowOpener>,
        };
        let native_popup = popup_opener.is_some();
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        let initial_download = Rc::new(Cell::new(native_popup));
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        let initial_download_deadline =
            std::time::Instant::now() + std::time::Duration::from_secs(30);
        #[cfg(target_os = "macos")]
        let initial_download_window = popup_opener
            .as_ref()
            .and_then(|opener| opener.webview.window());
        if self.erasure_tombstones.contains(&partition.profile()) {
            if report_failure {
                event_permit.emit(&self.sink, EngineEvent::ViewCreationFailed { id: id.get() });
            }
            return None;
        }
        let Some(content_policy) = self.applied_content_policy(partition.profile()) else {
            if report_failure {
                event_permit.emit(&self.sink, EngineEvent::ViewCreationFailed { id: id.get() });
            }
            return None;
        };
        #[cfg(all(unix, not(target_os = "macos")))]
        if self
            .linux_unverifiable_data_managers
            .contains(&partition.profile())
        {
            // Sticky native-storage debt is a construction barrier as well as
            // a deletion barrier. Repeated retries must not accumulate one
            // inaccessible manager per failed or malformed WebView.
            if report_failure {
                event_permit.emit(&self.sink, EngineEvent::ViewCreationFailed { id: id.get() });
            }
            return None;
        }
        #[cfg(target_os = "windows")]
        if self.windows_view_admission_blocked(partition.profile()) {
            // A ProcessFailed callback is not the process-group release
            // barrier. Do not bind a replacement controller until the
            // Environment5 event retires the exact previous PID.
            if report_failure {
                event_permit.emit(&self.sink, EngineEvent::ViewCreationFailed { id: id.get() });
            }
            return None;
        }
        #[cfg(target_os = "windows")]
        if !self.windows_profile_process_group_capacity_allows(partition.profile()) {
            eprintln!(
                "engine: WebView2 native profile process-group ceiling ({MAX_NATIVE_PROFILE_PROCESS_GROUPS}) reached"
            );
            if report_failure {
                event_permit.emit(&self.sink, EngineEvent::ViewCreationFailed { id: id.get() });
            }
            return None;
        }
        if !bind_profile_persistence_class(&mut self.profile_persistence_classes, partition) {
            eprintln!("privacy: rejected profile persistence-class mismatch or capacity");
            if report_failure {
                event_permit.emit(&self.sink, EngineEvent::ViewCreationFailed { id: id.get() });
            }
            return None;
        }
        let title_permit = event_permit.clone();
        let site_preferences = self
            .blocker_sites
            .entry(partition.profile())
            .or_default()
            .clone();
        let site_scope = super::content_styles::ViewSiteScope::new(site_preferences, url);
        let replay_safety = Rc::new(super::discard::ReplaySafety::new(
            !report_failure && !native_popup,
            !native_popup,
        ));
        if let Some(counter) = self.blocker_statistics.get(&partition.profile()) {
            site_scope.pause.set_statistics(counter.clone());
        }
        let load_site_scope = site_scope.clone();
        #[cfg(target_os = "windows")]
        let navigation_site_scope = site_scope.clone();
        let navigation = NavigationEpochTracker::new();
        #[cfg(target_os = "macos")]
        super::webext::bind_auth_cleanup_fence(
            partition.profile(),
            id.get(),
            &event_permit,
            &navigation,
        );
        let title_navigation = navigation.clone();
        let on_load = self.sink.clone();
        let load_permit = event_permit.clone();
        let load_navigation = navigation.clone();
        #[cfg(target_os = "macos")]
        let page_permission_pending = self.page_permissions.pending_presence();
        #[cfg(target_os = "macos")]
        let load_page_permission_pending = page_permission_pending.clone();
        let presentation_permit = Arc::new(AtomicBool::new(false));
        #[cfg(target_os = "macos")]
        let file_uploads = super::file_uploads::FileUploadBroker::new(
            event_permit.clone(),
            navigation.clone(),
            presentation_permit.clone(),
        );
        #[cfg(target_os = "macos")]
        let load_file_uploads = Rc::downgrade(&file_uploads);
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        let download_surface_intent = Arc::new(AtomicBool::new(false));
        let guard_presentation_permit = presentation_permit.clone();
        let load_presentation_permit = presentation_permit.clone();
        let crash_permit = event_permit.clone();
        let crash_id = id.clone();
        let navigation_permit = event_permit.clone();
        let policy_navigation = navigation.clone();
        #[cfg(target_os = "macos")]
        let (frame_permit, frame_navigation) = (event_permit.clone(), navigation.clone());
        // A page's link for another application never loads; it becomes a
        // request the person answers in chrome. One per second per view, so
        // a page cannot flood it.
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        let app_link = {
            let permit = event_permit.clone();
            let sink = self.sink.clone();
            let item = id.clone();
            let last = Cell::new(None::<std::time::Instant>);
            Rc::new(move |target: &str| -> bool {
                if zephium_core::navigation::external_app_link(target).is_none() {
                    return false;
                }
                let now = std::time::Instant::now();
                if last
                    .get()
                    .is_none_or(|old| now.duration_since(old) >= std::time::Duration::from_secs(1))
                {
                    last.set(Some(now));
                    permit.emit(
                        &sink,
                        EngineEvent::ExternalAppRequested {
                            id: item.get(),
                            url: target.to_owned(),
                            app: crate::platform::imp::external_app_name(target),
                        },
                    );
                }
                true
            })
        };
        let focus_shuts = {
            let gate = self.focus_gate.clone();
            let permit = event_permit.clone();
            let sink = self.sink.clone();
            let item = id.clone();
            move |target: &str| {
                let shut = super::focus::focus_shuts(&gate, target);
                if shut {
                    permit.emit(
                        &sink,
                        EngineEvent::FocusBlocked {
                            id: item.get(),
                            url: target.to_owned(),
                        },
                    );
                }
                shut
            }
        };
        let (title_id, load_id) = (id.clone(), id.clone());
        let scripts = self.scripts_for(partition);
        #[cfg(target_os = "windows")]
        let mut cached_environment = self.environments.get(&partition.profile()).cloned();
        #[cfg(target_os = "windows")]
        let construction_environment: Rc<RefCell<Option<ICoreWebView2Environment>>> =
            Rc::new(RefCell::new(None));
        #[cfg(target_os = "windows")]
        let construction_environment_capture_failed = Rc::new(Cell::new(false));

        // A profile owns its network/storage context. This is both the cookie
        // boundary and (on Windows) the WebView2 process-group boundary. The
        // audited Wry patch permits Linux incognito views to share only an
        // explicitly ephemeral supplied context, bounding native managers and
        // giving one private profile coherent in-memory cookie/storage state.
        #[cfg(all(unix, not(target_os = "macos")))]
        let mut pending_web_context: Option<wry::WebContext> = None;
        #[cfg(all(unix, not(target_os = "macos")))]
        let (builder, expected_website_data_directory, untracked_manager_on_build_failure) = {
            let profile = partition.profile();
            let expected_directory = match partition {
                Partition::Ephemeral(_) => None,
                Partition::Default(_) | Partition::Persistent(_) => {
                    match crate::erasure::prepare_profile_directory(&self.profiles_root, profile) {
                        Ok(path) => Some(path),
                        Err(error) => {
                            eprintln!("engine: cannot secure profile data directory: {error}");
                            if report_failure {
                                event_permit.emit(
                                    &self.sink,
                                    EngineEvent::ViewCreationFailed { id: id.get() },
                                );
                            }
                            return None;
                        }
                    }
                }
            };
            // Do not commit a newly-created Wry context to the host map before
            // a native view exposes its exact manager. Wry has no public
            // context->manager accessor; retaining an opaque context after
            // build failure would otherwise let a later empty retry fabricate
            // Verified.
            let context_is_new = !self.web_contexts.contains_key(&profile);
            let builder = if context_is_new {
                let context = match match partition {
                    Partition::Ephemeral(_) => wry::WebContext::new_ephemeral(),
                    Partition::Default(_) | Partition::Persistent(_) => {
                        wry::WebContext::try_new(expected_directory.clone())
                    }
                } {
                    Ok(context) => context,
                    Err(error) => {
                        self.linux_unverifiable_data_managers.insert(profile);
                        eprintln!("security: required WebKitGTK context policy failed: {error}");
                        if report_failure {
                            event_permit
                                .emit(&self.sink, EngineEvent::ViewCreationFailed { id: id.get() });
                        }
                        return None;
                    }
                };
                pending_web_context = Some(context);
                let Some(context) = pending_web_context.as_mut() else {
                    self.linux_unverifiable_data_managers.insert(profile);
                    return None;
                };
                WebViewBuilder::new_with_web_context(context)
            } else {
                let Some(context) = self.web_contexts.get_mut(&profile) else {
                    self.linux_unverifiable_data_managers.insert(profile);
                    return None;
                };
                WebViewBuilder::new_with_web_context(context)
            };
            (builder, expected_directory, context_is_new)
        };
        #[cfg(target_os = "windows")]
        let mut extension_startup = None;
        #[cfg(target_os = "windows")]
        let (builder, expected_user_data_folder) = {
            let profile = partition.profile();
            let root = match partition {
                Partition::Ephemeral(_) => self.private_runtime.root(),
                _ => &self.profiles_root,
            };
            let path = match crate::erasure::prepare_profile_directory(root, profile) {
                Ok(path) => path,
                Err(error) => {
                    eprintln!("engine: cannot secure profile data directory: {error}");
                    if report_failure {
                        event_permit
                            .emit(&self.sink, EngineEvent::ViewCreationFailed { id: id.get() });
                    }
                    return None;
                }
            };
            if !matches!(partition, Partition::Ephemeral(_)) {
                if let Err(error) = self.ensure_windows_profile_environment_at_path(
                    profile,
                    std::time::Instant::now() + std::time::Duration::from_secs(5),
                    path.clone(),
                    true,
                ) {
                    eprintln!("extensions: profile startup refused: {error:?}");
                    if report_failure {
                        event_permit
                            .emit(&self.sink, EngineEvent::ViewCreationFailed { id: id.get() });
                    }
                    return None;
                }
                extension_startup = self.windows_extension_startup(profile, &path).ok();
                extension_startup.as_ref()?;
                cached_environment = self.environments.get(&profile).cloned();
            }
            let api_path = match crate::platform::windows::webview2_user_data_path(&path) {
                Ok(api_path) => api_path,
                Err(error) => {
                    eprintln!("engine: native user-data path projection refused: {error}");
                    if report_failure {
                        event_permit
                            .emit(&self.sink, EngineEvent::ViewCreationFailed { id: id.get() });
                    }
                    return None;
                }
            };
            let builder = WebViewBuilder::new_with_web_context(
                self.web_contexts
                    .entry(profile)
                    .or_insert_with(|| wry::WebContext::new(Some(api_path))),
            );
            (builder, path)
        };
        #[cfg(target_os = "macos")]
        let (builder, expected_ephemeral_data_store) = {
            let builder = WebViewBuilder::new();
            match partition {
                Partition::Ephemeral(profile) => {
                    if self.macos_ephemeral_data_stores.len() >= MAX_PROFILE_PERSISTENCE_BINDINGS
                        && !self.macos_ephemeral_data_stores.contains_key(&profile)
                    {
                        eprintln!("privacy: macOS private data-store capacity exceeded");
                        if report_failure {
                            event_permit
                                .emit(&self.sink, EngineEvent::ViewCreationFailed { id: id.get() });
                        }
                        return None;
                    }
                    let store = match profile_scoped_value(
                        &mut self.macos_ephemeral_data_stores,
                        profile,
                        crate::platform::imp::new_ephemeral_data_store,
                    ) {
                        Ok(store) => store,
                        Err(error) => {
                            eprintln!(
                                "privacy: cannot allocate private WKWebsiteDataStore: {error}"
                            );
                            if report_failure {
                                event_permit.emit(
                                    &self.sink,
                                    EngineEvent::ViewCreationFailed { id: id.get() },
                                );
                            }
                            return None;
                        }
                    };
                    if !profile_value_is_isolated(
                        &self.macos_ephemeral_data_stores,
                        profile,
                        &store,
                        |left, right| Retained::as_ptr(left) == Retained::as_ptr(right),
                    ) {
                        // Do not permit an unexpected framework singleton to
                        // collapse two private profiles into one cookie jar.
                        // The other profile still owns the shared native
                        // handle, so removing this duplicate map entry loses no
                        // erasure obligation.
                        self.macos_ephemeral_data_stores.remove(&profile);
                        eprintln!("privacy: WKWebsiteDataStore crossed private profiles");
                        if report_failure {
                            event_permit
                                .emit(&self.sink, EngineEvent::ViewCreationFailed { id: id.get() });
                        }
                        return None;
                    }
                    let configuration =
                        match crate::platform::imp::new_configuration_with_data_store(&store) {
                            Ok(configuration) => configuration,
                            Err(error) => {
                                // Keep the newly-created store in the host map.
                                // It is now a native privacy obligation even
                                // though no view was successfully constructed.
                                eprintln!("privacy: cannot configure private WKWebView: {error}");
                                if report_failure {
                                    event_permit.emit(
                                        &self.sink,
                                        EngineEvent::ViewCreationFailed { id: id.get() },
                                    );
                                }
                                return None;
                            }
                        };
                    use wry::WebViewBuilderExtMacos;
                    (
                        builder.with_webview_configuration(configuration),
                        Some(store),
                    )
                }
                Partition::Default(profile) | Partition::Persistent(profile) => {
                    use wry::WebViewBuilderExtMacos;
                    let configuration = self.webext.configuration(profile, &self.sink);
                    (builder.with_webview_configuration(configuration), None)
                }
            }
        };

        // Website backgrounds remain native. Exact URL acknowledgement gates
        // reveal; macOS briefly covers the attributed page until its first frame.
        #[cfg(target_os = "macos")]
        let builder = if let Some(opener) = popup_opener.as_ref() {
            use wry::WebViewBuilderExtMacos;
            let config = opener.target_configuration.clone();
            let mtm = objc2_foundation::MainThreadMarker::new()?;
            unsafe {
                config.setUserContentController(&objc2_web_kit::WKUserContentController::new(mtm));
            }
            builder.with_webview_configuration(config)
        } else {
            builder
        };
        #[cfg(target_os = "windows")]
        let builder = if let Some(opener) = popup_opener.as_ref() {
            use wry::WebViewBuilderExtWindows;
            builder.with_environment(opener.environment.clone())
        } else {
            builder
        };

        #[cfg(target_os = "windows")]
        let permission_presentation = presentation_permit.clone();
        #[cfg(target_os = "windows")]
        let permission_window = match self.parent.0 {
            raw_window_handle::RawWindowHandle::Win32(handle) => handle.hwnd.get(),
            _ => 0,
        };
        #[cfg(target_os = "windows")]
        let navigation_app_link = app_link.clone();
        let mut builder = builder
            .with_bounds(to_wry(bounds))
            // Construction itself may enter a native message loop. On
            // WKWebView/WebView2 start hidden so their default white backing
            // store cannot paint before the host installs the view in its
            // presentation-gated stage. Linux retains a visible intent, but
            // the guarded Wry adapter keeps the GTK child unmapped until its
            // Stage performs the validated offscreen first map.
            .with_visible(cfg!(all(unix, not(target_os = "macos"))))
            // Native construction must never steal keyboard focus from the
            // privileged chrome. This is especially important for hidden
            // WebView2 warm spares; focus is granted only by explicit user
            // interaction with a presented content view.
            .with_focused(false)
            // Pages a person reads can be inspected, as in any browser; the
            // privileged chrome and agent views never can.
            .with_devtools(true)
            .with_autoplay(false)
            // A tab a person reads may take the screen for its video, as in
            // any browser; the host observes every transition and the shell
            // exits it whenever the tab stops being the one on screen. Other
            // views (extensions, agents, Work) keep it off per view: Cargo
            // feature unification makes compile-time support no page grant.
            .with_fullscreen_enabled(true)
            // A playing video may float above other apps; pages that play
            // are not suspended, so it keeps going while its tab sleeps.
            .with_picture_in_picture_enabled(true)
            // Two-finger swipes move through history, as in Safari.
            .with_back_forward_navigation_gestures(true)
            // WebView2 otherwise enables its address/contact suggestions by
            // default. Raw content should not silently inherit ambient form
            // data before Zephium has an explicit, profile-scoped autofill
            // policy. Wry currently ignores this setting on WebKit platforms.
            .with_general_autofill_enabled(false)
            .with_navigation_handler(move |target| {
                let admitted = navigation_permit.allows_navigation(&target)
                    && policy_navigation.admits_target(&target);
                // WebView2 may ask here before LaunchingExternalUriScheme; the
                // shared one-per-second limit keeps that to one request.
                #[cfg(target_os = "windows")]
                if !admitted {
                    navigation_app_link(&target);
                }
                // WebView2 asks only about top-level loads here; WebKit asks
                // about frames too, so focus is checked for it below, where
                // the main frame is known.
                #[cfg(target_os = "windows")]
                if admitted && focus_shuts(target.as_str()) {
                    return false;
                }
                #[cfg(target_os = "windows")]
                if admitted {
                    navigation_site_scope.navigating(&target);
                }
                admitted
            })
            // Raw content starts with no device or ambient capabilities. On
            // Windows, camera and microphone requests go to WebView2's own
            // origin-labelled prompt; macOS replaces this handler with the
            // browser-owned broker below.
            .with_permission_handler(move |kind| {
                #[cfg(target_os = "windows")]
                if !permission_presentation.load(Ordering::Acquire)
                    || !crate::platform::imp::permission_window_is_foreground(permission_window)
                {
                    return wry::PermissionResponse::Deny;
                }
                raw_content_permission(kind)
            })
            // This is a construction-time native policy, not a callback that
            // first materializes attacker-controlled URL/path metadata. Wry
            // installs the cancel handler before initial navigation on every
            // shipped desktop engine, and denial dominates callback settings.
            .with_download_policy(DownloadPolicy::DenyWithoutMetadata)
            // Browser chrome is the only authority for logical tab closure;
            // DOM close requests must not destroy a native child behind the
            // host's view/controller accounting.
            .with_page_close_policy(wry::PageClosePolicy::Ignore)
            // Wry hides at the native commit boundary before invoking the
            // identity callback. Host/stage readiness then remains the only
            // path that can reveal the exact URL-acknowledged document.
            .with_navigation_presentation_guard(move || {
                guard_presentation_permit.store(false, Ordering::Release);
            })
            .with_document_title_changed_handler(move |_title| {
                // Title callbacks carry no navigation identifier. Do not let
                // an inactive spare, transitional document, or callback
                // queued by the prior document publish directly into chrome.
                if let Some(epoch) = title_navigation.current_committed() {
                    if title_navigation.is_current(epoch) {
                        let id = title_id.get();
                        let queued_permit = title_permit.clone();
                        let queued_navigation = title_navigation.clone();
                        with_title_observation(id, move |host| {
                            host.emit_title_observation(
                                id,
                                &queued_permit,
                                &queued_navigation,
                                epoch,
                            );
                        });
                    }
                }
            });

        // WebKit consults this in place of the plain handler above and says
        // which frame is loading, so focus shuts only top-level documents.
        #[cfg(target_os = "macos")]
        {
            let replay = replay_safety.clone();
            let auth_tab = id.clone();
            let action_app_link = app_link.clone();
            builder = builder.with_apple_navigation_action_handler(move |target, action| {
                // A page's own srcdoc, data: and blob: frames (challenge and
                // payment widgets, editors) load under WebKit's origin rules;
                // browser policy governs what a tab itself may show.
                if action.target_is_main_frame == Some(false)
                    && zephium_core::navigation::is_subframe_document(&target)
                {
                    return frame_permit.allows_navigation("about:blank")
                        && frame_navigation.admits_target("about:blank");
                }
                let admitted = frame_permit.allows_navigation(&target)
                    && frame_navigation.admits_target(&target);
                if !admitted && action_app_link(&target) {
                    return false;
                }
                if admitted
                    && super::webext::intercept_auth_redirect(
                        partition.profile(),
                        auth_tab.get(),
                        &frame_permit,
                        &frame_navigation,
                        action.target_is_main_frame,
                        &target,
                    )
                {
                    return false;
                }
                if admitted {
                    super::webext::note_auth_native_navigation(&frame_navigation, action);
                }
                if admitted && action.target_is_main_frame == Some(true) && focus_shuts(&target) {
                    return false;
                }
                if admitted {
                    replay.observe(&target, action);
                }
                admitted
            });
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        let _ = focus_shuts;

        #[cfg(any(target_os = "macos", target_os = "windows"))]
        {
            if native_popup || cfg!(target_os = "windows") {
                let closing_item = id.clone();
                let closing_permit = event_permit.clone();
                let closing_sink = self.sink.clone();
                #[cfg(target_os = "windows")]
                let closing_navigation = navigation.clone();
                builder = builder.with_page_close_handler(move || {
                    #[cfg(target_os = "windows")]
                    if !native_popup
                        && !closing_navigation
                            .committed_snapshot()
                            .is_some_and(|(_, url)| {
                                closing_permit.allows_extension_document_close(&url)
                            })
                    {
                        return;
                    }
                    closing_permit.emit(
                        &closing_sink,
                        EngineEvent::NativeTabCloseRequested {
                            id: closing_item.get(),
                        },
                    )
                });
            }
            let source = id.clone();
            let permit = event_permit.clone();
            let tracker = navigation.clone();
            let burst = Cell::new((std::time::Instant::now(), 0u8));
            let last_blocked = Cell::new(None::<std::time::Instant>);
            let popup_sink = self.sink.clone();
            let popup_app_link = app_link.clone();
            builder = builder.with_new_window_req_handler(move |url, features| {
                if features.user_initiated && popup_app_link(&url) {
                    return wry::NewWindowResponse::Deny;
                }
                // Bound script-triggered bursts without delaying ordinary rapid
                // modifier-clicks. Physical controllers have a separate cap.
                let now = std::time::Instant::now();
                let report_blocked = || {
                    if last_blocked.get().is_none_or(|old| {
                        now.duration_since(old) >= std::time::Duration::from_secs(1)
                    }) {
                        last_blocked.set(Some(now));
                        permit.emit(
                            &popup_sink,
                            EngineEvent::PageOpenBlocked {
                                id: source.get(),
                                url: Some(url.clone()),
                            },
                        );
                    }
                };
                let (started, count) = burst.get();
                let (started, count) =
                    if now.duration_since(started) >= std::time::Duration::from_secs(1) {
                        (now, 0)
                    } else {
                        (started, count)
                    };
                if !features.user_initiated || count >= 8 {
                    report_blocked();
                    return wry::NewWindowResponse::Deny;
                }
                burst.set((started, count + 1));
                let Some(activity) = tracker.activity_snapshot() else {
                    report_blocked();
                    return wry::NewWindowResponse::Deny;
                };
                let response = super::dispatch::try_open_native_tab(
                    source.get(),
                    &permit,
                    activity,
                    &url,
                    features,
                );
                if matches!(response, wry::NewWindowResponse::Deny) {
                    report_blocked();
                }
                response
            });
        }

        // `scripts_for` prepends the protected host-owned registrations. Keep
        // that exact ordering so their captured intrinsics and observers are
        // installed before any caller-owned page content.
        #[cfg(target_os = "macos")]
        {
            let scope = site_scope.clone();
            builder = builder.with_main_frame_navigation_attempt_handler(move |target| {
                scope.navigating(&target)
            });
        }
        for script in wry_document_start_scripts(&scripts) {
            builder = builder.with_initialization_script_for_main_only(
                script.source.as_ref(),
                !script.all_frames,
            );
        }
        #[cfg(target_os = "macos")]
        {
            builder = builder.with_user_agent(crate::platform::imp::safari_user_agent());
        }

        #[cfg(target_os = "windows")]
        {
            use wry::WebViewBuilderExtWindows;
            let observed_environment = construction_environment.clone();
            let capture_failed = construction_environment_capture_failed.clone();
            builder = builder
                // Wry disables SmartScreen in its default argument set. Keep
                // only the browser-UI suppressions so content protection stays
                // enabled in the WebView2 runtime.
                .with_additional_browser_args("--disable-features=msWebOOUI,msPdfOOUI")
                .with_browser_accelerator_keys(false)
                // Runs after the exact environment exists and before Wry
                // starts controller construction. This closes the opaque-build
                // gap: even a later Wry error leaves a retained Environment5,
                // PID/generation, and exact process HANDLE obligation.
                .with_environment_created_handler(move |environment| {
                    let Ok(mut observed) = observed_environment.try_borrow_mut() else {
                        capture_failed.set(true);
                        return;
                    };
                    if observed.is_some() {
                        // The hook is an exactly-once construction stage. A
                        // duplicate callback cannot silently replace the
                        // environment whose process provenance was retained.
                        capture_failed.set(true);
                        return;
                    }
                    *observed = Some(environment.clone());
                });
            {
                let menu_permit = event_permit.clone();
                let menu_navigation = navigation.clone();
                let menu_presentation = presentation_permit.clone();
                builder = builder.with_native_context_menu_handler(move |controller, args| {
                    super::page_open::filter_windows_context_menu(
                        &menu_permit,
                        &menu_navigation,
                        &menu_presentation,
                        controller,
                        args,
                    )
                });
            }
            if let Some(downloads) = &self.downloads {
                let downloads = Rc::downgrade(downloads);
                let download_permit = event_permit.clone();
                let download_intent = download_surface_intent.clone();
                let download_navigation = navigation.clone();
                let initial_download = initial_download.clone();
                let download_item = id.clone();
                let download_sink = self.sink.clone();
                builder = builder
                    .with_download_policy(DownloadPolicy::UseHandlers)
                    .with_native_download_handler(move |controller, event| {
                        if let Some(manager) = downloads.upgrade() {
                            let initial = initial_download.replace(false)
                                && std::time::Instant::now() <= initial_download_deadline;
                            let started = initial.then(|| {
                                let item = download_item.get();
                                let permit = download_permit.clone();
                                let sink = download_sink.clone();
                                Box::new(move || {
                                    permit.emit(
                                        &sink,
                                        EngineEvent::LinkedDownloadStarted { id: item },
                                    )
                                }) as Box<dyn FnOnce()>
                            });
                            manager.admit(
                                partition,
                                download_permit.clone(),
                                download_intent.clone(),
                                download_navigation.clone(),
                                (controller, event),
                                started.map(|on_started| super::downloads::InitialDownload {
                                    on_started: Some(on_started),
                                }),
                            );
                        }
                    });
            }
            if let Some(environment) = cached_environment {
                builder = builder.with_environment(environment);
            }
            if let Some(startup) = extension_startup {
                builder = builder.with_browser_extension_startup_gate(move |environment, core| {
                    startup.authenticate(environment, core)
                });
            }
        }

        #[cfg(target_os = "macos")]
        {
            use wry::{WebViewBuilderExtDarwin, WebViewBuilderExtMacos};
            let permit = crash_permit.clone();
            let upload_broker = Rc::downgrade(&file_uploads);
            let crash_upload_broker = Rc::downgrade(&file_uploads);
            let drop_permit = event_permit.clone();
            let drop_navigation = navigation.clone();
            let drop_presentation = presentation_permit.clone();
            let drop_epoch = std::cell::Cell::new(None);
            let page_permission_item = id.clone();
            let page_permission_permit = event_permit.clone();
            let page_permission_navigation = navigation.clone();
            let page_permission_presence = page_permission_pending.clone();
            let profile = partition.profile();
            builder = builder
                // Link preview is a native WebKit UI/network surface outside
                // the popup broker. Keep it disabled until chrome can label
                // the origin and verify the initiating gesture.
                .with_allow_link_preview(false)
                .with_drag_drop_handler(move |event| {
                    let permitted = drop_permit.active_token().is_some()
                        && drop_presentation.load(Ordering::Acquire);
                    match event {
                        wry::DragDropEvent::Enter { .. } => {
                            let epoch = drop_navigation
                                .current_committed()
                                .filter(|epoch| permitted && drop_navigation.is_current(*epoch));
                            drop_epoch.set(epoch);
                            epoch.is_none()
                        }
                        wry::DragDropEvent::Drop { .. } => !drop_epoch
                            .take()
                            .is_some_and(|epoch| permitted && drop_navigation.is_current(epoch)),
                        wry::DragDropEvent::Over { .. } => !drop_epoch
                            .get()
                            .is_some_and(|epoch| permitted && drop_navigation.is_current(epoch)),
                        wry::DragDropEvent::Leave => {
                            drop_epoch.set(None);
                            false
                        }
                        _ => true,
                    }
                })
                .with_file_upload_handler(move |view, request, responder| {
                    if let Some(broker) = upload_broker.upgrade() {
                        broker.present(view, request, responder);
                    }
                })
                .with_permission_request_handler(move |request| {
                    super::page_permissions::admit_native_request(
                        profile,
                        page_permission_item.clone(),
                        page_permission_permit.clone(),
                        page_permission_navigation.clone(),
                        page_permission_presence.clone(),
                        request,
                    )
                })
                .with_on_web_content_process_terminate_handler(move || {
                    if let Some(broker) = crash_upload_broker.upgrade() {
                        broker.cancel();
                    }
                    let id = crash_id.get();
                    let queued_permit = permit.clone();
                    with_renderer_exit(id, move |host| {
                        host.on_renderer_process_exit(id, &queued_permit)
                    });
                });
            if let Some(downloads) = &self.downloads {
                let downloads = Rc::downgrade(downloads);
                let download_permit = event_permit.clone();
                let download_intent = download_surface_intent.clone();
                let download_navigation = navigation.clone();
                let initial_download = initial_download.clone();
                let download_item = id.clone();
                let download_sink = self.sink.clone();
                builder = builder
                    .with_download_policy(DownloadPolicy::UseHandlers)
                    .with_native_download_handler(move |native| {
                        if let Some(downloads) = downloads.upgrade() {
                            let initial = initial_download.replace(false)
                                && std::time::Instant::now() <= initial_download_deadline;
                            let started = initial.then(|| {
                                let item = download_item.get();
                                let permit = download_permit.clone();
                                let sink = download_sink.clone();
                                Box::new(move || {
                                    permit.emit(
                                        &sink,
                                        EngineEvent::LinkedDownloadStarted { id: item },
                                    )
                                }) as Box<dyn FnOnce()>
                            });
                            downloads.admit(
                                partition,
                                download_permit.clone(),
                                download_intent.clone(),
                                download_navigation.clone(),
                                native,
                                started.and_then(|on_started| {
                                    initial_download_window.clone().map(|window| {
                                        super::downloads::InitialDownload {
                                            window,
                                            on_started: Some(on_started),
                                        }
                                    })
                                }),
                            );
                        } else {
                            unsafe { native.cancel(None) };
                        }
                    });
            }
        }

        builder = match partition {
            Partition::Default(profile) | Partition::Persistent(profile) => {
                #[cfg(target_os = "macos")]
                {
                    use wry::WebViewBuilderExtDarwin;
                    builder.with_data_store_identifier(profile.bytes())
                }
                #[cfg(not(target_os = "macos"))]
                {
                    let _ = profile;
                    builder
                }
            }
            Partition::Ephemeral(_) => builder.with_incognito(true),
        };

        #[cfg(target_os = "macos")]
        let load_replay = replay_safety.clone();
        // The engine says why a navigation failed just before its Failed
        // event; chrome uses it to explain a failed address the person asked
        // for, in place of WebKit's blank or WebView2's own error page.
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        {
            let failure_permit = load_permit.clone();
            let failure_sink = on_load.clone();
            let failure_id = load_id.clone();
            builder = builder.with_navigation_failure_handler(move |_navigation, reason| {
                failure_permit.emit(
                    &failure_sink,
                    EngineEvent::NavigationFailureReported {
                        id: failure_id.get(),
                        reason: failure_reason(reason),
                    },
                );
            });
        }
        builder = builder.with_navigation_event_handler(move |event| {
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            if event.phase == wry::NavigationEventPhase::Committed && event.url != "about:blank" {
                initial_download.set(false);
            }
            let id = load_id.get();
            if event.phase == wry::NavigationEventPhase::Committed {
                // Wry already revoked this permit before its native hide. Do
                // it again at the public identity boundary so a future port
                // cannot accidentally weaken the stage-side invariant.
                load_presentation_permit.store(false, Ordering::Release);
            }
            let Some(transition) = load_navigation.observe_navigation(&event) else {
                return;
            };
            match transition {
                NavigationTransition::Started(epoch) => {
                    #[cfg(target_os = "macos")]
                    if let Some(broker) = load_file_uploads.upgrade() {
                        broker.cancel();
                    }
                    #[cfg(target_os = "macos")]
                    super::page_permissions::queue_navigation_revocation(
                        id,
                        &load_permit,
                        &load_navigation,
                        &load_page_permission_pending,
                    );
                    if load_navigation.is_current(epoch) {
                        load_permit
                            .emit(&on_load, EngineEvent::LoadingChanged { id, loading: true });
                    }
                }
                NavigationTransition::Redirected(_) => {
                    load_site_scope.navigating(&event.url);
                }
                NavigationTransition::Committed(epoch) => {
                    #[cfg(target_os = "macos")]
                    load_replay.commit();
                    load_site_scope.navigating(&event.url);
                    // This identity-bearing native commit, not URL equality or
                    // SourceChanged ordering, authorizes rendered-content
                    // attribution to the final redirect destination.
                    if load_navigation.is_current(epoch) {
                        queue_navigation_commit(id, &load_permit, &load_navigation, epoch);
                    }
                }
                NavigationTransition::Finished(epoch) => {
                    if load_navigation.is_current(epoch) {
                        load_permit
                            .emit(&on_load, EngineEvent::LoadingChanged { id, loading: false });
                        // Completion is a presentation signal, but it is not
                        // an attribution shortcut: the queued host task emits
                        // or verifies the exact committed URL before reveal.
                        queue_navigation_completion(id, &load_permit, &load_navigation, epoch);
                    }
                }
                NavigationTransition::Failed {
                    failed,
                    restored,
                    request,
                } => {
                    #[cfg(target_os = "macos")]
                    load_replay.abandon();
                    if let Some((_, url)) = load_navigation.committed_snapshot() {
                        load_site_scope.navigating(&url);
                    }
                    // The transition was current when accepted. End its
                    // loading state even when a provisional failure restored
                    // the still-visible previous committed document.
                    load_permit.emit(&on_load, EngineEvent::LoadingChanged { id, loading: false });
                    if let Some(request) = request {
                        load_permit.emit(&on_load, EngineEvent::NavigationFailed { id, request });
                    }
                    queue_navigation_failure(
                        id,
                        &load_permit,
                        &load_navigation,
                        failed,
                        restored,
                        event.phase == wry::NavigationEventPhase::Cancelled,
                    );
                }
            }
        });

        #[cfg(target_os = "windows")]
        if !self.construction_unproven.insert(partition.profile()) {
            eprintln!("engine: concurrent WebView2 construction debt for one profile");
            return None;
        }

        #[cfg(all(unix, not(target_os = "macos")))]
        let built = {
            use wry::WebViewBuilderExtUnix;
            match crate::platform::imp::container() {
                Some(container) => builder.build_gtk(&container),
                None => {
                    eprintln!("engine: gtk container not installed");
                    if report_failure {
                        event_permit
                            .emit(&self.sink, EngineEvent::ViewCreationFailed { id: id.get() });
                    }
                    return None;
                }
            }
        };
        #[cfg(not(all(unix, not(target_os = "macos"))))]
        let built = builder.build_as_child(&self.parent);

        #[cfg(target_os = "windows")]
        let captured_process = {
            use wry::WebViewExtWindows;
            let profile = partition.profile();
            let observed_environment = construction_environment
                .try_borrow_mut()
                .map(|mut environment| environment.take())
                .unwrap_or_else(|_| {
                    construction_environment_capture_failed.set(true);
                    None
                })
                // Defensive fallback for a future Wry refactor that returns a
                // view without invoking the pre-controller hook. A successful
                // build must still never escape native obligation capture.
                .or_else(|| built.as_ref().ok().map(|view| view.environment()));
            if construction_environment_capture_failed.get() {
                self.quarantine_unverifiable_windows_profile(profile);
                eprintln!("engine: WebView2 environment construction hook was not exactly once");
                if report_failure {
                    event_permit.emit(&self.sink, EngineEvent::ViewCreationFailed { id: id.get() });
                }
                return None;
            }
            let Some(environment) = observed_environment else {
                // The environment-completion hook is before controller
                // construction. If it did not run, Wry never returned an
                // environment and its construction guard owns any HWND cleanup.
                // No browser-process identity existed for Zephium to retain.
                if built.is_err() {
                    self.construction_unproven.remove(&profile);
                } else {
                    self.quarantine_unverifiable_windows_profile(profile);
                }
                eprintln!("engine: WebView2 construction exposed no environment obligation");
                if report_failure {
                    event_permit.emit(&self.sink, EngineEvent::ViewCreationFailed { id: id.get() });
                }
                return None;
            };
            let captured = self.capture_windows_environment(profile, environment);
            // From this point, successful capture is represented by the exact
            // environment/process/observer maps; failed capture is represented
            // by sticky unproven/unverifiable obligations. The temporary marker
            // is no longer needed in either case.
            self.construction_unproven.remove(&profile);
            match captured {
                Ok(captured) => captured,
                Err(error) => {
                    self.quarantine_unverifiable_windows_profile(profile);
                    eprintln!("engine: cannot retain early WebView2 process obligation: {error}");
                    if report_failure {
                        event_permit
                            .emit(&self.sink, EngineEvent::ViewCreationFailed { id: id.get() });
                    }
                    return None;
                }
            }
        };

        let view = match built {
            Ok(view) => view,
            Err(e) => {
                #[cfg(all(unix, not(target_os = "macos")))]
                if untracked_manager_on_build_failure {
                    // An incognito build owns an inaccessible per-view
                    // context; a first durable build owns the still-local
                    // context. Wry may fail after native construction, so no
                    // later retry may infer manager absence from this error.
                    self.linux_unverifiable_data_managers
                        .insert(partition.profile());
                }
                eprintln!("engine: build_view failed: {e}");
                if report_failure {
                    event_permit.emit(&self.sink, EngineEvent::ViewCreationFailed { id: id.get() });
                }
                return None;
            }
        };
        // Lets Safari's Web Inspector reach tab pages and the content scripts
        // running in them while diagnosing extensions.
        #[cfg(target_os = "macos")]
        if zephium_webext_macos::tracing() {
            unsafe { crate::platform::imp::native_webview(&view).setInspectable(true) };
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            // Capture before every fallible post-build step. In particular,
            // storage attestation, observer installation, and initial load
            // are not allowed to discard the only native erasure handle.
            let obligation = crate::platform::imp::website_data_manager_obligation(&view);
            self.retain_linux_data_manager_obligation(partition.profile(), obligation);
            if let Some(context) = pending_web_context.take() {
                match self.web_contexts.entry(partition.profile()) {
                    std::collections::hash_map::Entry::Vacant(entry) => {
                        entry.insert(context);
                    }
                    std::collections::hash_map::Entry::Occupied(_) => {
                        self.linux_unverifiable_data_managers
                            .insert(partition.profile());
                        eprintln!(
                            "privacy: Linux profile context changed during native construction"
                        );
                        return None;
                    }
                }
            }
        }
        // Cross-check the controller's process identity against the environment
        // captured before controller construction. A mismatched value is a
        // terminal provenance failure, never a new generation to adopt.
        #[cfg(target_os = "windows")]
        let (browser_process_id, browser_process_generation) = {
            let profile = partition.profile();
            let controller_process = match crate::platform::imp::browser_process(&view) {
                Ok(process) => process,
                Err(error) => {
                    self.quarantine_unverifiable_windows_profile(profile);
                    eprintln!("engine: cannot cross-check WebView2 controller process: {error}");
                    if report_failure {
                        event_permit
                            .emit(&self.sink, EngineEvent::ViewCreationFailed { id: id.get() });
                    }
                    return None;
                }
            };
            if controller_process.id() != captured_process.0 {
                self.unproven_browser_processes
                    .entry(profile)
                    .or_insert(controller_process);
                self.quarantine_unverifiable_windows_profile(profile);
                eprintln!("engine: controller process did not match its captured environment");
                if report_failure {
                    event_permit.emit(&self.sink, EngineEvent::ViewCreationFailed { id: id.get() });
                }
                return None;
            }
            captured_process
        };
        #[cfg(target_os = "windows")]
        if let Err(error) = self
            .environments
            .get(&partition.profile())
            .ok_or_else(|| {
                windows_core::Error::new(
                    windows::Win32::Foundation::E_UNEXPECTED,
                    "captured WebView2 environment disappeared before attestation",
                )
            })
            .and_then(|environment| {
                crate::platform::imp::attest_environment(environment, &expected_user_data_folder)
            })
        {
            self.quarantine_unverifiable_windows_profile(partition.profile());
            eprintln!("security: content WebView2 environment attestation failed: {error}");
            if report_failure {
                event_permit.emit(&self.sink, EngineEvent::ViewCreationFailed { id: id.get() });
            }
            return None;
        }
        #[cfg(target_os = "windows")]
        let mut security_policy = match crate::platform::imp::configure(
            &view,
            12.0,
            matches!(partition, Partition::Ephemeral(_)),
            &expected_user_data_folder,
        ) {
            Ok(policy) => policy,
            Err(error) => {
                // The exact environment was already attested above. Failures
                // from here are scoped to this new controller
                // (settings/handlers/profile postconditions); dropping Wry's
                // construction result closes it. A transient registration
                // failure must not poison healthy sibling controllers.
                eprintln!("security: content WebView2 controller hardening failed: {error}");
                if report_failure {
                    event_permit.emit(&self.sink, EngineEvent::ViewCreationFailed { id: id.get() });
                }
                return None;
            }
        };
        #[cfg(target_os = "windows")]
        {
            let link = app_link.clone();
            if let Err(error) = security_policy.route_external_uris(move |uri| {
                link(uri);
            }) {
                eprintln!("security: content WebView2 app-link routing failed: {error}");
                if report_failure {
                    event_permit.emit(&self.sink, EngineEvent::ViewCreationFailed { id: id.get() });
                }
                return None;
            }
        }
        #[cfg(target_os = "windows")]
        {
            use wry::WebViewExtWindows;
            // configure() deliberately denies menus for generic/agent views.
            // Only this human-view constructor installed both the bounded
            // ContextMenuRequested filter and the SaveAsUIShowing denial hook.
            // Tabs a person reads also get WebView2's own page dialogs, which
            // name the origin that asks; agent views keep them denied.
            if unsafe { view.webview().Settings() }
                .and_then(|settings| unsafe {
                    settings.SetAreDefaultContextMenusEnabled(true)?;
                    settings.SetAreDefaultScriptDialogsEnabled(true)
                })
                .is_err()
            {
                if report_failure {
                    event_permit.emit(&self.sink, EngineEvent::ViewCreationFailed { id: id.get() });
                }
                return None;
            }
            debug_assert!(!self.construction_unproven.contains(&partition.profile()));
        }
        #[cfg(target_os = "macos")]
        if let Err(error) = crate::platform::imp::configure(
            &view,
            12.0,
            partition,
            expected_ephemeral_data_store.as_ref(),
        ) {
            eprintln!("security: content WKWebView storage attestation failed: {error}");
            if report_failure {
                event_permit.emit(&self.sink, EngineEvent::ViewCreationFailed { id: id.get() });
            }
            return None;
        }
        #[cfg(target_os = "windows")]
        let request_witness = crate::platform::imp::RequestWitness::install(
            wry::WebViewExtWindows::webview(&view),
            !report_failure && !native_popup,
        )
        .ok();
        #[cfg(all(unix, not(target_os = "macos")))]
        if let Err(error) = crate::platform::imp::configure(
            &view,
            12.0,
            partition,
            expected_website_data_directory.as_deref(),
        ) {
            // The retained manager did not prove its persistence mode and
            // direct owned path. Keep its handle, but permanently deny disk
            // deletion for this profile rather than clearing an unknown
            // manager or allowing a later valid view to erase the debt.
            self.linux_unverifiable_data_managers
                .insert(partition.profile());
            eprintln!("security: content WebKitGTK storage attestation failed: {error}");
            if report_failure {
                event_permit.emit(&self.sink, EngineEvent::ViewCreationFailed { id: id.get() });
            }
            return None;
        }
        // Native hardening and profile-storage attestation must precede
        // content-policy registration, while registration must precede every
        // observer and the first network-producing load. This ordering is the
        // first-navigation protection boundary.
        let content_policy_registration =
            match crate::platform::imp::install_scoped_content_policy_on_view(
                &view,
                &content_policy,
                &site_scope.pause,
            ) {
                Ok(registration) => registration,
                Err(error) => {
                    eprintln!("content blocker: native view policy installation failed: {error:?}");
                    if report_failure {
                        event_permit
                            .emit(&self.sink, EngineEvent::ViewCreationFailed { id: id.get() });
                    }
                    return None;
                }
            };
        #[cfg(target_os = "windows")]
        let process_failure_permit = crash_permit.clone();
        #[cfg(target_os = "windows")]
        let crash_observer =
            match crate::platform::imp::install_crash_handler(&view, move |failure| match failure {
                crate::platform::imp::ProcessFailure::Renderer => {
                    let id = crash_id.get();
                    let queued_permit = process_failure_permit.clone();
                    with_renderer_exit(id, move |host| {
                        host.on_renderer_process_exit(id, &queued_permit)
                    });
                }
                crate::platform::imp::ProcessFailure::Browser => {
                    let profile = partition.profile();
                    with_profile_exit(profile, browser_process_generation, move |host| {
                        host.on_profile_process_exit(
                            profile,
                            browser_process_id,
                            browser_process_generation,
                        );
                    });
                }
            }) {
                Ok(observer) => observer,
                Err(error) => {
                    eprintln!("engine: required WebView2 process-failure handler failed: {error}");
                    if report_failure {
                        event_permit
                            .emit(&self.sink, EngineEvent::ViewCreationFailed { id: id.get() });
                    }
                    return None;
                }
            };
        // A dead web process must surface as an event, never as a silently
        // blank pane; the shell decides whether to relaunch.
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            use webkit2gtk::WebViewExt;
            use wry::WebViewExtUnix;
            let permit = crash_permit.clone();
            view.webview()
                .connect_web_process_terminated(move |_, reason| {
                    eprintln!("engine: web process terminated: {reason:?}");
                    let id = crash_id.get();
                    let queued_permit = permit.clone();
                    with_renderer_exit(id, move |host| {
                        host.on_renderer_process_exit(id, &queued_permit)
                    });
                });
        }
        #[cfg(target_os = "windows")]
        let shortcut_item = id.clone();
        #[cfg(target_os = "windows")]
        let accelerator_permit = event_permit.clone();
        #[cfg(target_os = "windows")]
        let accelerator_sink = self.sink.clone();
        #[cfg(target_os = "windows")]
        let accelerator_registration = match crate::platform::imp::install_accelerators(
            &view,
            self.shortcuts.clone(),
            Arc::new(move |event| accelerator_permit.emit(&accelerator_sink, event)),
            move || shortcut_item.get(),
        ) {
            Ok(registration) => registration,
            Err(error) => {
                eprintln!("engine: required accelerator registration failed: {error}");
                if report_failure {
                    event_permit.emit(&self.sink, EngineEvent::ViewCreationFailed { id: id.get() });
                }
                return None;
            }
        };
        #[cfg(any(target_os = "macos", all(unix, not(target_os = "macos"))))]
        for script in scripts
            .iter()
            .filter(|s| !(s.world == World::Page && s.run_at == RunAt::DocumentStart))
        {
            if let Err(error) = crate::platform::imp::add_user_script(&view, script) {
                eprintln!(
                    "engine: required user script {} was refused: {error}",
                    script.id
                );
                if report_failure {
                    event_permit.emit(&self.sink, EngineEvent::ViewCreationFailed { id: id.get() });
                }
                return None;
            }
        }

        let observation_id = id.clone();
        #[cfg(target_os = "macos")]
        let capture_observer = {
            let capture_id = id.clone();
            let capture_permit = event_permit.clone();
            let capture_navigation = navigation.clone();
            let capture_sink = self.sink.clone();
            crate::platform::macos::capture::CaptureObserver::install(&view, move |state| {
                if let Some(epoch) = capture_navigation.resident_media_epoch() {
                    capture_permit.emit(
                        &capture_sink,
                        EngineEvent::MediaCaptureChanged {
                            id: capture_id.get(),
                            navigation: epoch.presentation_id(),
                            state,
                        },
                    );
                }
            })
        };
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        let fullscreen_observer = {
            let fullscreen_id = id.clone();
            let fullscreen_permit = event_permit.clone();
            let changed = move || {
                if fullscreen_permit.active_token().is_none() {
                    return;
                }
                let id = fullscreen_id.get();
                let queued_permit = fullscreen_permit.clone();
                with_fullscreen_observation(id, move |host| {
                    host.native_fullscreen_changed(id, &queued_permit);
                });
            };
            #[cfg(target_os = "macos")]
            {
                crate::platform::macos::fullscreen::FullscreenObserver::install(&view, changed)
            }
            #[cfg(target_os = "windows")]
            match crate::platform::imp::install_fullscreen_observer(&view, changed) {
                Ok(observer) => observer,
                Err(error) => {
                    eprintln!("engine: required fullscreen observer failed: {error}");
                    if report_failure {
                        event_permit
                            .emit(&self.sink, EngineEvent::ViewCreationFailed { id: id.get() });
                    }
                    return None;
                }
            }
        };
        let observation_permit = event_permit.clone();
        let observation_navigation = navigation.clone();
        let observer = match crate::platform::imp::install_navigation_observer(
            &view,
            move |#[cfg(target_os = "macos")] _| {
                if observation_permit.active_token().is_none() {
                    return;
                }
                let Some(epoch) = observation_navigation.current() else {
                    return;
                };
                let id = observation_id.get();
                let queued_permit = observation_permit.clone();
                let queued_navigation = observation_navigation.clone();
                with_source_observation(id, move |host| {
                    host.emit_navigation_observation(id, &queued_permit, &queued_navigation, epoch);
                });
            },
        ) {
            Ok(observer) => observer,
            Err(error) => {
                eprintln!("engine: required native navigation observer failed: {error}");
                if report_failure {
                    event_permit.emit(&self.sink, EngineEvent::ViewCreationFailed { id: id.get() });
                }
                return None;
            }
        };
        // Linux uses a stronger native mapping barrier than Wry's generic
        // visibility API: the Stage performs its first offscreen map and is
        // the only component allowed to restore paint and input. This is also
        // why an unstaged Linux warm spare remains disabled.
        #[cfg(not(all(unix, not(target_os = "macos"))))]
        let _ = view.set_visible(false);
        if !event_permit.allows_navigation(url) {
            // WebView construction can pump WebView2 messages. Do not perform
            // the first content load after the outer generation was retired.
            return None;
        }
        let Some(epoch) = navigation.begin(url) else {
            eprintln!("engine: could not establish initial navigation epoch");
            if report_failure {
                event_permit.emit(&self.sink, EngineEvent::ViewCreationFailed { id: id.get() });
            }
            return None;
        };
        #[cfg(target_os = "macos")]
        let restored = self.restore_discarded_state(
            id.get(),
            partition,
            url,
            &view,
            &event_permit,
            &navigation,
        );
        #[cfg(not(target_os = "macos"))]
        let restored: Option<bool> = None;
        if let Err(error) = if native_popup {
            Ok(())
        } else {
            match restored {
                Some(true) => Ok(()),
                #[cfg(target_os = "macos")]
                Some(false) => Err(wry::Error::NativeObjectUnavailable(
                    "native session restoration",
                )),
                _ => view.load_url(url),
            }
        } {
            navigation.fail_synchronous(epoch);
            eprintln!("engine: initial navigation failed: {error}");
            if report_failure {
                event_permit.emit(&self.sink, EngineEvent::ViewCreationFailed { id: id.get() });
            }
            return None;
        }
        if !event_permit.allows_navigation(url) {
            // load_url itself may pump. Dropping here removes the controller
            // before it can be inserted into the live host maps.
            return None;
        }
        Some(ObservedView {
            #[cfg(target_os = "macos")]
            _capture_observer: capture_observer,
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            _fullscreen_observer: fullscreen_observer,
            replay_safety,
            discard_probe_lease: std::cell::RefCell::new(None),
            #[cfg(target_os = "macos")]
            final_discard: None,
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            discard_settle_timer: std::cell::RefCell::new(None),
            #[cfg(target_os = "windows")]
            request_witness,
            #[cfg(target_os = "windows")]
            windows_final_discard: None,
            #[cfg(target_os = "windows")]
            discard_deadline: None,
            #[cfg(target_os = "windows")]
            suspend_deadline: None,
            #[cfg(target_os = "windows")]
            suspend_attempt: None,
            site_scope,
            content_styles: Arc::new(super::content_styles::DocumentStyleState::default()),
            #[cfg(target_os = "macos")]
            file_uploads,
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            download_surface_intent,
            event_permit,
            navigation,
            presentation_permit,
            applied_zoom: 1.0,
            presentable: false,
            presentation_announced: None,
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            paint_cover: None,
            title_ready: None,
            nonpresentable_bootstrap: (!report_failure && !native_popup).then_some(epoch),
            #[cfg(target_os = "windows")]
            _crash_observer: crash_observer,
            #[cfg(target_os = "windows")]
            _accelerator_registration: accelerator_registration,
            #[cfg(target_os = "windows")]
            _security_policy: security_policy,
            content_policy_registration: Some(content_policy_registration),
            _observer: observer,
            #[cfg(target_os = "windows")]
            cleanup_profile: partition.profile(),
            #[cfg(target_os = "windows")]
            native_close_attempted: false,
            #[cfg(target_os = "windows")]
            native_terminal_failure: self.native_terminal_failure.clone(),
            find: None,
            view,
            native_resource: None,
        })
    }
}

/// Why a profile's WebView2 environment could not be established.
#[cfg(target_os = "windows")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum WindowsProfileEnvironmentFailure {
    /// Admission is closed or another construction holds the profile.
    Busy,
    /// The retained environment belongs to a different storage root.
    Mismatch,
    /// The page-inert bootstrap controller failed to build or close.
    Construction,
}

/// A page-inert, counted native owner that bridges environment bootstrap
/// into real Work controller construction. It never loads a site or survives
/// the synchronous constructor; failed close retains the exact native debt.
#[cfg(target_os = "windows")]
pub(super) struct WindowsWorkEnvironmentBootstrap {
    view: wry::WebView,
    profile: zephium_core::ids::ProfileId,
    resource: Option<super::resources::NativeResourceLease>,
}
#[cfg(target_os = "windows")]
impl Drop for WindowsWorkEnvironmentBootstrap {
    fn drop(&mut self) {
        if let Err(debt) = wry::WebViewExtWindows::close(&mut self.view) {
            super::queue_windows_cleanup_debt(
                self.profile,
                super::OwnedWindowsCleanupDebt::new(debt, self.resource.take()),
            );
        }
    }
}

#[cfg(target_os = "windows")]
impl EngineHost {
    /// Establishes or rejoins a profile's environment at one exact
    /// already-authorized storage root. Without a live environment, one
    /// short-lived, page-inert controller is created solely to obtain it and
    /// is closed before this function returns.
    pub(super) fn ensure_windows_profile_environment_at_path(
        &mut self,
        profile: zephium_core::ids::ProfileId,
        deadline: std::time::Instant,
        path: std::path::PathBuf,
        extensions_enabled: bool,
    ) -> Result<(), WindowsProfileEnvironmentFailure> {
        self.ensure_windows_profile_environment_impl(
            profile,
            deadline,
            path,
            extensions_enabled,
            false,
        )
        .map(|_| ())
    }

    /// Keep the exact bootstrap generation alive until a Work controller owns
    /// the same environment. All other callers retain immediate close behavior.
    #[cfg_attr(
        all(target_os = "windows", not(feature = "agentic-browser")),
        allow(dead_code)
    )]
    pub(super) fn begin_windows_work_profile_environment(
        &mut self,
        profile: zephium_core::ids::ProfileId,
        deadline: std::time::Instant,
        path: std::path::PathBuf,
        extensions_enabled: bool,
    ) -> Result<Option<WindowsWorkEnvironmentBootstrap>, WindowsProfileEnvironmentFailure> {
        self.ensure_windows_profile_environment_impl(
            profile,
            deadline,
            path,
            extensions_enabled,
            true,
        )
    }

    fn ensure_windows_profile_environment_impl(
        &mut self,
        profile: zephium_core::ids::ProfileId,
        deadline: std::time::Instant,
        path: std::path::PathBuf,
        extensions_enabled: bool,
        retain_bootstrap: bool,
    ) -> Result<Option<WindowsWorkEnvironmentBootstrap>, WindowsProfileEnvironmentFailure> {
        self.collect_pending_windows_cleanup_debts();
        if std::time::Instant::now() >= deadline
            || self.windows_view_admission_blocked(profile)
            || !self.windows_profile_process_group_capacity_allows(profile)
        {
            return Err(WindowsProfileEnvironmentFailure::Busy);
        }
        let startup = if extensions_enabled {
            Some(
                self.windows_extension_startup(profile, &path)
                    .map_err(|_| WindowsProfileEnvironmentFailure::Mismatch)?,
            )
        } else {
            None
        };
        let existing_environment = self.environments.get(&profile).cloned();
        if let Some(environment) = &existing_environment {
            if extensions_enabled
                && startup
                    .as_ref()
                    .is_none_or(|startup| !startup.initialized.get())
            {
                return Err(WindowsProfileEnvironmentFailure::Mismatch);
            }
            crate::platform::imp::attest_environment(environment, &path)
                .map_err(|_| WindowsProfileEnvironmentFailure::Mismatch)?;
            if !retain_bootstrap {
                return Ok(None);
            }
            let prior_exit_settled =
                self.settle_idle_windows_work_generation(profile, environment, None, deadline);
            // An empty controller cohort need not appear in GetProcessInfos.
            // Rejoin only the retained running HANDLE and exact pending exit
            // generation; the new bootstrap must then recapture that identity.
            let current = self.browser_processes.get(&profile).is_some_and(|process| {
                self.browser_process_exit_observers
                    .get(&profile)
                    .is_some_and(|observer| {
                        process.is_running()
                            && observer.is_pending()
                            && observer.expected_process_id() == process.id()
                            && self.browser_version_observers.contains_key(&profile)
                    })
            });
            if !current && !prior_exit_settled {
                return Err(WindowsProfileEnvironmentFailure::Busy);
            }
        }
        let mut construction_resource = Some(
            self.native_resources
                .try_acquire(NativeResourceClass::TransientConstruction)
                .map_err(|error| {
                    if error == NativeResourceAdmissionError::AccountingInvariant {
                        self.native_resource_accounting_failed = true;
                    }
                    WindowsProfileEnvironmentFailure::Busy
                })?,
        );
        let observed_environment: Rc<RefCell<Option<ICoreWebView2Environment>>> =
            Rc::new(RefCell::new(None));
        let capture_failed = Rc::new(Cell::new(false));
        let parent = super::ParentHandle(self.parent.0);

        let api_path = crate::platform::windows::webview2_user_data_path(&path)
            .map_err(|_| WindowsProfileEnvironmentFailure::Mismatch)?;
        if !self.construction_unproven.insert(profile) {
            return Err(WindowsProfileEnvironmentFailure::Busy);
        }
        let builder = {
            let observed = observed_environment.clone();
            let capture_failed = capture_failed.clone();
            let context = self
                .web_contexts
                .entry(profile)
                .or_insert_with(|| wry::WebContext::new(Some(api_path)));
            let builder = WebViewBuilder::new_with_web_context(context)
                .with_bounds(wry::Rect {
                    position: Position::Logical(LogicalPosition::new(0.0, 0.0)),
                    size: Size::Logical(LogicalSize::new(1.0, 1.0)),
                })
                .with_visible(false)
                .with_focused(false)
                .with_devtools(false)
                .with_autoplay(false)
                .with_fullscreen_enabled(false)
                .with_picture_in_picture_enabled(false)
                .with_general_autofill_enabled(false)
                .with_navigation_handler(|target| target == "about:blank")
                .with_new_window_req_handler(|_, _| wry::NewWindowResponse::Deny)
                .with_permission_handler(|_| wry::PermissionResponse::Deny)
                .with_download_policy(DownloadPolicy::DenyWithoutMetadata)
                .with_page_close_policy(wry::PageClosePolicy::Ignore);
            use wry::WebViewBuilderExtWindows;
            let builder = builder
                .with_additional_browser_args("--disable-features=msWebOOUI,msPdfOOUI")
                .with_browser_accelerator_keys(false)
                .with_environment_created_handler(move |environment| {
                    let Ok(mut slot) = observed.try_borrow_mut() else {
                        capture_failed.set(true);
                        return;
                    };
                    if slot.is_some() {
                        capture_failed.set(true);
                        return;
                    }
                    *slot = Some(environment.clone());
                });
            let builder = if let Some(environment) = existing_environment {
                builder.with_environment(environment)
            } else {
                builder
            };
            if let Some(startup) = startup {
                builder.with_browser_extension_startup_gate(move |environment, core| {
                    startup.authenticate(environment, core)
                })
            } else {
                builder
            }
        };

        let mut built = builder.build_as_child(&parent);
        #[cfg(feature = "native-agentic-work-lifetime-diagnostic")]
        {
            use wry::WebViewExtWindows;
            let mut bootstrap_pid = 0;
            let bootstrap_pid = built.as_ref().ok().and_then(|view| {
                // SAFETY: this diagnostic reads the exact live bootstrap core
                // on its owning apartment; no process authority is changed.
                unsafe { view.webview().BrowserProcessId(&mut bootstrap_pid) }
                    .ok()
                    .map(|()| bootstrap_pid)
            });
            let live_work_views = self
                .work_resources
                .values()
                .filter(|resource| {
                    resource
                        .view
                        .as_ref()
                        .is_some_and(|view| view.work_native_profile() == Some(profile))
                })
                .count();
            eprintln!("windows-work-construction: stage=bootstrap_process bootstrap_pid={bootstrap_pid:?} retained_pid={:?} live_work_views={live_work_views}; content=redacted", self.browser_processes.get(&profile).map(|process| process.id()));
        }
        if let Err(error) = &built {
            eprintln!("extensions: native bootstrap build failed: {error}");
        }
        let environment = observed_environment
            .try_borrow_mut()
            .map(|mut environment| environment.take())
            .unwrap_or_else(|_| {
                capture_failed.set(true);
                None
            })
            .or_else(|| built.as_ref().ok().map(wry::WebViewExtWindows::environment));
        if capture_failed.get() {
            self.quarantine_unverifiable_windows_profile(profile);
        }
        let mut failure = environment
            .ok_or(WindowsProfileEnvironmentFailure::Construction)
            .and_then(|environment| {
                crate::platform::imp::attest_environment(&environment, &path)
                    .map_err(|_| WindowsProfileEnvironmentFailure::Mismatch)?;
                let captured = if retain_bootstrap {
                    self.capture_windows_environment_impl(profile, environment, Some(deadline))
                } else {
                    self.capture_windows_environment(profile, environment)
                };
                captured
                    .map(|_| ())
                    .map_err(|_| WindowsProfileEnvironmentFailure::Construction)
            })
            .err();
        self.construction_unproven.remove(&profile);

        let construction_debts = wry::pending_webview2_cleanup_debts();
        if !construction_debts.is_empty() {
            if built.is_ok() || construction_debts.len() != 1 {
                self.fail_windows_cleanup_invariant();
            }
            for debt in construction_debts {
                let resource = construction_resource.take().or_else(|| {
                    self.native_resources
                        .try_acquire(NativeResourceClass::TeardownDebt)
                        .ok()
                });
                let debt = super::OwnedWindowsCleanupDebt::new(debt, resource);
                if !debt.accounted_as_debt() {
                    self.native_resource_accounting_failed = true;
                }
                self.retain_windows_cleanup_debt(profile, debt);
            }
        }
        if wry::webview2_cleanup_overflowed() {
            self.fail_windows_cleanup_invariant();
        }
        self.collect_pending_windows_cleanup_debts();

        if let Ok(view) = built.as_mut() {
            if failure.is_none() {
                if let Err(error) = crate::platform::imp::configure(view, 0.0, false, &path) {
                    eprintln!("extensions: native bootstrap hardening failed: {error:?}");
                    failure = Some(WindowsProfileEnvironmentFailure::Construction);
                }
            }
            if retain_bootstrap && failure.is_none() && std::time::Instant::now() < deadline {
                let view = built.map_err(|_| WindowsProfileEnvironmentFailure::Construction)?;
                return Ok(Some(WindowsWorkEnvironmentBootstrap {
                    view,
                    profile,
                    resource: construction_resource.take(),
                }));
            }
            if let Err(debt) = wry::WebViewExtWindows::close(view) {
                let debt = super::OwnedWindowsCleanupDebt::new(debt, construction_resource.take());
                if !debt.accounted_as_debt() {
                    self.native_resource_accounting_failed = true;
                }
                self.retain_windows_cleanup_debt(profile, debt);
                failure = Some(WindowsProfileEnvironmentFailure::Construction);
            }
        } else {
            failure.get_or_insert(WindowsProfileEnvironmentFailure::Construction);
        }
        drop(built);
        drop(construction_resource.take());
        if let Some(failure) = failure {
            return Err(failure);
        }
        if std::time::Instant::now() >= deadline {
            return Err(WindowsProfileEnvironmentFailure::Busy);
        }
        Ok(None)
    }
}

fn wry_document_start_scripts(scripts: &[UserScript]) -> impl Iterator<Item = &UserScript> {
    scripts
        .iter()
        .filter(|script| script.world == World::Page && script.run_at == RunAt::DocumentStart)
}

fn to_wry(r: Rect) -> wry::Rect {
    wry::Rect {
        position: Position::Logical(LogicalPosition::new(r.x, r.y)),
        size: Size::Logical(LogicalSize::new(r.width, r.height)),
    }
}

pub(super) fn raw_content_permission(kind: wry::PermissionKind) -> wry::PermissionResponse {
    match kind {
        wry::PermissionKind::Camera | wry::PermissionKind::Microphone if cfg!(windows) => {
            wry::PermissionResponse::Prompt
        }
        _ => wry::PermissionResponse::Deny,
    }
}

#[cfg(test)]
mod tests;

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn failure_reason(failure: wry::NavigationFailure) -> NavigationFailureReason {
    match failure {
        wry::NavigationFailure::Offline => NavigationFailureReason::Offline,
        wry::NavigationFailure::HostNotFound => NavigationFailureReason::HostNotFound,
        wry::NavigationFailure::Unreachable => NavigationFailureReason::Unreachable,
        wry::NavigationFailure::TimedOut => NavigationFailureReason::TimedOut,
        wry::NavigationFailure::Insecure => NavigationFailureReason::Insecure,
        wry::NavigationFailure::Other => NavigationFailureReason::Other,
    }
}
