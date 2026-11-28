//! The keys a frameless window's caption owns. WebView2 consumes them before
//! the window sees them, so the accelerator hook hands them over.

const VK_SPACE: u32 = 0x20;
const VK_F10: u32 = 0x79;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CaptionKey {
    /// F10: keyboard access to the caption buttons.
    Buttons,
    /// Alt+Space: the window's system menu.
    SystemMenu,
}

pub(crate) fn classify(key: u32, ctrl: bool, shift: bool, alt: bool) -> Option<CaptionKey> {
    match key {
        VK_F10 if !ctrl && !shift && !alt => Some(CaptionKey::Buttons),
        VK_SPACE if alt && !ctrl && !shift => Some(CaptionKey::SystemMenu),
        _ => None,
    }
}

/// Whether a WebView2 accelerator may end fullscreen before the page sees
/// the key. None may: other browsers leave a normal page fullscreen on a
/// bare Escape, and a page's own element fullscreen is handled by WebView2
/// itself. The browser's window fullscreen is left through the
/// `window.fullscreen` command (F11), matched as an ordinary shortcut with
/// the person's own binding.
pub(crate) fn accelerator_exits_fullscreen(
    _key: u32,
    _ctrl: bool,
    _shift: bool,
    _alt: bool,
) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f10_and_alt_space_belong_to_the_caption() {
        assert_eq!(
            classify(VK_F10, false, false, false),
            Some(CaptionKey::Buttons)
        );
        assert_eq!(
            classify(VK_SPACE, false, false, true),
            Some(CaptionKey::SystemMenu)
        );
    }

    #[test]
    fn no_accelerator_key_forces_a_fullscreen_exit() {
        // Escape is the page's key; F11 is the window.fullscreen shortcut,
        // which toggles fullscreen once through the command path.
        for (key, ctrl, shift, alt) in [
            (0x1B, false, false, false), // bare Escape
            (0x1B, true, false, false),
            (0x7A, false, false, false), // F11
            (VK_F10, false, false, false),
        ] {
            assert!(!accelerator_exits_fullscreen(key, ctrl, shift, alt));
        }
    }

    #[test]
    fn other_chords_stay_with_the_page() {
        // Shift+F10 is the context-menu key; Ctrl+Alt+Space is not a menu chord.
        assert_eq!(classify(VK_F10, false, true, false), None);
        assert_eq!(classify(VK_F10, true, false, false), None);
        assert_eq!(classify(VK_F10, false, false, true), None);
        assert_eq!(classify(VK_SPACE, false, false, false), None);
        assert_eq!(classify(VK_SPACE, true, false, true), None);
        assert_eq!(classify(VK_SPACE, false, true, true), None);
        assert_eq!(classify(0x41, false, false, true), None);
    }
}
