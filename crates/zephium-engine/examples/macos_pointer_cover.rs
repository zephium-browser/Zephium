//! Native check of the pointer cover: a canvas web view beneath a native pane
//! parks its tracking areas while the cover holds it, still takes hits, and
//! gets the areas back when the pane hides, the last cover goes, or the
//! window stops being key. Moves the system pointer into the window, so it
//! needs a desktop session; the pointer is put back at the end. Synthetic
//! enter and exit events do not reach the window, so the cover is re-read
//! with `sync` where AppKit would have sent them.
#[cfg(target_os = "macos")]
#[path = "../src/platform/macos/pointer_cover.rs"]
// The stage's own parking, for a chrome stacked over its pages, is not what
// this check covers.
#[allow(dead_code)]
mod pointer_cover;

#[cfg(target_os = "macos")]
fn main() {
    use objc2::rc::Retained;
    use objc2_app_kit::{
        NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSEvent, NSEventMask,
        NSScreen, NSWindow, NSWindowStyleMask,
    };
    use objc2_core_foundation::CGPoint;
    use objc2_core_graphics::CGWarpMouseCursorPosition;
    use objc2_foundation::{
        MainThreadMarker, NSDate, NSDefaultRunLoopMode, NSPoint, NSRect, NSSize,
    };
    use raw_window_handle::{
        AppKitWindowHandle, HandleError, HasWindowHandle, RawWindowHandle, WindowHandle,
    };
    use std::ptr::NonNull;
    use wry::{
        dpi::{LogicalPosition, LogicalSize},
        Rect, WebViewBuilder, WebViewExtMacOS,
    };

    struct Host(NonNull<std::ffi::c_void>);
    impl HasWindowHandle for Host {
        fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
            let handle = AppKitWindowHandle::new(self.0);
            // SAFETY: the content view outlives every use of this handle.
            Ok(unsafe { WindowHandle::borrow_raw(RawWindowHandle::AppKit(handle)) })
        }
    }

    let mtm = MainThreadMarker::new().expect("main thread");
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    app.finishLaunching();
    let pump = |secs: f64| {
        let until = NSDate::dateWithTimeIntervalSinceNow(secs);
        // SAFETY: plain event pump on the main thread.
        while let Some(event) = unsafe {
            app.nextEventMatchingMask_untilDate_inMode_dequeue(
                NSEventMask::Any,
                Some(&until),
                NSDefaultRunLoopMode,
                true,
            )
        } {
            app.sendEvent(&event);
            if until.timeIntervalSinceNow() <= 0.0 {
                break;
            }
        }
    };
    // SAFETY: a titled window created and used on the main thread.
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            mtm.alloc(),
            NSRect::new(NSPoint::new(200.0, 300.0), NSSize::new(600.0, 400.0)),
            NSWindowStyleMask::Titled | NSWindowStyleMask::Closable,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    let content = window.contentView().expect("content view");
    let host = Host(NonNull::new(Retained::as_ptr(&content) as *mut _).expect("view"));
    let build = |html: &str| {
        WebViewBuilder::new()
            .with_html(html)
            .with_bounds(Rect {
                position: LogicalPosition::new(0.0, 0.0).into(),
                size: LogicalSize::new(600.0, 400.0).into(),
            })
            .build_as_child(&host)
            .expect("webview")
    };
    let canvas = build("<body style='cursor:grab;margin:0'>canvas</body>");
    let pane = build("<body style='cursor:crosshair;margin:0'>pane</body>");
    #[allow(deprecated)]
    app.activateIgnoringOtherApps(true);
    window.makeKeyAndOrderFront(None);
    pump(2.0);

    let screen_height = NSScreen::screens(mtm)
        .iter()
        .next()
        .map_or(0.0, |screen| screen.frame().size.height);
    let origin = NSEvent::mouseLocation();
    let centre = window.convertPointToScreen(NSPoint::new(300.0, 200.0));
    CGWarpMouseCursorPosition(CGPoint {
        x: centre.x,
        y: screen_height - centre.y,
    });
    pump(0.3);

    let canvas_view = canvas.webview();
    let pane_view = pane.webview();
    let areas = || canvas_view.trackingAreas().len();
    let live = areas();

    let mut failures = Vec::new();
    let mut check = |name: &str, ok: bool| {
        println!("{} {name}", if ok { "ok  " } else { "FAIL" });
        if !ok {
            failures.push(name.to_owned());
        }
    };
    check("the canvas has tracking areas", live > 0);

    let cover = pointer_cover::PointerCover::attach(&pane_view).expect("cover");
    check(
        "a cover under the pointer parks the canvas areas",
        areas() == 0,
    );
    check(
        "the parked canvas still takes hits",
        canvas_view.hitTest(NSPoint::new(100.0, 100.0)).is_some(),
    );

    pane_view.setHidden(true);
    cover.sync();
    check("hiding the pane releases the canvas", areas() == live);
    pane_view.setHidden(false);
    cover.sync();
    check("showing it under a still pointer parks again", areas() == 0);

    // A window that stops being key stops covering.
    window.orderOut(None);
    pump(0.3);
    check("an unkeyed window releases the canvas", areas() == live);
    window.makeKeyAndOrderFront(None);
    pump(0.5);
    check("a keyed window parks it again", areas() == 0);

    // Two covers over one canvas: it returns only when both have let go.
    let second = pointer_cover::PointerCover::attach(&pane_view).expect("second cover");
    drop(cover);
    check("one of two covers leaving keeps it parked", areas() == 0);
    drop(second);
    check("the last cover leaving releases it", areas() == live);

    CGWarpMouseCursorPosition(CGPoint {
        x: origin.x,
        y: screen_height - origin.y,
    });
    window.close();
    if !failures.is_empty() {
        eprintln!("failed: {failures:?}");
        std::process::exit(1);
    }
}

#[cfg(not(target_os = "macos"))]
fn main() {}
