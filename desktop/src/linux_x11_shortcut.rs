use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError};
use std::sync::{Arc, Mutex, Weak};
use std::thread::JoinHandle;
use std::time::Duration;

use x11rb::connection::Connection;
use x11rb::errors::ReplyError;
use x11rb::protocol::xproto::{ConnectionExt, GrabMode, KeyButMask, Keycode, ModMask, Window};
use x11rb::protocol::{xkb, ErrorKind, Event};
use x11rb::rust_connection::RustConnection;
use xkeysym::RawKeysym;

use crate::linux_shortcut::{LinuxLauncherKey, LinuxLauncherShortcut};
use crate::linux_shortcut_portal::{ActivationContext, ActivationTarget, GlobalRegistration};

const EVENT_POLL: Duration = Duration::from_millis(50);
const SHUTDOWN_WAIT: Duration = Duration::from_millis(500);
const MAX_EVENTS_PER_TURN: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
enum WorkerTerminal {
    Running = 0,
    Clean = 1,
    AlreadyGrabbed = 2,
    Failed = 3,
}

enum WorkerFailure {
    AlreadyGrabbed,
    Other(String),
}

impl From<String> for WorkerFailure {
    fn from(error: String) -> Self {
        Self::Other(error)
    }
}

impl std::fmt::Display for WorkerFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AlreadyGrabbed => {
                formatter.write_str("shortcut is already grabbed by another application")
            }
            Self::Other(error) => formatter.write_str(error),
        }
    }
}

pub(crate) struct X11Shortcut {
    stop: Option<SyncSender<()>>,
    completed: Receiver<()>,
    worker: Option<JoinHandle<()>>,
    lifecycle: Arc<RegistrationLifecycle>,
    terminal: Arc<AtomicU8>,
    completed_observed: bool,
    stopped: bool,
}

impl X11Shortcut {
    pub(crate) fn spawn(
        shortcut: LinuxLauncherShortcut,
        target: Weak<ActivationTarget>,
        registration: GlobalRegistration,
    ) -> Result<Self, String> {
        let (stop_tx, stop_rx) = mpsc::sync_channel(1);
        let (completed_tx, completed_rx) = mpsc::sync_channel(1);
        let lifecycle = Arc::new(RegistrationLifecycle::new(registration));
        let worker_lifecycle = lifecycle.clone();
        let terminal = Arc::new(AtomicU8::new(WorkerTerminal::Running as u8));
        let worker_terminal = terminal.clone();
        let worker = std::thread::Builder::new()
            .name("zephium-x11-shortcut".to_owned())
            .spawn(move || {
                let _completion = CompletionSignal(completed_tx);
                let result = run_worker(shortcut, target, stop_rx, worker_lifecycle.clone());
                let terminal = match &result {
                    Ok(()) => WorkerTerminal::Clean,
                    Err(WorkerFailure::AlreadyGrabbed) => WorkerTerminal::AlreadyGrabbed,
                    Err(WorkerFailure::Other(_)) => WorkerTerminal::Failed,
                };
                worker_terminal.store(terminal as u8, Ordering::Release);
                match result {
                    Err(error) if !worker_lifecycle.is_cancelled() => {
                        crate::write_diagnostic(format_args!(
                            "global launcher shortcut unavailable on X11: {error}"
                        ));
                    }
                    _ => {}
                }
            })
            .map_err(|error| error.to_string())?;

        Ok(Self {
            stop: Some(stop_tx),
            completed: completed_rx,
            worker: Some(worker),
            lifecycle,
            terminal,
            completed_observed: false,
            stopped: false,
        })
    }

    pub(crate) fn shutdown(mut self) {
        self.stop_and_join();
    }

    fn stop_and_join(&mut self) {
        if self.stopped {
            return;
        }
        self.stopped = true;
        self.lifecycle.cancel();
        if let Some(stop) = self.stop.take() {
            let _ = stop.try_send(());
        }
        let completed = self.observe_completion(SHUTDOWN_WAIT);
        if completed {
            let terminal = self.terminal();
            // CompletionSignal is the worker's first local and therefore
            // drops last, after its X connection and registration guard. Once
            // observed, no native cleanup remains. Detach rather than making
            // an otherwise bounded browser shutdown depend on OS scheduling
            // between that final signal and the thread's return instruction.
            self.worker.take();
            if terminal == WorkerTerminal::Running {
                crate::write_diagnostic(format_args!(
                    "global shortcut: X11 worker ended without a terminal status"
                ));
            }
        } else {
            // A dead or unresponsive X server must not make browser shutdown
            // unbounded. The detached worker owns only weak/cancelled state;
            // its connection and passive grabs are released when it returns.
            self.worker.take();
            crate::write_diagnostic(format_args!(
                "global shortcut: X11 worker did not stop within the bounded shutdown window"
            ));
        }
        self.lifecycle.cancel();
    }

    fn observe_completion(&mut self, timeout: Duration) -> bool {
        if self.completed_observed {
            return true;
        }
        match self.completed.recv_timeout(timeout) {
            Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                self.completed_observed = true;
                true
            }
            Err(mpsc::RecvTimeoutError::Timeout) => false,
        }
    }

    fn terminal(&self) -> WorkerTerminal {
        match self.terminal.load(Ordering::Acquire) {
            value if value == WorkerTerminal::Clean as u8 => WorkerTerminal::Clean,
            value if value == WorkerTerminal::AlreadyGrabbed as u8 => {
                WorkerTerminal::AlreadyGrabbed
            }
            value if value == WorkerTerminal::Failed as u8 => WorkerTerminal::Failed,
            _ => WorkerTerminal::Running,
        }
    }
}

impl Drop for X11Shortcut {
    fn drop(&mut self) {
        self.stop_and_join();
    }
}

struct CompletionSignal(SyncSender<()>);

impl Drop for CompletionSignal {
    fn drop(&mut self) {
        let _ = self.0.try_send(());
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RegistrationState {
    Starting,
    Live,
    Cancelled,
}

struct RegistrationLifecycle {
    state: Mutex<RegistrationState>,
    registration: GlobalRegistration,
}

impl RegistrationLifecycle {
    fn new(registration: GlobalRegistration) -> Self {
        registration.set_live(false);
        Self {
            state: Mutex::new(RegistrationState::Starting),
            registration,
        }
    }

    fn publish_live(&self) -> bool {
        let mut state = lock_recover(&self.state);
        if *state != RegistrationState::Starting {
            return false;
        }
        self.registration.set_live(true);
        *state = RegistrationState::Live;
        true
    }

    fn clear(&self) {
        let mut state = lock_recover(&self.state);
        if *state == RegistrationState::Live {
            self.registration.set_live(false);
            *state = RegistrationState::Starting;
        }
    }

    fn cancel(&self) {
        let mut state = lock_recover(&self.state);
        self.registration.set_live(false);
        *state = RegistrationState::Cancelled;
    }

    fn is_cancelled(&self) -> bool {
        *lock_recover(&self.state) == RegistrationState::Cancelled
    }
}

struct ClearLiveOnDrop(Arc<RegistrationLifecycle>);

impl Drop for ClearLiveOnDrop {
    fn drop(&mut self) {
        self.0.clear();
    }
}

fn run_worker(
    shortcut: LinuxLauncherShortcut,
    target: Weak<ActivationTarget>,
    stop: Receiver<()>,
    lifecycle: Arc<RegistrationLifecycle>,
) -> Result<(), WorkerFailure> {
    if lifecycle.is_cancelled() || !target_is_active(&target) {
        return Ok(());
    }

    let (connection, screen) = RustConnection::connect(None)
        .map_err(|error| format!("could not open the X11 connection: {error}"))?;
    initialize_xkb(&connection)?;
    let root = connection
        .setup()
        .roots
        .get(screen)
        .ok_or_else(|| "X11 selected a screen outside the setup roots".to_owned())?
        .root;
    let keyboard = KeyboardMapping::load(&connection)?;
    let keycode = shortcut_keycode(&keyboard, shortcut)?;
    let modifiers = shortcut_modifiers(shortcut);
    let ignored = ignored_modifier_combinations(&connection, &keyboard, modifiers)?;
    let ignored_union = ignored
        .iter()
        .copied()
        .fold(ModMask::default(), |combined, mask| combined | mask);
    let grabs = install_grabs(&connection, root, keycode, modifiers, &ignored)?;

    if lifecycle.is_cancelled() || stop_requested(&stop) || !target_is_active(&target) {
        release_grabs(&connection, root, keycode, &grabs);
        return Ok(());
    }
    if !lifecycle.publish_live() {
        release_grabs(&connection, root, keycode, &grabs);
        return Ok(());
    }
    let clear_live = ClearLiveOnDrop(lifecycle);

    let result = event_loop(&connection, keycode, modifiers, ignored_union, target, stop)
        .map_err(WorkerFailure::from);
    // Revoke the capability before any best-effort X11 cleanup. A broken
    // connection must restore the focused fallback even if ungrabbing or
    // flushing the dead transport is slow.
    drop(clear_live);
    release_grabs(&connection, root, keycode, &grabs);
    result
}

fn initialize_xkb(connection: &RustConnection) -> Result<(), String> {
    let extension = xkb::ConnectionExt::xkb_use_extension(connection, 1, 0)
        .map_err(|error| format!("could not request the XKB extension: {error}"))?
        .reply()
        .map_err(|error| format!("XKB extension negotiation failed: {error}"))?;
    if !extension.supported {
        return Err(format!(
            "X11 rejected XKB 1.0 (server reported {}.{})",
            extension.server_major, extension.server_minor
        ));
    }
    let repeat = xkb::ConnectionExt::xkb_per_client_flags(
        connection,
        xkb::ID::USE_CORE_KBD.into(),
        xkb::PerClientFlag::DETECTABLE_AUTO_REPEAT,
        xkb::PerClientFlag::DETECTABLE_AUTO_REPEAT,
        Default::default(),
        Default::default(),
        Default::default(),
    )
    .map_err(|error| format!("could not enable detectable XKB auto-repeat: {error}"))?
    .reply()
    .map_err(|error| format!("enabling detectable XKB auto-repeat failed: {error}"))?;
    let flag = u32::from(xkb::PerClientFlag::DETECTABLE_AUTO_REPEAT);
    if u32::from(repeat.supported) & flag == 0 || u32::from(repeat.value) & flag == 0 {
        return Err("X11 did not prove detectable XKB auto-repeat support".to_owned());
    }
    Ok(())
}

fn shortcut_keycode(
    keyboard: &KeyboardMapping,
    shortcut: LinuxLauncherShortcut,
) -> Result<Keycode, String> {
    let keysym = launcher_key_to_keysym(shortcut.key())?;
    keyboard.unique_base_keycode_for(keysym).ok_or_else(|| {
        "X11 keymap has no unique base-level keycode for the launcher shortcut".to_owned()
    })
}

fn install_grabs(
    connection: &RustConnection,
    root: Window,
    keycode: Keycode,
    modifiers: ModMask,
    ignored_modifiers: &[ModMask],
) -> Result<Vec<ModMask>, WorkerFailure> {
    let mut installed = Vec::with_capacity(ignored_modifiers.len());
    for ignored in ignored_modifiers {
        let mask = modifiers | *ignored;
        let cookie =
            match connection.grab_key(false, root, mask, keycode, GrabMode::ASYNC, GrabMode::ASYNC)
            {
                Ok(cookie) => cookie,
                Err(error) => {
                    release_grabs(connection, root, keycode, &installed);
                    return Err(WorkerFailure::Other(format!(
                        "could not send XGrabKey: {error}"
                    )));
                }
            };
        if let Err(error) = cookie.check() {
            release_grabs(connection, root, keycode, &installed);
            return match error {
                ReplyError::X11Error(error) if error.error_kind == ErrorKind::Access => {
                    Err(WorkerFailure::AlreadyGrabbed)
                }
                other => Err(WorkerFailure::Other(format!(
                    "XGrabKey was rejected: {other}"
                ))),
            };
        }
        installed.push(mask);
    }
    if let Err(error) = connection.flush() {
        release_grabs(connection, root, keycode, &installed);
        return Err(WorkerFailure::Other(format!(
            "could not flush registered X11 shortcut: {error}"
        )));
    }
    Ok(installed)
}

fn release_grabs(
    connection: &RustConnection,
    root: Window,
    keycode: Keycode,
    modifiers: &[ModMask],
) {
    for modifiers in modifiers {
        if let Ok(cookie) = connection.ungrab_key(keycode, root, *modifiers) {
            cookie.ignore_error();
        }
    }
    let _ = connection.flush();
}

fn event_loop(
    connection: &RustConnection,
    keycode: Keycode,
    modifiers: ModMask,
    ignored_modifiers: ModMask,
    target: Weak<ActivationTarget>,
    stop: Receiver<()>,
) -> Result<(), String> {
    let mut pressed = false;
    loop {
        if !target_is_active(&target) {
            return Ok(());
        }
        for _ in 0..MAX_EVENTS_PER_TURN {
            let event = connection
                .poll_for_event()
                .map_err(|error| format!("X11 event connection failed: {error}"))?;
            let Some(event) = event else {
                break;
            };
            match event {
                Event::KeyPress(event) if event.detail == keycode => {
                    let chord_matches =
                        observed_modifiers(event.state, ignored_modifiers) == modifiers;
                    if admit_key_press(&mut pressed, chord_matches) {
                        // The registration mutex is intentionally not part of
                        // this function: application code must never run while
                        // capability publication or shutdown is locked.
                        if let Some(target) = target.upgrade() {
                            target.activate(ActivationContext {
                                activation_token: None,
                                timestamp: Some(event.time),
                            });
                        }
                    }
                }
                Event::KeyRelease(event) if event.detail == keycode => pressed = false,
                // A passive grab is tied to numeric keycode/modifier masks.
                // Once the server changes either mapping, keeping the old
                // capability marked live would suppress the focused fallback
                // while listening to a potentially different chord. End this
                // worker; its registration guard clears before stale grabs are
                // released, and the in-window path resumes immediately.
                Event::MappingNotify(_) => {
                    return Err(
                        "X11 keyboard mapping changed; discarded the stale global grab".to_owned(),
                    );
                }
                _ => {}
            }
        }
        match stop.recv_timeout(EVENT_POLL) {
            Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
}

fn admit_key_press(pressed: &mut bool, chord_matches: bool) -> bool {
    let first_press = !*pressed;
    *pressed = true;
    first_press && chord_matches
}

fn observed_modifiers(state: KeyButMask, ignored: ModMask) -> ModMask {
    let relevant = ModMask::SHIFT
        | ModMask::LOCK
        | ModMask::CONTROL
        | ModMask::M1
        | ModMask::M2
        | ModMask::M3
        | ModMask::M4
        | ModMask::M5;
    ModMask::from(u16::from(state) & u16::from(relevant) & !u16::from(ignored))
}

fn target_is_active(target: &Weak<ActivationTarget>) -> bool {
    target.upgrade().is_some_and(|target| target.is_active())
}

fn stop_requested(stop: &Receiver<()>) -> bool {
    match stop.try_recv() {
        Ok(()) | Err(TryRecvError::Disconnected) => true,
        Err(TryRecvError::Empty) => false,
    }
}

struct KeyboardMapping {
    min_keycode: Keycode,
    keycode_count: u8,
    keysyms_per_keycode: usize,
    keysyms: Vec<RawKeysym>,
}

impl KeyboardMapping {
    fn load(connection: &RustConnection) -> Result<Self, String> {
        let setup = connection.setup();
        let keycode_count = setup
            .max_keycode
            .checked_sub(setup.min_keycode)
            .and_then(|distance| distance.checked_add(1))
            .ok_or_else(|| "X11 returned an invalid keycode range".to_owned())?;
        let reply = connection
            .get_keyboard_mapping(setup.min_keycode, keycode_count)
            .map_err(|error| format!("could not request the X11 keyboard mapping: {error}"))?
            .reply()
            .map_err(|error| format!("X11 keyboard mapping failed: {error}"))?;
        let keysyms_per_keycode = usize::from(reply.keysyms_per_keycode);
        if keysyms_per_keycode == 0 {
            return Err("X11 returned a zero-width keyboard mapping".to_owned());
        }
        let expected = usize::from(keycode_count)
            .checked_mul(keysyms_per_keycode)
            .ok_or_else(|| "X11 keyboard mapping length overflowed".to_owned())?;
        if reply.keysyms.len() != expected {
            return Err("X11 returned an inconsistent keyboard mapping length".to_owned());
        }
        Ok(Self {
            min_keycode: setup.min_keycode,
            keycode_count,
            keysyms_per_keycode,
            keysyms: reply.keysyms,
        })
    }

    fn unique_base_keycode_for(&self, keysym: RawKeysym) -> Option<Keycode> {
        let mut offsets = self
            .keysyms
            .chunks_exact(self.keysyms_per_keycode)
            .enumerate()
            .filter(|(_, keysyms)| keysyms.first() == Some(&keysym))
            .map(|(offset, _)| offset);
        let offset = offsets.next()?;
        if offsets.next().is_some() {
            return None;
        }
        u8::try_from(offset)
            .ok()
            .and_then(|offset| self.min_keycode.checked_add(offset))
            .filter(|keycode| {
                keycode
                    .checked_sub(self.min_keycode)
                    .is_some_and(|offset| offset < self.keycode_count)
            })
    }

    fn keysyms_for(&self, keycode: Keycode) -> Option<&[RawKeysym]> {
        let offset = keycode.checked_sub(self.min_keycode)?;
        if offset >= self.keycode_count {
            return None;
        }
        let start = usize::from(offset).checked_mul(self.keysyms_per_keycode)?;
        let end = start.checked_add(self.keysyms_per_keycode)?;
        self.keysyms.get(start..end)
    }
}

fn ignored_modifier_combinations(
    connection: &RustConnection,
    keyboard: &KeyboardMapping,
    required: ModMask,
) -> Result<Vec<ModMask>, String> {
    let reply = connection
        .get_modifier_mapping()
        .map_err(|error| format!("could not request the X11 modifier mapping: {error}"))?
        .reply()
        .map_err(|error| format!("X11 modifier mapping failed: {error}"))?;
    if reply.keycodes.len() % 8 != 0 {
        return Err("X11 returned a malformed modifier mapping".to_owned());
    }
    let width = reply.keycodes.len() / 8;
    if width == 0 {
        return Ok(vec![ModMask::default()]);
    }

    let lock_keysyms = [
        xkeysym::key::Caps_Lock,
        xkeysym::key::Num_Lock,
        xkeysym::key::Scroll_Lock,
    ];
    let mut lock_masks = Vec::with_capacity(3);
    for (index, keycodes) in reply.keycodes.chunks_exact(width).enumerate() {
        let contains_lock = keycodes
            .iter()
            .copied()
            .filter(|keycode| *keycode != 0)
            .any(|keycode| {
                keyboard.keysyms_for(keycode).is_some_and(|keysyms| {
                    keysyms.iter().any(|keysym| lock_keysyms.contains(keysym))
                })
            });
        let Some(mask) = contains_lock.then(|| modifier_mask(index)).flatten() else {
            continue;
        };
        // If a pathological keymap puts a lock key in a modifier group the
        // requested chord itself needs, X state cannot distinguish them. Keep
        // that bit required instead of widening the grab to another chord.
        if u16::from(mask) & u16::from(required) == 0 && !lock_masks.contains(&mask) {
            lock_masks.push(mask);
        }
    }

    let mut combinations = vec![ModMask::default()];
    for mask in lock_masks {
        let additions: Vec<_> = combinations
            .iter()
            .copied()
            .map(|combination| combination | mask)
            .collect();
        for addition in additions {
            if !combinations.contains(&addition) {
                combinations.push(addition);
            }
        }
    }
    Ok(combinations)
}

fn modifier_mask(index: usize) -> Option<ModMask> {
    Some(match index {
        0 => ModMask::SHIFT,
        1 => ModMask::LOCK,
        2 => ModMask::CONTROL,
        3 => ModMask::M1,
        4 => ModMask::M2,
        5 => ModMask::M3,
        6 => ModMask::M4,
        7 => ModMask::M5,
        _ => return None,
    })
}

fn shortcut_modifiers(shortcut: LinuxLauncherShortcut) -> ModMask {
    let mut result = ModMask::default();
    if shortcut.shift() {
        result |= ModMask::SHIFT;
    }
    if shortcut.alt() {
        result |= ModMask::M1;
    }
    if shortcut.ctrl() {
        result |= ModMask::CONTROL;
    }
    result
}

fn launcher_key_to_keysym(key: LinuxLauncherKey) -> Result<RawKeysym, String> {
    match key {
        LinuxLauncherKey::Space => Ok(xkeysym::key::space),
        LinuxLauncherKey::Tab => Ok(xkeysym::key::Tab),
        // A core XGrabKey binds a physical keycode, while GTK and the portal
        // bind a logical symbol. Correctly translating a configurable text key
        // across active XKB groups and levels requires a full XKB keymap/state
        // resolver. Refuse the global X11 capability instead of silently
        // grabbing whichever physical key happens to contain that symbol;
        // the exact focused fallback remains available. Space and Tab are the
        // only layout-independent launcher keys currently admitted here.
        LinuxLauncherKey::AsciiAlphanumeric(_) => Err(
            "direct X11 global shortcuts currently require layout-independent Space or Tab"
                .to_owned(),
        ),
    }
}

fn lock_recover<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registration_lifecycle_cannot_republish_after_cancellation() {
        let registration = GlobalRegistration::default();
        let lifecycle = Arc::new(RegistrationLifecycle::new(registration.clone()));
        assert!(!registration.is_live());
        assert!(lifecycle.publish_live());
        assert!(registration.is_live());
        drop(ClearLiveOnDrop(lifecycle.clone()));
        assert!(!registration.is_live());
        lifecycle.cancel();
        assert!(!lifecycle.publish_live());
        assert!(!registration.is_live());
    }

    #[test]
    fn launcher_accelerator_maps_to_exact_x11_chord() {
        let shortcut =
            LinuxLauncherShortcut::parse("CmdOrCtrl+Shift+Space").expect("launcher accelerator");
        assert_eq!(
            launcher_key_to_keysym(shortcut.key()),
            Ok(xkeysym::key::space)
        );
        assert_eq!(
            shortcut_modifiers(shortcut),
            ModMask::CONTROL | ModMask::SHIFT
        );
        let layout_dependent =
            LinuxLauncherShortcut::parse("Ctrl+Alt+T").expect("valid shared accelerator");
        assert!(launcher_key_to_keysym(layout_dependent.key()).is_err());
    }

    #[test]
    fn x11_keycode_resolution_requires_one_exact_base_level_symbol() {
        let unique = KeyboardMapping {
            min_keycode: 8,
            keycode_count: 2,
            keysyms_per_keycode: 2,
            keysyms: vec![xkeysym::key::space, 0, xkeysym::key::Tab, 0],
        };
        assert_eq!(unique.unique_base_keycode_for(xkeysym::key::space), Some(8));
        assert_eq!(unique.unique_base_keycode_for(xkeysym::key::Tab), Some(9));

        let ambiguous = KeyboardMapping {
            min_keycode: 8,
            keycode_count: 2,
            keysyms_per_keycode: 2,
            keysyms: vec![xkeysym::key::space, 0, xkeysym::key::space, 0],
        };
        assert_eq!(ambiguous.unique_base_keycode_for(xkeysym::key::space), None);

        let alternate_level = KeyboardMapping {
            min_keycode: 8,
            keycode_count: 1,
            keysyms_per_keycode: 2,
            keysyms: vec![xkeysym::key::Tab, xkeysym::key::space],
        };
        assert_eq!(
            alternate_level.unique_base_keycode_for(xkeysym::key::space),
            None
        );
    }

    #[test]
    fn observed_chord_ignores_locks_but_rejects_unrelated_modifiers() {
        let ignored = ModMask::LOCK | ModMask::M2;
        assert_eq!(
            observed_modifiers(
                KeyButMask::CONTROL | KeyButMask::SHIFT | KeyButMask::LOCK,
                ignored,
            ),
            ModMask::CONTROL | ModMask::SHIFT
        );
        assert_eq!(
            observed_modifiers(
                KeyButMask::CONTROL | KeyButMask::SHIFT | KeyButMask::MOD2,
                ignored,
            ),
            ModMask::CONTROL | ModMask::SHIFT
        );
        assert_ne!(
            observed_modifiers(
                KeyButMask::CONTROL | KeyButMask::SHIFT | KeyButMask::MOD3,
                ignored,
            ),
            ModMask::CONTROL | ModMask::SHIFT
        );
    }

    #[test]
    fn repeat_and_modifier_changes_cannot_toggle_without_a_release() {
        let mut pressed = false;
        assert!(admit_key_press(&mut pressed, true));
        assert!(!admit_key_press(&mut pressed, true));
        pressed = false;
        assert!(!admit_key_press(&mut pressed, false));
        assert!(!admit_key_press(&mut pressed, true));
        pressed = false;
        assert!(admit_key_press(&mut pressed, true));
    }

    #[test]
    fn all_x11_modifier_groups_have_an_explicit_mapping() {
        assert_eq!(modifier_mask(0), Some(ModMask::SHIFT));
        assert_eq!(modifier_mask(1), Some(ModMask::LOCK));
        assert_eq!(modifier_mask(4), Some(ModMask::M2));
        assert_eq!(modifier_mask(7), Some(ModMask::M5));
        assert_eq!(modifier_mask(8), None);
    }

    #[test]
    fn event_processing_and_shutdown_are_statically_bounded() {
        let source = include_str!("linux_x11_shortcut.rs");
        assert!(source.contains("const MAX_EVENTS_PER_TURN: usize = 64;"));
        assert!(source.contains("const EVENT_POLL: Duration = Duration::from_millis(50);"));
        assert!(source.contains("const SHUTDOWN_WAIT: Duration = Duration::from_millis(500);"));
        assert!(source.contains("Event::MappingNotify(_)"));
        assert!(source.contains("discarded the stale global grab"));
    }

    #[test]
    fn production_x11_adapter_has_no_panicking_diagnostics_or_accessors() {
        let production = include_str!("linux_x11_shortcut.rs")
            .split("#[cfg(test)]")
            .next()
            .expect("production X11 adapter");
        for forbidden in [
            "eprintln!",
            ".unwrap(",
            ".expect(",
            "panic!",
            "unreachable!",
        ] {
            assert!(
                !production.contains(forbidden),
                "production X11 adapter contains {forbidden}"
            );
        }
        assert!(production.contains("MAX_EVENTS_PER_TURN"));
        assert!(production.contains("X11 event connection failed"));
        let spawn = production
            .split("pub(crate) fn spawn(")
            .nth(1)
            .and_then(|source| source.split("pub(crate) fn shutdown").next())
            .expect("bounded X11 spawn adapter");
        assert!(spawn.contains("zephium-x11-shortcut"));
        assert!(!spawn.contains("RustConnection::connect"));

        let worker = production
            .split("fn run_worker(")
            .nth(1)
            .and_then(|source| source.split("fn initialize_xkb").next())
            .expect("bounded X11 native worker");
        let connect = worker
            .find("RustConnection::connect")
            .expect("worker-owned X11 connection");
        let grabs = worker.find("install_grabs").expect("checked passive grabs");
        let publish = worker
            .find("publish_live")
            .expect("live capability publication");
        assert!(connect < grabs && grabs < publish);

        let event_loop = production
            .split("fn event_loop(")
            .nth(1)
            .and_then(|source| source.split("fn observed_modifiers").next())
            .expect("bounded X11 event loop");
        assert!(!event_loop.contains("RegistrationLifecycle"));
    }

    #[test]
    #[ignore = "requires a real X11 server (CI runs this under Xvfb)"]
    fn native_registration_is_live_collision_checked_and_releasable() {
        fn wait_for(expected: bool, registration: &GlobalRegistration) {
            let deadline = std::time::Instant::now() + Duration::from_secs(3);
            while std::time::Instant::now() < deadline {
                if registration.is_live() == expected {
                    return;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            assert_eq!(registration.is_live(), expected);
        }

        let first_registration = GlobalRegistration::default();
        let first_target = Arc::new(ActivationTarget::new(|_| {}));
        let first = X11Shortcut::spawn(
            LinuxLauncherShortcut::parse("CmdOrCtrl+Shift+Space").expect("launcher accelerator"),
            Arc::downgrade(&first_target),
            first_registration.clone(),
        )
        .expect("spawn first X11 worker");
        wait_for(true, &first_registration);

        let collision_registration = GlobalRegistration::default();
        let collision_target = Arc::new(ActivationTarget::new(|_| {}));
        let mut collision = X11Shortcut::spawn(
            LinuxLauncherShortcut::parse("CmdOrCtrl+Shift+Space").expect("launcher accelerator"),
            Arc::downgrade(&collision_target),
            collision_registration.clone(),
        )
        .expect("spawn collision worker");
        assert!(
            collision.observe_completion(Duration::from_secs(3)),
            "collision worker did not return its checked XGrabKey result"
        );
        assert_eq!(collision.terminal(), WorkerTerminal::AlreadyGrabbed);
        assert!(!collision_registration.is_live());
        collision_target.stop();
        collision.shutdown();

        first_target.stop();
        first.shutdown();
        assert!(!first_registration.is_live());

        let replacement_registration = GlobalRegistration::default();
        let replacement_target = Arc::new(ActivationTarget::new(|_| {}));
        let replacement = X11Shortcut::spawn(
            LinuxLauncherShortcut::parse("CmdOrCtrl+Shift+Space").expect("launcher accelerator"),
            Arc::downgrade(&replacement_target),
            replacement_registration.clone(),
        )
        .expect("spawn replacement X11 worker");
        wait_for(true, &replacement_registration);
        replacement_target.stop();
        replacement.shutdown();
        assert!(!replacement_registration.is_live());
    }
}
