//! What opens the launcher from anywhere: a global shortcut anyone can
//! record, and on macOS an opt-in double tap of a modifier. Every good
//! two-key combination is already claimed by some other tool, so the choice
//! belongs to the person, and the settings page reports whether it holds.
#[cfg(not(target_os = "linux"))]
use std::str::FromStr;
#[cfg(not(target_os = "linux"))]
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};
#[cfg(not(target_os = "linux"))]
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};
#[cfg(not(target_os = "linux"))]
use zephium_core::ports::store::Store;

#[cfg(target_os = "macos")]
mod double_tap;

pub const DEFAULT: &str = "CmdOrCtrl+Shift+Space";
#[cfg(not(target_os = "linux"))]
const SHORTCUT_KEY: &str = "launcher.shortcut";
#[cfg(not(target_os = "linux"))]
const DOUBLE_TAP_KEY: &str = "launcher.double-tap";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum DoubleTap {
    #[default]
    Off,
    Command,
    Option,
}

/// Why a recorded shortcut was not taken.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum Rejection {
    Invalid,
    /// Shift alone does not make a key global; it would type a capital.
    NeedsModifier,
    /// The system itself answers to it, such as Spotlight or input sources.
    System,
    /// Apps use it for their own commands, such as Redo.
    AppCommand,
    /// Registration failed, usually because another application holds it.
    Unavailable,
    /// This platform's shortcut is managed elsewhere.
    Unsupported,
}

#[derive(Clone, Debug, Serialize, specta::Type)]
pub struct LauncherTrigger {
    pub shortcut: String,
    pub default_shortcut: String,
    /// The shortcut is registered and will open the launcher.
    pub registered: bool,
    pub editable: bool,
    pub double_tap: DoubleTap,
    pub double_tap_supported: bool,
    /// Double tap needs Accessibility permission to observe modifier keys
    /// while another application is in front.
    pub accessibility: bool,
}

#[derive(Clone, Debug, Serialize, specta::Type)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TriggerChange {
    Applied {
        trigger: LauncherTrigger,
    },
    Rejected {
        reason: Rejection,
        trigger: LauncherTrigger,
    },
}

#[cfg(not(target_os = "linux"))]
#[derive(Default)]
pub struct Trigger(Mutex<State>);

#[cfg(target_os = "linux")]
#[derive(Default)]
pub struct Trigger;

#[cfg(not(target_os = "linux"))]
#[derive(Default)]
struct State {
    shortcut: String,
    registered: Option<Shortcut>,
    recording: bool,
    double_tap: DoubleTap,
}

/// Checks a recorded accelerator before anything is unregistered, so a bad
/// choice never costs the working one.
#[cfg(not(target_os = "linux"))]
pub fn validate(accelerator: &str) -> Result<Shortcut, Rejection> {
    if accelerator.is_empty() || accelerator.len() > 64 {
        return Err(Rejection::Invalid);
    }
    let shortcut = Shortcut::from_str(accelerator).map_err(|_| Rejection::Invalid)?;
    let mods = shortcut.mods;
    if !mods.intersects(Modifiers::SUPER | Modifiers::CONTROL | Modifiers::ALT) {
        return Err(Rejection::NeedsModifier);
    }
    if system(mods, shortcut.key) {
        return Err(Rejection::System);
    }
    // The primary modifier with a character is how every app spells its own
    // commands: ⌘⇧Z is Redo, ⌘⇧T reopens a tab. Taking one globally breaks
    // it in every app while Zephium runs.
    let primary = if cfg!(target_os = "macos") {
        Modifiers::SUPER
    } else {
        Modifiers::CONTROL
    };
    if mods.difference(Modifiers::SHIFT) == primary && character(shortcut.key) {
        return Err(Rejection::AppCommand);
    }
    Ok(shortcut)
}

#[cfg(not(target_os = "linux"))]
fn character(key: Code) -> bool {
    use Code::*;
    !matches!(
        key,
        Space
            | Escape
            | Tab
            | Enter
            | Backspace
            | Delete
            | ArrowUp
            | ArrowDown
            | ArrowLeft
            | ArrowRight
            | Home
            | End
            | PageUp
            | PageDown
            | F1
            | F2
            | F3
            | F4
            | F5
            | F6
            | F7
            | F8
            | F9
            | F10
            | F11
            | F12
            | F13
            | F14
            | F15
            | F16
            | F17
            | F18
            | F19
            | F20
    )
}

#[cfg(not(target_os = "linux"))]
fn system(mods: Modifiers, key: Code) -> bool {
    let (cmd, ctrl, alt, shift) = (
        Modifiers::SUPER,
        Modifiers::CONTROL,
        Modifiers::ALT,
        Modifiers::SHIFT,
    );
    if cfg!(target_os = "macos") {
        let reserved: &[(Modifiers, Code)] = &[
            (cmd, Code::Space),               // Spotlight
            (ctrl, Code::Space),              // Previous input source
            (ctrl.union(alt), Code::Space),   // Next input source
            (ctrl.union(cmd), Code::Space),   // Emoji & Symbols
            (alt.union(cmd), Code::Space),    // Finder search
            (cmd, Code::Tab),                 // App switcher
            (cmd.union(shift), Code::Tab),    // App switcher, backwards
            (cmd, Code::Backquote),           // Window switcher
            (cmd.union(alt), Code::Escape),   // Force Quit
            (cmd.union(shift), Code::Digit3), // Screenshots
            (cmd.union(shift), Code::Digit4),
            (cmd.union(shift), Code::Digit5),
            (ctrl.union(cmd), Code::KeyQ), // Lock Screen
            (ctrl.union(cmd), Code::KeyF), // Full Screen
        ];
        reserved.iter().any(|&(m, k)| m == mods && k == key)
    } else {
        mods.contains(cmd)
            || (mods == alt && matches!(key, Code::Tab | Code::F4 | Code::Space))
            || (mods == ctrl.union(alt) && key == Code::Delete)
            || (mods == ctrl.union(shift) && key == Code::Escape)
    }
}

#[cfg(not(target_os = "linux"))]
fn register(app: &AppHandle, shortcut: &Shortcut) -> bool {
    let registered = app
        .global_shortcut()
        .on_shortcut(*shortcut, |app, _shortcut, event| {
            if event.state() == ShortcutState::Pressed {
                let _ = crate::execute_command(app, "launcher.toggle");
            }
        });
    if let Err(error) = &registered {
        crate::write_diagnostic(format_args!("launcher: shortcut unavailable: {error}"));
    }
    registered.is_ok()
}

#[cfg(not(target_os = "linux"))]
fn unregister(app: &AppHandle, shortcut: &Shortcut) {
    let _ = app.global_shortcut().unregister(*shortcut);
}

#[cfg(not(target_os = "linux"))]
fn setting(key: &str) -> Option<String> {
    crate::APP_STORE
        .get()
        .and_then(|store| store.app_setting(key))
        .filter(|value| !value.is_empty())
}

#[cfg(not(target_os = "linux"))]
fn persist(key: &str, value: &str) {
    if let Some(store) = crate::APP_STORE.get() {
        store.set_app_setting(key.into(), value.into());
    }
}

/// Registers what was chosen last time, or the default. The keymap override
/// is honoured for a profile that set one before there was a recorder.
#[cfg(not(target_os = "linux"))]
pub fn install(app: &AppHandle, keymap_override: Option<String>) {
    let shortcut = setting(SHORTCUT_KEY)
        .or(keymap_override)
        .unwrap_or_else(|| DEFAULT.into());
    let parsed = Shortcut::from_str(&shortcut)
        .ok()
        .or_else(|| Shortcut::from_str(DEFAULT).ok());
    let registered = parsed.filter(|parsed| register(app, parsed));
    let double_tap = match setting(DOUBLE_TAP_KEY).as_deref() {
        Some("command") => DoubleTap::Command,
        Some("option") => DoubleTap::Option,
        _ => DoubleTap::Off,
    };
    if let Some(trigger) = app.try_state::<Trigger>() {
        let mut state = trigger.0.lock().unwrap_or_else(|e| e.into_inner());
        state.shortcut = shortcut;
        state.registered = registered;
        state.double_tap = double_tap;
    }
    #[cfg(target_os = "macos")]
    double_tap::apply(app, double_tap);
}

#[cfg(not(target_os = "linux"))]
pub fn snapshot(app: &AppHandle) -> LauncherTrigger {
    let (shortcut, registered, double_tap) = app.try_state::<Trigger>().map_or_else(
        || (DEFAULT.to_owned(), false, DoubleTap::Off),
        |trigger| {
            let state = trigger.0.lock().unwrap_or_else(|e| e.into_inner());
            (
                state.shortcut.clone(),
                state.registered.is_some() || state.recording,
                state.double_tap,
            )
        },
    );
    #[cfg(target_os = "macos")]
    let accessibility = {
        // Permission can be granted in System Settings at any time; the
        // settings page asks again when it is shown, which is when the
        // observers are brought in line with it.
        double_tap::apply(app, double_tap);
        double_tap::trusted()
    };
    #[cfg(not(target_os = "macos"))]
    let accessibility = false;
    LauncherTrigger {
        shortcut,
        default_shortcut: DEFAULT.into(),
        registered,
        editable: cfg!(not(target_os = "linux")),
        double_tap,
        double_tap_supported: cfg!(target_os = "macos"),
        accessibility,
    }
}

#[cfg(not(target_os = "linux"))]
pub fn set_shortcut(app: &AppHandle, accelerator: &str) -> TriggerChange {
    let rejected = |reason| TriggerChange::Rejected {
        reason,
        trigger: snapshot(app),
    };
    if cfg!(target_os = "linux") {
        return rejected(Rejection::Unsupported);
    }
    let shortcut = match validate(accelerator) {
        Ok(shortcut) => shortcut,
        Err(reason) => return rejected(reason),
    };
    let Some(trigger) = app.try_state::<Trigger>() else {
        return rejected(Rejection::Unavailable);
    };
    let previous = {
        let mut state = trigger.0.lock().unwrap_or_else(|e| e.into_inner());
        state.recording = false;
        state.registered.take()
    };
    if let Some(previous) = &previous {
        unregister(app, previous);
    }
    if !register(app, &shortcut) {
        // Put back what worked rather than leave the launcher unreachable.
        let restored = previous.filter(|previous| register(app, previous));
        trigger
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .registered = restored;
        return rejected(Rejection::Unavailable);
    }
    {
        let mut state = trigger.0.lock().unwrap_or_else(|e| e.into_inner());
        state.shortcut = accelerator.to_owned();
        state.registered = Some(shortcut);
    }
    persist(SHORTCUT_KEY, accelerator);
    TriggerChange::Applied {
        trigger: snapshot(app),
    }
}

/// While a new shortcut is being recorded the current one must not fire, or
/// pressing it to re-record would open the launcher instead.
#[cfg(not(target_os = "linux"))]
pub fn recording(app: &AppHandle, active: bool) {
    let Some(trigger) = app.try_state::<Trigger>() else {
        return;
    };
    let mut state = trigger.0.lock().unwrap_or_else(|e| e.into_inner());
    if state.recording == active {
        return;
    }
    state.recording = active;
    let current = state.registered.take();
    let shortcut = state.shortcut.clone();
    drop(state);
    if active {
        if let Some(current) = &current {
            unregister(app, current);
        }
        trigger
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .registered = current;
        return;
    }
    let restored = Shortcut::from_str(&shortcut)
        .ok()
        .filter(|shortcut| register(app, shortcut));
    trigger
        .0
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .registered = restored;
}

#[cfg(not(target_os = "linux"))]
pub fn set_double_tap(app: &AppHandle, mode: DoubleTap) -> LauncherTrigger {
    if let Some(trigger) = app.try_state::<Trigger>() {
        trigger
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .double_tap = mode;
    }
    persist(
        DOUBLE_TAP_KEY,
        match mode {
            DoubleTap::Off => "off",
            DoubleTap::Command => "command",
            DoubleTap::Option => "option",
        },
    );
    #[cfg(target_os = "macos")]
    if mode != DoubleTap::Off && !double_tap::trusted() {
        double_tap::request();
    }
    snapshot(app)
}

#[cfg(not(target_os = "linux"))]
pub fn open_accessibility_settings() {
    #[cfg(target_os = "macos")]
    double_tap::open_settings();
}

#[cfg(target_os = "linux")]
pub fn install(_app: &AppHandle, _keymap_override: Option<String>) {}

#[cfg(target_os = "linux")]
pub fn snapshot(app: &AppHandle) -> LauncherTrigger {
    let registered = app
        .try_state::<crate::linux_global_shortcuts::LinuxGlobalShortcuts>()
        .is_some_and(|state| state.is_registered());
    linux_snapshot(registered)
}

#[cfg(target_os = "linux")]
fn linux_snapshot(registered: bool) -> LauncherTrigger {
    LauncherTrigger {
        shortcut: DEFAULT.into(),
        default_shortcut: DEFAULT.into(),
        registered,
        editable: false,
        double_tap: DoubleTap::Off,
        double_tap_supported: false,
        accessibility: false,
    }
}

#[cfg(target_os = "linux")]
pub fn set_shortcut(app: &AppHandle, _accelerator: &str) -> TriggerChange {
    linux_rejected_shortcut_change(snapshot(app))
}

#[cfg(target_os = "linux")]
fn linux_rejected_shortcut_change(trigger: LauncherTrigger) -> TriggerChange {
    TriggerChange::Rejected {
        reason: Rejection::Unsupported,
        trigger,
    }
}

#[cfg(target_os = "linux")]
pub fn recording(_app: &AppHandle, _active: bool) {}

#[cfg(target_os = "linux")]
pub fn set_double_tap(app: &AppHandle, _mode: DoubleTap) -> LauncherTrigger {
    snapshot(app)
}

#[cfg(target_os = "linux")]
pub fn open_accessibility_settings() {}

#[cfg(all(test, not(target_os = "linux")))]
mod tests {
    use super::*;

    #[test]
    fn launcher_shortcuts_need_a_real_modifier() {
        assert!(validate(DEFAULT).is_ok());
        #[cfg(target_os = "macos")]
        assert!(validate("Alt+Space").is_ok());
        #[cfg(not(target_os = "macos"))]
        assert_eq!(validate("Alt+Space"), Err(Rejection::System));
        assert!(validate("Ctrl+Alt+KeyK").is_ok());
        assert_eq!(validate("Shift+Space"), Err(Rejection::NeedsModifier));
        assert_eq!(validate("Space"), Err(Rejection::NeedsModifier));
        assert_eq!(validate("Nonsense+Key"), Err(Rejection::Invalid));
        assert_eq!(validate(""), Err(Rejection::Invalid));
    }

    #[test]
    fn app_commands_cannot_be_taken_globally() {
        // Redo, reopen tab and find in every app.
        for taken in [
            "CmdOrCtrl+Shift+KeyZ",
            "CmdOrCtrl+Shift+KeyT",
            "CmdOrCtrl+KeyF",
        ] {
            assert_eq!(validate(taken), Err(Rejection::AppCommand), "{taken}");
        }
        assert!(validate("CmdOrCtrl+Shift+F5").is_ok());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_system_keeps_its_own_shortcuts() {
        for taken in [
            "Cmd+Space",
            "Ctrl+Space",
            "Ctrl+Alt+Space",
            "Ctrl+Cmd+Space",
            "Alt+Cmd+Space",
        ] {
            assert_eq!(validate(taken), Err(Rejection::System), "{taken}");
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
mod linux_tests {
    use super::*;

    #[test]
    fn shortcut_status_reports_registration_and_editing_fails_closed() {
        let trigger = linux_snapshot(false);
        assert_eq!(trigger.shortcut, DEFAULT);
        assert_eq!(trigger.default_shortcut, DEFAULT);
        assert!(!trigger.registered);
        assert!(!trigger.editable);
        assert_eq!(trigger.double_tap, DoubleTap::Off);
        assert!(!trigger.double_tap_supported);
        assert!(!trigger.accessibility);

        assert!(matches!(
            linux_rejected_shortcut_change(linux_snapshot(true)),
            TriggerChange::Rejected {
                reason: Rejection::Unsupported,
                trigger: rejected,
            } if rejected.registered && rejected.shortcut == DEFAULT
        ));
        assert!(linux_snapshot(true).registered);
    }
}
