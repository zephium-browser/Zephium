//! Main-window placement and debounced persistence. Auxiliary windows never
//! install this owner; private Browse sessions share the same native frame.

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos as native;
#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
use windows as native;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{Manager, WebviewWindow};
use zephium_store::{SqliteStore, WindowFrame, WindowMonitor, WindowRect};

const DEBOUNCE: Duration = Duration::from_millis(300);

#[derive(Clone, Debug)]
struct Monitor {
    identity: WindowMonitor,
    work: WindowRect,
}

#[derive(Clone, Debug, PartialEq)]
struct Placement {
    monitor: usize,
    bounds: WindowRect,
    maximized: bool,
}

fn placement(
    saved: Option<&WindowFrame>,
    monitors: &[Monitor],
    primary: usize,
    current: usize,
    minimum: (f64, f64),
    after_onboarding: bool,
) -> Option<Placement> {
    let fallback = if saved.is_none() || after_onboarding {
        current
    } else {
        primary
    };
    let fallback = monitors.get(fallback).map(|monitor| Placement {
        monitor: fallback,
        bounds: monitor.work,
        maximized: true,
    });
    let Some(saved) = saved else { return fallback };
    let max_width = monitors
        .iter()
        .map(|m| m.identity.bounds.width)
        .fold(0.0, f64::max);
    let max_height = monitors
        .iter()
        .map(|m| m.identity.bounds.height)
        .fold(0.0, f64::max);
    if !saved.is_valid()
        || saved.bounds.width > max_width * 4.0
        || saved.bounds.height > max_height * 4.0
        || saved.monitor.bounds.width > max_width * 4.0
        || saved.monitor.bounds.height > max_height * 4.0
    {
        return fallback;
    }
    let Some((index, monitor)) = monitors
        .iter()
        .enumerate()
        .find(|(_, m)| m.identity == saved.monitor)
    else {
        return fallback;
    };
    let frame = saved.bounds;
    let work = monitor.work;
    let overlap_width =
        ((frame.x + frame.width).min(work.x + work.width) - frame.x.max(work.x)).max(0.0);
    let overlap_height =
        ((frame.y + frame.height).min(work.y + work.height) - frame.y.max(work.y)).max(0.0);
    if overlap_width * overlap_height * 3.0 < frame.width * frame.height {
        return fallback;
    }
    let width = frame.width.max(minimum.0).min(work.width);
    let height = frame.height.max(minimum.1).min(work.height);
    Some(Placement {
        monitor: index,
        bounds: WindowRect {
            x: frame
                .x
                .clamp(work.x, (work.x + (work.width - width)).max(work.x)),
            y: frame
                .y
                .clamp(work.y, (work.y + (work.height - height)).max(work.y)),
            width,
            height,
        },
        maximized: saved.maximized,
    })
}

struct Pending {
    frame: Option<WindowFrame>,
    revision: u64,
    recording: bool,
    due: Instant,
    running: bool,
    closed: bool,
}

impl Pending {
    fn new() -> Self {
        Self {
            frame: None,
            revision: 0,
            recording: false,
            due: Instant::now(),
            running: false,
            closed: false,
        }
    }

    fn start(
        &mut self,
        placement: Result<WindowFrame, String>,
        capture_default: impl FnOnce() -> Option<WindowFrame>,
    ) {
        self.frame = match placement {
            Ok(frame) => Some(frame),
            Err(error) => {
                crate::write_diagnostic(format_args!(
                    "window: placement unavailable; keeping configured frame: {error}"
                ));
                capture_default()
            }
        }
        .filter(WindowFrame::is_valid);
        self.recording = true;
    }

    fn can_record(&self) -> bool {
        self.recording && !self.closed
    }

    fn finish(&mut self, captured: Option<WindowFrame>, revision: u64) {
        // A move/resize delivered during capture has already remembered a
        // newer frame. Keep that frame for the final persistence barrier.
        if self.revision == revision {
            if let Some(frame) = captured {
                remember(self, frame);
            }
        }
        self.closed = true;
    }
}

struct FrameOwner {
    pending: Arc<Mutex<Pending>>,
    store: Arc<SqliteStore>,
    minimum: (f64, f64),
    #[cfg(target_os = "windows")]
    maximize_on_reveal: std::sync::atomic::AtomicBool,
}

pub(crate) fn install(
    window: &WebviewWindow,
    store: Arc<SqliteStore>,
    minimum: (f64, f64),
    onboarding: bool,
) {
    if window.label() != crate::MAIN_LABEL {
        crate::write_diagnostic(format_args!(
            "window: frame persistence is restricted to the main window"
        ));
        return;
    }
    window.app_handle().manage(FrameOwner {
        pending: Arc::new(Mutex::new(Pending::new())),
        store,
        minimum,
        #[cfg(target_os = "windows")]
        maximize_on_reveal: std::sync::atomic::AtomicBool::new(false),
    });
    let observed = window.clone();
    window.on_window_event(move |event| match event {
        tauri::WindowEvent::Moved(_)
        | tauri::WindowEvent::Resized(_)
        | tauri::WindowEvent::ScaleFactorChanged { .. } => changed(&observed),
        _ => {}
    });
    if !onboarding {
        activate(window, false);
    }
}

fn restore(
    window: &WebviewWindow,
    owner: &FrameOwner,
    after_onboarding: bool,
) -> Result<WindowFrame, String> {
    let (monitors, primary, current) = native::monitors(window)?;
    let saved = owner.store.main_window_frame();
    // Malformed JSON is an invalid saved frame, not a first run.
    use zephium_core::ports::store::Store;
    let invalid_saved =
        saved.is_none() && owner.store.app_setting("window.main.frame.v1").is_some();
    let plan = placement(
        saved.as_ref(),
        &monitors,
        primary,
        if invalid_saved && !after_onboarding {
            primary
        } else {
            current
        },
        owner.minimum,
        after_onboarding,
    )
    .ok_or("main window has no usable monitor")?;
    native::apply(window, &plan, &monitors[plan.monitor])?;
    Ok(WindowFrame {
        bounds: plan.bounds,
        monitor: monitors[plan.monitor].identity.clone(),
        maximized: plan.maximized,
    })
}

pub(crate) fn activate(window: &WebviewWindow, after_onboarding: bool) {
    let Some(owner) = window.try_state::<FrameOwner>() else {
        return;
    };
    {
        let pending = owner.pending.lock().unwrap_or_else(|e| e.into_inner());
        if pending.recording || pending.closed {
            return;
        }
    }
    // Applying a native frame can synchronously deliver move/resize events.
    // Keep recording disabled and the lock released until placement settles.
    let restored = restore(window, &owner, after_onboarding);
    #[cfg(target_os = "windows")]
    owner.maximize_on_reveal.store(
        restored.as_ref().is_ok_and(|frame| frame.maximized),
        std::sync::atomic::Ordering::Release,
    );
    let default = restored.is_err().then(|| native::capture(window)).flatten();
    let mut pending = owner.pending.lock().unwrap_or_else(|e| e.into_inner());
    pending.start(restored, || default);
}

fn remember(pending: &mut Pending, mut frame: WindowFrame) {
    if !pending.can_record() {
        return;
    }
    // Keep the normal frame through zoom, maximize, minimize and fullscreen.
    if let Some(previous) = pending
        .frame
        .as_ref()
        .filter(|previous| frame.maximized && frame.monitor == previous.monitor)
    {
        frame.bounds = previous.bounds;
    }
    if frame.is_valid() {
        pending.frame = Some(frame);
        pending.revision = pending.revision.wrapping_add(1);
    }
}

fn changed(window: &WebviewWindow) {
    let Some(owner) = window.try_state::<FrameOwner>() else {
        return;
    };
    if !owner
        .pending
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .can_record()
    {
        return;
    }
    let captured = native::capture(window);
    let mut pending = owner.pending.lock().unwrap_or_else(|e| e.into_inner());
    if !pending.can_record() {
        return;
    }
    if let Some(frame) = captured {
        remember(&mut pending, frame);
    }
    pending.due = Instant::now() + DEBOUNCE;
    if pending.running {
        return;
    }
    pending.running = true;
    let state = owner.pending.clone();
    let store = owner.store.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(DEBOUNCE).await;
            let mut pending = state.lock().unwrap_or_else(|e| e.into_inner());
            if !pending.can_record() {
                return;
            }
            if Instant::now() < pending.due {
                continue;
            }
            if pending
                .frame
                .as_ref()
                .is_some_and(|frame| !store.save_main_window_frame(frame))
            {
                crate::write_diagnostic(format_args!("window: frame persistence admission failed"));
            }
            pending.running = false;
            return;
        }
    });
}

fn save_now(window: &WebviewWindow) {
    let Some(owner) = window.app_handle().try_state::<FrameOwner>() else {
        return;
    };
    let (revision, can_record) = {
        let pending = owner.pending.lock().unwrap_or_else(|e| e.into_inner());
        (pending.revision, pending.can_record())
    };
    // Tauri getters may wait for the main thread; never hold Pending here.
    let captured = if can_record {
        native::capture(window)
    } else {
        None
    };
    let mut pending = owner.pending.lock().unwrap_or_else(|e| e.into_inner());
    pending.finish(captured, revision);
    if pending
        .frame
        .as_ref()
        .is_some_and(|frame| !owner.store.save_main_window_frame(frame))
    {
        crate::write_diagnostic(format_args!(
            "window: final frame persistence admission failed"
        ));
    }
}

pub(crate) fn closing(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window(crate::MAIN_LABEL) {
        save_now(&window);
    } else if let Some(owner) = app.try_state::<FrameOwner>() {
        // Programmatic destruction can precede the shutdown callback. The
        // latest captured frame must still join the store's final barrier.
        let mut pending = owner.pending.lock().unwrap_or_else(|e| e.into_inner());
        if pending.can_record()
            && pending
                .frame
                .as_ref()
                .is_some_and(|frame| !owner.store.save_main_window_frame(frame))
        {
            crate::write_diagnostic(format_args!(
                "window: final frame persistence admission failed"
            ));
        }
        pending.closed = true;
    }
}

#[cfg(target_os = "windows")]
pub(crate) fn show(window: &WebviewWindow) -> Result<(), String> {
    if window.try_state::<FrameOwner>().is_some_and(|owner| {
        owner
            .maximize_on_reveal
            .swap(false, std::sync::atomic::Ordering::AcqRel)
    }) {
        // Tao's Windows maximize calls ShowWindow(SW_MAXIMIZE). Do this only
        // at the trusted first reveal: calling it during setup exposes chrome.
        if let Err(error) = window.maximize() {
            crate::write_diagnostic(format_args!(
                "window: maximization unavailable; keeping normal frame: {error}"
            ));
        }
    }
    window.show().map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f64, y: f64, width: f64, height: f64) -> WindowRect {
        WindowRect {
            x,
            y,
            width,
            height,
        }
    }

    fn monitors() -> Vec<Monitor> {
        vec![
            Monitor {
                identity: WindowMonitor {
                    name: Some("Primary".into()),
                    bounds: rect(0.0, 0.0, 1800.0, 1100.0),
                },
                work: rect(0.0, 40.0, 1800.0, 1000.0),
            },
            Monitor {
                identity: WindowMonitor {
                    name: Some("External".into()),
                    bounds: rect(-1600.0, 0.0, 1600.0, 1000.0),
                },
                work: rect(-1600.0, 30.0, 1600.0, 970.0),
            },
        ]
    }

    #[test]
    fn main_window_frame_placement_failure_keeps_default_and_recording() {
        let default = WindowFrame {
            bounds: rect(200.0, 100.0, 1200.0, 800.0),
            monitor: monitors()[0].identity.clone(),
            maximized: false,
        };
        for error in [
            "monitor enumeration failed",
            "main window has no usable monitor",
            "native placement failed",
        ] {
            for captured in [Some(default.clone()), None] {
                let mut pending = Pending::new();
                pending.start(Err(error.into()), || captured.clone());
                assert!(pending.can_record(), "{error}");
                assert_eq!(pending.frame, captured, "{error}");

                // Even if no monitor was available at startup, a later native
                // move/resize can supply the first persistable frame.
                let moved = WindowFrame {
                    bounds: rect(300.0, 200.0, 1200.0, 800.0),
                    ..default.clone()
                };
                remember(&mut pending, moved.clone());
                assert_eq!(pending.frame, Some(moved), "{error}");
            }
        }
    }

    #[test]
    fn main_window_frame_onboarding_ignores_frames_until_browser_handover() {
        let browser = WindowFrame {
            bounds: rect(100.0, 100.0, 1000.0, 700.0),
            monitor: monitors()[0].identity.clone(),
            maximized: false,
        };
        let onboarding = WindowFrame {
            bounds: rect(200.0, 100.0, 1200.0, 800.0),
            ..browser.clone()
        };
        let mut pending = Pending::new();
        assert!(!pending.can_record(), "onboarding must not save on close");
        remember(&mut pending, onboarding.clone());
        assert_eq!(pending.frame, None);

        pending.start(Ok(browser.clone()), || {
            panic!("successful placement must not capture onboarding's frame")
        });
        assert!(pending.can_record());
        assert_eq!(pending.frame, Some(browser.clone()));
        let moved = WindowFrame {
            bounds: rect(300.0, 200.0, 1000.0, 700.0),
            ..browser
        };
        remember(&mut pending, moved.clone());
        assert_eq!(pending.frame, Some(moved.clone()));
        pending.closed = true;
        assert!(!pending.can_record());
        remember(&mut pending, onboarding);
        assert_eq!(pending.frame, Some(moved));
    }

    #[test]
    fn main_window_frame_onboarding_fallback_fills_current_work_area() {
        let monitors = monitors();
        let saved = WindowFrame {
            bounds: rect(100.0, 100.0, 1000.0, 700.0),
            monitor: monitors[0].identity.clone(),
            maximized: false,
        };
        let restored = placement(Some(&saved), &monitors, 0, 1, (680.0, 480.0), true).unwrap();
        assert_eq!(restored.bounds, saved.bounds);
        assert_eq!(restored.monitor, 0);
        assert!(!restored.maximized);

        let mut disconnected = saved.clone();
        disconnected.monitor.name = Some("Disconnected".into());
        let malformed = WindowFrame {
            bounds: rect(100.0, 100.0, f64::NAN, 700.0),
            ..saved.clone()
        };
        let offscreen = WindowFrame {
            bounds: rect(9000.0, 100.0, 1000.0, 700.0),
            ..saved
        };
        for frame in [None, Some(disconnected), Some(malformed), Some(offscreen)] {
            assert_eq!(
                placement(frame.as_ref(), &monitors, 0, 1, (680.0, 480.0), true),
                Some(Placement {
                    monitor: 1,
                    bounds: monitors[1].work,
                    maximized: true,
                }),
                "{frame:?}"
            );
        }
    }

    #[test]
    fn main_window_frame_install_is_best_effort_and_owner_optional() {
        let production = include_str!("window_frame.rs")
            .split("#[cfg(test)]")
            .next()
            .unwrap();
        let install = production
            .split("pub(crate) fn install(")
            .nth(1)
            .and_then(|body| body.split("fn restore(").next())
            .unwrap();
        assert!(!install.contains("-> Result"));
        assert!(!install.contains('?'));
        assert!(
            install.find("manage(FrameOwner").unwrap()
                < install.find("activate(window, false)").unwrap()
        );
        assert!(install.contains("if !onboarding"));
        let changed = production
            .split("fn changed(")
            .nth(1)
            .and_then(|body| body.split("fn save_now(").next())
            .unwrap();
        let show = production.split("pub(crate) fn show(").nth(1).unwrap();
        for callback in [changed, show] {
            assert!(callback.contains("try_state::<FrameOwner>()"));
            assert!(!callback.contains("window.state::<FrameOwner>()"));
        }
        assert!(show.contains("if let Err(error) = window.maximize()"));
        assert!(show.contains("window.show().map_err"));
    }

    #[test]
    fn main_window_frame_restore_decisions() {
        let monitors = monitors();
        let saved = WindowFrame {
            bounds: rect(100.0, 100.0, 1000.0, 700.0),
            monitor: monitors[0].identity.clone(),
            maximized: false,
        };
        let fallback = Placement {
            monitor: 0,
            bounds: monitors[0].work,
            maximized: true,
        };
        let cases = [
            ("restore", saved.clone(), Some(saved.bounds)),
            (
                "clamp left and top",
                WindowFrame {
                    bounds: rect(-100.0, 0.0, 1000.0, 700.0),
                    ..saved.clone()
                },
                Some(rect(0.0, 40.0, 1000.0, 700.0)),
            ),
            (
                "clamp right and bottom",
                WindowFrame {
                    bounds: rect(1000.0, 500.0, 1000.0, 700.0),
                    ..saved.clone()
                },
                Some(rect(800.0, 340.0, 1000.0, 700.0)),
            ),
            (
                "shrink to work area",
                WindowFrame {
                    bounds: rect(0.0, 40.0, 2000.0, 1200.0),
                    ..saved.clone()
                },
                Some(monitors[0].work),
            ),
            (
                "exactly one third",
                WindowFrame {
                    bounds: rect(-600.0, 40.0, 900.0, 600.0),
                    ..saved.clone()
                },
                Some(rect(0.0, 40.0, 900.0, 600.0)),
            ),
            (
                "less than one third",
                WindowFrame {
                    bounds: rect(-601.0, 40.0, 900.0, 600.0),
                    ..saved.clone()
                },
                None,
            ),
            (
                "offscreen",
                WindowFrame {
                    bounds: rect(9000.0, 40.0, 900.0, 600.0),
                    ..saved.clone()
                },
                None,
            ),
            (
                "minimum size",
                WindowFrame {
                    bounds: rect(100.0, 100.0, 200.0, 100.0),
                    ..saved.clone()
                },
                Some(rect(100.0, 100.0, 680.0, 480.0)),
            ),
        ];
        for (name, frame, expected) in cases {
            let result = placement(Some(&frame), &monitors, 0, 1, (680.0, 480.0), false);
            let expected = expected.map_or_else(
                || fallback.clone(),
                |bounds| Placement {
                    monitor: 0,
                    bounds,
                    maximized: false,
                },
            );
            assert_eq!(result, Some(expected), "{name}");
        }
        let external = WindowFrame {
            bounds: rect(-1500.0, 100.0, 1000.0, 700.0),
            monitor: monitors[1].identity.clone(),
            maximized: true,
        };
        assert_eq!(
            placement(Some(&external), &monitors, 0, 0, (680.0, 480.0), false),
            Some(Placement {
                monitor: 1,
                bounds: external.bounds,
                maximized: true
            })
        );
        assert_eq!(
            placement(Some(&external), &monitors[..1], 0, 0, (680.0, 480.0), false),
            Some(fallback)
        );
    }

    #[test]
    fn main_window_frame_clamps_fractional_work_area_without_panicking() {
        let mut monitors = monitors();
        monitors[0].work = rect(2.0 / 1.5, 2.0 / 1.5, 1280.0, 720.0);
        let saved = WindowFrame {
            bounds: monitors[0].work,
            monitor: monitors[0].identity.clone(),
            maximized: false,
        };
        assert_eq!(
            placement(Some(&saved), &monitors, 0, 0, (680.0, 480.0), false),
            Some(Placement {
                monitor: 0,
                bounds: monitors[0].work,
                maximized: false,
            })
        );
    }

    #[test]
    fn main_window_frame_captures_before_locking_pending() {
        let production = include_str!("window_frame.rs")
            .split("#[cfg(test)]")
            .next()
            .unwrap();
        assert!(!production.contains("fn update("));
        for (start, end) in [
            ("pub(crate) fn activate(", "fn remember("),
            ("fn changed(", "fn save_now("),
            ("fn save_now(", "pub(crate) fn closing("),
        ] {
            let callback = production
                .split(start)
                .nth(1)
                .and_then(|body| body.split(end).next())
                .unwrap();
            assert!(
                callback.find("native::capture(window)").unwrap()
                    < callback
                        .find("let mut pending = owner.pending.lock()")
                        .unwrap(),
                "{start} must capture without holding Pending across native getters"
            );
        }
    }

    #[test]
    fn main_window_frame_final_capture_keeps_latest_frame() {
        let normal = WindowFrame {
            bounds: rect(100.0, 100.0, 1000.0, 700.0),
            monitor: monitors()[0].identity.clone(),
            maximized: false,
        };
        let moved = WindowFrame {
            bounds: rect(200.0, 150.0, 1100.0, 750.0),
            ..normal.clone()
        };
        for concurrent_move in [false, true] {
            for captured in [Some(moved.clone()), None] {
                let mut pending = Pending::new();
                pending.start(Ok(normal.clone()), || panic!("placement succeeded"));
                let revision = pending.revision;
                if concurrent_move {
                    remember(&mut pending, moved.clone());
                }
                // An older in-flight capture must not overwrite the newer event.
                let capture = if concurrent_move {
                    Some(normal.clone())
                } else {
                    captured.clone()
                };
                pending.finish(capture, revision);
                assert!(!pending.can_record());
                assert_eq!(
                    pending.frame,
                    Some(if concurrent_move || captured.is_some() {
                        moved.clone()
                    } else {
                        normal.clone()
                    })
                );
            }
        }
    }

    #[test]
    fn main_window_frame_windows_calls_document_safety() {
        let source = include_str!("window_frame/windows.rs");
        for (offset, _) in source.match_indices("unsafe {") {
            let comment = source[..offset]
                .lines()
                .rev()
                .skip(1)
                .take_while(|line| line.trim_start().starts_with("//"))
                .collect::<Vec<_>>()
                .join("\n");
            for required in ["SAFETY:", "HWND", "thread", "buffer"] {
                assert!(comment.contains(required), "missing {required} at {offset}");
            }
        }
    }

    #[test]
    fn main_window_frame_first_run_fills_current_work_area() {
        let monitors = monitors();
        for current in 0..monitors.len() {
            assert_eq!(
                placement(None, &monitors, 0, current, (680.0, 480.0), false),
                Some(Placement {
                    monitor: current,
                    bounds: monitors[current].work,
                    maximized: true,
                })
            );
        }
        assert_eq!(placement(None, &[], 0, 0, (680.0, 480.0), false), None);
    }

    #[test]
    fn main_window_frame_invalid_records_fall_back_to_primary() {
        let monitors = monitors();
        let frame = WindowFrame {
            bounds: rect(100.0, 100.0, 1000.0, 700.0),
            monitor: monitors[0].identity.clone(),
            maximized: false,
        };
        for bad in [
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
            -1.0,
            0.0,
            7201.0,
        ] {
            let saved = WindowFrame {
                bounds: WindowRect {
                    width: bad,
                    ..frame.bounds
                },
                ..frame.clone()
            };
            assert_eq!(
                placement(Some(&saved), &monitors, 0, 1, (680.0, 480.0), false),
                Some(Placement {
                    monitor: 0,
                    bounds: monitors[0].work,
                    maximized: true
                }),
                "width {bad}"
            );
        }
        let mut renamed = frame.clone();
        renamed.monitor.name = Some("Disconnected".into());
        let mut moved_monitor = frame.clone();
        moved_monitor.monitor.bounds.x = 1800.0;
        let mut absurd_monitor = frame.clone();
        absurd_monitor.monitor.bounds.height = 4401.0;
        let mut negative_height = frame.clone();
        negative_height.bounds.height = -700.0;
        for saved in [renamed, moved_monitor, absurd_monitor, negative_height] {
            assert_eq!(
                placement(Some(&saved), &monitors, 0, 1, (680.0, 480.0), false),
                Some(Placement {
                    monitor: 0,
                    bounds: monitors[0].work,
                    maximized: true
                })
            );
        }
        for bad in [f64::NAN, f64::INFINITY, 10_000_001.0] {
            let saved = WindowFrame {
                bounds: WindowRect {
                    x: bad,
                    ..frame.bounds
                },
                ..frame.clone()
            };
            assert!(
                placement(Some(&saved), &monitors, 0, 1, (680.0, 480.0), false)
                    .unwrap()
                    .maximized
            );
        }
    }

    #[test]
    fn main_window_frame_zoom_preserves_normal_bounds() {
        let monitors = monitors();
        let normal = WindowFrame {
            bounds: rect(100.0, 100.0, 1000.0, 700.0),
            monitor: monitors[0].identity.clone(),
            maximized: false,
        };
        let mut pending = Pending::new();
        pending.start(Ok(normal.clone()), || panic!("placement succeeded"));
        remember(
            &mut pending,
            WindowFrame {
                bounds: monitors[0].work,
                maximized: true,
                ..normal.clone()
            },
        );
        assert_eq!(pending.frame.as_ref().unwrap().bounds, normal.bounds);
        assert!(pending.frame.as_ref().unwrap().maximized);
        remember(&mut pending, normal.clone());
        assert_eq!(pending.frame, Some(normal));
    }
}
