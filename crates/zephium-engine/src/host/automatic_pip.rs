//! Automatic video PiP is initiated only by the browser's tab-selection port.
use super::EngineHost;
use crate::fullscreen::NativeFullscreen;
use std::rc::Rc;
use zephium_core::ids::ItemId;

const SCRIPT: &str = include_str!("../platform/automatic_pip.js");

impl EngineHost {
    pub(crate) fn automatic_pip(&self, id: ItemId, background: bool) {
        let Some(view) = self.views.get(&id) else {
            return;
        };
        let sequence = view
            .automatic_pip_sequence
            .get()
            .checked_add(1)
            .filter(|sequence| *sequence <= 9_007_199_254_740_991);
        let Some(sequence) = sequence else {
            return;
        };
        view.automatic_pip_sequence.set(sequence);
        // The shell asks a page out of element fullscreen as its tab is left,
        // and a page does not offer its video until it is out. WebKit leaves
        // fullscreen asynchronously, so ask again once it has.
        view.automatic_pip_after_fullscreen.set(
            background
                && crate::platform::imp::fullscreen_state(&view.view) != NativeFullscreen::Inactive,
        );
        if view.automatic_pip_after_fullscreen.get() {
            return;
        }
        let Some((epoch, url)) = view.navigation.committed_snapshot() else {
            return;
        };
        if background
            && (!view.presentable || !url.starts_with("https://") && !url.starts_with("http://"))
        {
            return;
        }
        let permit = view.event_permit.clone();
        let navigation = view.navigation.clone();
        let current_sequence = view.automatic_pip_sequence.clone();
        let current: Rc<dyn Fn() -> bool> = Rc::new(move || {
            permit.active_token().is_some()
                && navigation.current_committed() == Some(epoch)
                && current_sequence.get() == sequence
        });
        let action = if background { "leave" } else { "return" };
        let url = serde_json::to_string(&url).expect("a string is serializable");
        let script =
            format!("location.href === {url} ? ({SCRIPT})('{action}', {sequence}) : false");
        crate::platform::imp::automatic_pip(&view.view, script, current);
    }

    /// `id` has left element fullscreen: offer its video now if its tab was
    /// left while it was still fullscreen and has not been shown since.
    pub(super) fn automatic_pip_after_fullscreen(&self, id: ItemId) {
        let Some(view) = self
            .views
            .get(&id)
            .filter(|view| view.automatic_pip_after_fullscreen.get())
        else {
            return;
        };
        #[cfg(target_os = "macos")]
        {
            // WebKit reports the view out of fullscreen before its document
            // hears of it, and the page declines while it is still fullscreen.
            let sequence = view.automatic_pip_sequence.get();
            let current = view.automatic_pip_sequence.clone();
            crate::platform::imp::after_document_fullscreen(&view.view, move || {
                if current.get() == sequence {
                    let _ = super::dispatch::try_with(move |host| host.automatic_pip(id, true));
                }
            });
        }
        #[cfg(target_os = "windows")]
        {
            let _ = view;
            self.automatic_pip(id, true);
        }
    }

    pub(crate) fn cancel_automatic_pip(&self) {
        for (&id, view) in &self.views {
            if view.automatic_pip_sequence.get() != 0 {
                self.automatic_pip(id, false);
            }
        }
    }
}
