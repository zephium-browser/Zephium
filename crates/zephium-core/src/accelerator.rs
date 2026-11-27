//! Keyboard accelerators as data. One parser serves the registry defaults,
//! user overrides, native menus and every per-platform key table, so a
//! shortcut means the same keys wherever it is read.

use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Platform {
    Mac,
    Other,
}

impl Platform {
    pub const CURRENT: Platform = if cfg!(target_os = "macos") {
        Platform::Mac
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
}

impl Key {
    fn parse(token: &str) -> Option<Key> {
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
            _ => return None,
        })
    }

    /// The token written into canonical accelerator text. Every token is
    /// accepted by both this parser and the native menu parser.
    fn token(self) -> String {
        let named = match self {
            Key::Letter(byte) | Key::Digit(byte) => return char::from(byte).to_string(),
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
        };
        named.to_owned()
    }

    /// The character the key types with no modifiers held, for keys that
    /// type one. Letters are lowercase.
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
            _ => return None,
        })
    }

    /// Windows virtual-key code. The engine key tables speak VK on every
    /// platform; Linux translates them to keyvals.
    pub fn virtual_key(self) -> u32 {
        match self {
            Key::Letter(byte) | Key::Digit(byte) => u32::from(byte),
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
        }
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
                ("CMDORCTRL" | "COMMANDORCONTROL", Platform::Other) => &mut ctrl,
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
        let key = Key::parse(if last == "+" { "=" } else { last })?;
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
            Platform::Other => self.ctrl,
        }
    }

    /// Whether a person may bind this to a command. Defaults in the registry
    /// are held to the same rule except for the few bare keys they declare.
    pub fn bindable(&self, platform: Platform) -> Result<(), AcceleratorError> {
        let function = matches!(self.key, Key::Function(_));
        // Alt+Left, Alt+Right and Alt+Home are the browser's own keys on
        // Windows and Linux. Without Control they are not AltGr, and no text
        // field gives them a meaning. Option+Arrow moves by word on macOS.
        let alt_navigation = platform == Platform::Other
            && self.alt
            && !self.ctrl
            && !self.shift
            && matches!(self.key, Key::Left | Key::Right | Key::Home);
        if !self.has_primary(platform) && !function && !alt_navigation {
            return Err(AcceleratorError::Unbindable);
        }
        // Ctrl+Alt is AltGr on many Windows and Linux layouts: it types text.
        if platform == Platform::Other
            && self.ctrl
            && self.alt
            && matches!(self.key, Key::Letter(_) | Key::Digit(_))
        {
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
            Platform::Other => {
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
        for platform in [Platform::Mac, Platform::Other] {
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
