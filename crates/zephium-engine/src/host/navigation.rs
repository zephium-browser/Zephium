use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use zephium_core::ids::ItemId;
use zephium_core::navigation;
use zephium_core::ports::engine::{
    EngineEvent, NativeAction, NavigationPresentationId, NavigationRequestId,
};

use crate::navigation_epoch::{NavigationEpoch, NavigationEpochTracker};

use super::permits::{navigation_callback_matches, EventPermit};
use super::{EngineHost, NavigationSnapshot};

pub(super) fn bounded_title(title: &str) -> String {
    zephium_core::item::sanitize_page_title(title)
}

#[derive(Debug, PartialEq, Eq)]
enum ObservedUrl {
    Unavailable,
    Allowed(String),
    Forbidden,
}

fn classify_observed_url(url: Option<String>) -> ObservedUrl {
    match url {
        None => ObservedUrl::Unavailable,
        Some(url) if url.is_empty() => ObservedUrl::Unavailable,
        Some(url) if navigation::is_browser_target_str(&url) => ObservedUrl::Allowed(url),
        Some(_) => ObservedUrl::Forbidden,
    }
}

fn resolve_committed_observed_url(
    navigation: &NavigationEpochTracker,
    epoch: NavigationEpoch,
    url: Option<String>,
) -> ObservedUrl {
    match classify_observed_url(url) {
        ObservedUrl::Unavailable => navigation
            .committed_snapshot()
            .filter(|(committed, _)| *committed == epoch)
            .map_or(ObservedUrl::Unavailable, |(_, target)| {
                ObservedUrl::Allowed(target)
            }),
        ObservedUrl::Allowed(url) => {
            if !navigation.observe_source(epoch, &url) {
                return ObservedUrl::Unavailable;
            }
            navigation
                .committed_snapshot()
                .filter(|(committed, _)| *committed == epoch)
                .map_or(ObservedUrl::Unavailable, |(_, target)| {
                    ObservedUrl::Allowed(target)
                })
        }
        ObservedUrl::Forbidden => ObservedUrl::Forbidden,
    }
}

fn navigation_observation_events(
    id: ItemId,
    previous: &mut NavigationSnapshot,
    url: Option<&str>,
    history: Option<(bool, bool)>,
) -> Vec<EngineEvent> {
    // A single native notification produces at most these two bounded events,
    // and duplicate Source/History/KVO notifications become no-ops.
    let mut events = Vec::with_capacity(2);
    if let Some(url) = url.filter(|url| navigation::is_browser_target_str(url)) {
        if previous.url.as_deref() != Some(url) {
            let url = url.to_owned();
            previous.url = Some(url.clone());
            events.push(EngineEvent::UrlChanged { id, url });
        }
    }
    if let Some((can_go_back, can_go_forward)) = history {
        if previous.history != Some((can_go_back, can_go_forward)) {
            previous.history = Some((can_go_back, can_go_forward));
            events.push(EngineEvent::NavState {
                id,
                can_go_back,
                can_go_forward,
            });
        }
    }
    events
}

fn restored_navigation_can_present(
    already_presentable: bool,
    nonpresentable_bootstrap: Option<NavigationEpoch>,
    restored: NavigationEpoch,
) -> bool {
    already_presentable || nonpresentable_bootstrap != Some(restored)
}

impl EngineHost {
    /// Re-arm the native presentation gate for one exact identity-bearing
    /// main-frame commit. Provisional loads leave the prior document visible;
    /// this transition runs only at commit, before the new document is allowed
    /// to borrow the prior epoch's chrome acknowledgement.
    pub(super) fn rearm_navigation_presentation(
        &mut self,
        id: ItemId,
        source_permit: &EventPermit,
        source_navigation: &NavigationEpochTracker,
        epoch: NavigationEpoch,
    ) -> bool {
        let Some((needs_rearm, token)) = self.views.get(&id).map(|view| {
            (
                navigation_callback_matches(
                    &view.event_permit,
                    &view.navigation,
                    source_permit,
                    source_navigation,
                    epoch,
                ) && view.navigation.current_committed() == Some(epoch)
                    && view.presentation_announced != Some(epoch),
                view.event_permit.active_token(),
            )
        }) else {
            return false;
        };
        if !needs_rearm {
            return self.views.get(&id).is_some_and(|view| {
                navigation_callback_matches(
                    &view.event_permit,
                    &view.navigation,
                    source_permit,
                    source_navigation,
                    epoch,
                ) && view.navigation.current_committed() == Some(epoch)
            });
        }

        if let Some(view) = self.views.get_mut(&id) {
            // Change the Rust-owned authority before any native call can pump
            // callbacks. A re-entrant acknowledgement for the previous epoch
            // therefore cannot reveal this committed document.
            view.presentable = false;
            view.presentation_permit.store(false, Ordering::Release);
            view.title_ready = None;
        }
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        self.fullscreen_document_committed(id);
        let mut stages_pending = true;
        for stage in self.stages.values() {
            // Do not short-circuit: every retained stage must lose the old
            // readiness bit even if an earlier stage reported re-entry.
            if !stage.set_pending(id) {
                stages_pending = false;
            }
        }
        let native_hidden = self
            .views
            .get(&id)
            .is_some_and(|view| crate::platform::imp::enforce_navigation_pending(view));

        let still_current = self.views.get(&id).is_some_and(|view| {
            navigation_callback_matches(
                &view.event_permit,
                &view.navigation,
                source_permit,
                source_navigation,
                epoch,
            ) && view.navigation.current_committed() == Some(epoch)
        });
        if !still_current {
            // Native calls can pump a newer navigation. The newer epoch owns
            // the now-hidden surface and will issue its own exact reveal.
            return false;
        }
        if stages_pending && native_hidden {
            return true;
        }

        // An unretained gate or a failed native hide could expose a committed
        // document under stale privileged chrome. Retire this exact generation
        // instead of degrading to best-effort presentation.
        eprintln!("security: could not re-arm committed-document presentation gate");
        self.close(id);
        if let Some(token) = token {
            self.sink
                .emit_for(token, EngineEvent::ViewCreationFailed { id });
        }
        false
    }

    pub(super) fn complete_title_attribution(
        &mut self,
        id: ItemId,
        source_permit: &EventPermit,
        source_navigation: &NavigationEpochTracker,
        epoch: NavigationEpoch,
    ) {
        if !self.navigation_is_attributed(id, source_permit, source_navigation, epoch) {
            return;
        }
        // Every desktop backend reads the current native document title in
        // this call and bounds it before allocating Rust data. Doing this only
        // after the exact navigation Finished event avoids trusting callback
        // payload/order from the document that was replaced at commit.
        let title = self
            .views
            .get(&id)
            .and_then(|view| view.document_title().ok().flatten())
            .map(|title| bounded_title(&title));
        let Some(view) = self.views.get_mut(&id) else {
            return;
        };
        if !navigation_callback_matches(
            &view.event_permit,
            &view.navigation,
            source_permit,
            source_navigation,
            epoch,
        ) || view.navigation.current_committed() != Some(epoch)
        {
            return;
        }
        view.title_ready = Some(epoch);
        if let Some(title) = title {
            source_permit.emit(&self.sink, EngineEvent::TitleChanged { id, title });
        }
    }

    pub(super) fn emit_title_observation(
        &self,
        id: ItemId,
        source_permit: &EventPermit,
        source_navigation: &NavigationEpochTracker,
        epoch: NavigationEpoch,
    ) {
        if !self.navigation_is_attributed(id, source_permit, source_navigation, epoch) {
            return;
        }
        let Some(view) = self.views.get(&id) else {
            return;
        };
        if !view.presentable || view.title_ready != Some(epoch) {
            return;
        }
        // KVO/COM title observations may wait behind navigation settlement.
        // A pre-finish empty title must not overwrite the freshly sampled title
        // of that same epoch. Read current native state when this task runs.
        let title = view
            .document_title()
            .ok()
            .flatten()
            .map(|title| bounded_title(&title));
        let still_current = self
            .views
            .get(&id)
            .is_some_and(|view| view.presentable && view.title_ready == Some(epoch))
            && self.navigation_is_attributed(id, source_permit, source_navigation, epoch);
        if still_current {
            if let Some(title) = title {
                source_permit.emit(&self.sink, EngineEvent::TitleChanged { id, title });
            }
        }
    }

    pub(super) fn emit_navigation_observation(
        &mut self,
        id: ItemId,
        source_permit: &EventPermit,
        source_navigation: &NavigationEpochTracker,
        epoch: NavigationEpoch,
    ) -> bool {
        let (event_permit, navigation, url, history) = {
            let Some(view) = self.views.get_mut(&id) else {
                return false;
            };
            if !navigation_callback_matches(
                &view.event_permit,
                &view.navigation,
                source_permit,
                source_navigation,
                epoch,
            ) {
                return false;
            }
            if view.navigation.current_committed() != Some(epoch) {
                // Source/KVO notifications can precede the identity-bearing
                // main-frame commit. They may query useful provisional state,
                // but can never authorize either chrome attribution or first
                // presentation.
                return false;
            }
            let url = crate::platform::imp::current_url(view);
            let history = match (view.can_go_back(), view.can_go_forward()) {
                (Ok(can_go_back), Ok(can_go_forward)) => Some((can_go_back, can_go_forward)),
                _ => None,
            };
            (
                view.event_permit.clone(),
                view.navigation.clone(),
                url,
                history,
            )
        };
        let previous_committed_url = navigation
            .committed_snapshot()
            .filter(|(committed, _)| *committed == epoch)
            .map(|(_, url)| url);
        let url = match resolve_committed_observed_url(&navigation, epoch, url) {
            ObservedUrl::Unavailable => {
                // Neither the native view nor the exact committed tracker can
                // identify an allowed source. Keep this generation hidden and
                // withhold its history facts until a later valid observation.
                return false;
            }
            ObservedUrl::Allowed(url) => url,
            ObservedUrl::Forbidden => {
                // A native getter may pump the run loop. Revalidate the exact
                // physical generation and committed epoch before closing so a
                // stale observation can never tear down its replacement.
                let still_current = self.views.get(&id).is_some_and(|view| {
                    navigation_callback_matches(
                        &view.event_permit,
                        &view.navigation,
                        &event_permit,
                        &navigation,
                        epoch,
                    ) && view.navigation.current_committed() == Some(epoch)
                });
                if !still_current {
                    return false;
                }
                // Navigation callbacks should have prevented this. A History
                // API mutation can still create an overlong same-document URL
                // without a navigation callback, so never leave trusted
                // chrome showing the previous address over that document.
                eprintln!("security: native content source escaped the URL policy; closing view");
                let token = event_permit.active_token();
                // The terminal event must never race a still-reachable native
                // object. Revoke callbacks and remove the physical view first.
                self.close(id);
                if let Some(token) = token {
                    self.sink
                        .emit_for(token, EngineEvent::ViewCreationFailed { id });
                }
                return false;
            }
        };
        let same_document_url_changed = previous_committed_url
            .as_deref()
            .is_some_and(|previous| previous != url);
        let became_presentable = {
            let Some(view) = self.views.get_mut(&id) else {
                return false;
            };
            if !navigation_callback_matches(
                &view.event_permit,
                &view.navigation,
                &event_permit,
                &navigation,
                epoch,
            ) || !view.navigation.matches_committed_snapshot(epoch, &url)
            {
                return false;
            }
            let announce = !view.presentable && view.presentation_announced != Some(epoch);
            if announce {
                view.presentation_announced = Some(epoch);
            }
            announce
        };
        let previous = self.navigation_snapshots.entry(id).or_default();
        for event in navigation_observation_events(id, previous, Some(&url), history) {
            event_permit.emit(&self.sink, event);
        }
        if became_presentable || same_document_url_changed {
            // The URL event above enters the shell's ordered critical band
            // before this exact acknowledgement token. The shell presents as
            // soon as it has applied that URL; it never waits for Finished.
            event_permit.emit(
                &self.sink,
                EngineEvent::PresentationPending {
                    id,
                    navigation: epoch.presentation_id(),
                    url: url.clone(),
                },
            );
            self.refresh_document_styles(id);
        }
        #[cfg(target_os = "macos")]
        if let Some(view) = self
            .views
            .get(&id)
            .filter(|view| view.navigation.current_committed() == Some(epoch))
        {
            let state = crate::platform::macos::capture::sample(
                &crate::platform::macos::native_webview(&view.view),
            );
            if view.navigation.current_committed() == Some(epoch) {
                event_permit.emit(
                    &self.sink,
                    EngineEvent::MediaCaptureChanged {
                        id,
                        navigation: epoch.presentation_id(),
                        state,
                    },
                );
            }
        }
        self.navigation_snapshots
            .get(&id)
            .and_then(|snapshot| snapshot.url.as_deref())
            == Some(url.as_str())
    }

    fn navigation_is_attributed(
        &self,
        id: ItemId,
        source_permit: &EventPermit,
        source_navigation: &NavigationEpochTracker,
        epoch: NavigationEpoch,
    ) -> bool {
        let Some((target_epoch, target)) = self
            .views
            .get(&id)
            .and_then(|view| view.navigation.committed_snapshot())
        else {
            return false;
        };
        if target_epoch != epoch
            || self
                .navigation_snapshots
                .get(&id)
                .and_then(|snapshot| snapshot.url.as_deref())
                != Some(target.as_str())
        {
            return false;
        }
        let Some(view) = self.views.get(&id) else {
            return false;
        };
        navigation_callback_matches(
            &view.event_permit,
            &view.navigation,
            source_permit,
            source_navigation,
            epoch,
        ) && view.navigation.current_committed() == Some(epoch)
    }

    pub(super) fn emit_navigation_ready(
        &self,
        id: ItemId,
        source_permit: &EventPermit,
        source_navigation: &NavigationEpochTracker,
        epoch: NavigationEpoch,
    ) {
        if !self.navigation_is_attributed(id, source_permit, source_navigation, epoch) {
            return;
        }
        let Some((committed, url)) = source_navigation.committed_snapshot() else {
            return;
        };
        if committed != epoch {
            return;
        }
        source_permit.emit(
            &self.sink,
            EngineEvent::PresentationReady {
                id,
                navigation: epoch.presentation_id(),
                url,
            },
        );
    }

    fn present_navigation_epoch(
        &mut self,
        id: ItemId,
        source_permit: &EventPermit,
        source_navigation: &NavigationEpochTracker,
        epoch: NavigationEpoch,
    ) {
        if !self.navigation_is_attributed(id, source_permit, source_navigation, epoch) {
            return;
        }
        // Install every stage's logical readiness while the shared physical
        // permit is still false. A RefCell/COM admission failure can then be
        // retired without any sibling stage briefly revealing the document.
        let mut prepared = true;
        for stage in self.stages.values() {
            if stage.has_view(id) && !stage.set_ready(id) {
                prepared = false;
            }
        }
        if !prepared {
            self.fail_navigation_presentation_application(
                id,
                source_permit,
                source_navigation,
                epoch,
            );
            return;
        }
        // URL/chrome authority is already established. Cover only the first
        // native frame of a newly committed document; the page can render
        // normally underneath, without a load-finished or screenshot gate.
        #[cfg(target_os = "macos")]
        let snapshot = self
            .views
            .get(&id)
            .filter(|view| !view.presentable)
            .and_then(|view| view.navigation.committed_snapshot())
            .and_then(|(committed, url)| {
                let (_, captured, snapshot) = self.restore_snapshots.remove(&id)?;
                (committed == epoch && url == captured).then_some(snapshot)
            });
        #[cfg(target_os = "macos")]
        let cover = self
            .views
            .get(&id)
            .filter(|view| !view.presentable)
            .and_then(|view| {
                crate::platform::imp::PaintCover::begin(
                    id,
                    &view.view,
                    self.stages.values().cloned(),
                    snapshot,
                )
            });
        #[cfg(target_os = "windows")]
        let cover = self
            .views
            .get(&id)
            .filter(|view| !view.presentable)
            .and_then(|view| {
                crate::platform::imp::PaintCover::begin(
                    id,
                    &view.view,
                    self.stages.values().cloned(),
                )
            });
        if !self.navigation_is_attributed(id, source_permit, source_navigation, epoch) {
            return;
        }
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        {
            let previous = self.views.get_mut(&id).and_then(|view| {
                if view.presentable {
                    // A duplicate acknowledgement must not shorten the same
                    // document's current first-frame handoff.
                    None
                } else {
                    std::mem::replace(&mut view.paint_cover, cover)
                }
            });
            // Removing an old native cover may re-enter the platform UI. Do
            // not hold a view borrow or publish a permit until exact
            // attribution is rechecked after that cleanup.
            drop(previous);
        }
        if !self.navigation_is_attributed(id, source_permit, source_navigation, epoch) {
            return;
        }
        let Some(view) = self.views.get_mut(&id) else {
            return;
        };
        view.presentable = true;
        view.nonpresentable_bootstrap = None;
        // Publish only after every stage retained the exact readiness fact.
        // A re-entrant newer commit flips this same atomic false before any
        // native reveal primitive can execute.
        view.presentation_permit.store(true, Ordering::Release);
        let mut applied = true;
        for stage in self.stages.values() {
            if stage.has_view(id) && !stage.set_ready(id) {
                applied = false;
            }
        }
        if !applied {
            self.fail_navigation_presentation_application(
                id,
                source_permit,
                source_navigation,
                epoch,
            );
        } else {
            // Title KVO may have arrived while this exact document was still
            // behind the chrome acknowledgement barrier. Resample now rather
            // than trusting or replaying an earlier callback payload.
            self.emit_title_observation(id, source_permit, source_navigation, epoch);
            self.refresh_generic_styles(id);
        }
    }

    fn fail_navigation_presentation_application(
        &mut self,
        id: ItemId,
        source_permit: &EventPermit,
        source_navigation: &NavigationEpochTracker,
        epoch: NavigationEpoch,
    ) {
        if !self.navigation_is_attributed(id, source_permit, source_navigation, epoch) {
            return;
        }
        let token = source_permit.active_token();
        if let Some(view) = self.views.get_mut(&id) {
            view.presentable = false;
            view.presentation_permit.store(false, Ordering::Release);
        }
        for stage in self.stages.values() {
            let _ = stage.set_pending(id);
        }
        eprintln!("security: exact native presentation could not be applied");
        self.close(id);
        if let Some(token) = token {
            self.sink
                .emit_for(token, EngineEvent::ViewCreationFailed { id });
        }
    }

    pub(crate) fn present_navigation(
        &mut self,
        id: ItemId,
        presentation: NavigationPresentationId,
    ) {
        let Some((permit, navigation, epoch)) = self.views.get(&id).and_then(|view| {
            view.navigation
                .committed_epoch_for_presentation(presentation)
                .map(|epoch| (view.event_permit.clone(), view.navigation.clone(), epoch))
        }) else {
            return;
        };
        self.present_navigation_epoch(id, &permit, &navigation, epoch);
    }

    pub(super) fn settle_navigation_failure(
        &mut self,
        id: ItemId,
        source_permit: &EventPermit,
        source_navigation: &NavigationEpochTracker,
        failed: NavigationEpoch,
        restored: Option<NavigationEpoch>,
        cancelled: bool,
    ) {
        let Some((same_generation, current, current_committed, presentable, bootstrap, token)) =
            self.views.get(&id).map(|view| {
                (
                    view.event_permit.same_generation(source_permit)
                        && view.navigation.same_generation(source_navigation),
                    view.navigation.current(),
                    view.navigation.current_committed(),
                    view.presentable,
                    view.nonpresentable_bootstrap,
                    view.event_permit.active_token(),
                )
            })
        else {
            return;
        };
        if !same_generation {
            return;
        }

        // A failure after commit may leave a native error/partial document.
        // It is still bound to the exact attributed epoch and is safe to
        // present. A provisional first-load failure has no renderable browser
        // document and must become a terminal creation failure instead of a
        // permanently hidden white tab.
        if restored == Some(failed) {
            if current_committed != Some(failed) {
                return;
            }
            if self.rearm_navigation_presentation(id, source_permit, source_navigation, failed)
                && self.emit_navigation_observation(id, source_permit, source_navigation, failed)
            {
                self.emit_navigation_ready(id, source_permit, source_navigation, failed);
            }
            return;
        }

        if current != restored {
            return;
        }
        if let Some(restored) = restored
            .filter(|restored| restored_navigation_can_present(presentable, bootstrap, *restored))
        {
            let attributed =
                self.emit_navigation_observation(id, source_permit, source_navigation, restored);
            if attributed {
                // Re-drive even when the prior document was logically
                // presentable: a rejected/stale overlapping commit may have
                // synchronously revoked the shared native reveal permit.
                self.emit_navigation_ready(id, source_permit, source_navigation, restored);
            }
            return;
        }

        if cancelled {
            // A policy cancellation (including conversion to WKDownload) did
            // not fail controller construction. Keep its uncommitted view
            // hidden and reusable. Closing here would revoke a pending native
            // download save panel before the user can choose its destination.
            return;
        }
        eprintln!("view-create: provisional first-load navigation failed before commit");
        self.close(id);
        if let Some(token) = token {
            self.sink
                .emit_for(token, EngineEvent::ViewCreationFailed { id });
        }
    }

    pub(crate) fn navigate(
        &self,
        id: ItemId,
        url: &str,
        request: NavigationRequestId,
        event_token: Arc<AtomicBool>,
    ) {
        if self
            .partitions
            .get(&id)
            .is_some_and(|partition| self.erasure_tombstones.contains(&partition.profile()))
        {
            eprintln!("privacy: rejected navigation for tombstoned profile");
            self.sink
                .emit_for(event_token, EngineEvent::NavigationFailed { id, request });
            return;
        }
        if !navigation::is_browser_target_str(url) {
            eprintln!("security: rejected invalid native navigation target");
            self.sink
                .emit_for(event_token, EngineEvent::NavigationFailed { id, request });
            return;
        }
        let Some(view) = self.views.get(&id) else {
            // Core believed this id had a live controller. A missing native
            // view is a lifecycle failure, not merely a rejected URL.
            self.sink
                .emit_for(event_token, EngineEvent::ViewCreationFailed { id });
            return;
        };
        if !view.event_permit.matches_token(&event_token) {
            // This host task belongs to a prior same-id generation that was
            // displaced while a native message loop was reentrant.
            return;
        }
        if !view.event_permit.allows_navigation(url) {
            self.sink
                .emit_for(event_token, EngineEvent::NavigationFailed { id, request });
            return;
        }
        let Some(epoch) = view.navigation.begin_request(url, request) else {
            eprintln!("engine: could not establish navigation epoch");
            self.sink
                .emit_for(event_token, EngineEvent::NavigationFailed { id, request });
            return;
        };
        if view.load_url(url).is_err() {
            view.navigation.fail_synchronous(epoch);
            // The engine's error text can carry the address.
            eprintln!("engine: navigation could not start");
            self.sink
                .emit_for(event_token, EngineEvent::NavigationFailed { id, request });
        }
    }

    pub(crate) fn reload(&mut self, id: ItemId) {
        #[cfg(target_os = "macos")]
        if self.webext.navigate_page(id, 0) {
            return;
        }
        self.invoke_navigation_action(id, NativeAction::Reload);
    }

    pub(crate) fn stop(&self, id: ItemId) {
        #[cfg(target_os = "macos")]
        if self.webext.navigate_page(id, 3) {
            return;
        }
        if let Some(view) = self.views.get(&id) {
            view.navigation.release_auth_cleanup();
            crate::platform::imp::stop_loading(view);
        }
    }

    pub(crate) fn go_back(&mut self, id: ItemId) {
        #[cfg(target_os = "macos")]
        if self.webext.navigate_page(id, 1) {
            return;
        }
        self.invoke_navigation_action(id, NativeAction::GoBack);
    }

    pub(crate) fn go_forward(&mut self, id: ItemId) {
        #[cfg(target_os = "macos")]
        if self.webext.navigate_page(id, 2) {
            return;
        }
        self.invoke_navigation_action(id, NativeAction::GoForward);
    }

    fn invoke_navigation_action(&mut self, id: ItemId, action: NativeAction) {
        let Some((permit, navigation, failed)) = self.views.get(&id).map(|view| {
            // The user has taken control of this tab even if the native
            // reload/history call fails. Auth cleanup cannot take it back.
            view.navigation.release_auth_cleanup();
            let result = match action {
                NativeAction::Reload => view.reload(),
                NativeAction::GoBack => view.go_back(),
                NativeAction::GoForward => view.go_forward(),
            };
            (
                view.event_permit.clone(),
                view.navigation.clone(),
                result.is_err(),
            )
        }) else {
            return;
        };
        if !failed {
            return;
        }

        // Platform errors and action names are bounded native facts; never
        // surface a page-derived URL or native error string through privileged
        // IPC. Revalidate the exact view generation before reporting or
        // querying source/history because the platform call may pump.
        let current = self.views.get(&id).is_some_and(|view| {
            view.event_permit.same_generation(&permit)
                && view.navigation.same_generation(&navigation)
        });
        if !current {
            return;
        }
        eprintln!("engine: native reload/history invocation failed");
        permit.emit(&self.sink, EngineEvent::NativeActionFailed { id, action });
        if let Some(epoch) = navigation.current_committed() {
            let _ = self.emit_navigation_observation(id, &permit, &navigation, epoch);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_navigation_observations_are_bounded_deduplicated_and_filtered() {
        let id = ItemId::from(7);
        let mut previous = NavigationSnapshot::default();

        let initial = navigation_observation_events(
            id,
            &mut previous,
            Some("https://example.com/"),
            Some((false, false)),
        );
        assert_eq!(initial.len(), 2);
        assert!(matches!(
            &initial[0],
            EngineEvent::UrlChanged { id: observed, url }
                if *observed == id && url == "https://example.com/"
        ));
        assert!(matches!(
            initial[1],
            EngineEvent::NavState {
                id: observed,
                can_go_back: false,
                can_go_forward: false,
            } if observed == id
        ));

        assert!(navigation_observation_events(
            id,
            &mut previous,
            Some("https://example.com/"),
            Some((false, false)),
        )
        .is_empty());

        let same_document = navigation_observation_events(
            id,
            &mut previous,
            Some("https://example.com/#state"),
            Some((true, false)),
        );
        assert_eq!(same_document.len(), 2);

        let forbidden = navigation_observation_events(
            id,
            &mut previous,
            Some("file:///etc/passwd"),
            Some((true, true)),
        );
        assert_eq!(forbidden.len(), 1);
        assert!(matches!(
            forbidden[0],
            EngineEvent::NavState {
                can_go_back: true,
                can_go_forward: true,
                ..
            }
        ));
        assert_eq!(previous.url.as_deref(), Some("https://example.com/#state"));
    }

    #[test]
    fn native_url_policy_distinguishes_unavailable_from_forbidden_sources() {
        assert_eq!(classify_observed_url(None), ObservedUrl::Unavailable);
        assert_eq!(
            classify_observed_url(Some(String::new())),
            ObservedUrl::Unavailable
        );
        assert_eq!(
            classify_observed_url(Some("https://example.com/#state".to_owned())),
            ObservedUrl::Allowed("https://example.com/#state".to_owned())
        );
        assert_eq!(
            classify_observed_url(Some("file:///etc/passwd".to_owned())),
            ObservedUrl::Forbidden
        );
    }

    fn commit_test_navigation(
        tracker: &NavigationEpochTracker,
        native_id: u64,
        target: &str,
    ) -> NavigationEpoch {
        let epoch = tracker.begin(target).expect("allowed test URL");
        for phase in [
            wry::NavigationEventPhase::Started,
            wry::NavigationEventPhase::Committed,
        ] {
            assert!(tracker
                .observe_navigation(&wry::NavigationEvent {
                    id: wry::NavigationId::from_raw(native_id),
                    phase,
                    url: target.to_owned(),
                })
                .is_some());
        }
        epoch
    }

    #[test]
    fn unavailable_native_url_uses_only_the_exact_committed_snapshot() {
        let tracker = NavigationEpochTracker::new();
        let epoch = commit_test_navigation(&tracker, 71, "https://committed.example/path");
        assert_eq!(
            resolve_committed_observed_url(&tracker, epoch, None),
            ObservedUrl::Allowed("https://committed.example/path".to_owned())
        );

        let pending = tracker.begin("https://pending.example/").unwrap();
        assert_eq!(
            resolve_committed_observed_url(&tracker, pending, None),
            ObservedUrl::Unavailable
        );
        assert_eq!(
            resolve_committed_observed_url(&tracker, epoch, None),
            ObservedUrl::Unavailable
        );
    }

    #[test]
    fn provisional_failure_can_restore_real_hidden_commit_but_not_spare_bootstrap() {
        let tracker = NavigationEpochTracker::new();
        let bootstrap = tracker.begin("about:blank").unwrap();
        let real = tracker.begin("https://real.example/").unwrap();

        assert!(!restored_navigation_can_present(
            false,
            Some(bootstrap),
            bootstrap
        ));
        assert!(restored_navigation_can_present(
            false,
            Some(bootstrap),
            real
        ));
        assert!(restored_navigation_can_present(
            true,
            Some(bootstrap),
            bootstrap
        ));
    }

    #[test]
    fn failed_navigation_cannot_reveal_its_stale_epoch() {
        let tracker = NavigationEpochTracker::new();
        let visible = commit_test_navigation(&tracker, 81, "https://visible.example/");
        let failed = tracker.begin("https://failed.example/").unwrap();
        assert!(tracker
            .observe_navigation(&wry::NavigationEvent {
                id: wry::NavigationId::from_raw(82),
                phase: wry::NavigationEventPhase::Started,
                url: "https://failed.example/".to_owned(),
            })
            .is_some());
        assert!(tracker
            .observe_navigation(&wry::NavigationEvent {
                id: wry::NavigationId::from_raw(82),
                phase: wry::NavigationEventPhase::Failed,
                url: "https://failed.example/".to_owned(),
            })
            .is_some());

        assert_eq!(
            resolve_committed_observed_url(&tracker, failed, None),
            ObservedUrl::Unavailable
        );
        assert_eq!(
            resolve_committed_observed_url(&tracker, visible, None),
            ObservedUrl::Allowed("https://visible.example/".to_owned())
        );
    }

    #[test]
    fn forbidden_native_url_never_falls_back_to_trusted_chrome() {
        let tracker = NavigationEpochTracker::new();
        let epoch = commit_test_navigation(&tracker, 91, "https://trusted.example/");
        assert_eq!(
            resolve_committed_observed_url(&tracker, epoch, Some("file:///etc/passwd".to_owned())),
            ObservedUrl::Forbidden
        );
        assert_eq!(
            tracker.committed_snapshot(),
            Some((epoch, "https://trusted.example/".to_owned()))
        );
    }
}
