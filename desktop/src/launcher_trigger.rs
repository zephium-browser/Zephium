//! What opens the launcher from anywhere: a global shortcut anyone can
//! record, and on macOS an opt-in double tap of a modifier. Every good
//! two-key combination is already claimed by some other tool, so the choice
//! belongs to the person, and the settings page reports whether it holds.
use std::str::FromStr;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};
use zephium_core::ports::store::Store;

#[cfg(target_os = "macos")]
mod double_tap;

pub const DEFAULT: &str = "CmdOrCtrl+Shift+Space";
const SHORTCUT_KEY: &str = "launcher.shortcut";
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
    pub ownership_available: bool,
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

#[derive(Default)]
pub struct Trigger(Mutex<State>);

#[derive(Default)]
struct State {
    shortcut: String,
    registered: Option<Shortcut>,
    recording: bool,
    double_tap: DoubleTap,
    install_pending: bool,
}

/// Checks a recorded accelerator before anything is unregistered, so a bad
/// choice never costs the working one.
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

fn unregister(app: &AppHandle, shortcut: &Shortcut) {
    let _ = app.global_shortcut().unregister(*shortcut);
}

fn setting(key: &str) -> Option<String> {
    crate::APP_STORE
        .get()
        .and_then(|store| store.app_setting(key))
        .filter(|value| !value.is_empty())
}

fn persist(key: &str, value: &str) {
    if let Some(store) = crate::APP_STORE.get() {
        store.set_app_setting(key.into(), value.into());
    }
}

/// Registers what was chosen last time, or the default. The keymap override
/// is honoured for a profile that set one before there was a recorder.
pub fn install(app: &AppHandle, keymap_override: Option<String>) {
    install_with_settings(
        app,
        keymap_override,
        setting(SHORTCUT_KEY),
        setting(DOUBLE_TAP_KEY),
    );
}

fn install_with_settings(
    app: &AppHandle,
    keymap_override: Option<String>,
    shortcut_setting: Option<String>,
    double_tap_setting: Option<String>,
) {
    let owns_global_shortcut = app
        .try_state::<crate::instance_lease::GlobalShortcutOwner>()
        .is_some_and(|owner| owner.owns());
    let shortcut = shortcut_setting
        .or(keymap_override)
        .unwrap_or_else(|| DEFAULT.into());
    let parsed = if owns_global_shortcut {
        Shortcut::from_str(&shortcut)
            .ok()
            .or_else(|| Shortcut::from_str(DEFAULT).ok())
    } else {
        None
    };
    let registered =
        register_owned_shortcut(owns_global_shortcut, parsed, |parsed| register(app, parsed));
    let double_tap = match double_tap_setting.as_deref() {
        Some("command") => DoubleTap::Command,
        Some("option") => DoubleTap::Option,
        _ => DoubleTap::Off,
    };
    if let Some(trigger) = app.try_state::<Trigger>() {
        let mut state = trigger.0.lock().unwrap_or_else(|e| e.into_inner());
        state.shortcut = shortcut;
        state.registered = registered;
        state.double_tap = double_tap;
        state.install_pending = false;
    }
    #[cfg(target_os = "macos")]
    if should_apply_double_tap(owns_global_shortcut) {
        double_tap::apply(app, double_tap);
    }
}

pub fn monitor_global_shortcut_owner(app: &AppHandle, keymap_override: Option<String>) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            if app
                .try_state::<crate::ShutdownCoordinator>()
                .is_some_and(|shutdown| shutdown.terminal_started())
            {
                return;
            }
            let owns = app
                .try_state::<crate::instance_lease::GlobalShortcutOwner>()
                .is_some_and(|owner| {
                    owner.owns()
                        || owner.try_claim().unwrap_or_else(|error| {
                            crate::write_diagnostic(format_args!(
                                "launcher: shortcut ownership unavailable: {error}"
                            ));
                            false
                        })
                });
            let retry = should_attempt_install(owns, false)
                && app
                    .try_state::<Trigger>()
                    .as_deref()
                    .is_some_and(reserve_install);
            if retry {
                let app = app.clone();
                let handle = app.clone();
                let keymap_override = keymap_override.clone();
                let scheduled = app
                    .run_on_main_thread(move || {
                        let still_owns = handle
                            .try_state::<crate::instance_lease::GlobalShortcutOwner>()
                            .is_some_and(|owner| owner.owns());
                        let shutting_down = handle
                            .try_state::<crate::ShutdownCoordinator>()
                            .is_some_and(|shutdown| shutdown.terminal_started());
                        let can_install = handle
                            .try_state::<Trigger>()
                            .is_some_and(|trigger| install_is_available(&trigger))
                            && should_attempt_install(still_owns, shutting_down);
                        if can_install {
                            let shortcut_setting = setting(SHORTCUT_KEY);
                            let double_tap_setting = setting(DOUBLE_TAP_KEY);
                            install_with_settings(
                                &handle,
                                keymap_override,
                                shortcut_setting,
                                double_tap_setting,
                            );
                        } else if let Some(trigger) = handle.try_state::<Trigger>() {
                            release_install_reservation(&trigger);
                        }
                    })
                    .is_ok();
                if let Some(trigger) = app.try_state::<Trigger>() {
                    release_reservation_after_schedule(&trigger, scheduled);
                }
            }
        }
    });
}

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
        if app
            .try_state::<crate::instance_lease::GlobalShortcutOwner>()
            .is_some_and(|owner| should_apply_double_tap(owner.owns()))
        {
            double_tap::apply(app, double_tap);
        }
        double_tap::trusted()
    };
    #[cfg(not(target_os = "macos"))]
    let accessibility = false;
    let owns_global_shortcut = app
        .try_state::<crate::instance_lease::GlobalShortcutOwner>()
        .is_some_and(|owner| owner.owns());
    let (ownership_available, editable) =
        shortcut_access(cfg!(target_os = "linux"), owns_global_shortcut);
    LauncherTrigger {
        shortcut,
        default_shortcut: DEFAULT.into(),
        registered,
        editable,
        ownership_available,
        double_tap,
        double_tap_supported: cfg!(target_os = "macos"),
        accessibility,
    }
}

pub fn set_shortcut(app: &AppHandle, accelerator: &str) -> TriggerChange {
    let rejected = |reason| TriggerChange::Rejected {
        reason,
        trigger: snapshot(app),
    };
    let owns = app
        .try_state::<crate::instance_lease::GlobalShortcutOwner>()
        .is_some_and(|owner| owner.owns());
    if let Some(reason) = shortcut_owner_rejection(cfg!(target_os = "linux"), owns) {
        return rejected(reason);
    }
    let shortcut = match validate(accelerator) {
        Ok(shortcut) => shortcut,
        Err(reason) => return rejected(reason),
    };
    if let Some(reason) = shortcut_state_rejection(app.try_state::<Trigger>().is_some()) {
        return rejected(reason);
    }
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
pub fn recording(app: &AppHandle, active: bool) {
    let owns = app
        .try_state::<crate::instance_lease::GlobalShortcutOwner>()
        .is_some_and(|owner| owner.owns());
    let Some(trigger) = app.try_state::<Trigger>() else {
        return;
    };
    let mut state = trigger.0.lock().unwrap_or_else(|e| e.into_inner());
    if clear_recording_without_ownership(&mut state, owns) {
        return;
    }
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

fn reserve_install(trigger: &Trigger) -> bool {
    let mut state = trigger.0.lock().unwrap_or_else(|e| e.into_inner());
    if state.registered.is_some() || state.install_pending || state.recording {
        return false;
    }
    state.install_pending = true;
    true
}

fn release_install_reservation(trigger: &Trigger) {
    trigger
        .0
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .install_pending = false;
}

fn release_reservation_after_schedule(trigger: &Trigger, scheduled: bool) {
    if !scheduled {
        release_install_reservation(trigger);
    }
}

fn clear_recording_without_ownership(state: &mut State, owns: bool) -> bool {
    if owns {
        return false;
    }
    state.recording = false;
    state.registered = None;
    true
}

fn shortcut_access(is_linux: bool, owns: bool) -> (bool, bool) {
    (is_linux || owns, !is_linux && owns)
}

fn shortcut_owner_rejection(is_linux: bool, owns: bool) -> Option<Rejection> {
    if is_linux {
        Some(Rejection::Unsupported)
    } else if !owns {
        Some(Rejection::Unavailable)
    } else {
        None
    }
}

fn shortcut_state_rejection(has_trigger: bool) -> Option<Rejection> {
    (!has_trigger).then_some(Rejection::Unavailable)
}

#[cfg(any(target_os = "macos", test))]
fn should_apply_double_tap(owns: bool) -> bool {
    owns
}

fn install_is_available(trigger: &Trigger) -> bool {
    let state = trigger.0.lock().unwrap_or_else(|e| e.into_inner());
    state.registered.is_none() && !state.recording
}

fn should_attempt_install(owns: bool, shutting_down: bool) -> bool {
    owns && !shutting_down
}

#[cfg(test)]
fn should_install_after_reservation(owns: bool, shutting_down: bool, can_record: bool) -> bool {
    should_attempt_install(owns, shutting_down) && can_record
}

fn register_owned_shortcut(
    owns: bool,
    shortcut: Option<Shortcut>,
    register: impl FnOnce(&Shortcut) -> bool,
) -> Option<Shortcut> {
    if owns {
        shortcut.filter(register)
    } else {
        None
    }
}

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

pub fn open_accessibility_settings() {
    #[cfg(target_os = "macos")]
    double_tap::open_settings();
}

#[cfg(test)]
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

    #[test]
    fn shortcut_registration_retries_only_for_the_owner() {
        let trigger = Trigger::default();
        assert!(should_attempt_install(true, false));
        assert!(!should_attempt_install(false, false));
        assert!(!should_attempt_install(true, true));
        assert!(should_install_after_reservation(true, false, true));
        assert!(!should_install_after_reservation(false, false, true));
        assert!(!should_install_after_reservation(true, true, true));
        assert!(!should_install_after_reservation(true, false, false));
        assert!(install_is_available(&trigger));
        assert!(reserve_install(&trigger));
        assert!(!reserve_install(&trigger));
        trigger.0.lock().unwrap().install_pending = false;
        trigger.0.lock().unwrap().recording = true;
        assert!(!reserve_install(&trigger));
        trigger.0.lock().unwrap().recording = false;
        assert!(reserve_install(&trigger));
        trigger.0.lock().unwrap().registered = Some(Shortcut::from_str(DEFAULT).unwrap());
        trigger.0.lock().unwrap().install_pending = false;
        assert!(!install_is_available(&trigger));
        assert!(!reserve_install(&trigger));
        trigger.0.lock().unwrap().install_pending = true;
        release_reservation_after_schedule(&trigger, false);
        assert!(!trigger.0.lock().unwrap().install_pending);
        trigger.0.lock().unwrap().install_pending = true;
        release_reservation_after_schedule(&trigger, true);
        assert!(trigger.0.lock().unwrap().install_pending);
    }

    #[test]
    fn failed_owner_registration_leaves_no_registered_shortcut() {
        let shortcut = Shortcut::from_str(DEFAULT).unwrap();
        let registered = register_owned_shortcut(true, Some(shortcut), |_| false);
        assert!(registered.is_none());
        let attempted = std::cell::Cell::new(false);
        let registered =
            register_owned_shortcut(false, Some(Shortcut::from_str(DEFAULT).unwrap()), |_| {
                attempted.set(true);
                true
            });
        assert!(registered.is_none());
        assert!(!attempted.get());
    }

    #[test]
    fn shortcut_changes_require_a_supported_owner_and_state() {
        assert_eq!(
            shortcut_owner_rejection(false, false),
            Some(Rejection::Unavailable)
        );
        assert_eq!(shortcut_owner_rejection(false, true), None);
        assert_eq!(
            shortcut_owner_rejection(true, true),
            Some(Rejection::Unsupported)
        );
        assert_eq!(
            shortcut_state_rejection(false),
            Some(Rejection::Unavailable)
        );
        assert_eq!(shortcut_state_rejection(true), None);
    }

    #[test]
    fn nonowners_cannot_restore_a_recorded_shortcut() {
        let mut state = State {
            shortcut: "Ctrl+Alt+KeyK".into(),
            registered: Some(Shortcut::from_str(DEFAULT).unwrap()),
            recording: true,
            ..State::default()
        };
        assert!(clear_recording_without_ownership(&mut state, false));
        assert!(!state.recording);
        assert!(state.registered.is_none());
        assert_eq!(state.shortcut, "Ctrl+Alt+KeyK");
        assert!(!clear_recording_without_ownership(&mut state, true));
    }

    #[test]
    fn snapshot_access_reflects_platform_and_lease_ownership() {
        assert_eq!(shortcut_access(false, false), (false, false));
        assert_eq!(shortcut_access(false, true), (true, true));
        assert_eq!(shortcut_access(true, false), (true, false));
        assert!(!should_apply_double_tap(false));
        assert!(should_apply_double_tap(true));
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
