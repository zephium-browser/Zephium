//! Page-overlapping guides in the composition root, without popup placement.

use std::cell::Cell;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{cairo, glib};
use zephium_core::geometry::Rect;

const RADIUS: f64 = 10.0;
const BORDER: f64 = 1.0;
const FILL: f64 = 0.06;
const STROKE: f64 = 0.16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum GuideStyle {
    Drop,
    Resize,
}

#[derive(Clone, Copy)]
struct Zone {
    rect: Rect,
    style: GuideStyle,
}

struct Inner {
    fixed: gtk::Fixed,
    area: gtk::DrawingArea,
    style: Rc<Cell<GuideStyle>>,
    zone: Cell<Option<Zone>>,
    revision: Cell<u64>,
    allocation_handler: Cell<Option<glib::SignalHandlerId>>,
}

#[derive(Clone)]
pub(super) struct Guide(Rc<Inner>);

impl Guide {
    pub(super) fn new(fixed: &gtk::Fixed) -> Self {
        let area = gtk::DrawingArea::new();
        area.set_app_paintable(true);
        area.set_can_focus(false);
        area.set_focus_on_click(false);
        area.set_sensitive(false);
        area.set_no_show_all(true);
        area.set_child_visible(false);
        area.set_visual(fixed.visual().as_ref());
        area.connect_realize(|area| {
            if let Some(window) = area.window() {
                window.set_pass_through(true);
                window.input_shape_combine_region(&cairo::Region::create(), 0, 0);
            }
        });
        let style = Rc::new(Cell::new(GuideStyle::Drop));
        let drawn_style = style.clone();
        area.connect_draw(move |area, cr| {
            paint(
                cr,
                f64::from(area.allocated_width()),
                f64::from(area.allocated_height()),
                drawn_style.get(),
            );
            glib::Propagation::Stop
        });
        fixed.put(&area, 0, 0);
        let inner = Rc::new(Inner {
            fixed: fixed.clone(),
            area,
            style,
            zone: Cell::new(None),
            revision: Cell::new(0),
            allocation_handler: Cell::new(None),
        });
        let weak = Rc::downgrade(&inner);
        let handler = fixed.connect_size_allocate(move |_, _| {
            if let Some(inner) = weak.upgrade() {
                inner.advance();
                inner.refresh();
            }
        });
        inner.allocation_handler.set(Some(handler));
        Self(inner)
    }

    /// The zone is in root logical coordinates, including the content origin.
    pub(super) fn show(&self, rect: Rect, style: GuideStyle) -> bool {
        self.0.advance();
        let revision = self.0.revision.get();
        self.0.zone.set(Some(Zone { rect, style }));
        if self.0.refresh() {
            self.raise();
            true
        } else {
            if self.0.revision.get() == revision {
                self.0.zone.set(None);
            }
            false
        }
    }

    pub(super) fn hide(&self) {
        self.0.advance();
        self.0.zone.set(None);
        self.0.conceal();
    }

    /// A later WebKit child may map above an already visible guide.
    pub(super) fn raise(&self) {
        if self.0.zone.get().is_some() && self.0.area.is_child_visible() {
            let fixed = &self.0.fixed;
            let area = &self.0.area;
            if area.parent().as_ref() != Some(fixed.upcast_ref::<gtk::Widget>()) {
                self.0.conceal();
                return;
            }
            if fixed.children().last() != Some(area.upcast_ref::<gtk::Widget>()) {
                // Wayland follows GtkFixed paint order, not GdkWindow.raise.
                // Reinsert only after a later child, never on each guide move.
                self.0.advance();
                let revision = self.0.revision.get();
                area.set_child_visible(false);
                if self.0.revision.get() != revision
                    || area.parent().as_ref() != Some(fixed.upcast_ref::<gtk::Widget>())
                {
                    return;
                }
                fixed.remove(area);
                // Restore ownership after a reentrant hide, without showing it.
                if area.parent().is_none() {
                    fixed.put(area, 0, 0);
                }
                if self.0.revision.get() != revision
                    || area.parent().as_ref() != Some(fixed.upcast_ref::<gtk::Widget>())
                {
                    return;
                }
                if !self.0.refresh() {
                    self.0.conceal();
                    return;
                }
            }
            if let Some(window) = self.0.area.window() {
                window.raise();
            }
        }
    }
}

impl Inner {
    fn advance(&self) {
        self.revision.set(self.revision.get().wrapping_add(1));
    }

    fn conceal(&self) {
        self.area.set_child_visible(false);
        self.area.hide();
    }

    fn refresh(&self) -> bool {
        let Some(zone) = self.zone.get() else {
            return false;
        };
        let root = self.fixed.allocation();
        let Some((x, y, width, height)) = geometry(zone.rect, (root.width(), root.height())) else {
            self.conceal();
            return false;
        };
        if self.area.parent().as_ref() != Some(self.fixed.upcast_ref::<gtk::Widget>()) {
            self.conceal();
            return false;
        }
        // A windowless Fixed allocates in its parent's coordinates.
        let (parent_x, parent_y) = if self.fixed.has_window() {
            (0, 0)
        } else {
            (root.x(), root.y())
        };
        let Some((allocated_x, allocated_y)) = x.checked_add(parent_x).zip(y.checked_add(parent_y))
        else {
            self.conceal();
            return false;
        };
        let allocation = gtk::Allocation::new(allocated_x, allocated_y, width, height);
        if self.area.allocation() == allocation
            && self.style.get() == zone.style
            && self.area.is_visible()
            && self.area.is_child_visible()
        {
            return true;
        }
        let revision = self.revision.get();
        self.style.set(zone.style);
        self.fixed.move_(&self.area, x, y);
        if self.revision.get() != revision {
            return false;
        }
        if self.area.size_request() != (width, height) {
            self.area.set_size_request(width, height);
        }
        if self.revision.get() != revision {
            return false;
        }
        self.area.show();
        if self.revision.get() != revision {
            return false;
        }
        self.area.size_allocate(&allocation);
        if self.revision.get() != revision {
            return false;
        }
        self.area.set_child_visible(true);
        if self.revision.get() != revision {
            return false;
        }
        if let Some(window) = self.area.window() {
            window.raise();
        }
        self.area.queue_draw();
        true
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        if let Some(handler) = self.allocation_handler.take() {
            self.fixed.disconnect(handler);
        }
        if self.area.parent().as_ref() == Some(self.fixed.upcast_ref::<gtk::Widget>()) {
            self.fixed.remove(&self.area);
        }
    }
}

fn geometry(rect: Rect, root: (i32, i32)) -> Option<(i32, i32, i32, i32)> {
    if root.0 <= 0
        || root.1 <= 0
        || ![rect.x, rect.y, rect.width, rect.height]
            .into_iter()
            .all(f64::is_finite)
        || rect.x < 0.0
        || rect.y < 0.0
        || rect.width <= 0.0
        || rect.height <= 0.0
    {
        return None;
    }
    let right = rect.x + rect.width;
    let bottom = rect.y + rect.height;
    if right > f64::from(root.0) || bottom > f64::from(root.1) {
        return None;
    }
    let (x, y, right, bottom) = (
        rect.x.ceil() as i32,
        rect.y.ceil() as i32,
        right.floor() as i32,
        bottom.floor() as i32,
    );
    (right > x && bottom > y).then_some((x, y, right - x, bottom - y))
}

fn paint(cr: &cairo::Context, width: f64, height: f64, style: GuideStyle) {
    if style == GuideStyle::Resize {
        // The page can be white beneath a dark window, or black beneath a light one.
        cr.rectangle(0.0, 0.0, width, height);
        cr.set_source_rgb(0.7, 0.7, 0.7);
        let _ = cr.fill();
        cr.rectangle(0.25, 0.25, (width - 0.5).max(0.0), (height - 0.5).max(0.0));
        cr.set_source_rgba(0.0, 0.0, 0.0, 0.22);
        cr.set_line_width(0.5);
        let _ = cr.stroke();
        return;
    }
    let (width, height) = ((width - BORDER).max(0.0), (height - BORDER).max(0.0));
    let inset = BORDER / 2.0;
    let radius = RADIUS.min(width / 2.0).min(height / 2.0);
    let (right, bottom) = (inset + width, inset + height);
    cr.new_sub_path();
    cr.arc(
        right - radius,
        inset + radius,
        radius,
        -0.5 * std::f64::consts::PI,
        0.0,
    );
    cr.arc(
        right - radius,
        bottom - radius,
        radius,
        0.0,
        0.5 * std::f64::consts::PI,
    );
    cr.arc(
        inset + radius,
        bottom - radius,
        radius,
        0.5 * std::f64::consts::PI,
        std::f64::consts::PI,
    );
    cr.arc(
        inset + radius,
        inset + radius,
        radius,
        std::f64::consts::PI,
        1.5 * std::f64::consts::PI,
    );
    cr.close_path();
    cr.set_source_rgba(0.6, 0.6, 0.6, FILL);
    let _ = cr.fill_preserve();
    cr.set_source_rgba(0.6, 0.6, 0.6, STROKE);
    cr.set_line_width(BORDER);
    let _ = cr.stroke();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    use webkit2gtk::{WebContextExt, WebViewExt};

    #[test]
    fn guide_geometry_stays_inside_the_root_and_rejects_malformed_zones() {
        assert_eq!(
            geometry(Rect::new(8.25, 12.25, 200.75, 300.75), (640, 480)),
            Some((9, 13, 200, 300))
        );
        for rect in [
            Rect {
                x: f64::NAN,
                y: 0.0,
                width: 20.0,
                height: 20.0,
            },
            Rect::new(0.0, f64::INFINITY, 20.0, 20.0),
            Rect::new(0.0, 0.0, f64::MAX, 20.0),
            Rect::new(-1.0, 0.0, 20.0, 20.0),
            Rect::new(0.0, 0.0, 0.0, 20.0),
            Rect::new(639.5, 0.0, 0.5, 20.0),
            Rect::new(630.0, 0.0, 20.0, 20.0),
            Rect::new(0.0, 470.0, 20.0, 20.0),
        ] {
            assert!(geometry(rect, (640, 480)).is_none());
        }
        assert!(geometry(Rect::new(0.0, 0.0, 2.0, 20.0), (0, 480)).is_none());
    }

    #[test]
    fn resize_guide_remains_visible_over_both_white_and_black_pages() {
        for background in [0.0, 1.0] {
            let mut surface = cairo::ImageSurface::create(cairo::Format::ARgb32, 2, 20)
                .expect("guide paint surface");
            let cr = cairo::Context::new(&surface).expect("guide paint context");
            cr.set_source_rgb(background, background, background);
            cr.paint().expect("page background");
            paint(&cr, 2.0, 20.0, GuideStyle::Resize);
            drop(cr);
            let stride = surface.stride() as usize;
            let data = surface.data().expect("guide paint pixels");
            let offset = 10 * stride + 4;
            let pixel =
                u32::from_ne_bytes(data[offset..offset + 4].try_into().expect("ARGB pixel"));
            let red = (pixel >> 16) & 0xff;
            assert!(
                (130..=200).contains(&red),
                "guide must contrast with the page"
            );
            assert_eq!((pixel >> 8) & 0xff, red);
            assert_eq!(pixel & 0xff, red);
        }
    }

    #[test]
    fn drop_wash_and_outline_contrast_with_both_white_and_black_pages() {
        for background in [0.0, 1.0] {
            let mut surface = cairo::ImageSurface::create(cairo::Format::ARgb32, 32, 24)
                .expect("drop paint surface");
            let cr = cairo::Context::new(&surface).expect("drop paint context");
            cr.set_source_rgb(background, background, background);
            cr.paint().expect("page background");
            paint(&cr, 32.0, 24.0, GuideStyle::Drop);
            drop(cr);
            let stride = surface.stride() as usize;
            let data = surface.data().expect("drop paint pixels");
            let channel = |y: usize| {
                let offset = y * stride + 16 * 4;
                (u32::from_ne_bytes(data[offset..offset + 4].try_into().expect("ARGB pixel"))
                    & 0xff) as i32
            };
            let background = (background * 255.0) as i32;
            assert!((4..=16).contains(&(channel(12) - background).abs()));
            assert!((12..=40).contains(&(channel(0) - background).abs()));
        }
    }

    fn pump_until(mut ready: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            while gtk::events_pending() {
                gtk::main_iteration_do(false);
            }
            if ready() {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "native guide condition timed out"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn pixel(window: &gtk::Window, x: i32, y: i32) -> Option<[u8; 3]> {
        let pixbuf = window.window()?.pixbuf(x, y, 1, 1)?;
        let bytes = pixbuf.read_pixel_bytes();
        bytes.as_ref().get(..3)?.try_into().ok()
    }

    fn page(fixed: &gtk::Fixed, context: &webkit2gtk::WebContext) -> webkit2gtk::WebView {
        let view = webkit2gtk::WebView::with_context(context);
        view.set_size_request(640, 400);
        fixed.put(&view, 0, 0);
        view.load_html("<!doctype html><html style='height:100%'><title>ready</title><body style='margin:0;min-height:100%;background:#204080' onclick='document.title=\"clicked\"'>", None);
        view.show();
        view
    }

    #[test]
    #[ignore = "requires private X11 readback/input and sandboxed ephemeral WebKitGTK views"]
    fn guide_paints_above_ephemeral_webkit_and_remains_input_transparent_x11() {
        assert_eq!(
            std::env::var("GDK_BACKEND").as_deref(),
            Ok("x11"),
            "run with scripts/ci/linux-test.sh; Wayland needs compositor capture/input"
        );
        gtk::init().expect("GTK test display");
        let window = gtk::Window::new(gtk::WindowType::Toplevel);
        window.set_default_size(640, 400);
        let fixed = gtk::Fixed::new();
        window.add(&fixed);
        let context = webkit2gtk::WebContext::new_ephemeral();
        context.set_sandbox_enabled(true);
        assert!(context.is_ephemeral());
        assert!(context.is_sandbox_enabled());
        let first = page(&fixed, &context);
        window.show_all();
        pump_until(|| pixel(&window, 150, 120) == Some([32, 64, 128]));
        window.set_focus(Some(&first));
        let focus = window.focused_widget();
        let guide = Guide::new(&fixed);
        assert!(guide.show(Rect::new(50.0, 50.0, 220.0, 160.0), GuideStyle::Drop));
        pump_until(|| {
            pixel(&window, 150, 120)
                .is_some_and(|value| value[0] >= 38 && value[1] >= 68 && value[2] >= 129)
        });
        assert_eq!(window.focused_widget(), focus);
        assert!(!guide.0.area.can_focus());
        assert!(!guide.0.area.is_sensitive());
        assert!(guide
            .0
            .area
            .window()
            .expect("guide GDK window")
            .is_pass_through());
        let later = page(&fixed, &context);
        pump_until(|| later.title().as_deref() == Some("ready"));
        guide.raise();
        pump_until(|| {
            pixel(&window, 150, 120)
                .is_some_and(|value| value[0] >= 38 && value[1] >= 68 && value[2] >= 129)
        });
        {
            let root = window.window().expect("native input root");
            let pointer = root
                .display()
                .default_seat()
                .and_then(|seat| seat.pointer())
                .expect("private X11 pointer");
            let (_, root_x, root_y) = root.origin();
            pointer.warp(&root.screen(), root_x + 150, root_y + 120);
            root.display().sync();
            let (target, _, _) = pointer.window_at_position();
            assert_eq!(
                target,
                later.window(),
                "the guide must not win native hit testing"
            );
            #[link(name = "xdo")]
            unsafe extern "C" {
                fn xdo_new(display: *const std::ffi::c_char) -> *mut std::ffi::c_void;
                fn xdo_click_window(
                    handle: *const std::ffi::c_void,
                    window: usize,
                    button: i32,
                ) -> i32;
                fn xdo_free(handle: *mut std::ffi::c_void);
            }
            // XTest follows real hit testing; XSendEvent is not a WebKit input event.
            // SAFETY: the test owns this private X11 display and libxdo handle.
            let clicked = unsafe {
                let handle = xdo_new(std::ptr::null());
                assert!(!handle.is_null(), "private X11 input connection");
                let result = xdo_click_window(handle, 0, 1);
                xdo_free(handle);
                result
            };
            assert_eq!(clicked, 0);
            pump_until(|| later.title().as_deref() == Some("clicked"));
        }
        assert!(guide.show(Rect::new(149.0, 50.0, 2.0, 160.0), GuideStyle::Resize));
        pump_until(|| {
            pixel(&window, 150, 120)
                .is_some_and(|value| value[0] > 100 && value[1] > 120 && value[2] > 160)
        });
        guide.hide();
        window.show_all();
        pump_until(|| pixel(&window, 150, 120) == Some([32, 64, 128]));
        assert!(!guide.0.area.is_child_visible());
        assert!(!guide.show(Rect::new(630.0, 50.0, 20.0, 160.0), GuideStyle::Drop));
        assert!(!guide.0.area.is_child_visible());
        let child_count = fixed.children().len();
        drop(guide);
        assert_eq!(fixed.children().len(), child_count - 1);
        window.close();
    }
}
