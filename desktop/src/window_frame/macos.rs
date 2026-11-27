use objc2_app_kit::{NSScreen, NSWindow, NSWindowStyleMask};
use objc2_foundation::{MainThreadMarker, NSPoint, NSRect, NSSize};
use tauri::WebviewWindow;

use super::{Monitor, Placement, WindowFrame, WindowMonitor, WindowRect};

fn ns_window(window: &WebviewWindow) -> Result<&NSWindow, String> {
    MainThreadMarker::new().ok_or("window frame accessed off the main thread")?;
    let ptr = window.ns_window().map_err(|error| error.to_string())?;
    // SAFETY: Tauri owns this live NSWindow; borrow it only on the main thread.
    unsafe { (ptr as *const NSWindow).as_ref() }.ok_or("main NSWindow is missing".into())
}

fn rect(frame: NSRect) -> WindowRect {
    WindowRect {
        x: frame.origin.x,
        y: frame.origin.y,
        width: frame.size.width,
        height: frame.size.height,
    }
}

fn monitor(screen: &NSScreen) -> Monitor {
    Monitor {
        identity: WindowMonitor {
            name: Some(screen.localizedName().to_string()),
            bounds: rect(screen.frame()),
        },
        work: rect(screen.visibleFrame()),
    }
}

pub(super) fn monitors(window: &WebviewWindow) -> Result<(Vec<Monitor>, usize, usize), String> {
    let mtm = MainThreadMarker::new().ok_or("monitor enumeration off the main thread")?;
    let current = ns_window(window)?
        .screen()
        .map(|screen| monitor(&screen).identity);
    // AppKit lists the primary display (the menu-bar origin) first.
    let monitors: Vec<_> = NSScreen::screens(mtm)
        .to_vec()
        .iter()
        .map(|s| monitor(s))
        .collect();
    let index = monitors
        .iter()
        .position(|m| Some(&m.identity) == current.as_ref())
        .unwrap_or(0);
    Ok((monitors, 0, index))
}

pub(super) fn apply(
    window: &WebviewWindow,
    plan: &Placement,
    monitor: &Monitor,
) -> Result<(), String> {
    let window = ns_window(window)?;
    let frame = plan.bounds;
    window.setFrame_display(
        NSRect::new(
            NSPoint::new(frame.x, frame.y),
            NSSize::new(frame.width, frame.height),
        ),
        false,
    );
    if !plan.maximized {
        return Ok(());
    }
    // Seed AppKit's normal frame before zooming, so the zoom button can also
    // return to the restored size during this launch.
    if !window.isZoomed() {
        window.zoom(None);
    }
    let frame = monitor.work;
    // A zoomed normal window, never a fullscreen Space. Setting visibleFrame
    // also avoids AppKit choosing a document-dependent standard zoom size.
    window.setFrame_display(
        NSRect::new(
            NSPoint::new(frame.x, frame.y),
            NSSize::new(frame.width, frame.height),
        ),
        false,
    );
    Ok(())
}

pub(super) fn capture(window: &WebviewWindow) -> Option<WindowFrame> {
    let window = ns_window(window).ok()?;
    if window.isMiniaturized() {
        return None;
    }
    let screen = window.screen()?;
    let monitor = monitor(&screen);
    let fullscreen = window.styleMask().contains(NSWindowStyleMask::FullScreen);
    let bounds = rect(window.frame());
    Some(WindowFrame {
        bounds: if fullscreen { monitor.work } else { bounds },
        maximized: fullscreen || window.isZoomed() || bounds == monitor.work,
        monitor: monitor.identity,
    })
}
