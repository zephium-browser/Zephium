//! The browser's install button on Chrome Web Store listings.
//!
//! Privileged chrome decides what the button says; the page script draws it
//! where the store's own "Add to Chrome" is. A click is admitted only for the
//! listing the view has committed, while the browser shows the button there,
//! outside private windows, and at most once a second; it then travels to the
//! shell, which starts the same reviewed install as the address field.

use std::time::{Duration, Instant};

use zephium_core::ids::ItemId;
use zephium_core::ports::engine::{
    EngineEvent, Partition, StoreButton, StoreButtonState, MAX_STORE_BUTTON_LABEL_CHARS,
};

use super::EngineHost;

const CLICK_INTERVAL: Duration = Duration::from_secs(1);
#[cfg(target_os = "windows")]
const POLL_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Default)]
pub(super) struct StoreButtonSlot {
    shown: Option<StoreButton>,
    last_click: Option<Instant>,
    #[cfg(target_os = "macos")]
    native: Option<crate::platform::macos::store_button::StoreButton>,
    #[cfg(target_os = "windows")]
    poll: Option<crate::platform::imp::ContentPolicyTimeout>,
    // Polling stopped because the listing went out of view, and starts again
    // when it comes back.
    #[cfg(target_os = "windows")]
    paused: bool,
}

impl StoreButtonSlot {
    #[cfg(target_os = "macos")]
    pub(super) fn native(
        native: Option<crate::platform::macos::store_button::StoreButton>,
    ) -> Self {
        Self {
            native,
            ..Self::default()
        }
    }
}

fn encode(button: &StoreButton) -> String {
    let kind = match button.state {
        StoreButtonState::Add => "add",
        StoreButtonState::Adding => "adding",
        StoreButtonState::Remove => "remove",
    };
    serde_json::json!({ "id": button.extension, "kind": kind, "label": button.label }).to_string()
}

/// A click from the page counts only on the listing it is for, while that
/// extension's button is offered, and not twice in quick succession.
fn click_admitted(
    listed: bool,
    shown: Option<&StoreButton>,
    last_click: Option<Instant>,
    extension: &str,
    now: Instant,
) -> bool {
    let offered = shown.is_some_and(|button| {
        button.extension == extension && button.state != StoreButtonState::Adding
    });
    listed && offered && last_click.is_none_or(|last| now.duration_since(last) >= CLICK_INTERVAL)
}

impl EngineHost {
    fn committed_listing(&self, id: ItemId) -> Option<zephium_webext::ExtensionId> {
        if self
            .partitions
            .get(&id)
            .is_none_or(|partition| matches!(partition, Partition::Ephemeral(_)))
        {
            return None;
        }
        let (_, url) = self.views.get(&id)?.navigation.committed_snapshot()?;
        zephium_webext::store::listing_id(&url)
    }

    pub(crate) fn show_store_button(&mut self, id: ItemId, button: StoreButton) {
        let label_chars = button.label.chars().count();
        if label_chars == 0
            || label_chars > MAX_STORE_BUTTON_LABEL_CHARS
            || self
                .committed_listing(id)
                .is_none_or(|listing| listing.as_str() != button.extension)
        {
            return;
        }
        let Some(view) = self.views.get_mut(&id) else {
            return;
        };
        view.store_button.shown = Some(button);
        self.send_store_button(id);
        #[cfg(target_os = "windows")]
        {
            // Only the listing in front is polled.
            for (other, view) in &mut self.views {
                if *other != id {
                    view.store_button.poll = None;
                    view.store_button.paused = false;
                }
            }
            self.schedule_store_button_poll(id);
        }
    }

    fn send_store_button(&self, id: ItemId) {
        let Some(view) = self.views.get(&id) else {
            return;
        };
        let Some(state) = view.store_button.shown.as_ref().map(encode) else {
            return;
        };
        #[cfg(target_os = "macos")]
        if let Some(native) = &view.store_button.native {
            native.show(&state);
        }
        #[cfg(target_os = "windows")]
        let _ = view
            .view
            .evaluate_script(&format!("window.__zephium_store_v1__?.show({state})"));
    }

    fn store_button_clicked(&mut self, id: ItemId, extension: &str) {
        let listed = self
            .committed_listing(id)
            .is_some_and(|listing| listing.as_str() == extension);
        let sink = self.sink.clone();
        let Some(view) = self.views.get_mut(&id) else {
            return;
        };
        let slot = &mut view.store_button;
        if !click_admitted(
            listed,
            slot.shown.as_ref(),
            slot.last_click,
            extension,
            Instant::now(),
        ) {
            return;
        }
        slot.last_click = Some(Instant::now());
        view.event_permit.emit(
            &sink,
            EngineEvent::StoreButtonClicked {
                id,
                extension: extension.to_owned(),
            },
        );
    }

    #[cfg(target_os = "macos")]
    pub(super) fn store_button_message(
        &mut self,
        id: ItemId,
        message: crate::platform::macos::store_button::StoreMessage,
    ) {
        use crate::platform::macos::store_button::StoreMessage;
        match message {
            StoreMessage::Ready => self.send_store_button(id),
            StoreMessage::Clicked(extension) => self.store_button_clicked(id, extension.as_str()),
        }
    }

    /// WebView2 runs the store script in the page's own world, which has no
    /// bridge to the browser, so the listing in front is asked for clicks.
    #[cfg(target_os = "windows")]
    fn schedule_store_button_poll(&mut self, id: ItemId) {
        let hidden = self.hidden.contains(&id);
        let Some(view) = self.views.get_mut(&id) else {
            return;
        };
        // A listing out of view cannot be clicked.
        view.store_button.paused = hidden;
        if hidden {
            view.store_button.poll = None;
            return;
        }
        view.store_button.poll =
            crate::platform::imp::schedule_browser_timeout(POLL_INTERVAL, move || {
                let _ = super::try_with(move |host| host.poll_store_button(id));
            });
    }

    #[cfg(target_os = "windows")]
    pub(super) fn resume_store_button_poll(&mut self, id: ItemId) {
        if self
            .views
            .get(&id)
            .is_some_and(|view| view.store_button.paused && view.store_button.shown.is_some())
        {
            self.schedule_store_button_poll(id);
        }
    }

    #[cfg(target_os = "windows")]
    fn poll_store_button(&mut self, id: ItemId) {
        let listing = self.committed_listing(id);
        let Some(view) = self.views.get_mut(&id) else {
            return;
        };
        view.store_button.poll = None;
        let current = view.store_button.shown.as_ref().zip(listing.as_ref());
        if current.is_none_or(|(button, listing)| button.extension != listing.as_str()) {
            view.store_button.shown = None;
            return;
        }
        let Some(epoch) = view.navigation.current() else {
            return;
        };
        let (permit, navigation) = (view.event_permit.clone(), view.navigation.clone());
        let polled = view.view.evaluate_script_with_callback(
            "window.__zephium_store_v1__?.poll() ?? ''",
            move |result| {
                let (permit, navigation) = (permit.clone(), navigation.clone());
                let _ = super::try_with(move |host| {
                    let current = host.views.get(&id).is_some_and(|view| {
                        super::permits::navigation_callback_matches(
                            &view.event_permit,
                            &view.navigation,
                            &permit,
                            &navigation,
                            epoch,
                        )
                    });
                    if current {
                        host.store_button_polled(id, &result);
                    }
                });
            },
        );
        if polled.is_err() {
            self.schedule_store_button_poll(id);
        }
    }

    #[cfg(target_os = "windows")]
    fn store_button_polled(&mut self, id: ItemId, result: &str) {
        match serde_json::from_str::<String>(result).as_deref() {
            Ok("?") => self.send_store_button(id),
            Ok(extension) if zephium_webext::ExtensionId::parse(extension).is_some() => {
                self.store_button_clicked(id, extension);
            }
            _ => {}
        }
        if self
            .views
            .get(&id)
            .is_some_and(|view| view.store_button.shown.is_some())
        {
            self.schedule_store_button_poll(id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_page_gets_exactly_the_button_state() {
        let button = StoreButton {
            extension: "ddkjiahejlhfcafbddmgiahcphecmpfh".into(),
            state: StoreButtonState::Adding,
            label: "Adding</script>\u{2028}".into(),
        };
        let value: serde_json::Value = serde_json::from_str(&encode(&button)).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "id": "ddkjiahejlhfcafbddmgiahcphecmpfh",
                "kind": "adding",
                "label": "Adding</script>\u{2028}",
            })
        );
    }

    #[test]
    fn a_click_is_admitted_only_for_an_offered_button_once_a_second() {
        const ID: &str = "ddkjiahejlhfcafbddmgiahcphecmpfh";
        let button = |extension: &str, state| StoreButton {
            extension: extension.into(),
            state,
            label: "Add".into(),
        };
        let now = Instant::now();
        let add = button(ID, StoreButtonState::Add);
        let admitted = |listed, shown: Option<&StoreButton>, last| {
            click_admitted(listed, shown, last, ID, now)
        };
        assert!(admitted(true, Some(&add), None));
        assert!(!admitted(false, Some(&add), None), "another listing");
        assert!(!admitted(true, None, None), "no button shown");
        assert!(
            !admitted(
                true,
                Some(&button(
                    "ponfpcnoihfmfllpaingbgckeeldkhle",
                    StoreButtonState::Add
                )),
                None
            ),
            "the button is for another extension"
        );
        assert!(!admitted(
            true,
            Some(&button(ID, StoreButtonState::Adding)),
            None
        ));
        assert!(!admitted(true, Some(&add), Some(now)), "a repeat click");
        assert!(admitted(true, Some(&add), Some(now - CLICK_INTERVAL)));
    }
}
