//! Whether the corner controls stay revealed in macOS fullscreen.
//!
//! AppKit hides the traffic lights when a window enters fullscreen and shows
//! them again only while the pointer is in the top-left region. The controls
//! Zephium draws in their place must follow that pointer, not a one-shot
//! entrance animation: they appear as soon as the pointer enters the corner
//! and stay there however long it lingers. They go only when the pointer
//! leaves. This is the state behind those decisions, independent of AppKit so
//! the behaviour can be tested off macOS.

use std::cell::Cell;

#[derive(Default)]
pub(crate) struct FullscreenReveal {
    fullscreen: Cell<bool>,
    inside: Cell<bool>,
    /// WillExitFullScreen has hidden the native buttons but DidExit has not
    /// laid the titlebar out again. No corner callback may change opacity in
    /// this window: the buttons must stay hidden until the final restore.
    exiting: Cell<bool>,
}

impl FullscreenReveal {
    /// Fullscreen begins or ends. Either way the old corner state is stale:
    /// a pointer already in the corner is rediscovered from its position.
    pub(crate) fn set_fullscreen(&self, fullscreen: bool) {
        self.fullscreen.set(fullscreen);
        self.inside.set(false);
        self.exiting.set(false);
    }

    /// The window is between WillExitFullScreen and DidExitFullScreen.
    pub(crate) fn begin_exit(&self) {
        self.exiting.set(true);
        self.inside.set(false);
    }

    pub(crate) fn exiting(&self) -> bool {
        self.exiting.get()
    }

    /// The pointer entered or left the top-left region.
    pub(crate) fn pointer(&self, inside: bool) {
        if !self.exiting.get() {
            self.inside.set(self.fullscreen.get() && inside);
        }
    }

    /// Whether the controls must be visible now. There is deliberately no
    /// timer that hides them while the pointer remains inside.
    pub(crate) fn visible(&self) -> bool {
        !self.exiting.get() && self.fullscreen.get() && self.inside.get()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_corner_reveal_only_exists_in_fullscreen() {
        let reveal = FullscreenReveal::default();
        reveal.pointer(true);
        assert!(!reveal.visible(), "in a normal window AppKit owns them");
        reveal.set_fullscreen(true);
        assert!(!reveal.visible(), "entering is not the same as hovering");
    }

    #[test]
    fn the_reveal_stays_as_long_as_the_pointer_stays_in_the_corner() {
        let reveal = FullscreenReveal::default();
        reveal.set_fullscreen(true);
        reveal.pointer(true);
        for _ in 0..1_000 {
            assert!(
                reveal.visible(),
                "no timer may hide them under a resting pointer"
            );
        }
    }

    #[test]
    fn leaving_the_corner_hides_the_reveal() {
        let reveal = FullscreenReveal::default();
        reveal.set_fullscreen(true);
        reveal.pointer(true);
        assert!(reveal.visible());
        reveal.pointer(false);
        assert!(!reveal.visible());
        reveal.pointer(false);
        assert!(!reveal.visible(), "repeated exits do not resurrect it");
    }

    #[test]
    fn will_exit_mouse_exited_did_exit_keeps_the_buttons_hidden() {
        let reveal = FullscreenReveal::default();
        reveal.set_fullscreen(true);
        reveal.pointer(true);
        assert!(reveal.visible());
        // WillExit -> a tracking-area exit before DidExit.
        reveal.begin_exit();
        reveal.pointer(false);
        assert!(reveal.exiting());
        assert!(!reveal.visible(), "no corner fade during the exit animation");
        // DidExit restores the ordinary controls and clears the barrier.
        reveal.set_fullscreen(false);
        assert!(!reveal.exiting());
        reveal.set_fullscreen(true);
        assert!(!reveal.visible(), "the stale corner state is not replayed");
    }

    #[test]
    fn exiting_fullscreen_drops_the_reveal_and_its_pending_corner_state() {
        let reveal = FullscreenReveal::default();
        reveal.set_fullscreen(true);
        reveal.pointer(true);
        reveal.set_fullscreen(false);
        assert!(!reveal.visible());
        reveal.set_fullscreen(true);
        assert!(!reveal.visible(), "the old pointer state is not replayed");
    }
}
