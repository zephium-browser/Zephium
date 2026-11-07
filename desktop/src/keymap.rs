//! The person's keymap: registry defaults plus their overrides, held once in
//! memory and pushed to every surface that matches keys whenever it changes.
//! macOS matches through the menu bar and a key monitor, Windows through each
//! page's engine key table, Linux through the GTK window handler, and
//! privileged chrome through the projection it reads back.

use super::*;
use std::collections::{BTreeMap, HashMap};
use std::sync::RwLock;
use zephium_core::accelerator::{Accelerator, AcceleratorError, Platform};
use zephium_core::commands::{self, Group, KeymapError, ResolvedCommand};
use zephium_core::ports::engine::Shortcut;

const SETTING: &str = "keymap";
const FEATURES: [&str; 5] = ["ai", "work", "notes", "tasks", "time"];
const MAX_ACCELERATOR_BYTES: usize = 64;

type EngineTable = Box<dyn Fn(Vec<Shortcut>) + Send + Sync>;

#[derive(Default)]
pub(crate) struct Keymap {
    overrides: RwLock<HashMap<String, String>>,
    work_pane_shown: AtomicBool,
    /// Feature switches as stored, in `FEATURES` order; a feature off takes
    /// its keys and menu items away.
    off: [AtomicBool; FEATURES.len()],
    /// Keyboard settings is recording a shortcut: window-level handlers stand
    /// aside so the keys reach the recorder instead of running commands.
    recording: AtomicBool,
    engine: OnceLock<EngineTable>,
    /// Read by the GTK key handlers of the main and launcher windows.
    #[cfg(target_os = "linux")]
    window_table: Arc<RwLock<Vec<Shortcut>>>,
    #[cfg(target_os = "linux")]
    linux_launcher: OnceLock<Option<linux_shortcut::LinuxLauncherShortcut>>,
    /// Keyboard-only commands, read by the macOS key monitor. Everything else
    /// on macOS is a menu key equivalent.
    #[cfg(target_os = "macos")]
    key_table: Arc<RwLock<Vec<(Accelerator, &'static str)>>>,
}

impl Keymap {
    pub(crate) fn load() -> Self {
        let keymap = Self::default();
        if let Some(store) = APP_STORE.get() {
            for (flag, feature) in keymap.off.iter().zip(FEATURES) {
                let key = format!("{feature}.enabled");
                flag.store(
                    store.app_setting(&key).as_deref() == Some("false"),
                    Ordering::Release,
                );
            }
        }
        let stored = APP_STORE
            .get()
            .and_then(|store| store.app_setting(SETTING))
            .and_then(|text| serde_json::from_str::<HashMap<String, String>>(&text).ok())
            .unwrap_or_default();
        *write(&keymap.overrides) = stored
            .into_iter()
            .filter(|(id, value)| {
                commands::get(id).is_some_and(|spec| spec.customizable())
                    && value.len() <= MAX_ACCELERATOR_BYTES
            })
            .take(commands::MAX_OVERRIDES)
            .collect();
        keymap.refresh_tables();
        keymap
    }

    pub(crate) fn overrides(&self) -> HashMap<String, String> {
        read(&self.overrides).clone()
    }

    pub(crate) fn resolved(&self) -> Vec<ResolvedCommand> {
        commands::resolve(&read(&self.overrides))
    }

    /// The engine applies its table on its own thread; installing it pushes
    /// the current table immediately.
    pub(crate) fn attach_engine(&self, apply: EngineTable) {
        apply(self.engine_table());
        let _ = self.engine.set(apply);
    }

    /// Commands a page-level key table may fire. The launcher is global and
    /// registered on its own; Work keys exist only while the pane is shown, so
    /// a bare Escape never leaves an ordinary page.
    fn engine_table(&self) -> Vec<Shortcut> {
        let shown = self.work_pane_shown.load(Ordering::Acquire);
        self.resolved()
            .iter()
            .filter(|command| command.group != Group::Global)
            .filter(|command| shown || command.group != Group::Work)
            .filter(|command| self.offers(command.id))
            .filter_map(|command| shortcut(command.id, command.accelerator.as_deref()?))
            .collect()
    }

    #[cfg(target_os = "linux")]
    pub(crate) fn window_table(
        &self,
        launcher: Option<linux_shortcut::LinuxLauncherShortcut>,
    ) -> Arc<RwLock<Vec<Shortcut>>> {
        let _ = self.linux_launcher.set(launcher);
        self.refresh_tables();
        self.window_table.clone()
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn key_table(&self) -> Arc<RwLock<Vec<(Accelerator, &'static str)>>> {
        self.key_table.clone()
    }

    fn refresh_tables(&self) {
        if let Some(apply) = self.engine.get() {
            apply(self.engine_table());
        }
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        let recording = self.recording.load(Ordering::Acquire);
        #[cfg(target_os = "linux")]
        {
            let mut table = if recording {
                Vec::new()
            } else {
                self.engine_table()
            };
            if let Some(Some(launcher)) = self.linux_launcher.get().filter(|_| !recording) {
                table.push(launcher.focused_shortcut());
            }
            *write(&self.window_table) = table;
        }
        #[cfg(target_os = "macos")]
        {
            *write(&self.key_table) = self
                .resolved()
                .iter()
                .filter(|_| !recording)
                .filter(|command| command.group == Group::Keys)
                .filter(|command| self.offers(command.id))
                .filter_map(|command| {
                    let accelerator = command.accelerator.as_deref()?;
                    Some((Accelerator::parse(accelerator, Platform::Mac)?, command.id))
                })
                .collect();
        }
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn work_pane_shown(&self) -> bool {
        self.work_pane_shown.load(Ordering::Acquire)
    }

    fn feature_on(&self, feature: &str) -> bool {
        FEATURES
            .iter()
            .position(|known| *known == feature)
            .is_none_or(|index| !self.off[index].load(Ordering::Acquire))
    }

    pub(crate) fn ai_enabled(&self) -> bool {
        self.feature_on("ai")
    }

    /// Whether a command's feature is on, so it is matched, listed and run.
    pub(crate) fn offers(&self, command: &str) -> bool {
        commands::feature_of(command).is_none_or(|feature| self.feature_on(feature))
    }

    /// Follows a stored feature switch: off, its keys stop matching anywhere.
    /// Returns whether anything changed, so the menus can be rebuilt.
    pub(crate) fn follow_preference(&self, key: &str, value: &str) -> bool {
        let Some(index) = key
            .strip_suffix(".enabled")
            .and_then(|feature| FEATURES.iter().position(|known| *known == feature))
        else {
            return false;
        };
        let changed =
            self.off[index].swap(value == "false", Ordering::AcqRel) != (value == "false");
        if changed {
            self.refresh_tables();
        }
        changed
    }

    fn set_recording(&self, recording: bool) {
        if self.recording.swap(recording, Ordering::AcqRel) != recording {
            self.refresh_tables();
        }
    }

    pub(crate) fn set_work_pane_shown(&self, app: &tauri::AppHandle, shown: bool) {
        if self.work_pane_shown.swap(shown, Ordering::AcqRel) == shown {
            return;
        }
        self.refresh_tables();
        self.rebuild_menu_bar(app);
    }

    /// Off the main thread a menu setter blocks on the main thread, which may
    /// be waiting on this caller; hand the rebuild over instead. It reads the
    /// state when it runs, so out-of-order changes settle on the latest one.
    #[cfg_attr(not(target_os = "macos"), allow(unused_variables))]
    pub(crate) fn rebuild_menu_bar(&self, app: &tauri::AppHandle) {
        #[cfg(target_os = "macos")]
        {
            let handle = app.clone();
            let queued = app.run_on_main_thread(move || {
                let Some(keymap) = handle.try_state::<Keymap>() else {
                    return;
                };
                if install_menu_bar(&handle, &keymap.overrides(), keymap.work_pane_shown()).is_err()
                {
                    write_diagnostic(format_args!("menu: menu bar was not rebuilt"));
                }
            });
            if queued.is_err() {
                write_diagnostic(format_args!("menu: menu bar was not rebuilt"));
            }
        }
    }

    fn update(&self, app: &tauri::AppHandle, change: impl FnOnce(&mut HashMap<String, String>)) {
        // Saved under the lock, so two quick changes are stored in the order
        // they were made.
        let persisted = {
            let mut overrides = write(&self.overrides);
            change(&mut overrides);
            let snapshot: BTreeMap<_, _> = overrides.iter().collect();
            serde_json::to_string(&snapshot).is_ok_and(|text| {
                APP_STORE
                    .get()
                    .is_some_and(|store| store.set_app_setting(SETTING.into(), text))
            })
        };
        if !persisted {
            write_diagnostic(format_args!(
                "keymap: change applied for this session but not saved"
            ));
        }
        self.refresh_tables();
        #[cfg(target_os = "macos")]
        if install_menu_bar(app, &self.overrides(), self.work_pane_shown()).is_err() {
            write_diagnostic(format_args!("keymap: menu bar was not rebuilt"));
        }
        emit_to_privileged(
            app,
            MAIN_LABEL,
            EVENT_KEYMAP_CHANGED,
            &KeymapChanged { version: 1 },
        );
    }
}

fn read<T>(lock: &RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
    lock.read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn write<T>(lock: &RwLock<T>) -> std::sync::RwLockWriteGuard<'_, T> {
    lock.write()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// A page-level key for platforms that match virtual keys. The system key is
/// never part of one; on macOS these tables are unused.
pub(crate) fn shortcut(id: &str, accelerator: &str) -> Option<Shortcut> {
    let parsed = Accelerator::parse(accelerator, Platform::CURRENT)?;
    if parsed.meta {
        return None;
    }
    Some(Shortcut {
        id: id.to_owned(),
        ctrl: parsed.ctrl,
        shift: parsed.shift,
        alt: parsed.alt,
        key: parsed.key.virtual_key(),
    })
}

pub(crate) const EVENT_KEYMAP_CHANGED: &str = "zephium:keymap-changed";

/// The keymap changed; read it again.
#[derive(Clone, Debug, Serialize, Deserialize, specta::Type, tauri_specta::Event)]
#[tauri_specta(event_name = "zephium:keymap-changed")]
pub(crate) struct KeymapChanged {
    pub(crate) version: u32,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, specta::Type, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum KeymapGroup {
    App,
    File,
    Edit,
    View,
    History,
    Bookmarks,
    Window,
    Help,
    Keys,
    Global,
    Work,
}

impl From<Group> for KeymapGroup {
    fn from(group: Group) -> Self {
        match group {
            Group::App => Self::App,
            Group::File => Self::File,
            Group::Edit => Self::Edit,
            Group::View => Self::View,
            Group::History => Self::History,
            Group::Bookmarks => Self::Bookmarks,
            Group::Window => Self::Window,
            Group::Help => Self::Help,
            Group::Keys => Self::Keys,
            Group::Global => Self::Global,
            Group::Work => Self::Work,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, specta::Type)]
pub(crate) struct KeymapEntry {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) group: KeymapGroup,
    /// Canonical text for this platform, or null when unbound.
    pub(crate) accelerator: Option<String>,
    pub(crate) default_accelerator: Option<String>,
    pub(crate) customizable: bool,
    pub(crate) customized: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, specta::Type, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum KeymapOutcome {
    Applied,
    /// Already bound to `command`; nothing changed.
    Conflict {
        command: String,
    },
    Invalid,
    /// Needs Command or Control, or would type text.
    Unbindable,
    /// Owned by the system or by text editing.
    Reserved,
    Fixed,
    Unavailable,
}

#[tauri::command]
#[specta::specta]
pub(crate) fn keymap_entries(caller: WebviewWindow, app: tauri::AppHandle) -> Vec<KeymapEntry> {
    if !authorize(&caller, CallerPolicy::Main, "keymap_entries") {
        return Vec::new();
    }
    let Some(keymap) = app.try_state::<Keymap>() else {
        return Vec::new();
    };
    keymap
        .resolved()
        .into_iter()
        .map(|command| {
            let spec = commands::get(command.id);
            KeymapEntry {
                id: command.id.to_owned(),
                title: command.title.to_owned(),
                group: command.group.into(),
                default_accelerator: spec
                    .and_then(|spec| spec.default_accelerator())
                    .and_then(|text| Accelerator::parse(text, Platform::CURRENT))
                    .map(|parsed| parsed.format(Platform::CURRENT)),
                customizable: spec.is_some_and(|spec| spec.customizable()),
                accelerator: command.accelerator,
                customized: command.customized,
            }
        })
        .collect()
}

/// Binds `id` to `accelerator`, or unbinds it when null. A conflict is
/// reported, never resolved silently: the person decides what loses its key,
/// and `replace` takes it from the other command in the same change.
#[tauri::command]
#[specta::specta]
pub(crate) fn keymap_bind(
    caller: WebviewWindow,
    app: tauri::AppHandle,
    id: String,
    accelerator: Option<String>,
    replace: bool,
) -> KeymapOutcome {
    if !authorize(&caller, CallerPolicy::Main, "keymap_bind")
        || !bounded(&id, MAX_COMMAND_ID_BYTES)
        || accelerator
            .as_deref()
            .is_some_and(|text| !bounded(text, MAX_ACCELERATOR_BYTES))
    {
        return KeymapOutcome::Invalid;
    }
    let Some(keymap) = app.try_state::<Keymap>() else {
        return KeymapOutcome::Unavailable;
    };
    let resolved = keymap.resolved();
    let mut result = commands::validate_binding(&resolved, &id, accelerator.as_deref());
    let mut displaced = None;
    if let Err(KeymapError::Conflict(other)) = result {
        if replace && commands::get(other).is_some_and(|spec| spec.customizable()) {
            let freed: Vec<_> = resolved
                .into_iter()
                .map(|mut command| {
                    if command.id == other {
                        command.accelerator = None;
                    }
                    command
                })
                .collect();
            displaced = Some(other);
            result = commands::validate_binding(&freed, &id, accelerator.as_deref());
        }
    }
    match result {
        Ok(canonical) => {
            let default = commands::get(&id)
                .and_then(|spec| spec.default_accelerator())
                .and_then(|text| Accelerator::parse(text, Platform::CURRENT))
                .map(|parsed| parsed.format(Platform::CURRENT));
            keymap.update(&app, |overrides| {
                if let Some(other) = displaced {
                    overrides.insert(other.to_owned(), String::new());
                }
                // Choosing the default again is a reset, so a later change to
                // the default reaches this person too.
                if canonical == default {
                    overrides.remove(&id);
                } else {
                    overrides.insert(id.clone(), canonical.unwrap_or_default());
                }
            });
            KeymapOutcome::Applied
        }
        Err(KeymapError::Conflict(command)) => KeymapOutcome::Conflict {
            command: command.to_owned(),
        },
        Err(KeymapError::UnknownCommand) => KeymapOutcome::Invalid,
        Err(KeymapError::Fixed) => KeymapOutcome::Fixed,
        Err(KeymapError::Accelerator(AcceleratorError::Invalid)) => KeymapOutcome::Invalid,
        Err(KeymapError::Accelerator(AcceleratorError::Unbindable)) => KeymapOutcome::Unbindable,
        Err(KeymapError::Accelerator(AcceleratorError::Reserved)) => KeymapOutcome::Reserved,
    }
}

/// Starts or ends recording in Keyboard settings. Chrome ends it on every exit
/// path: a recorded key, Escape, losing focus, and leaving the page.
#[tauri::command]
#[specta::specta]
pub(crate) fn keymap_record(caller: WebviewWindow, app: tauri::AppHandle, active: bool) -> bool {
    if !authorize(&caller, CallerPolicy::Main, "keymap_record") {
        return false;
    }
    let Some(keymap) = app.try_state::<Keymap>() else {
        return false;
    };
    keymap.set_recording(active);
    true
}

/// Restores one command's default, or every default when `id` is null.
#[tauri::command]
#[specta::specta]
pub(crate) fn keymap_reset(
    caller: WebviewWindow,
    app: tauri::AppHandle,
    id: Option<String>,
) -> bool {
    if !authorize(&caller, CallerPolicy::Main, "keymap_reset")
        || id
            .as_deref()
            .is_some_and(|id| !bounded(id, MAX_COMMAND_ID_BYTES) || commands::get(id).is_none())
    {
        return false;
    }
    let Some(keymap) = app.try_state::<Keymap>() else {
        return false;
    };
    keymap.update(&app, |overrides| match &id {
        Some(id) => {
            overrides.remove(id);
        }
        None => overrides.clear(),
    });
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_tables_carry_work_keys_only_while_the_pane_is_shown() {
        let keymap = Keymap::default();
        let has_escape = |table: &[Shortcut]| table.iter().any(|s| s.id == "work.pane.close");
        assert!(!has_escape(&keymap.engine_table()));
        keymap.work_pane_shown.store(true, Ordering::Release);
        assert!(has_escape(&keymap.engine_table()));
        assert!(!keymap
            .engine_table()
            .iter()
            .any(|s| s.id == "launcher.toggle"));
    }

    #[test]
    fn each_feature_switch_takes_only_its_own_keys() {
        let keymap = Keymap::default();
        keymap.follow_preference("notes.enabled", "false");
        keymap.follow_preference("ai.enabled", "false");
        assert!(!keymap.offers("tool.notes") && !keymap.offers("note.new"));
        assert!(!keymap.offers("work.dictate"));
        assert!(keymap.offers("tool.tasks") && keymap.offers("tool.time"));
        assert!(keymap.offers("work.pane.close") && keymap.offers("tab.new"));
        assert!(!keymap.engine_table().iter().any(|s| s.id == "tool.notes"));
        assert!(!keymap.follow_preference("ui.theme", "nord"));
    }

    #[test]
    fn work_keys_leave_every_table_while_work_is_off() {
        let keymap = Keymap::default();
        keymap.work_pane_shown.store(true, Ordering::Release);
        assert!(keymap.follow_preference("work.enabled", "false"));
        assert!(!keymap.follow_preference("work.enabled", "false"));
        assert!(keymap.ai_enabled());
        assert!(!keymap.offers("work.pane.close"));
        assert!(!keymap
            .engine_table()
            .iter()
            .any(|s| s.id.starts_with("work.pane.")));
        #[cfg(target_os = "macos")]
        assert!(!read(&keymap.key_table)
            .iter()
            .any(|(_, id)| id.starts_with("work.pane.")));
        keymap.follow_preference("work.enabled", "true");
        assert!(keymap.offers("work.pane.close"));
        assert!(keymap
            .engine_table()
            .iter()
            .any(|s| s.id == "work.pane.close"));
        #[cfg(target_os = "macos")]
        assert!(read(&keymap.key_table)
            .iter()
            .any(|(_, id)| *id == "work.dictate"));
    }

    #[test]
    fn page_keys_use_the_platform_primary_modifier() {
        let reload = shortcut("nav.reload", "CmdOrCtrl+R");
        if cfg!(target_os = "macos") {
            assert_eq!(reload, None);
        } else {
            let reload = reload.expect("reload key");
            assert!(reload.ctrl && !reload.shift && !reload.alt);
            assert_eq!(reload.key, 0x52);
        }
    }

    #[test]
    fn bare_function_keys_and_alt_arrows_match_as_written() {
        let f5 = shortcut("nav.reload.alt", "F5").expect("F5 key");
        assert!(!f5.ctrl && !f5.shift && !f5.alt);
        assert_eq!(f5.key, 0x74);
        let back = shortcut("nav.back.alt", "Alt+Left").expect("Alt+Left key");
        assert!(back.alt && !back.ctrl && !back.shift);
        assert_eq!(back.key, 0x25);
    }
}
