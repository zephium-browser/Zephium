use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::{Manager, WebviewWindow};
use zephium_app::{Command, Handle};
use zephium_ipc::{TabSwitcherAction, TabSwitcherState};

#[derive(Default)]
pub(crate) struct Switcher(Mutex<Gesture>);
#[derive(Default)]
struct Gesture {
    generation: u64,
    active: Option<Active>,
}
struct Active {
    id: u64,
    keyboard: bool,
    started: Instant,
    revealed: bool,
    settling: bool,
    commit: bool,
}

#[derive(Debug, PartialEq, Eq)]
enum Decision {
    Wait,
    Reveal,
    Commit,
}
fn keyboard_decision(control: bool, elapsed: Duration, repeated: bool, revealed: bool) -> Decision {
    if !control {
        Decision::Commit
    } else if !revealed && (repeated || elapsed >= Duration::from_millis(120)) {
        Decision::Reveal
    } else {
        Decision::Wait
    }
}

pub(crate) fn control_down() -> bool {
    #[cfg(target_os = "macos")]
    {
        #[link(name = "CoreGraphics", kind = "framework")]
        unsafe extern "C" {
            fn CGEventSourceFlagsState(state: i32) -> u64;
        }
        unsafe { CGEventSourceFlagsState(1) & (1 << 18) != 0 }
    }
    #[cfg(target_os = "windows")]
    {
        unsafe {
            windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState(
                windows::Win32::UI::Input::KeyboardAndMouse::VK_CONTROL.0 as i32,
            ) < 0
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        false
    }
}

pub(crate) fn step(app: &tauri::AppHandle, reverse: bool, keyboard: bool) -> bool {
    let Some(state) = app.try_state::<Switcher>() else {
        return false;
    };
    let Some(shell) = app.try_state::<Handle>() else {
        return false;
    };
    let mut gesture = state.0.lock().unwrap_or_else(|e| e.into_inner());
    if keyboard {
        if let Some(active) = gesture.active.as_mut().filter(|a| !a.settling) {
            let id = active.id;
            let accepted = shell.dispatch(Command::TabSwitcher {
                session: id,
                action: TabSwitcherAction::Step { reverse },
            });
            if !accepted {
                return false;
            }
            if active.keyboard
                && keyboard_decision(
                    control_down(),
                    active.started.elapsed(),
                    true,
                    active.revealed,
                ) == Decision::Commit
            {
                let accepted = shell.dispatch(Command::TabSwitcher {
                    session: id,
                    action: TabSwitcherAction::Commit,
                });
                active.settling = accepted;
                active.commit = accepted;
                return accepted;
            }
            if active.revealed {
                return true;
            }
            active.revealed = shell.dispatch(Command::RevealTabSwitcher { session: id });
            return active.revealed;
        }
    }
    let Some(id) = gesture.generation.checked_add(1) else {
        return false;
    };
    if let Some(previous) = gesture.active.as_ref().filter(|a| !a.settling) {
        if !shell.dispatch(Command::TabSwitcher {
            session: previous.id,
            action: TabSwitcherAction::Cancel,
        }) {
            return false;
        }
    }
    gesture.generation = id;
    gesture.active = Some(Active {
        id,
        keyboard,
        started: Instant::now(),
        revealed: !keyboard,
        settling: false,
        commit: false,
    });
    if !shell.dispatch(Command::BeginTabSwitcher {
        session: id,
        reverse,
        keyboard,
    }) {
        gesture.active = None;
        return false;
    }
    drop(gesture);
    if keyboard {
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            loop {
                let Some(state) = app.try_state::<Switcher>() else {
                    return;
                };
                let Some(shell) = app.try_state::<Handle>() else {
                    return;
                };
                {
                    let mut gesture = state.0.lock().unwrap_or_else(|e| e.into_inner());
                    let Some(active) = gesture
                        .active
                        .as_mut()
                        .filter(|a| a.id == id && !a.settling)
                    else {
                        return;
                    };
                    if super::shutdown_started(&app) {
                        gesture.active = None;
                        return;
                    }
                    let decision = keyboard_decision(
                        control_down(),
                        active.started.elapsed(),
                        false,
                        active.revealed,
                    );
                    if decision == Decision::Commit
                        && shell.dispatch(Command::TabSwitcher {
                            session: id,
                            action: TabSwitcherAction::Commit,
                        })
                    {
                        active.settling = true;
                        active.commit = true;
                        return;
                    }
                    if decision == Decision::Reveal {
                        active.revealed =
                            shell.dispatch(Command::RevealTabSwitcher { session: id });
                    }
                }
                tokio::time::sleep(Duration::from_millis(8)).await;
            }
        });
    }
    true
}

pub(crate) fn close_selected(app: &tauri::AppHandle) -> bool {
    let Some(state) = app.try_state::<Switcher>() else {
        return false;
    };
    let id = state
        .0
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .active
        .as_ref()
        .filter(|a| a.revealed && !a.settling)
        .map(|a| a.id);
    id.is_some_and(|id| action(app, id, TabSwitcherAction::Close))
}

pub(crate) fn cancel(app: &tauri::AppHandle) {
    let Some(state) = app.try_state::<Switcher>() else {
        return;
    };
    let id = state
        .0
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .active
        .as_ref()
        .map(|a| a.id);
    if let Some(id) = id {
        action(app, id, TabSwitcherAction::Cancel);
    }
}

pub(crate) fn action(app: &tauri::AppHandle, id: u64, action: TabSwitcherAction) -> bool {
    let Some(state) = app.try_state::<Switcher>() else {
        return false;
    };
    let mut gesture = state.0.lock().unwrap_or_else(|e| e.into_inner());
    let Some(active) = gesture.active.as_mut().filter(|a| a.id == id) else {
        return false;
    };
    if active.settling {
        return matches!(action, TabSwitcherAction::Commit) && active.commit
            || matches!(action, TabSwitcherAction::Cancel) && !active.commit;
    }
    let accepted = app.try_state::<Handle>().is_some_and(|shell| {
        shell.dispatch(Command::TabSwitcher {
            session: id,
            action: action.clone(),
        })
    });
    if accepted
        && matches!(
            action,
            TabSwitcherAction::Commit | TabSwitcherAction::Cancel
        )
    {
        active.settling = true;
        active.commit = action == TabSwitcherAction::Commit;
    }
    accepted
}

pub(crate) fn may_present(app: &tauri::AppHandle, snapshot: &TabSwitcherState) -> bool {
    let Ok(id) = snapshot.session_id.parse() else {
        return false;
    };
    let Some(state) = app.try_state::<Switcher>() else {
        return false;
    };
    let live = state
        .0
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .active
        .as_ref()
        .is_some_and(|a| a.id == id && !a.settling);
    if !live {
        return false;
    }
    if snapshot.keyboard && !control_down() {
        action(app, id, TabSwitcherAction::Commit);
        return false;
    }
    true
}

pub(crate) fn project(app: &tauri::AppHandle, session: u64, snapshot: Option<TabSwitcherState>) {
    let Some(state) = app.try_state::<Switcher>() else {
        return;
    };
    let mut gesture = state.0.lock().unwrap_or_else(|e| e.into_inner());
    let Some(active) = gesture.active.as_ref().filter(|a| a.id == session) else {
        drop(gesture);
        if snapshot.is_none() {
            if let Some(panel) = app.try_state::<super::overlay::Overlay>() {
                panel.finish_switcher(session, false);
            }
        }
        return;
    };
    if snapshot.is_some() && active.settling {
        return;
    }
    let commit = active.commit;
    if snapshot.is_none() {
        gesture.active = None;
    }
    drop(gesture);
    if let Some(snapshot) = snapshot {
        if may_present(app, &snapshot) {
            super::overlay::request(app, move |panel| panel.switcher(snapshot));
        }
    } else if let Some(panel) = app.try_state::<super::overlay::Overlay>() {
        panel.finish_switcher(session, commit);
    }
}

#[tauri::command]
#[specta::specta]
pub(crate) fn tab_switcher_open(caller: WebviewWindow, app: tauri::AppHandle) -> bool {
    super::authorize(&caller, super::CallerPolicy::Main, "tab_switcher_open")
        && !super::shutdown_started(&app)
        && step(&app, false, false)
}
#[tauri::command]
#[specta::specta]
pub(crate) fn tab_switcher_action(
    caller: WebviewWindow,
    app: tauri::AppHandle,
    session_id: String,
    intent: TabSwitcherAction,
) -> bool {
    if !super::authorize(&caller, super::CallerPolicy::Panel, "tab_switcher_action")
        || session_id.len() > 20
    {
        return false;
    }
    session_id
        .parse::<u64>()
        .ok()
        .filter(|id| id.to_string() == session_id)
        .is_some_and(|id| action(&app, id, intent))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn release_wins_at_every_reveal_boundary_and_mouse_has_no_gesture_task() {
        for millis in [0, 119, 120, 121] {
            for repeated in [false, true] {
                assert_eq!(
                    keyboard_decision(false, Duration::from_millis(millis), repeated, false),
                    Decision::Commit
                );
            }
        }
        assert_eq!(
            keyboard_decision(true, Duration::from_millis(119), false, false),
            Decision::Wait
        );
        assert_eq!(
            keyboard_decision(true, Duration::from_millis(120), false, false),
            Decision::Reveal
        );
        assert_eq!(
            keyboard_decision(true, Duration::from_millis(1), true, false),
            Decision::Reveal
        );
        assert_eq!(
            keyboard_decision(true, Duration::from_secs(1), false, true),
            Decision::Wait
        );
        assert!(Gesture::default().active.is_none());
    }
}
