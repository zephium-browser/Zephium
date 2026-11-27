use std::collections::HashSet;
use zephium_core::accelerator::{Accelerator, AcceleratorError, Key, Platform};
use zephium_core::commands::{KeymapError, ResolvedCommand};
use zephium_core::ports::engine::Shortcut;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct Chord {
    key: u32,
    ctrl: bool,
    shift: bool,
    alt: bool,
}

impl Chord {
    pub(super) fn shortcut(self, id: &str) -> Shortcut {
        Shortcut {
            id: id.to_owned(),
            key: self.key,
            ctrl: self.ctrl,
            shift: self.shift,
            alt: self.alt,
        }
    }

    fn reserves_editing_key(self) -> bool {
        let Ok(key) = u8::try_from(self.key) else {
            return false;
        };
        key.is_ascii_uppercase()
            && Accelerator {
                key: Key::Letter(key),
                ctrl: self.ctrl,
                shift: self.shift,
                alt: self.alt,
                meta: false,
            }
            .bindable(Platform::Windows)
                == Err(AcceleratorError::Reserved)
    }
}

fn chord(accelerator: Accelerator, scan: impl FnOnce(u16) -> i16) -> Option<Chord> {
    const SHIFT: u16 = 1;
    const CTRL: u16 = 2;
    const ALT: u16 = 4;
    let mut key = accelerator.key.virtual_key();
    let mut required = 0;
    let character = accelerator.key.character();
    // Number row bindings distinguish Ctrl+digit (tabs) from Ctrl+Shift+digit
    // (spaces), even on layouts where Shift types the digit.
    if let Some(character) = character.filter(|_| !matches!(accelerator.key, Key::Digit(_))) {
        let mapped = u16::try_from(u32::from(character)).ok().map(scan);
        match mapped {
            Some(scan) if scan != -1 => {
                let scan = scan as u16;
                key = u32::from(scan & 0xff);
                required = scan >> 8;
                if required & !(SHIFT | CTRL | ALT) != 0 {
                    return None;
                }
            }
            _ if matches!(accelerator.key, Key::Character(_)) => return None,
            _ => {}
        }
    }
    let ctrl = accelerator.ctrl || required & CTRL != 0;
    let alt = accelerator.alt || required & ALT != 0;
    if ctrl && alt && accelerator.key.is_windows_text_key() {
        return None;
    }
    Some(Chord {
        key,
        ctrl,
        shift: accelerator.shift || required & SHIFT != 0,
        alt,
    })
}

fn mapped(text: &str, scan: &impl Fn(u16) -> i16) -> Option<(Accelerator, Chord)> {
    let accelerator = Accelerator::parse(text, Platform::Windows)?;
    Some((accelerator, chord(accelerator, scan)?))
}

pub(super) fn bindings(
    resolved: Vec<ResolvedCommand>,
    scan: impl Fn(u16) -> i16,
) -> Vec<(ResolvedCommand, Option<Chord>)> {
    let mut bindings: Vec<_> = resolved
        .into_iter()
        .map(|command| (command, None))
        .collect();
    let mut held = HashSet::new();
    // A layout change can make two saved spellings share a chord. Keep the
    // defaults ahead of overrides until the chords are distinct again.
    for customized in [false, true] {
        for (command, binding) in &mut bindings {
            if command.id == "launcher.toggle" || command.customized != customized {
                continue;
            }
            command.accelerator = command
                .accelerator
                .as_deref()
                .and_then(|text| mapped(text, &scan))
                .filter(|(_, chord)| !chord.reserves_editing_key() && held.insert(*chord))
                .map(|(mut accelerator, chord)| {
                    *binding = Some(chord);
                    accelerator.ctrl = chord.ctrl;
                    accelerator.shift = chord.shift;
                    accelerator.alt = chord.alt;
                    accelerator.format(Platform::Windows)
                });
        }
    }
    bindings
}

pub(super) fn page_shortcuts(
    resolved: Vec<ResolvedCommand>,
    scan: impl Fn(u16) -> i16,
    available: impl Fn(&ResolvedCommand) -> bool,
) -> Vec<Shortcut> {
    let mut exact = Vec::new();
    let mut aliases = Vec::new();
    for (command, chord) in bindings(resolved, &scan) {
        if !available(&command) {
            continue;
        }
        if let Some(chord) = chord {
            exact.push(chord.shortcut(command.id));
        }
        if command.customized {
            continue;
        }
        let keypad = match command.id {
            "zoom.in" => {
                if let Some((_, plus)) = mapped("Ctrl++", &scan) {
                    aliases.push(plus.shortcut(command.id));
                }
                0x6b // VK_ADD
            }
            "zoom.out" => 0x6d, // VK_SUBTRACT
            _ => continue,
        };
        aliases.push(Shortcut {
            id: command.id.to_owned(),
            key: keypad,
            ctrl: true,
            shift: false,
            alt: false,
        });
    }
    exact.extend(aliases);
    exact
}

#[cfg(target_os = "windows")]
pub(super) fn validate_binding(
    resolved: &[ResolvedCommand],
    id: &str,
    text: Option<&str>,
    scan: &impl Fn(u16) -> i16,
) -> Result<Option<String>, KeymapError> {
    let canonical = zephium_core::commands::validate_binding(resolved, id, text)?;
    if let Some(text) = canonical.as_deref() {
        validate_chord(resolved, id, text, scan)?;
    }
    Ok(canonical)
}

fn validate_chord(
    resolved: &[ResolvedCommand],
    id: &str,
    text: &str,
    scan: &impl Fn(u16) -> i16,
) -> Result<(), KeymapError> {
    let (_, candidate) =
        mapped(text, scan).ok_or(KeymapError::Accelerator(AcceleratorError::Unbindable))?;
    if candidate.reserves_editing_key() {
        return Err(KeymapError::Accelerator(AcceleratorError::Reserved));
    }
    if let Some(other) = resolved.iter().find(|command| {
        command.id != id
            && command.id != "launcher.toggle"
            && command
                .accelerator
                .as_deref()
                .and_then(|text| mapped(text, scan))
                .is_some_and(|(_, chord)| chord == candidate)
    }) {
        return Err(KeymapError::Conflict(other.id));
    }
    Ok(())
}

#[cfg(target_os = "windows")]
mod native {
    use super::super::Keymap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::OnceLock;
    use tauri::{Manager, WebviewWindow};
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyboardLayout, VkKeyScanExW};
    use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
    use windows::Win32::UI::WindowsAndMessaging::{
        PostMessageW, WM_ACTIVATE, WM_APP, WM_INPUTLANGCHANGE, WM_NCDESTROY,
    };

    const SUBCLASS: usize = 0x5a4b_4c59;
    const REFRESH: u32 = WM_APP + 0x4b;
    static APP: OnceLock<tauri::AppHandle> = OnceLock::new();

    #[derive(Default)]
    pub(crate) struct Layout {
        thread: u32,
        observed: AtomicUsize,
    }

    impl Layout {
        pub(crate) fn on_window_thread() -> Self {
            Self {
                thread: unsafe { GetCurrentThreadId() },
                observed: AtomicUsize::new(unsafe { GetKeyboardLayout(0) }.0 as usize),
            }
        }

        pub(crate) fn scan(&self) -> impl Fn(u16) -> i16 {
            let layout = unsafe { GetKeyboardLayout(self.thread) };
            move |character| unsafe { VkKeyScanExW(character, layout) }
        }

        fn changed(&self) -> bool {
            let layout = unsafe { GetKeyboardLayout(self.thread) }.0 as usize;
            self.observed.swap(layout, Ordering::Relaxed) != layout
        }
    }

    pub(crate) fn install(window: &WebviewWindow) {
        let Ok(hwnd) = window.hwnd() else {
            return;
        };
        APP.get_or_init(|| window.app_handle().clone());
        if !unsafe { SetWindowSubclass(HWND(hwnd.0), Some(procedure), SUBCLASS, 0) }.as_bool() {
            crate::write_diagnostic(format_args!("keymap: layout observer was not installed"));
        }
    }

    unsafe extern "system" fn procedure(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
        _: usize,
        _: usize,
    ) -> LRESULT {
        if message == WM_NCDESTROY {
            let _ = unsafe { RemoveWindowSubclass(hwnd, Some(procedure), SUBCLASS) };
        }
        if message == REFRESH {
            if let Some((app, keymap)) = APP.get().and_then(|app| {
                let keymap = app.try_state::<Keymap>()?;
                keymap.layout.changed().then_some((app, keymap))
            }) {
                keymap.refresh_engine();
                Keymap::emit_changed(app);
            }
            return LRESULT(0);
        }
        let result = unsafe { DefSubclassProc(hwnd, message, wparam, lparam) };
        if message == WM_INPUTLANGCHANGE || (message == WM_ACTIVATE && wparam.0 & 0xffff != 0) {
            // An engine call can activate the window synchronously.
            let _ = unsafe { PostMessageW(Some(hwnd), REFRESH, WPARAM(0), LPARAM(0)) };
        }
        result
    }
}

#[cfg(target_os = "windows")]
pub(crate) use native::{install, Layout};

#[cfg(test)]
mod tests {
    use super::*;
    use zephium_core::commands::Group;

    fn command(id: &'static str, text: &str, customized: bool) -> ResolvedCommand {
        ResolvedCommand {
            id,
            title: id,
            accelerator: Some(text.to_owned()),
            group: Group::Keys,
            customized,
        }
    }

    fn us_scan(character: u16) -> i16 {
        match character {
            0x3d => 0xbb,
            0x2b => 0x1bb,
            0x2d => 0xbd,
            _ => -1,
        }
    }

    #[test]
    fn windows_zoom_accepts_layout_plus_and_keypad_keys() {
        let resolved = vec![
            command("zoom.in", "Ctrl+=", false),
            command("zoom.out", "Ctrl+-", false),
        ];
        let shortcuts = page_shortcuts(resolved.clone(), us_scan, |_| true);
        for (key, shift, id) in [
            (0xbb, false, "zoom.in"),
            (0xbb, true, "zoom.in"),
            (0x6b, false, "zoom.in"),
            (0x6d, false, "zoom.out"),
        ] {
            assert_eq!(
                Shortcut::match_windows(&shortcuts, key, true, shift, false).map(|s| s.id.as_str()),
                Some(id)
            );
            assert!(Shortcut::match_windows(&shortcuts, key, false, shift, false).is_none());
            assert!(Shortcut::match_windows(&shortcuts, key, true, shift, true).is_none());
        }
        let german = |character| match character {
            0x3d => 0x130, // Equals uses Shift+0.
            0x2b => 0xbb,  // Plus has its own key.
            _ => us_scan(character),
        };
        let shortcuts = page_shortcuts(resolved, german, |_| true);
        for (key, shift) in [(0x30, true), (0xbb, false), (0x6b, false)] {
            assert_eq!(
                Shortcut::match_windows(&shortcuts, key, true, shift, false).map(|s| s.id.as_str()),
                Some("zoom.in")
            );
        }
    }

    #[test]
    fn custom_theme_keys_take_precedence_over_zoom_aliases_in_pages() {
        for id in ["theme.system", "theme.light", "theme.dark"] {
            let mut theme = command(id, "Ctrl+Shift+=", true);
            let zoom = command("zoom.in", "Ctrl+=", false);
            let launcher = command("launcher.toggle", "Ctrl+Shift+Space", false);
            let shortcuts =
                page_shortcuts(vec![zoom.clone(), launcher, theme.clone()], us_scan, |_| {
                    true
                });
            assert_eq!(
                Shortcut::match_windows(&shortcuts, 0xbb, true, true, false)
                    .map(|shortcut| shortcut.id.as_str()),
                Some(id)
            );
            assert!(!shortcuts
                .iter()
                .any(|shortcut| shortcut.id == "launcher.toggle"));
            theme.accelerator = None;
            let shortcuts = page_shortcuts(vec![zoom, theme], us_scan, |_| true);
            assert_eq!(
                Shortcut::match_windows(&shortcuts, 0xbb, true, true, false)
                    .map(|shortcut| shortcut.id.as_str()),
                Some("zoom.in")
            );
        }
    }

    #[test]
    fn keypad_alias_is_secondary_and_rebound_or_disabled_zoom_has_no_alias() {
        let mut shortcuts =
            page_shortcuts(vec![command("zoom.in", "Ctrl+=", false)], us_scan, |_| true);
        shortcuts.insert(
            0,
            Shortcut {
                id: "tab.close".to_owned(),
                key: 0x6b,
                ctrl: true,
                shift: false,
                alt: false,
            },
        );
        assert_eq!(
            Shortcut::match_windows(&shortcuts, 0x6b, true, false, false).map(|s| s.id.as_str()),
            Some("tab.close")
        );
        let mut zoom = command("zoom.in", "Ctrl+J", true);
        let shortcuts = page_shortcuts(vec![zoom.clone()], us_scan, |_| true);
        assert!(Shortcut::match_windows(&shortcuts, 0x6b, true, false, false).is_none());
        zoom.accelerator = None;
        let shortcuts = page_shortcuts(vec![zoom], us_scan, |_| true);
        assert!(Shortcut::match_windows(&shortcuts, 0xbb, true, true, false).is_none());
    }

    #[test]
    fn azerty_maps_letters_punctuation_and_keeps_number_row_modifiers() {
        let azerty = |character| match character {
            0x61 => 0x41,  // A, at the US Q position.
            0x7a => 0x5a,  // Z, at the US W position.
            0x2c => 0x4d,  // Comma, at the US M position.
            0x31 => 0x131, // Shift is needed to type 1.
            _ => -1,
        };
        for (text, key, shift) in [
            ("Ctrl+A", 0x41, false),
            ("Ctrl+Shift+Z", 0x5a, true),
            ("Ctrl+,", 0x4d, false),
            ("Ctrl+1", 0x31, false),
            ("Ctrl+Shift+1", 0x31, true),
        ] {
            let binding = mapped(text, &azerty).expect("AZERTY chord").1;
            assert_eq!(
                (binding.key, binding.ctrl, binding.shift, binding.alt),
                (key, true, shift, false)
            );
        }
    }

    #[test]
    fn required_modifiers_are_checked_before_admitting_a_chord() {
        let scan = |character| match character {
            0x21 => 0x131, // US Shift+1.
            0x40 => 0x651, // German AltGr+Q.
            0x3f => 0x8bf, // Unsupported state bit.
            0x2c => 0x2bc, // Control required.
            _ => -1,
        };
        let shifted = mapped("Ctrl+!", &scan).expect("shifted punctuation").1;
        assert_eq!(
            (shifted.key, shifted.ctrl, shifted.shift, shifted.alt),
            (0x31, true, true, false)
        );
        assert!(mapped("!", &scan).expect("test chord").1.shift);
        assert!(mapped(",", &scan).expect("test chord").1.ctrl);
        assert!(mapped("Ctrl+@", &scan).is_none());
        assert!(mapped("Ctrl+Alt+@", &scan).is_none());
        assert!(mapped("Ctrl+?", &scan).is_none());
        assert!(mapped("Ctrl+é", &scan).is_none());
        assert_eq!(
            validate_chord(&[], "tab.new", "Ctrl+@", &scan),
            Err(KeymapError::Accelerator(AcceleratorError::Unbindable))
        );
        let requires_alt = |_| 0x4bc;
        assert!(mapped(",", &requires_alt).expect("test chord").1.alt);
        assert!(mapped("Ctrl+,", &requires_alt).is_none());
    }

    #[test]
    fn a_layout_character_cannot_take_a_reserved_editing_key() {
        let cyrillic = |_| 0x41;
        assert_eq!(
            validate_chord(&[], "tab.new", "Ctrl+ф", &cyrillic),
            Err(KeymapError::Accelerator(AcceleratorError::Reserved))
        );
        let bindings = bindings(vec![command("tab.new", "Ctrl+ф", true)], cyrillic);
        assert_eq!(bindings[0].1, None);
    }

    #[test]
    fn layout_switch_suppresses_a_new_collision_and_restores_the_binding() {
        let resolved = vec![
            command("tab.new", "Ctrl+!", true),
            command("space.select.1", "Ctrl+Shift+1", false),
        ];
        let azerty = |_| 0xdf;
        let qwerty = |_| 0x131;
        assert_eq!(
            validate_chord(&resolved, "tab.new", "Ctrl+!", &azerty),
            Ok(())
        );
        for text in ["Ctrl+!", "Ctrl+Shift+!"] {
            assert_eq!(
                validate_chord(&resolved, "tab.new", text, &qwerty),
                Err(KeymapError::Conflict("space.select.1"))
            );
        }
        let before = bindings(resolved.clone(), azerty);
        assert_eq!(before[0].1.expect("AZERTY chord").key, 0xdf);
        let during = bindings(resolved.clone(), qwerty);
        assert_eq!(during[0].0.accelerator, None);
        assert_eq!(during[0].1, None);
        assert!(during[1].1.expect("space chord").shift);
        assert_eq!(bindings(resolved, azerty), before);
    }

    #[test]
    fn keypad_digits_and_named_keys_use_their_virtual_keys() {
        let no_scan = |_| -> i16 { panic!("named keys bypass layout conversion") };
        for (text, vk, ctrl, alt) in [
            ("Ctrl+Numpad0", 0x60, true, false),
            ("Ctrl+Numpad1", 0x61, true, false),
            ("Ctrl+Numpad9", 0x69, true, false),
            ("Ctrl+1", 0x31, true, false),
            ("F5", 0x74, false, false),
            ("Alt+Left", 0x25, false, true),
        ] {
            let binding = mapped(text, &no_scan)
                .expect("named chord")
                .1
                .shortcut("test");
            assert_eq!(
                (binding.key, binding.ctrl, binding.shift, binding.alt),
                (vk, ctrl, false, alt)
            );
        }
        let reload = mapped("CmdOrCtrl+R", &|_| 0x52).expect("reload chord").1;
        assert_eq!(
            (reload.key, reload.ctrl, reload.shift, reload.alt),
            (0x52, true, false, false)
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn page_punctuation_uses_the_active_windows_layout() {
        let layout = Layout::on_window_thread();
        let scan = layout.scan();
        let mut checked = 0;
        for character in [',', ';', '!', '?', 'é', 'ù'] {
            let codepoint =
                u16::try_from(u32::from(character)).expect("test characters fit in UTF-16");
            let code = scan(codepoint);
            if code == -1 {
                continue;
            }
            checked += 1;
            let code = code as u16;
            let modifiers = code >> 8;
            let binding = mapped(&format!("Ctrl+{character}"), &scan);
            if modifiers & !3 != 0 {
                assert!(binding.is_none(), "{character}");
            } else {
                let binding = binding.expect("page chord").1;
                assert_eq!(binding.key, u32::from(code & 0xff), "{character}");
                assert_eq!(binding.shift, modifiers & 1 != 0, "{character}");
                assert!(binding.ctrl && !binding.alt);
            }
        }
        assert!(
            checked > 0,
            "the active layout must type at least one test character"
        );
    }
}
