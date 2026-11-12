//! Site settings: what the focused profile decided for each site, from where
//! each platform keeps it. macOS keeps camera, microphone and location in the
//! page-permission catalog; Windows keeps every engine kind in its WebView2
//! profile. Pop-ups and downloads are the browser's own lists on both.
//! Private profiles show nothing here and are never read or written.

use zephium_core::ids::PagePermissionGrantId;
use zephium_core::permissions::{
    PageOrigin, PagePermissionChange, PagePermissionKind, PagePermissionPatch,
    RememberedPagePermission, SitePermission, SitePermissionKind,
};
use zephium_core::ports::store::{
    PagePermissionCatalogLoadOutcome, PagePermissionCatalogMutationOutcome,
};
use zephium_ipc::{
    SitePermissionDecisionView, SitePermissionKindView, SitePermissionView, SitePermissionsView,
};

use super::*;

/// Windows saves every prompt answer in its WebView2 profile; macOS keeps
/// Zephium's own catalog.
const ENGINE_KEEPS_DECISIONS: bool = cfg!(target_os = "windows");

fn kinds() -> Vec<SitePermissionKind> {
    use SitePermissionKind as Kind;
    if ENGINE_KEEPS_DECISIONS {
        vec![
            Kind::Camera,
            Kind::Microphone,
            Kind::Geolocation,
            Kind::Notifications,
            Kind::ClipboardRead,
            Kind::Midi,
            Kind::Sensors,
            Kind::LocalFonts,
            Kind::AutomaticDownloads,
            Kind::Popups,
            Kind::Downloads,
            Kind::AppLinks,
        ]
    } else {
        vec![
            Kind::Camera,
            Kind::Microphone,
            Kind::Geolocation,
            Kind::Popups,
            Kind::Downloads,
            Kind::AppLinks,
        ]
    }
}

pub(super) fn kind_view(kind: SitePermissionKind) -> SitePermissionKindView {
    use SitePermissionKind as Kind;
    use SitePermissionKindView as View;
    match kind {
        Kind::Camera => View::Camera,
        Kind::Microphone => View::Microphone,
        Kind::Geolocation => View::Geolocation,
        Kind::Notifications => View::Notifications,
        Kind::ClipboardRead => View::ClipboardRead,
        Kind::Midi => View::Midi,
        Kind::Sensors => View::Sensors,
        Kind::LocalFonts => View::LocalFonts,
        Kind::AutomaticDownloads => View::AutomaticDownloads,
        Kind::Popups => View::Popups,
        Kind::Downloads => View::Downloads,
        Kind::AppLinks => View::AppLinks,
    }
}

/// The listing, and whether it holds every decision rather than stopping at
/// the cap.
fn entries(
    mut decisions: Vec<SitePermission>,
    origin: Option<&PageOrigin>,
) -> (Vec<SitePermissionView>, bool) {
    decisions.retain(|entry| origin.is_none_or(|origin| entry.origin == *origin));
    decisions.sort_by(|a, b| (a.kind, &a.origin, &a.scheme).cmp(&(b.kind, &b.origin, &b.scheme)));
    decisions.dedup_by(|a, b| a.kind == b.kind && a.origin == b.origin && a.scheme == b.scheme);
    let whole = decisions.len() <= zephium_core::permissions::MAX_SITE_PERMISSIONS;
    let listed = decisions
        .into_iter()
        .take(zephium_core::permissions::MAX_SITE_PERMISSIONS)
        .map(|entry| SitePermissionView {
            origin: entry.origin.to_string(),
            kind: kind_view(entry.kind),
            decision: match entry.decision {
                RememberedPagePermission::Allow => SitePermissionDecisionView::Allow,
                RememberedPagePermission::Deny => SitePermissionDecisionView::Block,
            },
            scheme: entry.scheme,
        })
        .collect();
    (listed, whole)
}

fn catalog_change(
    catalog: &zephium_core::permissions::PagePermissionCatalog,
    origin: &PageOrigin,
    kind: PagePermissionKind,
    decision: Option<RememberedPagePermission>,
) -> Option<PagePermissionChange> {
    let current = catalog
        .grants()
        .iter()
        .find(|grant| grant.origin == *origin && grant.kind == kind);
    match (current, decision) {
        (None, None) => None,
        (Some(grant), Some(decision)) if grant.decision == decision => None,
        (Some(grant), Some(decision)) => Some(PagePermissionChange::Update {
            id: grant.id,
            expected: grant.revision,
            decision,
        }),
        (Some(grant), None) => Some(PagePermissionChange::Delete {
            id: grant.id,
            expected: grant.revision,
        }),
        (None, Some(decision)) => Some(PagePermissionChange::Create {
            id: PagePermissionGrantId::generate(),
            origin: origin.clone(),
            kind,
            decision,
        }),
    }
}

impl Shell {
    fn private_profile(&self, profile: ProfileId) -> bool {
        self.profiles
            .get(profile)
            .is_none_or(|profile| profile.kind == ProfileKind::Incognito)
    }

    /// A deleted profile's lists are never read back while its data is
    /// still being erased.
    fn known_profile(&self, profile: ProfileId) -> bool {
        self.profiles.get(profile).is_some()
    }

    /// Reads a saved profile's pop-up list once and hands it to the engine.
    pub(super) fn ensure_popup_sites(&mut self, profile: ProfileId) {
        if self.popup_sites.contains_key(&profile) || self.private_profile(profile) {
            return;
        }
        let Some(origins) = self.store.popup_sites(profile) else {
            return;
        };
        self.engine.set_popup_sites(profile, origins.clone());
        self.popup_sites
            .insert(profile, origins.into_iter().collect());
    }

    /// Reads a profile's application link grants once. A private profile
    /// starts with none.
    pub(super) fn ensure_app_link_sites(&mut self, profile: ProfileId) {
        if self.app_link_sites.contains_key(&profile) || !self.known_profile(profile) {
            return;
        }
        let sites = if self.private_profile(profile) {
            Vec::new()
        } else {
            let Some(sites) = self.store.app_link_sites(profile) else {
                return;
            };
            sites
        };
        self.app_link_sites
            .insert(profile, sites.into_iter().collect());
    }

    /// Lets `origin` open `scheme` links without asking, or forgets it. A
    /// private profile keeps it in memory only.
    pub(super) fn set_app_link_site(
        &mut self,
        profile: ProfileId,
        origin: PageOrigin,
        scheme: String,
        allowed: bool,
    ) -> bool {
        if !zephium_core::navigation::app_link_scheme(&scheme) {
            return false;
        }
        self.ensure_app_link_sites(profile);
        let Some(current) = self.app_link_sites.get(&profile) else {
            return false;
        };
        let key = (origin, scheme);
        if current.contains(&key) == allowed {
            return true;
        }
        if allowed && current.len() >= zephium_core::navigation::MAX_APP_LINK_SITES {
            return false;
        }
        if !self.private_profile(profile)
            && !self
                .store
                .set_app_link_site(profile, key.0.clone(), key.1.clone(), allowed)
        {
            // The write may still land, so the next check reads the store
            // again rather than trust what was held.
            self.app_link_sites.remove(&profile);
            return false;
        }
        let Some(sites) = self.app_link_sites.get_mut(&profile) else {
            return false;
        };
        if allowed {
            sites.insert(key);
        } else {
            sites.remove(&key);
        }
        true
    }

    /// Reads a profile's download decisions once and hands them to the
    /// engine. A private profile starts with none.
    pub(super) fn ensure_download_sites(&mut self, profile: ProfileId) {
        if self.download_sites.contains_key(&profile) || !self.known_profile(profile) {
            return;
        }
        let sites = if self.private_profile(profile) {
            Vec::new()
        } else {
            let Some(sites) = self.store.download_sites(profile) else {
                return;
            };
            sites
        };
        self.engine.set_download_sites(profile, sites.clone());
        self.download_sites
            .insert(profile, sites.into_iter().collect());
    }

    /// Keeps one site's download decision, or forgets it with None. A private
    /// profile keeps it in memory only.
    pub(super) fn set_download_site(
        &mut self,
        profile: ProfileId,
        origin: PageOrigin,
        allowed: Option<bool>,
    ) -> bool {
        self.ensure_download_sites(profile);
        let Some(current) = self.download_sites.get(&profile) else {
            return false;
        };
        if current.get(&origin).copied() == allowed {
            return true;
        }
        if allowed.is_some()
            && !current.contains_key(&origin)
            && current.len() >= zephium_core::permissions::MAX_DOWNLOAD_SITES
        {
            return false;
        }
        if !self.private_profile(profile)
            && !self
                .store
                .set_download_site(profile, origin.clone(), allowed)
        {
            return false;
        }
        let Some(sites) = self.download_sites.get_mut(&profile) else {
            return false;
        };
        match allowed {
            Some(allowed) => {
                sites.insert(origin, allowed);
            }
            None => {
                sites.remove(&origin);
            }
        }
        let sites = sites
            .iter()
            .map(|(origin, allowed)| (origin.clone(), *allowed))
            .collect();
        self.engine.set_download_sites(profile, sites);
        true
    }

    /// The site a tab's blocked pop-up would be allowed for, when its profile
    /// keeps such decisions.
    pub(super) fn popup_site(&self, id: ItemId) -> Option<(ProfileId, PageOrigin)> {
        let profile = self.profile_of_item(id)?;
        if self.private_profile(profile) {
            return None;
        }
        let url = self.items.tab(id)?.url.as_ref()?;
        Some((profile, PageOrigin::from_url(url).ok()?))
    }

    fn set_popup_site(&mut self, profile: ProfileId, origin: PageOrigin, allowed: bool) -> bool {
        if self.private_profile(profile) {
            return false;
        }
        self.ensure_popup_sites(profile);
        let Some(current) = self.popup_sites.get(&profile) else {
            return false;
        };
        if current.contains(&origin) == allowed {
            return true;
        }
        if !self.store.set_popup_site(profile, origin.clone(), allowed) {
            return false;
        }
        let Some(sites) = self.popup_sites.get_mut(&profile) else {
            return false;
        };
        if allowed {
            sites.insert(origin);
        } else {
            sites.remove(&origin);
        }
        self.engine
            .set_popup_sites(profile, sites.iter().cloned().collect());
        true
    }

    /// Lets the tab's site open pop-ups from now on, from its blocked card.
    pub(super) fn allow_popups_for_tab(&mut self, id: ItemId) -> bool {
        self.popup_site(id)
            .is_some_and(|(profile, origin)| self.set_popup_site(profile, origin, true))
    }

    pub(super) fn query_site_permissions(
        &mut self,
        origin: Option<PageOrigin>,
        reply: std::sync::mpsc::SyncSender<Option<SitePermissionsView>>,
    ) {
        let Some(profile) = self.windows.focused().map(|window| window.profile) else {
            let _ = reply.try_send(None);
            return;
        };
        let kinds: Vec<_> = kinds().into_iter().map(kind_view).collect();
        if self.private_profile(profile) {
            let _ = reply.try_send(Some(SitePermissionsView {
                private: true,
                complete: true,
                kinds,
                entries: Vec::new(),
            }));
            return;
        }
        self.ensure_popup_sites(profile);
        self.ensure_download_sites(profile);
        self.ensure_app_link_sites(profile);
        let popups = self.popup_sites.get(&profile);
        let downloads = self.download_sites.get(&profile);
        let app_links = self.app_link_sites.get(&profile);
        let popups_read = popups.is_some() && downloads.is_some() && app_links.is_some();
        let popups: Vec<SitePermission> = popups
            .into_iter()
            .flatten()
            .map(|origin| SitePermission {
                scheme: None,
                origin: origin.clone(),
                kind: SitePermissionKind::Popups,
                decision: RememberedPagePermission::Allow,
            })
            .chain(
                downloads
                    .into_iter()
                    .flatten()
                    .map(|(origin, allowed)| SitePermission {
                        scheme: None,
                        origin: origin.clone(),
                        kind: SitePermissionKind::Downloads,
                        decision: if *allowed {
                            RememberedPagePermission::Allow
                        } else {
                            RememberedPagePermission::Deny
                        },
                    }),
            )
            .chain(
                app_links
                    .into_iter()
                    .flatten()
                    .map(|(origin, scheme)| SitePermission {
                        scheme: Some(scheme.clone()),
                        origin: origin.clone(),
                        kind: SitePermissionKind::AppLinks,
                        decision: RememberedPagePermission::Allow,
                    }),
            )
            .collect();
        let finish = move |native: Option<Vec<SitePermission>>| {
            let read = popups_read && native.is_some();
            let mut decisions = native.unwrap_or_default();
            decisions.extend(popups);
            let (entries, whole) = entries(decisions, origin.as_ref());
            let _ = reply.try_send(Some(SitePermissionsView {
                private: false,
                complete: read && whole,
                kinds,
                entries,
            }));
        };
        if ENGINE_KEEPS_DECISIONS {
            self.engine.site_permissions(profile, Box::new(finish));
            return;
        }
        let finish = std::sync::Arc::new(std::sync::Mutex::new(Some(finish)));
        let fallback = finish.clone();
        let accepted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.store.load_page_permission_catalog(
                profile,
                Box::new(move |outcome| {
                    let native = match outcome {
                        PagePermissionCatalogLoadOutcome::Loaded(catalog) => Some(
                            catalog
                                .grants()
                                .iter()
                                .filter_map(|grant| {
                                    Some(SitePermission {
                                        scheme: None,
                                        origin: grant.origin.clone(),
                                        kind: SitePermissionKind::from_page(grant.kind)?,
                                        decision: grant.decision,
                                    })
                                })
                                .collect(),
                        ),
                        _ => None,
                    };
                    if let Some(finish) = finish.lock().ok().and_then(|mut slot| slot.take()) {
                        finish(native);
                    }
                }),
            )
        }))
        .unwrap_or(false);
        if !accepted {
            if let Some(finish) = fallback.lock().ok().and_then(|mut slot| slot.take()) {
                finish(None);
            }
        }
    }

    pub(super) fn begin_site_permission_change(
        &mut self,
        operation_id: String,
        origin: PageOrigin,
        kind: SitePermissionKind,
        decision: Option<RememberedPagePermission>,
        scheme: Option<String>,
    ) -> Option<OperationDisposition> {
        let Some(profile) = self.windows.focused().map(|window| window.profile) else {
            return Some(operation_result(
                OperationOutcome::Rejected,
                OperationReason::NoFocusedWindow,
            ));
        };
        if self.private_profile(profile) || !kinds().contains(&kind) {
            return Some(operation_result(
                OperationOutcome::Rejected,
                OperationReason::InvalidScope,
            ));
        }
        if kind == SitePermissionKind::Notifications {
            self.invalidate_notification_sleep(Some(profile));
        }
        if kind == SitePermissionKind::AppLinks {
            // Asking is the default, so anything but Allow is a removal.
            let allowed = decision == Some(RememberedPagePermission::Allow);
            let applied = scheme
                .is_some_and(|scheme| self.set_app_link_site(profile, origin, scheme, allowed));
            return Some(if applied {
                operation_result(OperationOutcome::Applied, OperationReason::MutationApplied)
            } else {
                operation_result(
                    OperationOutcome::Rejected,
                    OperationReason::StoreAdmissionRejected,
                )
            });
        }
        if kind == SitePermissionKind::Downloads {
            let allowed = decision.map(|decision| decision == RememberedPagePermission::Allow);
            return Some(if self.set_download_site(profile, origin, allowed) {
                operation_result(OperationOutcome::Applied, OperationReason::MutationApplied)
            } else {
                operation_result(
                    OperationOutcome::Rejected,
                    OperationReason::StoreAdmissionRejected,
                )
            });
        }
        if kind == SitePermissionKind::Popups {
            // Blocking is the default for every site, so a block is a removal.
            let allowed = decision == Some(RememberedPagePermission::Allow);
            return Some(if self.set_popup_site(profile, origin, allowed) {
                operation_result(OperationOutcome::Applied, OperationReason::MutationApplied)
            } else {
                operation_result(
                    OperationOutcome::Rejected,
                    OperationReason::StoreAdmissionRejected,
                )
            });
        }
        if let Some(page_kind) = kind.page() {
            self.page_permissions
                .forget_site(profile, &origin, page_kind);
        }
        let Some(queue) = self.self_queue.as_ref() else {
            return Some(operation_result(
                OperationOutcome::Rejected,
                OperationReason::StoreAdmissionRejected,
            ));
        };
        let callback = CallbackHandle {
            queue: Arc::downgrade(&queue.inner),
        };
        if ENGINE_KEEPS_DECISIONS {
            let settled = operation_id.clone();
            self.engine.set_site_permission(
                profile,
                origin,
                kind,
                decision,
                Box::new(move |applied| {
                    let _ = callback.dispatch(Command::SitePermissionSettled {
                        operation_id: settled,
                        applied,
                    });
                }),
            );
            return None;
        }
        let Some(page_kind) = kind.page() else {
            return Some(operation_result(
                OperationOutcome::Rejected,
                OperationReason::InvalidScope,
            ));
        };
        let loaded = operation_id.clone();
        let accepted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.store.load_page_permission_catalog(
                profile,
                Box::new(move |outcome| {
                    let _ = callback.dispatch(Command::SitePermissionCatalogLoaded {
                        operation_id: loaded,
                        profile,
                        origin,
                        kind: page_kind,
                        decision,
                        outcome: Box::new(outcome),
                    });
                }),
            )
        }))
        .unwrap_or(false);
        if accepted {
            None
        } else {
            Some(operation_result(
                OperationOutcome::Rejected,
                OperationReason::StoreAdmissionRejected,
            ))
        }
    }

    pub(super) fn site_permission_catalog_loaded(
        &mut self,
        operation_id: String,
        profile: ProfileId,
        origin: PageOrigin,
        kind: PagePermissionKind,
        decision: Option<RememberedPagePermission>,
        outcome: PagePermissionCatalogLoadOutcome,
    ) {
        let settle = |shell: &Self, outcome, reason| {
            (shell.emit)(Projection::OperationProcessed(OperationDisposition {
                operation_id: operation_id.clone(),
                outcome,
                reason,
            }));
        };
        let PagePermissionCatalogLoadOutcome::Loaded(catalog) = outcome else {
            return settle(
                self,
                OperationOutcome::Rejected,
                OperationReason::StoreAdmissionRejected,
            );
        };
        let Some(change) = catalog_change(&catalog, &origin, kind, decision) else {
            return settle(
                self,
                OperationOutcome::NoOp,
                OperationReason::StateUnchanged,
            );
        };
        let (Ok(patch), Some(queue)) = (PagePermissionPatch::new(vec![change]), &self.self_queue)
        else {
            return settle(
                self,
                OperationOutcome::Rejected,
                OperationReason::StoreAdmissionRejected,
            );
        };
        let callback = CallbackHandle {
            queue: Arc::downgrade(&queue.inner),
        };
        let settled = operation_id.clone();
        let accepted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.store.mutate_page_permission_catalog(
                profile,
                catalog.revision(),
                patch,
                Box::new(move |outcome| {
                    let _ = callback.dispatch(Command::SitePermissionSettled {
                        operation_id: settled,
                        applied: matches!(
                            outcome,
                            PagePermissionCatalogMutationOutcome::Applied(_)
                        ),
                    });
                }),
            )
        }))
        .unwrap_or(false);
        if !accepted {
            settle(
                self,
                OperationOutcome::Rejected,
                OperationReason::StoreAdmissionRejected,
            );
        }
    }

    pub(super) fn site_permission_settled(&mut self, operation_id: String, applied: bool) {
        self.invalidate_notification_sleep(None);
        self.maintain_views();
        let (outcome, reason) = if applied {
            (OperationOutcome::Applied, OperationReason::MutationApplied)
        } else {
            (
                OperationOutcome::Rejected,
                OperationReason::StoreReconciliationFailed,
            )
        };
        (self.emit)(Projection::OperationProcessed(OperationDisposition {
            operation_id,
            outcome,
            reason,
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zephium_core::permissions::{
        PagePermissionCatalog, PagePermissionCatalogRevision, PagePermissionGrant,
        PagePermissionGrantRevision,
    };

    fn origin(value: &str) -> PageOrigin {
        PageOrigin::parse_exact(value).unwrap()
    }

    #[test]
    fn a_change_creates_updates_forgets_or_does_nothing() {
        let catalog = PagePermissionCatalog::new(
            PagePermissionCatalogRevision::INITIAL,
            vec![PagePermissionGrant {
                id: PagePermissionGrantId::from(1),
                revision: PagePermissionGrantRevision::INITIAL,
                origin: origin("https://meet.example"),
                kind: PagePermissionKind::Camera,
                decision: RememberedPagePermission::Allow,
            }],
        )
        .unwrap();
        let meet = origin("https://meet.example");
        let camera = PagePermissionKind::Camera;
        assert_eq!(
            catalog_change(
                &catalog,
                &meet,
                camera,
                Some(RememberedPagePermission::Allow)
            ),
            None
        );
        assert!(matches!(
            catalog_change(
                &catalog,
                &meet,
                camera,
                Some(RememberedPagePermission::Deny)
            ),
            Some(PagePermissionChange::Update {
                decision: RememberedPagePermission::Deny,
                ..
            })
        ));
        assert!(matches!(
            catalog_change(&catalog, &meet, camera, None),
            Some(PagePermissionChange::Delete { .. })
        ));
        assert_eq!(
            catalog_change(&catalog, &origin("https://other.example"), camera, None),
            None
        );
        assert!(matches!(
            catalog_change(
                &catalog,
                &meet,
                PagePermissionKind::Microphone,
                Some(RememberedPagePermission::Allow)
            ),
            Some(PagePermissionChange::Create { .. })
        ));
    }

    #[test]
    fn a_listing_is_filtered_sorted_and_free_of_duplicates() {
        let decision = |site: &str, kind| SitePermission {
            scheme: None,
            origin: origin(site),
            kind,
            decision: RememberedPagePermission::Allow,
        };
        let (listed, whole) = entries(
            vec![
                decision("https://b.example", SitePermissionKind::Popups),
                decision("https://a.example", SitePermissionKind::Camera),
                decision("https://b.example", SitePermissionKind::Camera),
                decision("https://b.example", SitePermissionKind::Camera),
            ],
            None,
        );
        assert!(whole);
        assert_eq!(
            listed
                .iter()
                .map(|entry| (entry.origin.as_str(), entry.kind))
                .collect::<Vec<_>>(),
            [
                ("https://a.example", SitePermissionKindView::Camera),
                ("https://b.example", SitePermissionKindView::Camera),
                ("https://b.example", SitePermissionKindView::Popups),
            ]
        );
        let (one, _) = entries(
            vec![
                decision("https://a.example", SitePermissionKind::Camera),
                decision("https://b.example", SitePermissionKind::Popups),
            ],
            Some(&origin("https://b.example")),
        );
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].kind, SitePermissionKindView::Popups);
    }

    #[test]
    fn a_listing_past_the_cap_says_it_is_not_whole() {
        let many = |count: usize| {
            (0..count)
                .map(|index| SitePermission {
                    scheme: None,
                    origin: origin(&format!("https://site-{index}.example")),
                    kind: SitePermissionKind::Popups,
                    decision: RememberedPagePermission::Allow,
                })
                .collect::<Vec<_>>()
        };
        let cap = zephium_core::permissions::MAX_SITE_PERMISSIONS;
        assert!(entries(many(cap), None).1);
        let (listed, whole) = entries(many(cap + 1), None);
        assert_eq!(listed.len(), cap);
        assert!(!whole);
    }
}
