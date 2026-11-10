use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::Duration,
};

use gtk::{cairo, gio, glib, prelude::*};
use webkit2gtk::{SnapshotOptions, SnapshotRegion, WebViewExt};
use wry::WebViewExtUnix;

const INPUT_SIDE: i32 = 4096;
const INPUT_PIXELS: i64 = 3840 * 2400;
const OUTPUT_SIDE: i32 = 480;
const OUTPUT_BYTES: usize = 80 * 1024;
type Completion = Box<dyn FnOnce(Option<Vec<u8>>) + Send>;

thread_local! {
    static CAPTURE_BUSY: Rc<Cell<bool>> = Rc::new(Cell::new(false));
}

struct CaptureLease(Rc<Cell<bool>>);

impl Drop for CaptureLease {
    fn drop(&mut self) {
        self.0.set(false);
    }
}

pub(crate) fn capture(view: &wry::WebView, done: Completion) {
    capture_page(&view.webview(), done);
}

fn capture_page(page: &webkit2gtk::WebView, done: Completion) {
    let dimensions = (page.allocated_width(), page.allocated_height());
    let scale = page.scale_factor();
    let physical = dimensions
        .0
        .checked_mul(scale)
        .zip(dimensions.1.checked_mul(scale));
    if !page.is_mapped()
        || !page.is_child_visible()
        || !page.is_sensitive()
        || page.opacity() <= 0.0
        || scale <= 0
        || !physical.is_some_and(input_dimensions)
    {
        done(None);
        return;
    }
    let physical = physical.expect("validated snapshot dimensions");
    let slot = CAPTURE_BUSY.with(Rc::clone);
    if slot.replace(true) {
        done(None);
        return;
    }
    let lease = CaptureLease(slot);
    let pending = Rc::new(RefCell::new(Some(done)));
    let cancellable = gio::Cancellable::new();
    let timed_out = pending.clone();
    let cancellation = cancellable.clone();
    let timeout = glib::timeout_add_local_once(Duration::from_secs(2), move || {
        let done = timed_out.borrow_mut().take();
        cancellation.cancel();
        if let Some(done) = done {
            done(None);
        }
    });
    page.snapshot(
        SnapshotRegion::Visible,
        SnapshotOptions::NONE,
        Some(&cancellable),
        move |result| {
            // A timeout settles the caller, but only WebKit can drain its capture.
            let _lease = lease;
            let Some(done) = pending.borrow_mut().take() else {
                return;
            };
            timeout.remove();
            done(result.ok().and_then(|surface| thumbnail(surface, physical)));
        },
    );
}

fn input_dimensions((width, height): (i32, i32)) -> bool {
    width > 0
        && height > 0
        && width <= INPUT_SIDE
        && height <= INPUT_SIDE
        && i64::from(width) * i64::from(height) <= INPUT_PIXELS
}

fn thumbnail(surface: cairo::Surface, expected: (i32, i32)) -> Option<Vec<u8>> {
    let source = cairo::ImageSurface::try_from(surface).ok()?;
    let physical = (source.width(), source.height());
    if !input_dimensions(physical) || physical != expected {
        return None;
    }
    let fit = (f64::from(OUTPUT_SIDE) / f64::from(physical.0.max(physical.1))).min(1.0);
    let width = (f64::from(physical.0) * fit).floor().max(1.0) as i32;
    let height = (f64::from(physical.1) * fit).floor().max(1.0) as i32;
    let small = cairo::ImageSurface::create(cairo::Format::Rgb24, width, height).ok()?;
    let cr = cairo::Context::new(&small).ok()?;
    cr.set_source_rgb(1.0, 1.0, 1.0);
    cr.paint().ok()?;
    source.set_device_scale(1.0, 1.0);
    cr.scale(fit, fit);
    cr.set_source_surface(&source, 0.0, 0.0).ok()?;
    cr.source().set_filter(cairo::Filter::Good);
    cr.paint().ok()?;
    drop(cr);
    let pixbuf = gtk::gdk::pixbuf_get_from_surface(&small, 0, 0, width, height)?;
    let bytes = pixbuf.save_to_bufferv("jpeg", &[("quality", "80")]).ok()?;
    (!bytes.is_empty() && bytes.len() <= OUTPUT_BYTES).then_some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;
    use webkit2gtk::WebContextExt;

    #[test]
    fn capture_limits_bound_native_snapshot_pixels() {
        assert!(input_dimensions((3840, 2400)));
        for size in [(0, 1), (1, 0), (-1, 1), (4096, 4096), (i32::MAX, 1)] {
            assert!(!input_dimensions(size));
        }
    }

    #[test]
    #[ignore = "requires an isolated display and admitted sandboxed WebKitGTK"]
    fn native_preview_is_bounded_and_hidden_pages_are_refused() {
        super::super::enforce_runtime_security_floor().expect("admitted WebKitGTK");
        gtk::init().expect("isolated GTK display");
        let window = gtk::Window::new(gtk::WindowType::Toplevel);
        window.set_default_size(800, 600);
        let context = webkit2gtk::WebContext::new_ephemeral();
        context.set_sandbox_enabled(true);
        let page = webkit2gtk::WebView::with_context(&context);
        window.add(&page);
        // The completion is Send; keep assertions in its owned result channel.
        let (sender, receiver) = std::sync::mpsc::channel();
        capture_page(&page, Box::new(move |result| sender.send(result).unwrap()));
        assert!(receiver.recv().unwrap().is_none());
        page.load_html(
            "<!doctype html><title>ready</title><style>html{background:#204080}</style>",
            None,
        );
        window.show_all();
        let deadline = Instant::now() + Duration::from_secs(10);
        while page.title().as_deref() != Some("ready") {
            while gtk::events_pending() {
                gtk::main_iteration_do(false);
            }
            assert!(Instant::now() < deadline, "local page did not load");
            std::thread::sleep(Duration::from_millis(5));
        }
        for transparent in [true, false] {
            page.set_opacity(if transparent { 0.0 } else { 1.0 });
            page.set_sensitive(transparent);
            let (sender, receiver) = std::sync::mpsc::channel();
            capture_page(&page, Box::new(move |result| sender.send(result).unwrap()));
            assert!(
                receiver.recv().unwrap().is_none(),
                "parked pages are not previews"
            );
        }
        page.set_sensitive(true);
        let (sender, receiver) = std::sync::mpsc::channel();
        capture_page(&page, Box::new(move |result| sender.send(result).unwrap()));
        let (second_sender, second_receiver) = std::sync::mpsc::channel();
        capture_page(
            &page,
            Box::new(move |result| second_sender.send(result).unwrap()),
        );
        assert!(
            second_receiver.recv().unwrap().is_none(),
            "one native capture at a time"
        );
        let bytes = loop {
            while gtk::events_pending() {
                gtk::main_iteration_do(false);
            }
            if let Ok(result) = receiver.try_recv() {
                break result.expect("native JPEG preview");
            }
            assert!(Instant::now() < deadline, "snapshot did not settle");
            std::thread::sleep(Duration::from_millis(5));
        };
        assert!(bytes.len() <= OUTPUT_BYTES);
        let loader = gtk::gdk_pixbuf::PixbufLoader::with_type("jpeg").unwrap();
        loader.write(&bytes).unwrap();
        loader.close().unwrap();
        let image = loader.pixbuf().unwrap();
        assert_eq!((image.width(), image.height()), (480, 360));
        let pixels = image.read_pixel_bytes();
        let offset = usize::try_from(180 * image.rowstride() + 240 * image.n_channels()).unwrap();
        for (actual, expected) in pixels[offset..offset + 3].iter().zip([32u8, 64, 128]) {
            assert!(
                actual.abs_diff(expected) <= 3,
                "preview must contain the page pixels"
            );
        }
        for scale in [1.0, 2.0] {
            let source = cairo::ImageSurface::create(cairo::Format::Rgb24, 1200, 800).unwrap();
            source.set_device_scale(scale, scale);
            let expected = (1200, 800);
            assert!(thumbnail(source.as_ref().clone(), expected).is_some());
            assert!(thumbnail(source.as_ref().clone(), (30, 40)).is_none());
        }
        window.close();
    }
}
