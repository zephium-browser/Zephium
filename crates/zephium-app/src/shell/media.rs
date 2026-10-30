//! The sidebar's media player. A tab's page is watched from the moment it
//! starts making sound until it has no media left; the player follows the
//! latest of the focused profile's tabs to start playing that is not on
//! screen. Runtime only, private tabs alike; agent pages have no tab.

use super::*;
use zephium_core::item::TabContent;
use zephium_core::media::MediaSnapshot;
use zephium_core::ports::engine::MediaAction;
use zephium_ipc::{MediaPlayerAction, MediaPlayerView};

impl Shell {
    /// A tab began making sound: its media may be worth a player.
    pub(super) fn watch_tab_media(&self, id: ItemId) {
        if self
            .items
            .tab(id)
            .is_some_and(|tab| tab.content == TabContent::Web && tab.has_view())
        {
            let _ = self.engine.watch_media(id);
        }
    }

    pub(super) fn on_media_changed(&mut self, id: ItemId, media: Option<MediaSnapshot>) {
        let live = self
            .items
            .tab(id)
            .is_some_and(|tab| tab.content == TabContent::Web && tab.has_view());
        let media = media.filter(|_| live);
        if self.media.observe(id, media, std::time::Instant::now()) {
            self.project_media_player();
        }
    }

    /// The tab closed, or its page went away with its media.
    pub(super) fn forget_tab_media(&mut self, id: ItemId) {
        if self.media.forget(id) {
            self.project_media_player();
        }
    }

    fn on_screen(&self, id: ItemId) -> bool {
        self.windows.iter().any(|window| {
            window.active == Some(id)
                || window.splits.as_ref().is_some_and(|tree| tree.contains(id))
        })
    }

    fn shown_media(&self) -> Option<(ItemId, &MediaSnapshot, std::time::Instant)> {
        let profile = self.windows.focused()?.profile;
        if self.profile_deletion_quarantines(profile) {
            return None;
        }
        self.media.shown(
            |id| {
                self.items
                    .tab(id)
                    .is_some_and(|tab| tab.content == TabContent::Web && tab.has_view())
                    && self.profile_of_item(id) == Some(profile)
            },
            |id| self.on_screen(id),
        )
    }

    pub(super) fn media_player_tab(&self) -> Option<ItemId> {
        self.shown_media().map(|(id, ..)| id)
    }

    /// The player as of the page's last report, so a later projection of the
    /// same report compares equal.
    fn media_player_view(&self) -> Option<(MediaPlayerView, std::time::Instant)> {
        let (id, media, at) = self.shown_media()?;
        let tab = self.items.tab(id)?;
        let profile = self.profile_of_item(id);
        let title = if media.title.is_empty() {
            tab.title.clone()
        } else {
            media.title.clone()
        };
        let artist = if media.artist.is_empty() {
            tab.url
                .as_ref()
                .and_then(|url| url.host_str())
                .map(|host| host.strip_prefix("www.").unwrap_or(host).to_owned())
                .unwrap_or_default()
        } else {
            media.artist.clone()
        };
        let view = MediaPlayerView {
            tab_id: id.to_string(),
            title,
            artist,
            icon: self.tab_icon(id, tab, profile),
            duration: media.duration,
            position: media.position,
            rate: media.rate,
            playing: media.playing,
            waiting: media.waiting,
            can_pip: media.can_pip,
            in_pip: media.in_pip,
            muted: tab.audio.muted,
            can_mute: self.engine.can_mute(),
        };
        Some((view, at))
    }

    /// Sends the player when it differs from what chrome holds. The position
    /// goes out as of now; chrome runs it on from there.
    pub(super) fn project_media_player(&self) {
        let current = self.media_player_view();
        let reported = current.as_ref().map(|(view, _)| view.clone());
        {
            let Ok(mut last) = self.presentation.last_media_player.try_borrow_mut() else {
                return;
            };
            if last.as_ref() == Some(&reported) {
                return;
            }
            *last = Some(reported);
        }
        let view = current.map(|(mut view, at)| {
            if let Some((_, media, _)) = self.shown_media() {
                view.position = media.position_after(at.elapsed().as_secs_f64());
            }
            view
        });
        self.publish_icons();
        (self.emit)(Projection::MediaPlayer(view));
    }

    /// The player can show a tab of another space; going to it shows that
    /// space, arriving on the tab.
    pub(super) fn show_media_tab(&mut self, id: ItemId) -> OperationDisposition {
        if self.item_in_focused_scope(id) {
            return self.operation_activate(id);
        }
        let Some(&(Placement::Favorites { space } | Placement::Space { space, .. })) =
            self.items.get(id).map(|item| &item.placement)
        else {
            return operation_result(OperationOutcome::Rejected, OperationReason::InvalidScope);
        };
        if self
            .active_browser_page()
            .is_some_and(|page| page != crate::BrowserPage::Work)
            && !self.browser_return_ready
        {
            self.browser_after_return = Some(Box::new(Command::MediaPlayer {
                id,
                action: MediaPlayerAction::Show,
            }));
            return self.operation_show_browser_page(None);
        }
        let (effects, blank) = self.show_space_at(space, id);
        let native = self.commit(effects);
        if let Some(blank) = blank {
            self.close_in_any_space(blank);
        }
        mutation_result(native)
    }

    pub(super) fn operation_media_player(
        &mut self,
        id: ItemId,
        action: MediaPlayerAction,
    ) -> OperationDisposition {
        if self.media_player_tab() != Some(id) {
            return operation_result(OperationOutcome::Rejected, OperationReason::InvalidScope);
        }
        let action = match action {
            MediaPlayerAction::Close => {
                self.media.dismiss(id);
                self.project_media_player();
                return operation_result(
                    OperationOutcome::Applied,
                    OperationReason::MutationApplied,
                );
            }
            MediaPlayerAction::Show => return self.show_media_tab(id),
            MediaPlayerAction::Play => MediaAction::Play,
            MediaPlayerAction::Pause => MediaAction::Pause,
            MediaPlayerAction::Seek { seconds } if seconds.is_finite() && seconds >= 0.0 => {
                MediaAction::Seek(seconds)
            }
            MediaPlayerAction::Seek { .. } => {
                return operation_result(OperationOutcome::Rejected, OperationReason::InvalidInput);
            }
            MediaPlayerAction::PictureInPicture => MediaAction::PictureInPicture,
        };
        // The page's answer comes back through the watch.
        if self.engine.media_action(id, action) == NativeDispatch::Scheduled {
            operation_result(OperationOutcome::Applied, OperationReason::MutationApplied)
        } else {
            operation_result(
                OperationOutcome::NativeAdmissionFailed,
                OperationReason::NativeDispatchRejected,
            )
        }
    }
}
