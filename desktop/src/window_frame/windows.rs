use tauri::WebviewWindow;
use windows::Win32::UI::WindowsAndMessaging::{
    GetWindowPlacement, GetWindowRect, IsZoomed, SetWindowPos, SWP_NOACTIVATE, SWP_NOZORDER,
    WINDOWPLACEMENT,
};

use super::{Monitor, Placement, WindowFrame, WindowMonitor, WindowRect};

fn monitor(monitor: &tauri::Monitor) -> Monitor {
    let scale = monitor.scale_factor();
    let work = monitor.work_area();
    Monitor {
        identity: WindowMonitor {
            name: monitor.name().cloned(),
            bounds: WindowRect {
                x: f64::from(monitor.position().x) / scale,
                y: f64::from(monitor.position().y) / scale,
                width: f64::from(monitor.size().width) / scale,
                height: f64::from(monitor.size().height) / scale,
            },
        },
        work: WindowRect {
            x: f64::from(work.position.x) / scale,
            y: f64::from(work.position.y) / scale,
            width: f64::from(work.size.width) / scale,
            height: f64::from(work.size.height) / scale,
        },
    }
}

pub(super) fn monitors(window: &WebviewWindow) -> Result<(Vec<Monitor>, usize, usize), String> {
    let error = |error: tauri::Error| error.to_string();
    let primary = window
        .primary_monitor()
        .map_err(error)?
        .map(|m| monitor(&m).identity);
    let current = window
        .current_monitor()
        .map_err(error)?
        .map(|m| monitor(&m).identity);
    let monitors: Vec<_> = window
        .available_monitors()
        .map_err(error)?
        .iter()
        .map(monitor)
        .collect();
    let primary = monitors
        .iter()
        .position(|m| Some(&m.identity) == primary.as_ref())
        .unwrap_or(0);
    let current = monitors
        .iter()
        .position(|m| Some(&m.identity) == current.as_ref())
        .unwrap_or(primary);
    Ok((monitors, primary, current))
}

pub(super) fn apply(
    window: &WebviewWindow,
    plan: &Placement,
    target: &Monitor,
) -> Result<(), String> {
    let monitor = window
        .available_monitors()
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|m| monitor(m).identity == target.identity)
        .ok_or("window monitor disconnected during placement")?;
    let scale = monitor.scale_factor();
    let rect = plan.bounds;
    // Set the outer frame, including any nonclient border, without showing or
    // activating it. Maximization is deferred to the trusted first reveal.
    // SAFETY: acquire this Tauri window's HWND for this synchronous call only;
    // never retain or dereference it. SetWindowPos permits a non-owner thread
    // and validates the OS handle if destruction races. Coordinates and flags
    // are passed by value; there is no output buffer.
    unsafe {
        SetWindowPos(
            window.hwnd().map_err(|e| e.to_string())?,
            None,
            (rect.x * scale).round() as i32,
            (rect.y * scale).round() as i32,
            (rect.width * scale).round() as i32,
            (rect.height * scale).round() as i32,
            SWP_NOACTIVATE | SWP_NOZORDER,
        )
    }
    .map_err(|e| e.to_string())
}

pub(super) fn capture(window: &WebviewWindow) -> Option<WindowFrame> {
    if window.is_minimized().ok()? {
        return None;
    }
    let display = window.current_monitor().ok()??;
    let monitor = monitor(&display);
    let scale = display.scale_factor();
    let hwnd = window.hwnd().ok()?;
    let fullscreen = window.is_fullscreen().ok()?;
    let mut bounds = windows::Win32::Foundation::RECT::default();
    // Native state is queried directly: Tao's cached maximized flag can lag
    // WM_SIZE delivery during maximize/restore.
    // SAFETY: HWND was just acquired from Tauri and is used only during this
    // capture. IsZoomed accepts an opaque OS handle from any thread, including
    // the shutdown worker; it retains no pointer and has no output buffer.
    let maximized = unsafe { IsZoomed(hwnd).as_bool() };
    // SAFETY: use the same transient Tauri HWND; GetWindowRect may run on the
    // event thread or the shutdown worker. Its output buffer is an initialized,
    // exclusively borrowed RECT, live for the synchronous call and not retained.
    unsafe { GetWindowRect(hwnd, &mut bounds) }.ok()?;
    if maximized && !fullscreen {
        let mut placement = WINDOWPLACEMENT {
            length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
            ..Default::default()
        };
        // SAFETY: use the same transient Tauri HWND on the event or shutdown
        // thread. The output buffer is an initialized, exclusively borrowed
        // WINDOWPLACEMENT with length set to its exact size; it remains live
        // throughout the synchronous call, which does not retain the pointer.
        unsafe { GetWindowPlacement(hwnd, &mut placement) }.ok()?;
        // WINDOWPLACEMENT uses workspace coordinates. Convert to screen
        // coordinates when a taskbar occupies the left or top display edge.
        bounds = placement.rcNormalPosition;
        let work = display.work_area();
        let dx = work.position.x - display.position().x;
        let dy = work.position.y - display.position().y;
        bounds.left += dx;
        bounds.right += dx;
        bounds.top += dy;
        bounds.bottom += dy;
    }
    Some(WindowFrame {
        bounds: if fullscreen {
            monitor.work
        } else {
            WindowRect {
                x: f64::from(bounds.left) / scale,
                y: f64::from(bounds.top) / scale,
                width: (f64::from(bounds.right) - f64::from(bounds.left)) / scale,
                height: (f64::from(bounds.bottom) - f64::from(bounds.top)) / scale,
            }
        },
        monitor: monitor.identity,
        maximized: maximized || fullscreen,
    })
}
