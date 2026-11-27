//! Shell-owned logical browser routing for authorized extension profiles.

use std::collections::HashMap;

use super::*;

#[derive(Default)]
pub(super) struct ExtensionBrowserSurfaceState {
    active_profiles: zephium_core::extensions::ExtensionActiveProfiles,
    retry_profiles: zephium_core::extensions::ExtensionActiveProfiles,
    retiring_profiles: zephium_core::extensions::ExtensionActiveProfiles,
    published: HashMap<ProfileId, ExtensionBrowserSurface>,
}

#[derive(Default)]
pub(super) struct ExtensionBrowserSurfaceSync {
    pub(super) native: NativeWork,
    failed_profiles: zephium_core::extensions::ExtensionActiveProfiles,
}

impl ExtensionBrowserSurfaceSync {
    pub(super) fn failed(&self, profile: ProfileId) -> bool {
        self.failed_profiles.contains(profile)
    }

    fn record_failure(&mut self, profile: ProfileId) {
        // The active profile set is already bounded by the same constant, so
        // insertion cannot fail unless internal state lost its invariant.
        if !self.failed_profiles.try_insert(profile) {
            self.native.rejected = true;
        }
    }

    fn merge(&mut self, other: Self) {
        self.native.merge(other.native);
        for profile in other.failed_profiles.iter() {
            self.record_failure(profile);
        }
    }
}

impl ExtensionBrowserSurfaceState {
    pub(super) fn retire_profile(&mut self, profile: ProfileId) {
        self.active_profiles.remove(profile);
        self.retry_profiles.remove(profile);
        self.retiring_profiles.remove(profile);
        self.published.remove(&profile);
    }

    /// Replaces the complete same-turn runtime routing cohort. Profiles that
    /// lost their final runtime are retained in a fixed-size retirement set
    /// until an empty native browser surface is accepted.
    pub(super) fn replace_active_profiles(
        &mut self,
        profiles: zephium_core::extensions::ExtensionActiveProfiles,
    ) -> bool {
        for profile in self.active_profiles.iter() {
            if !profiles.contains(profile) && !self.retiring_profiles.try_insert(profile) {
                return false;
            }
        }
        for profile in profiles.iter() {
            self.retiring_profiles.remove(profile);
        }
        self.active_profiles = profiles;
        true
    }

    pub(super) fn active_profiles(&self) -> zephium_core::extensions::ExtensionActiveProfiles {
        self.active_profiles
    }

    pub(super) fn is_active(&self, profile: ProfileId) -> bool {
        self.active_profiles.contains(profile)
    }

    fn published(&self, profile: ProfileId) -> Option<&ExtensionBrowserSurface> {
        self.published.get(&profile)
    }

    pub(super) fn published_surface(&self, profile: ProfileId) -> Option<&ExtensionBrowserSurface> {
        self.published(profile)
    }

    fn record_published(&mut self, surface: ExtensionBrowserSurface) {
        self.retry_profiles.remove(surface.profile());
        self.published.insert(surface.profile(), surface);
    }

    fn record_retry(&mut self, profile: ProfileId) -> bool {
        self.retry_profiles.try_insert(profile)
    }

    fn retry_profiles(&self) -> zephium_core::extensions::ExtensionActiveProfiles {
        self.retry_profiles
    }

    fn retiring_profiles(&self) -> zephium_core::extensions::ExtensionActiveProfiles {
        self.retiring_profiles
    }
}

impl Shell {
    /// Publishes only changed logical routing state and never constructs a
    /// native view. With no active runtime profile this is an allocation-free
    /// branch over one fixed-size value and an empty `HashMap`.
    pub(super) fn sync_extension_browser_surfaces(&mut self) -> ExtensionBrowserSurfaceSync {
        let mut settlement = self.retire_extension_browser_surfaces();
        let profiles = self.extension_browser_surfaces.active_profiles();
        settlement.merge(self.sync_extension_browser_surface_profiles(profiles));
        settlement
    }

    /// Retries only transient native admissions retained from an earlier
    /// projection. The ordinary maintenance tick stays allocation-free when
    /// no extension surface is waiting for native queue capacity.
    pub(super) fn retry_extension_browser_surfaces(&mut self) -> ExtensionBrowserSurfaceSync {
        let mut settlement = self.retire_extension_browser_surfaces();
        let profiles = self.extension_browser_surfaces.retry_profiles();
        settlement.merge(self.sync_extension_browser_surface_profiles(profiles));
        settlement
    }

    fn retire_extension_browser_surfaces(&mut self) -> ExtensionBrowserSurfaceSync {
        let profiles = self.extension_browser_surfaces.retiring_profiles();
        let mut settlement = ExtensionBrowserSurfaceSync::default();
        for profile in profiles.iter() {
            let generation = match self
                .extension_browser_surfaces
                .published(profile)
                .map(ExtensionBrowserSurface::generation)
            {
                Some(current) => match current.next() {
                    Some(next) => next,
                    None => {
                        settlement.native.rejected = true;
                        settlement.record_failure(profile);
                        continue;
                    }
                },
                None => {
                    self.extension_browser_surfaces
                        .retiring_profiles
                        .remove(profile);
                    self.extension_actions.retire_profile(profile);
                    self.project_extension_actions(profile);
                    continue;
                }
            };
            let surface = ExtensionBrowserSurface::new(profile, generation, None, Vec::new())
                .expect("empty extension browser surface is bounded and valid");
            let admission = self.engine.set_extension_browser_surface(surface.clone());
            settlement.native.record(admission);
            match admission {
                NativeDispatch::Scheduled => {
                    // Retain the accepted empty generation as a compact
                    // monotonic floor. Re-enabling the first runtime in this
                    // profile must publish generation N+1, never restart at
                    // INITIAL against an engine/controller that already saw
                    // the retirement generation.
                    self.extension_browser_surfaces.record_published(surface);
                    self.extension_browser_surfaces
                        .retry_profiles
                        .remove(profile);
                    self.extension_browser_surfaces
                        .retiring_profiles
                        .remove(profile);
                    self.extension_actions.retire_profile(profile);
                    self.project_extension_actions(profile);
                }
                NativeDispatch::Rejected | NativeDispatch::Unsupported => {
                    settlement.record_failure(profile)
                }
            }
        }
        settlement
    }

    pub(super) fn sync_extension_browser_surface(
        &mut self,
        profile: ProfileId,
    ) -> ExtensionBrowserSurfaceSync {
        let mut profiles = zephium_core::extensions::ExtensionActiveProfiles::EMPTY;
        if self
            .extension_browser_surfaces
            .active_profiles()
            .contains(profile)
        {
            // A member of the bounded active set always fits in an empty set.
            let inserted = profiles.try_insert(profile);
            debug_assert!(inserted);
        }
        self.sync_extension_browser_surface_profiles(profiles)
    }

    fn sync_extension_browser_surface_profiles(
        &mut self,
        profiles: zephium_core::extensions::ExtensionActiveProfiles,
    ) -> ExtensionBrowserSurfaceSync {
        if profiles.is_empty() {
            return ExtensionBrowserSurfaceSync::default();
        }

        let mut settlement = ExtensionBrowserSurfaceSync::default();
        for profile in profiles.iter() {
            let generation = match self
                .extension_browser_surfaces
                .published(profile)
                .map(ExtensionBrowserSurface::generation)
            {
                Some(current) => match current.next() {
                    Some(next) => next,
                    None => {
                        settlement.native.rejected = true;
                        settlement.record_failure(profile);
                        continue;
                    }
                },
                None => ExtensionBrowserSurfaceGeneration::INITIAL,
            };
            let surface = match self.build_extension_browser_surface(profile, generation) {
                Ok(surface) => surface,
                Err(reason) => {
                    crate::diagnostic!(
                        "extensions: logical browser surface is unavailable: {reason}"
                    );
                    settlement.native.rejected = true;
                    settlement.record_failure(profile);
                    continue;
                }
            };
            if self
                .extension_browser_surfaces
                .published(profile)
                .is_some_and(|current| same_logical_surface(current, &surface))
            {
                continue;
            }
            let admission = self.engine.set_extension_browser_surface(surface.clone());
            settlement.native.record(admission);
            match admission {
                NativeDispatch::Scheduled => {
                    self.extension_browser_surfaces.record_published(surface);
                    // Replace any snapshot tied to the previous surface
                    // generation before the asynchronous native read. A
                    // same-tab reconstruction must never leave a stale button
                    // actionable or visible while its replacement settles.
                    self.project_extension_actions(profile);
                    let actions = self.refresh_extension_actions(profile);
                    if actions.rejected {
                        crate::diagnostic!(
                            "extensions: toolbar action refresh awaits maintenance retry"
                        );
                    }
                }
                NativeDispatch::Rejected => {
                    if !self.extension_browser_surfaces.record_retry(profile) {
                        settlement.native.rejected = true;
                    }
                    settlement.record_failure(profile)
                }
                NativeDispatch::Unsupported => settlement.record_failure(profile),
            }
        }
        settlement
    }

    fn build_extension_browser_surface(
        &self,
        profile: ProfileId,
        generation: ExtensionBrowserSurfaceGeneration,
    ) -> Result<ExtensionBrowserSurface, &'static str> {
        let mut windows = self
            .windows
            .iter()
            .filter(|window| window.profile == profile);
        let first = windows.next();
        if windows.next().is_some() {
            // Items are currently profile/space-owned rather than window-owned.
            // Duplicating one ItemId across native windows would be an unsafe
            // lie; keep this explicit until the product model gains ownership.
            return Err("multiple windows share one profile without tab ownership");
        }
        let Some(window) = first else {
            return ExtensionBrowserSurface::new(profile, generation, None, Vec::new())
                .map_err(|_| "empty browser surface failed validation");
        };
        if self
            .spaces
            .get(window.space)
            .is_none_or(|space| space.profile != profile)
        {
            return Err("window space crossed profile ownership");
        }

        let mut tabs = Vec::new();
        // Current Zephium sessions expose at most one extension-visible window
        // per profile. Reuse immutable metadata from the prior ordered surface
        // on the common unchanged-order path, avoiding URL/title copies on
        // unrelated Shell commits.
        let previous_tabs = self
            .extension_browser_surfaces
            .published(profile)
            .and_then(|surface| surface.windows().first())
            .map_or(&[][..], ExtensionBrowserWindow::tabs);
        let mut previous_index = 0_usize;
        for space in self.spaces.iter().filter(|space| space.profile == profile) {
            for placement in zephium_core::session::space_placements(space.id) {
                self.append_extension_placement_tabs(
                    placement,
                    previous_tabs,
                    &mut previous_index,
                    &mut tabs,
                )?;
            }
        }

        let private = self
            .profiles
            .get(profile)
            .ok_or("active extension profile is absent from the session")?
            .kind
            == ProfileKind::Incognito;
        let active = window
            .active
            .filter(|id| self.extension_tab_in_scope(*id, profile));
        let logical = ExtensionBrowserWindow::new(window.id, private, active, tabs)
            .map_err(|_| "logical extension window failed validation")?;
        let focused = self
            .windows
            .focused()
            .filter(|focused| focused.id == window.id)
            .map(|focused| focused.id);
        ExtensionBrowserSurface::new(profile, generation, focused, vec![logical])
            .map_err(|_| "logical extension surface failed validation")
    }

    fn append_extension_placement_tabs(
        &self,
        placement: Placement,
        previous_tabs: &[ExtensionBrowserTab],
        previous_index: &mut usize,
        tabs: &mut Vec<ExtensionBrowserTab>,
    ) -> Result<(), &'static str> {
        for &id in self.items.roots(placement) {
            self.append_extension_node_tabs(
                id,
                None,
                placement,
                previous_tabs,
                previous_index,
                tabs,
            )?;
        }
        Ok(())
    }

    fn append_extension_node_tabs(
        &self,
        id: ItemId,
        parent: Option<ItemId>,
        placement: Placement,
        previous_tabs: &[ExtensionBrowserTab],
        previous_index: &mut usize,
        tabs: &mut Vec<ExtensionBrowserTab>,
    ) -> Result<(), &'static str> {
        let Some(item) = self
            .items
            .get(id)
            .filter(|item| item.parent == parent && item.placement == placement)
        else {
            return Ok(());
        };
        match &item.kind {
            ItemKind::Tab(tab) => {
                if self
                    .profile_of_item(id)
                    .is_none_or(|profile| !self.extension_tab_in_scope(id, profile))
                {
                    return Ok(());
                }
                let previous = previous_tabs
                    .get(*previous_index)
                    .filter(|previous| previous.id() == id);
                *previous_index = previous_index
                    .checked_add(1)
                    .ok_or("extension browser tab index overflowed")?;
                let pinned = !matches!(
                    placement,
                    Placement::Space {
                        section: SpaceSection::Today,
                        ..
                    }
                );
                // One odd tab must not take every extension's view of the
                // window down: an unshowable title or address is withheld.
                let title = if zephium_core::item::page_title_is_sanitized(&tab.title) {
                    std::borrow::Cow::Borrowed(tab.title.as_str())
                } else {
                    std::borrow::Cow::Owned(zephium_core::item::sanitize_page_title(&tab.title))
                };
                let url = tab
                    .url
                    .as_ref()
                    .filter(|url| zephium_core::navigation::is_browser_target(url));
                tabs.push(
                    ExtensionBrowserTab::from_snapshot(
                        previous,
                        id,
                        tab.has_view(),
                        &title,
                        url,
                        tab.loading,
                        pinned,
                    )
                    .map_err(|_| "extension browser tab metadata failed validation")?
                    .with_audio(tab.audio.muted, tab.audio.shown()),
                );
            }
            ItemKind::Folder { .. } => {
                for &child in self.items.children(id) {
                    self.append_extension_node_tabs(
                        child,
                        Some(id),
                        placement,
                        previous_tabs,
                        previous_index,
                        tabs,
                    )?;
                }
            }
        }
        Ok(())
    }
}

fn same_logical_surface(
    current: &ExtensionBrowserSurface,
    candidate: &ExtensionBrowserSurface,
) -> bool {
    current.profile() == candidate.profile()
        && current.focused() == candidate.focused()
        && current.windows() == candidate.windows()
}
