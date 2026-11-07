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
