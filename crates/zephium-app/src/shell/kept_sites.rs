//! The sites onboarding offers to keep. Chrome names a site by its catalog id
//! and never supplies a URL or pixels: both are fixed here, and each mark is
//! the exact raster the favicon store accepts, so seeding one decodes nothing.
use super::*;

/// Essentials taken from another browser at once; a row, not a library.
const MAX_IMPORTED_ESSENTIALS: usize = 64;

pub(super) struct KeptSite {
    pub(super) id: &'static str,
    pub(super) title: &'static str,
    pub(super) url: &'static str,
    pub(super) mark: &'static [u8; zephium_core::icon::RGBA32_BYTES],
}

macro_rules! site {
    ($id:literal, $title:literal, $url:literal) => {
        KeptSite {
            id: $id,
            title: $title,
            url: $url,
            mark: include_bytes!(concat!("../../../../assets/kept-sites/", $id, ".rgba")),
        }
    };
}

pub(super) const KEPT_SITES: &[KeptSite] = &[
    site!("gmail", "Gmail", "https://mail.google.com/"),
    site!(
        "google-calendar",
        "Google Calendar",
        "https://calendar.google.com/"
    ),
    site!("slack", "Slack", "https://app.slack.com/client"),
    site!("notion", "Notion", "https://www.notion.so/"),
    site!("linear", "Linear", "https://linear.app/"),
    site!("github", "GitHub", "https://github.com/"),
    site!("figma", "Figma", "https://www.figma.com/files"),
    site!("chatgpt", "ChatGPT", "https://chatgpt.com/"),
    site!("claude", "Claude", "https://claude.ai/"),
    site!("youtube", "YouTube", "https://www.youtube.com/"),
    site!("spotify", "Spotify", "https://open.spotify.com/"),
    site!("whatsapp", "WhatsApp", "https://web.whatsapp.com/"),
];

pub(super) fn kept_site(id: &str) -> Option<&'static KeptSite> {
    KEPT_SITES.iter().find(|site| site.id == id)
}

impl Shell {
    /// Keeps one catalog site in the focused space's Essentials, unloaded,
    /// with its mark already in place. A site already kept is left as it is,
    /// so a repeated request cannot stack duplicates.
    pub(super) fn operation_keep_site(&mut self, id: &str) -> OperationDisposition {
        let Some(site) = kept_site(id) else {
            return operation_result(OperationOutcome::Rejected, OperationReason::InvalidInput);
        };
        let Some((profile, space)) = self
            .windows
            .focused()
            .map(|window| (window.profile, window.space))
        else {
            return operation_result(OperationOutcome::Rejected, OperationReason::NoFocusedWindow);
        };
        let Ok(url) = url::Url::parse(site.url) else {
            return operation_result(OperationOutcome::Rejected, OperationReason::InvalidInput);
        };
        let placement = Placement::Favorites { space };
        let kept = self.items.roots(placement).iter().any(|existing| {
            self.items
                .tab(*existing)
                .and_then(|tab| tab.url.as_ref())
                .is_some_and(|existing| existing == &url)
        });
        if kept {
            return operation_result(OperationOutcome::NoOp, OperationReason::StateUnchanged);
        }
        let inserted = (0..8).any(|_| {
            self.items
                .insert_unloaded_tab(ItemId::generate(), placement, url.clone(), site.title)
        });
        if !inserted {
            return operation_result(OperationOutcome::Rejected, OperationReason::InvalidInput);
        }
        self.seed_kept_site_mark(profile, &url, site.mark);
        mutation_result(self.commit(Vec::new()))
    }

    /// Lets go of a catalog site kept and never opened: it leaves the
    /// Essentials whole, where closing an Essential would put it back. One
    /// that has been opened is the sidebar's to manage, not onboarding's.
    pub(super) fn operation_release_site(&mut self, id: &str) -> OperationDisposition {
        let Some(site) = kept_site(id) else {
            return operation_result(OperationOutcome::Rejected, OperationReason::InvalidInput);
        };
        let Some(space) = self.windows.focused().map(|window| window.space) else {
            return operation_result(OperationOutcome::Rejected, OperationReason::NoFocusedWindow);
        };
        let Ok(url) = url::Url::parse(site.url) else {
            return operation_result(OperationOutcome::Rejected, OperationReason::InvalidInput);
        };
        let placement = Placement::Favorites { space };
        let Some(kept) = self
            .items
            .roots(placement)
            .iter()
            .copied()
            .find(|existing| {
                self.items
                    .tab(*existing)
                    .and_then(|tab| tab.url.as_ref())
                    .is_some_and(|existing| existing == &url)
            })
        else {
            return operation_result(OperationOutcome::NoOp, OperationReason::StateUnchanged);
        };
        if self.items.tab(kept).is_none_or(|tab| tab.has_view()) {
            return operation_result(OperationOutcome::Rejected, OperationReason::InvalidInput);
        }
        self.cancel_favicon_attempt(kept);
        let fx = self.items.remove(kept);
        mutation_result(self.commit(fx))
    }

    /// Keeps sites another browser kept in `space`, after those already kept
    /// and without repeating one. They wait unloaded, as kept sites do until
    /// opened.
    pub(super) fn keep_imported_sites(
        &mut self,
        space: SpaceId,
        sites: Vec<crate::ImportedSite>,
    ) -> u32 {
        let placement = Placement::Favorites { space };
        let mut kept: std::collections::HashSet<url::Url> = self
            .items
            .roots(placement)
            .iter()
            .filter_map(|existing| self.items.tab(*existing)?.url.clone())
            .collect();
        let mut added = 0;
        for site in sites.into_iter().take(MAX_IMPORTED_ESSENTIALS) {
            let Some(url) = zephium_core::navigation::external_target(&site.url) else {
                continue;
            };
            if !kept.insert(url.clone()) {
                continue;
            }
            let title = zephium_core::item::sanitize_page_title(&site.title);
            if (0..8).any(|_| {
                self.items
                    .insert_unloaded_tab(ItemId::generate(), placement, url.clone(), &title)
            }) {
                added += 1;
            }
        }
        if added > 0 {
            let _ = self.commit(Vec::new());
        }
        added
    }

    /// The mark stands in until the site is opened and supplies its own. An
    /// icon already cached for the origin came from the site and is kept.
    fn seed_kept_site_mark(&mut self, profile: ProfileId, url: &url::Url, mark: &[u8]) {
        let Some(origin) = origin_of(url) else {
            return;
        };
        let key = (profile, origin.clone());
        if self.favicons.icon_values.contains_key(&key) || !self.cache_icon(key.clone(), mark) {
            return;
        }
        self.favicons.icons_checked.insert(key);
        if !self.incognito_profile(profile) {
            self.store.save_favicon(
                profile,
                origin,
                Some(zephium_core::icon::RGBA32_MIME.to_owned()),
                mark.to_vec(),
            );
        }
    }

    /// Names the focused profile, which is who the browser greets.
    pub(super) fn operation_rename_focused_profile(&mut self, name: &str) -> OperationDisposition {
        let Some(profile) = self.windows.focused().map(|window| window.profile) else {
            return operation_result(OperationOutcome::Rejected, OperationReason::NoFocusedWindow);
        };
        if !self.profiles.rename(profile, name) {
            return operation_result(OperationOutcome::Rejected, OperationReason::InvalidInput);
        }
        self.project_items();
        self.schedule_persist();
        operation_result(OperationOutcome::Applied, OperationReason::MutationApplied)
    }
}
