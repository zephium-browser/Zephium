//! Keyboard accelerators as data. One parser serves the registry defaults,
//! user overrides, native menus and every per-platform key table, so a
//! shortcut means the same keys wherever it is read.

use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Platform {
    Mac,
    Windows,
    Other,
}

impl Platform {
    pub const CURRENT: Platform = if cfg!(target_os = "macos") {
        Platform::Mac
    } else if cfg!(target_os = "windows") {
        Platform::Windows
    } else {
        Platform::Other
    };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    /// Uppercase ASCII letter.
    Letter(u8),
    /// ASCII digit.
    Digit(u8),
    /// ASCII digit on the Windows numeric keypad.
    NumpadDigit(u8),
    Comma,
    Period,
    Minus,
    Equal,
    BracketLeft,
    BracketRight,
    Semicolon,
    Quote,
    Slash,
    Backslash,
    Backquote,
    Tab,
    Space,
    Enter,
    Escape,
    Backspace,
    Delete,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    /// F1 through F12.
    Function(u8),
    /// Any other printable character, matched by the active keyboard layout.
    Character(char),
}

impl Key {
    fn parse(token: &str, platform: Platform) -> Option<Key> {
        let upper = token.to_ascii_uppercase();
        let bytes = upper.as_bytes();
        if bytes.len() == 1 {
            let byte = bytes[0];
            if byte.is_ascii_uppercase() {
                return Some(Key::Letter(byte));
            }
            if byte.is_ascii_digit() {
                return Some(Key::Digit(byte));
            }
        }
        // Physical key codes, as a recorder reads them from a key event.
        if let Some(rest) = upper
            .strip_prefix("KEY")
            .or_else(|| upper.strip_prefix("DIGIT"))
        {
            return match rest.as_bytes() {
                [byte] if byte.is_ascii_uppercase() && upper.starts_with("KEY") => {
                    Some(Key::Letter(*byte))
                }
                [byte] if byte.is_ascii_digit() && upper.starts_with("DIGIT") => {
                    Some(Key::Digit(*byte))
                }
                _ => None,
            };
        }
        if platform == Platform::Windows {
            if let Some(rest) = upper.strip_prefix("NUMPAD") {
                return match rest.as_bytes() {
                    [byte] if byte.is_ascii_digit() => Some(Key::NumpadDigit(*byte)),
                    _ => None,
                };
            }
        }
        if let Some(number) = upper.strip_prefix('F').and_then(|n| n.parse::<u8>().ok()) {
            return (1..=12).contains(&number).then_some(Key::Function(number));
        }
        Some(match upper.as_str() {
            "," | "COMMA" => Key::Comma,
            "." | "PERIOD" => Key::Period,
            "-" | "MINUS" => Key::Minus,
            "=" | "EQUAL" | "PLUS" => Key::Equal,
            "[" | "BRACKETLEFT" => Key::BracketLeft,
            "]" | "BRACKETRIGHT" => Key::BracketRight,
            ";" | "SEMICOLON" => Key::Semicolon,
            "'" | "QUOTE" => Key::Quote,
            "/" | "SLASH" => Key::Slash,
            "\\" | "BACKSLASH" => Key::Backslash,
            "`" | "BACKQUOTE" => Key::Backquote,
            "TAB" => Key::Tab,
            "SPACE" => Key::Space,
            "ENTER" | "RETURN" => Key::Enter,
            "ESCAPE" | "ESC" => Key::Escape,
            "BACKSPACE" => Key::Backspace,
            "DELETE" => Key::Delete,
            "UP" | "ARROWUP" => Key::Up,
            "DOWN" | "ARROWDOWN" => Key::Down,
            "LEFT" | "ARROWLEFT" => Key::Left,
            "RIGHT" | "ARROWRIGHT" => Key::Right,
            "HOME" => Key::Home,
            "END" => Key::End,
            "PAGEUP" => Key::PageUp,
            "PAGEDOWN" => Key::PageDown,
            _ if platform == Platform::Windows => {
                let mut characters = token.chars();
                match (characters.next(), characters.next()) {
                    (Some(character), None)
                        if !character.is_control() && !character.is_whitespace() =>
                    {
                        Key::Character(character)
                    }
                    _ => return None,
                }
            }
            _ => return None,
        })
    }

    /// The token written into canonical accelerator text.
    fn token(self) -> String {
        let named = match self {
            Key::Letter(byte) | Key::Digit(byte) => return char::from(byte).to_string(),
            Key::NumpadDigit(byte) => return format!("Numpad{}", char::from(byte)),
            Key::Function(number) => return format!("F{number}"),
            Key::Comma => ",",
            Key::Period => ".",
            Key::Minus => "-",
            Key::Equal => "=",
            Key::BracketLeft => "[",
            Key::BracketRight => "]",
            Key::Semicolon => ";",
            Key::Quote => "'",
            Key::Slash => "/",
            Key::Backslash => "\\",
            Key::Backquote => "`",
            Key::Tab => "Tab",
            Key::Space => "Space",
            Key::Enter => "Enter",
            Key::Escape => "Escape",
            Key::Backspace => "Backspace",
            Key::Delete => "Delete",
            Key::Up => "Up",
            Key::Down => "Down",
            Key::Left => "Left",
            Key::Right => "Right",
            Key::Home => "Home",
            Key::End => "End",
            Key::PageUp => "PageUp",
            Key::PageDown => "PageDown",
            Key::Character(character) => return character.to_string(),
        };
        named.to_owned()
    }

    /// The character named by the accelerator. Letters are lowercase.
    pub fn character(self) -> Option<char> {
        Some(match self {
            Key::Letter(byte) => char::from(byte.to_ascii_lowercase()),
            Key::Digit(byte) => char::from(byte),
            Key::Comma => ',',
            Key::Period => '.',
            Key::Minus => '-',
            Key::Equal => '=',
            Key::BracketLeft => '[',
            Key::BracketRight => ']',
            Key::Semicolon => ';',
            Key::Quote => '\'',
            Key::Slash => '/',
            Key::Backslash => '\\',
            Key::Backquote => '`',
            Key::Character(character) => character,
            _ => return None,
        })
    }

    /// Windows virtual-key code. The engine key tables speak VK on every
    /// platform; Linux translates them to keyvals.
    pub fn virtual_key(self) -> u32 {
        match self {
            Key::Letter(byte) | Key::Digit(byte) => u32::from(byte),
            Key::NumpadDigit(byte) => 0x60 + u32::from(byte - b'0'),
            Key::Function(number) => 0x6F + u32::from(number),
            Key::Comma => 0xBC,
            Key::Period => 0xBE,
            Key::Minus => 0xBD,
            Key::Equal => 0xBB,
            Key::BracketLeft => 0xDB,
            Key::BracketRight => 0xDD,
            Key::Semicolon => 0xBA,
            Key::Quote => 0xDE,
            Key::Slash => 0xBF,
            Key::Backslash => 0xDC,
            Key::Backquote => 0xC0,
            Key::Tab => 0x09,
            Key::Space => 0x20,
            Key::Enter => 0x0D,
            Key::Escape => 0x1B,
            Key::Backspace => 0x08,
            Key::Delete => 0x2E,
            Key::Up => 0x26,
            Key::Down => 0x28,
            Key::Left => 0x25,
            Key::Right => 0x27,
            Key::Home => 0x24,
            Key::End => 0x23,
            Key::PageUp => 0x21,
            Key::PageDown => 0x22,
            Key::Character(character) => u32::from(character),
        }
    }

    /// Whether a Windows binding could intercept Ctrl+Alt text entry.
    pub fn is_windows_text_key(self) -> bool {
        self.character().is_some() || matches!(self, Key::Space | Key::NumpadDigit(_))
    }
}

/// Physical modifiers on one platform. `meta` is Command on macOS; elsewhere
/// it would be the system key, which is never admitted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Accelerator {
    pub meta: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub key: Key,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AcceleratorError {
    /// Not parseable, or uses a modifier this platform does not offer.
    Invalid,
    /// Would type text or fight the system instead of reaching the browser.
    Unbindable,
    /// Owned by the operating system or by text editing.
    Reserved,
}

impl Accelerator {
    pub fn parse(text: &str, platform: Platform) -> Option<Accelerator> {
        if text.is_empty() || text.len() > 64 {
            return None;
        }
        let mut meta = false;
        let mut ctrl = false;
        let mut alt = false;
        let mut shift = false;
        // "Ctrl++" names the plus key; splitting on '+' would lose it.
        let (modifiers, last) = match text.strip_suffix("++") {
            Some(head) => (head, "+"),
            None => match text.rsplit_once('+') {
                Some((head, last)) => (head, last),
                None => ("", text),
            },
        };
        for part in modifiers.split('+').filter(|part| !part.is_empty()) {
            let flag = match (part.to_ascii_uppercase().as_str(), platform) {
                ("CMDORCTRL" | "COMMANDORCONTROL", Platform::Mac) => &mut meta,
                ("CMDORCTRL" | "COMMANDORCONTROL", _) => &mut ctrl,
                ("CMD" | "COMMAND" | "SUPER" | "META", Platform::Mac) => &mut meta,
                ("CTRL" | "CONTROL", _) => &mut ctrl,
                ("ALT" | "OPTION", _) => &mut alt,
                ("SHIFT", _) => &mut shift,
                _ => return None,
            };
            if *flag {
                return None;
            }
            *flag = true;
        }
        let key = Key::parse(
            if last == "+" && platform != Platform::Windows {
                "="
            } else {
                last
            },
            platform,
        )?;
        Some(Accelerator {
            meta,
            ctrl,
            alt,
            shift,
            key,
        })
    }

    /// Canonical text: platform-specific modifier names in a fixed order.
    pub fn format(&self, platform: Platform) -> String {
        let mut out = String::new();
        let mut push = |name: &str| {
            out.push_str(name);
            out.push('+');
        };
        if self.meta {
            push("Cmd");
        }
        if self.ctrl {
            push("Ctrl");
        }
        if self.alt {
            push(if platform == Platform::Mac {
                "Option"
            } else {
                "Alt"
            });
        }
        if self.shift {
            push("Shift");
        }
        out.push_str(&self.key.token());
        out
    }

    /// The primary browser modifier: Command on macOS, Control elsewhere.
    fn has_primary(&self, platform: Platform) -> bool {
        match platform {
            Platform::Mac => self.meta || self.ctrl,
            Platform::Windows | Platform::Other => self.ctrl,
        }
    }

    /// Whether a person may bind this to a command. Defaults in the registry
    /// are held to the same rule except for the few bare keys they declare.
    pub fn bindable(&self, platform: Platform) -> Result<(), AcceleratorError> {
        let function = matches!(self.key, Key::Function(_));
        // Alt+Left, Alt+Right and Alt+Home are the browser's own keys on
        // Windows and Linux. Without Control they are not AltGr, and no text
        // field gives them a meaning. Option+Arrow moves by word on macOS.
        let alt_navigation = platform != Platform::Mac
            && self.alt
            && !self.ctrl
            && !self.shift
            && matches!(self.key, Key::Left | Key::Right | Key::Home);
        if !self.has_primary(platform) && !function && !alt_navigation {
            return Err(AcceleratorError::Unbindable);
        }
        // Ctrl+Alt is AltGr on many Windows and Linux layouts: it types text.
        let altgr_key = match platform {
            Platform::Windows => self.key.is_windows_text_key(),
            Platform::Other => matches!(self.key, Key::Letter(_) | Key::Digit(_)),
            Platform::Mac => false,
        };
        if self.ctrl && self.alt && altgr_key {
            return Err(AcceleratorError::Unbindable);
        }
        if self.reserved(platform) {
            return Err(AcceleratorError::Reserved);
        }
        Ok(())
    }

    fn reserved(&self, platform: Platform) -> bool {
        let Accelerator {
            meta,
            ctrl,
            alt,
            shift,
            key,
        } = *self;
        let letter = |c: u8| key == Key::Letter(c);
        match platform {
            Platform::Mac => {
                let plain_cmd = meta && !ctrl && !alt;
                (plain_cmd
                    && !shift
                    && (letter(b'Q')
                        || letter(b'H')
                        || letter(b'M')
                        || letter(b'C')
                        || letter(b'V')
                        || letter(b'X')
                        || letter(b'A')
                        || letter(b'Z')
                        || key == Key::Tab
                        || key == Key::Space
                        || key == Key::Backquote))
                    || (plain_cmd && shift && letter(b'Z'))
                    || (meta && alt && !ctrl && !shift && (letter(b'H') || key == Key::Escape))
                    || (meta && ctrl && !alt && !shift && (letter(b'Q') || letter(b'F')))
                    || (ctrl && !meta && !alt && key == Key::Space)
            }
            Platform::Windows | Platform::Other => {
                let plain_ctrl = ctrl && !alt && !shift;
                (plain_ctrl
                    && (letter(b'C')
                        || letter(b'V')
                        || letter(b'X')
                        || letter(b'A')
                        || letter(b'Z')
                        || letter(b'Y')
                        || key == Key::Escape))
                    || (ctrl && shift && !alt && (letter(b'Z') || key == Key::Escape))
                    || (alt && !ctrl && key == Key::Function(4))
                    || (ctrl && alt && key == Key::Delete)
            }
        }
    }
}

impl fmt::Display for Accelerator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.format(Platform::CURRENT))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str, platform: Platform) -> Accelerator {
        Accelerator::parse(text, platform).unwrap_or_else(|| panic!("{text} parses"))
    }

    #[test]
    fn primary_modifier_follows_the_platform() {
        let mac = parse("CmdOrCtrl+Shift+T", Platform::Mac);
        assert!(mac.meta && mac.shift && !mac.ctrl);
        let other = parse("CmdOrCtrl+Shift+T", Platform::Other);
        assert!(other.ctrl && other.shift && !other.meta);
        assert_eq!(mac.format(Platform::Mac), "Cmd+Shift+T");
        assert_eq!(other.format(Platform::Other), "Ctrl+Shift+T");
    }

    #[test]
    fn canonical_text_round_trips() {
        for platform in [Platform::Mac, Platform::Windows, Platform::Other] {
            for text in [
                "CmdOrCtrl+,",
                "CmdOrCtrl+Alt+N",
                "Ctrl+Shift+Tab",
                "CmdOrCtrl+[",
                "CmdOrCtrl+9",
                "Escape",
                "CmdOrCtrl+Shift+Enter",
                "F5",
                "Ctrl++",
            ] {
                let first = parse(text, platform);
                let again = parse(&first.format(platform), platform);
                assert_eq!(first, again, "{text} on {platform:?}");
            }
        }
    }

    #[test]
    fn recorded_key_codes_read_as_their_keys() {
        assert_eq!(
            parse("Ctrl+Shift+KeyK", Platform::Other),
            parse("Ctrl+Shift+K", Platform::Other)
        );
        assert_eq!(
            parse("Cmd+Digit3", Platform::Mac),
            parse("Cmd+3", Platform::Mac)
        );
        assert_eq!(
            parse("Cmd+BracketLeft", Platform::Mac),
            parse("Cmd+[", Platform::Mac)
        );
        assert_eq!(Accelerator::parse("Cmd+KeyKK", Platform::Mac), None);
        assert_eq!(Accelerator::parse("Cmd+Digit", Platform::Mac), None);
    }

    #[test]
    fn printable_layout_characters_round_trip() {
        for text in ["Ctrl++", "Ctrl+!", "Ctrl+?", "Ctrl+é", "Ctrl+ù"] {
            let accelerator = parse(text, Platform::Windows);
            assert_eq!(accelerator.format(Platform::Windows), text);
        }
        for platform in [Platform::Mac, Platform::Other] {
            assert_eq!(parse("Ctrl++", platform), parse("Ctrl+=", platform));
            assert_eq!(Accelerator::parse("Ctrl+é", platform), None);
        }
    }

    #[test]
    fn windows_bindings_leave_altgr_text_available() {
        let platform = Platform::Windows;
        for text in [
            "Ctrl+Alt+@",
            "Ctrl+Alt+Shift+é",
            "Ctrl+Alt+[",
            "Ctrl+Alt+Space",
            "Ctrl+Alt+Numpad1",
        ] {
            assert_eq!(
                parse(text, platform).bindable(platform),
                Err(AcceleratorError::Unbindable)
            );
        }
        assert_eq!(parse("Ctrl+Alt+Right", platform).bindable(platform), Ok(()));
        assert_eq!(
            parse("Ctrl+A", platform).bindable(platform),
            Err(AcceleratorError::Reserved)
        );
    }

    #[test]
    fn windows_keypad_digits_have_their_own_tokens() {
        let platform = Platform::Windows;
        for digit in 0..=9 {
            let text = format!("Ctrl+Numpad{digit}");
            let parsed = parse(&text, platform);
            assert_eq!(parsed.format(platform), text);
            assert_eq!(parsed.key.virtual_key(), 0x60 + digit);
        }
        for text in ["Ctrl+Numpad00", "Ctrl+NumpadAdd"] {
            assert_eq!(Accelerator::parse(text, platform), None);
        }
        assert_eq!(Accelerator::parse("Ctrl+Numpad1", Platform::Mac), None);
    }

    #[test]
    fn malformed_text_is_rejected() {
        for text in [
            "",
            "Shift+",
            "CmdOrCtrl+Hyper+T",
            "CmdOrCtrl+CmdOrCtrl+T",
            "CmdOrCtrl+TT",
            "F13",
        ] {
            assert_eq!(Accelerator::parse(text, Platform::Mac), None, "{text}");
        }
        // The system key is never ours outside macOS.
        assert_eq!(Accelerator::parse("Super+T", Platform::Other), None);
    }

    #[test]
    fn binding_needs_the_primary_modifier_and_spares_the_system() {
        let mac = Platform::Mac;
        let other = Platform::Other;
        assert_eq!(
            parse("Shift+K", mac).bindable(mac),
            Err(AcceleratorError::Unbindable)
        );
        assert_eq!(parse("F6", other).bindable(other), Ok(()));
        assert_eq!(parse("Alt+Left", other).bindable(other), Ok(()));
        assert_eq!(parse("Alt+Home", other).bindable(other), Ok(()));
        assert_eq!(
            parse("Alt+Left", mac).bindable(mac),
            Err(AcceleratorError::Unbindable)
        );
        assert_eq!(
            parse("Alt+Up", other).bindable(other),
            Err(AcceleratorError::Unbindable)
        );
        assert_eq!(
            parse("Alt+Shift+Left", other).bindable(other),
            Err(AcceleratorError::Unbindable)
        );
        assert_eq!(
            parse("Ctrl+Alt+N", other).bindable(other),
            Err(AcceleratorError::Unbindable)
        );
        assert_eq!(parse("Cmd+Option+N", mac).bindable(mac), Ok(()));
        for (text, platform) in [
            ("Cmd+Q", mac),
            ("Cmd+C", mac),
            ("Cmd+Shift+Z", mac),
            ("Cmd+Space", mac),
            ("Ctrl+V", other),
            ("Ctrl+Y", other),
            ("Alt+F4", other),
        ] {
            assert_eq!(
                parse(text, platform).bindable(platform),
                Err(AcceleratorError::Reserved),
                "{text}"
            );
        }
    }

    #[test]
    fn virtual_keys_match_windows() {
        assert_eq!(parse("Ctrl+T", Platform::Other).key.virtual_key(), 0x54);
        assert_eq!(parse("Ctrl+1", Platform::Other).key.virtual_key(), 0x31);
        assert_eq!(parse("F5", Platform::Other).key.virtual_key(), 0x74);
        assert_eq!(parse("Ctrl+,", Platform::Other).key.virtual_key(), 0xBC);
    }
}
