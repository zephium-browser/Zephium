use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::f64::consts::{FRAC_PI_2, PI};
use std::rc::Rc;

use gtk::cairo;
use gtk::glib::{self, Propagation};
use gtk::prelude::*;
use webkit2gtk::WebViewExt;

use crate::linux_display::{display_backend, DisplayBackend};

const SHADOW_PADDING: i32 = 20;
const MAX_EXTENT: i32 = 16_384;
const SWITCHER_RADIUS: i32 = 26;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Geometry {
    width: i32,
    height: i32,
    padding: i32,
}

impl Geometry {
    fn new(width: i32, height: i32, shadow_room: f64) -> Option<Self> {
        if !(1..=MAX_EXTENT).contains(&width) || !(1..=MAX_EXTENT).contains(&height) {
            return None;
        }
        let padding = if shadow_room.is_finite() {
            shadow_room.floor().clamp(0.0, f64::from(SHADOW_PADDING)) as i32
        } else {
            0
        };
        Some(Self {
            width,
            height,
            padding,
        })
    }

    fn window_size(self) -> (i32, i32) {
        (
            self.width + 2 * self.padding,
            self.height + 2 * self.padding,
        )
    }

    fn allocated(self, width: i32, height: i32) -> Self {
        let padding = self
            .padding
            .min(width.min(height).saturating_sub(1).max(0) / 2);
        Self {
            width: (width - 2 * padding).clamp(1, MAX_EXTENT),
            height: (height - 2 * padding).clamp(1, MAX_EXTENT),
            padding,
        }
    }
}

struct Surface {
    window: glib::WeakRef<gtk::ApplicationWindow>,
    view: glib::WeakRef<webkit2gtk::WebView>,
    host: glib::WeakRef<gtk::Fixed>,
    geometry: Cell<Geometry>,
    allocated: Cell<Geometry>,
    shadow_padding: Cell<Option<i32>>,
    switcher: Cell<bool>,
    high_contrast: Cell<bool>,
}

thread_local! {
    static SURFACES: RefCell<HashMap<usize, Rc<Surface>>> = RefCell::new(HashMap::new());
}

fn key(window: &gtk::ApplicationWindow) -> usize {
    window.as_ptr() as usize
}

fn find(window: &gtk::ApplicationWindow) -> Option<Rc<Surface>> {
    SURFACES.with(|surfaces| surfaces.borrow().get(&key(window)).cloned())
}

pub(crate) fn install(window: &tauri::WebviewWindow) -> Result<(), String> {
    if !gtk::is_initialized_main_thread() {
        return Err("launcher surface requires GTK's main thread".into());
    }
    let native = window.gtk_window().map_err(|error| error.to_string())?;
    let vbox = window.default_vbox().map_err(|error| error.to_string())?;
    let children = vbox.children();
    let [child] = children.as_slice() else {
        return Err("launcher surface requires one privileged WebView".into());
    };
    let view = child
        .clone()
        .downcast::<webkit2gtk::WebView>()
        .map_err(|_| "launcher surface child is not a WebView")?;
    install_native(&native, &vbox, &view).map(|_| ())
}

fn install_native(
    window: &gtk::ApplicationWindow,
    vbox: &gtk::Box,
    view: &webkit2gtk::WebView,
) -> Result<Rc<Surface>, String> {
    if window.is_visible() || window.is_mapped() || window.is_realized() {
        return Err("launcher surface must install before realization and presentation".into());
    }
    if find(window).is_some() {
        return Err("launcher surface is already installed".into());
    }
    if view.parent().as_ref() != Some(vbox.upcast_ref())
        || vbox.parent().as_ref() != Some(window.upcast_ref())
    {
        return Err("launcher surface hierarchy does not match Tauri's host".into());
    }
    let (width, height) = window.size();
    let geometry = Geometry::new(width.max(1), height.max(1), 0.0)
        .ok_or("launcher surface initial size is out of bounds")?;
    let host: gtk::Fixed = fixed::PanelFixed::new().upcast();
    vbox.remove(view);
    window.remove(vbox);
    host.put(view, 0, 0);
    window.add(&host);
    host.show_all();
    if view.parent().as_ref() != Some(host.upcast_ref())
        || host.parent().as_ref() != Some(window.upcast_ref())
        || window.is_visible()
        || window.is_mapped()
    {
        host.remove(view);
        window.remove(&host);
        vbox.pack_start(view, true, true, 0);
        window.add(vbox);
        window.hide();
        return Err("launcher surface reparenting failed its hidden-host postcondition".into());
    }
    if display_backend(window) == DisplayBackend::Wayland {
        // Tao's unused titlebar would make GTK overwrite our shadow margins.
        window.set_titlebar(None::<&gtk::Widget>);
    }

    // Only GTK's shadow canvas is transparent. WebKit always renders opaque.
    view.set_background_color(&gtk::gdk::RGBA::new(
        26.0 / 255.0,
        26.0 / 255.0,
        29.0 / 255.0,
        1.0,
    ));
    let state = Rc::new(Surface {
        window: window.downgrade(),
        view: view.downgrade(),
        host: host.downgrade(),
        geometry: Cell::new(geometry),
        allocated: Cell::new(geometry),
        shadow_padding: Cell::new(None),
        switcher: Cell::new(false),
        high_contrast: Cell::new(false),
    });
    let allocated = state.clone();
    host.connect_size_allocate(move |host, allocation| {
        let geometry = allocated
            .geometry
            .get()
            .allocated(allocation.width(), allocation.height());
        allocated.allocated.set(geometry);
        if let Some(view) = allocated.view.upgrade() {
            if host.child_property::<i32>(&view, "x") != geometry.padding
                || host.child_property::<i32>(&view, "y") != geometry.padding
            {
                host.move_(&view, geometry.padding, geometry.padding);
            }
            if view.size_request() != (geometry.width, geometry.height) {
                view.set_size_request(geometry.width, geometry.height);
            }
            view.size_allocate(&gtk::Allocation::new(
                allocation.x() + geometry.padding,
                allocation.y() + geometry.padding,
                geometry.width,
                geometry.height,
            ));
        }
        allocated.refresh();
    });
    let drawing = state.clone();
    host.connect_draw(move |_, cr| {
        if !drawing.high_contrast.get() && drawing.composited() {
            draw_shadow(cr, drawing.allocated.get(), drawing.radius());
        }
        Propagation::Proceed
    });
    let clipping = state.clone();
    view.connect_draw(move |view, cr| {
        let geometry = clipping.allocated.get();
        rounded_path(
            cr,
            0.0,
            0.0,
            f64::from(geometry.width),
            f64::from(geometry.height),
            f64::from(clipping.radius()),
        );
        cr.clip();
        draw_webkit(view, cr);
        Propagation::Stop
    });
    let realizing = state.clone();
    view.connect_realize(move |_| realizing.refresh());
    let realizing = state.clone();
    window.connect_realize(move |_| realizing.refresh());
    let unrealizing = state.clone();
    window.connect_unrealize(move |_| unrealizing.shadow_padding.set(None));
    if let Some(settings) = window.settings() {
        state.high_contrast.set(high_contrast(&settings));
        let weak = Rc::downgrade(&state);
        let handler = settings.connect_gtk_theme_name_notify(move |settings| {
            if let Some(state) = weak.upgrade() {
                state.high_contrast.set(high_contrast(settings));
                state.refresh();
            }
        });
        let handler = RefCell::new(Some(handler));
        window.connect_destroy(move |_| {
            if let Some(handler) = handler.take() {
                settings.disconnect(handler);
            }
        });
    }
    if let Some(screen) = GtkWindowExt::screen(window) {
        let weak = Rc::downgrade(&state);
        let handler = screen.connect_composited_changed(move |_| {
            if let Some(state) = weak.upgrade() {
                state.refresh();
            }
        });
        let handler = RefCell::new(Some(handler));
        window.connect_destroy(move |_| {
            if let Some(handler) = handler.take() {
                screen.disconnect(handler);
            }
        });
    }
    let window_key = key(window);
    window.connect_destroy(move |_| {
        SURFACES.with(|surfaces| surfaces.borrow_mut().remove(&window_key));
    });
    SURFACES.with(|surfaces| surfaces.borrow_mut().insert(window_key, state.clone()));
    state.refresh();
    Ok(state)
}

pub(super) fn place(
    window: &gtk::ApplicationWindow,
    width: i32,
    height: i32,
    shadow_room: f64,
) -> Option<(i32, i32, i32)> {
    let Some(state) = find(window) else {
        return Some((width, height, 0));
    };
    let geometry = Geometry::new(width, height, shadow_room)?;
    state.geometry.set(geometry);
    if let Some(host) = state.host.upgrade() {
        host.queue_resize();
    }
    let (width, height) = geometry.window_size();
    Some((width, height, geometry.padding))
}

pub(super) fn resize(
    window: &gtk::ApplicationWindow,
    width: i32,
    height: i32,
) -> Option<(i32, i32, i32)> {
    let padding = find(window).map_or(0, |state| state.geometry.get().padding);
    place(window, width, height, f64::from(padding))
}

pub(crate) fn set_switcher(window: &gtk::ApplicationWindow, switcher: bool) {
    if let Some(state) = find(window) {
        if state.switcher.replace(switcher) != switcher {
            state.refresh();
        }
    }
}

impl Surface {
    fn radius(&self) -> i32 {
        if self.high_contrast.get() {
            0
        } else if self.switcher.get() {
            SWITCHER_RADIUS
        } else {
            i32::from(super::super::model::RADIUS)
        }
    }

    fn composited(&self) -> bool {
        self.window
            .upgrade()
            .and_then(|window| GtkWindowExt::screen(&window))
            .is_some_and(|screen| screen.is_composited())
    }

    fn refresh(&self) {
        let geometry = self.allocated.get();
        let region = rounded_region(geometry.width, geometry.height, self.radius());
        if let Some(view) = self.view.upgrade() {
            if let Some(window) = view.window() {
                window.shape_combine_region(Some(&region), 0, 0);
                window.input_shape_combine_region(&region, 0, 0);
            }
            view.queue_draw();
        }
        region.translate(geometry.padding, geometry.padding);
        if let Some(window) = self.window.upgrade() {
            if let Some(native) = window.window() {
                if display_backend(&window) == DisplayBackend::Wayland
                    && self.shadow_padding.replace(Some(geometry.padding)) != Some(geometry.padding)
                {
                    let (width, height) = (native.width(), native.height());
                    native.set_shadow_width(
                        geometry.padding,
                        geometry.padding,
                        geometry.padding,
                        geometry.padding,
                    );
                    // GDK grows the canvas when margins change; retain our viewport.
                    native.resize(width, height);
                }
                native.input_shape_combine_region(&region, 0, 0);
                // Without a compositor, there is no alpha channel for a shadow.
                native.shape_combine_region((!self.composited()).then_some(&region), 0, 0);
            }
        }
        if let Some(host) = self.host.upgrade() {
            host.queue_draw();
        }
    }
}

fn high_contrast(settings: &gtk::Settings) -> bool {
    settings
        .gtk_theme_name()
        .is_some_and(|name| name.to_ascii_lowercase().contains("highcontrast"))
}

fn rounded_region(width: i32, height: i32, radius: i32) -> cairo::Region {
    let region = cairo::Region::create();
    if width <= 0 || height <= 0 || width > MAX_EXTENT || height > MAX_EXTENT {
        return region;
    }
    let radius = radius.clamp(0, width.min(height) / 2);
    let _ = region.union_rectangle(&cairo::RectangleInt::new(
        0,
        radius,
        width,
        height - 2 * radius,
    ));
    for row in 0..radius {
        let dy = f64::from(radius - row) - 0.5;
        let inset =
            (f64::from(radius) - (f64::from(radius * radius) - dy * dy).sqrt()).floor() as i32;
        for y in [row, height - row - 1] {
            let _ =
                region.union_rectangle(&cairo::RectangleInt::new(inset, y, width - 2 * inset, 1));
        }
    }
    region
}

fn rounded_path(cr: &cairo::Context, x: f64, y: f64, width: f64, height: f64, radius: f64) {
    let radius = radius.clamp(0.0, width.min(height) / 2.0);
    cr.new_path();
    cr.arc(x + width - radius, y + radius, radius, -FRAC_PI_2, 0.0);
    cr.arc(
        x + width - radius,
        y + height - radius,
        radius,
        0.0,
        FRAC_PI_2,
    );
    cr.arc(x + radius, y + height - radius, radius, FRAC_PI_2, PI);
    cr.arc(x + radius, y + radius, radius, PI, PI + FRAC_PI_2);
    cr.close_path();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn shadow_extents_preserve_viewport_and_fit_available_room() {
        let card = Geometry::new(680, 250, 24.0).unwrap();
        assert_eq!(card.padding, SHADOW_PADDING);
        assert_eq!(card.window_size(), (720, 290));
        assert_eq!(card.allocated(720, 290), card);
        let tiny = Geometry::new(1, 1, 0.5).unwrap();
        assert_eq!(tiny.window_size(), (1, 1));
        assert_eq!(tiny.padding, 0);
        for room in [-1.0, f64::NAN, f64::INFINITY] {
            assert_eq!(Geometry::new(680, 56, room).unwrap().padding, 0);
        }
        for size in [(0, 56), (680, -1), (i32::MAX, 56), (680, i32::MAX)] {
            assert!(Geometry::new(size.0, size.1, 20.0).is_none());
        }
        assert_eq!(card.allocated(2, 2).window_size(), (2, 2));
        assert_eq!(card.allocated(1, 1).window_size(), (1, 1));
        assert_eq!(card.allocated(i32::MIN, i32::MIN).window_size(), (1, 1));
    }

    #[test]
    fn rounded_clip_excludes_corners_and_keeps_symmetric_paint_and_input_bounds() {
        for (width, height, radius) in [(680, 56, 20), (852, 500, 26), (5, 3, 26)] {
            let region = rounded_region(width, height, radius);
            assert!(region.contains_point(width / 2, 0));
            assert!(region.contains_point(0, height / 2));
            assert!(region.contains_point(width / 2, height / 2));
            for y in 0..height {
                for x in 0..width {
                    assert_eq!(
                        region.contains_point(x, y),
                        region.contains_point(width - x - 1, y)
                    );
                    assert_eq!(
                        region.contains_point(x, y),
                        region.contains_point(x, height - y - 1)
                    );
                }
            }
            assert!(!region.contains_point(-1, 0));
            assert!(!region.contains_point(width, height / 2));
        }
        assert!(!rounded_region(680, 56, 20).contains_point(0, 0));
        assert!(rounded_region(680, 56, 0).contains_point(0, 0));
        for (width, height) in [(0, 1), (1, -1), (i32::MAX, 1)] {
            assert!(rounded_region(width, height, 20).is_empty());
        }
    }

    fn pixel(surface: &cairo::ImageSurface, x: i32, y: i32) -> u32 {
        let offset = (y * surface.stride() + 4 * x) as usize;
        let mut pixel = 0;
        surface
            .with_data(|data| {
                pixel = u32::from_ne_bytes(data[offset..offset + 4].try_into().unwrap());
            })
            .unwrap();
        pixel
    }

    #[test]
    fn native_shadow_fades_inside_padding_and_clip_leaves_transparent_corners() {
        let geometry = Geometry::new(180, 56, 20.0).unwrap();
        let (width, height) = geometry.window_size();
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let surface = cairo::ImageSurface::create(
                cairo::Format::ARgb32,
                (f64::from(width) * scale).ceil() as i32,
                (f64::from(height) * scale).ceil() as i32,
            )
            .unwrap();
            surface.set_device_scale(scale, scale);
            assert_raster_bounds(&surface, geometry, scale);
        }
    }

    fn assert_raster_bounds(surface: &cairo::ImageSurface, geometry: Geometry, scale: f64) {
        let (width, height) = geometry.window_size();
        let at = |x, y| pixel(surface, (x * scale) as i32, (y * scale) as i32);
        let cr = cairo::Context::new(surface).unwrap();
        draw_shadow(&cr, geometry, 20);
        assert_eq!(at(0.0, 0.0), 0);
        assert_eq!(at(f64::from(width - 1), f64::from(height - 1)), 0);
        let outer = at(f64::from(width / 2), f64::from(height - 2)) >> 24;
        let inner = at(
            f64::from(width / 2),
            f64::from(height - geometry.padding + 2),
        ) >> 24;
        assert!(inner > outer && inner < 255);
        cr.translate(f64::from(geometry.padding), f64::from(geometry.padding));
        rounded_path(
            &cr,
            0.0,
            0.0,
            f64::from(geometry.width),
            f64::from(geometry.height),
            20.0,
        );
        cr.clip();
        cr.set_source_rgb(1.0, 0.0, 0.0);
        cr.paint().unwrap();
        assert_eq!(at(f64::from(width / 2), f64::from(height / 2)), 0xffff_0000);
        assert_ne!(
            at(f64::from(geometry.padding), f64::from(geometry.padding)),
            0xffff_0000
        );
    }

    fn drain_for(duration: Duration) {
        let deadline = Instant::now() + duration;
        while Instant::now() < deadline {
            while gtk::events_pending() {
                gtk::main_iteration_do(false);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn assert_x11_shadow_passes_pointer(window: &gtk::ApplicationWindow, geometry: Geometry) {
        let native = window.window().unwrap();
        let (_, origin_x, origin_y) = native.origin();
        let behind = gtk::Window::new(gtk::WindowType::Toplevel);
        behind.set_decorated(false);
        behind.set_accept_focus(false);
        behind.set_focus_on_map(false);
        let (width, height) = geometry.window_size();
        behind.set_default_size(1, 1);
        behind.resize(width, height);
        behind.move_(origin_x, origin_y);
        let owner = window.transient_for();
        window.set_transient_for(Some(&behind));
        behind.show();
        wait_for(|| behind.is_mapped());
        let behind_native = behind.window().unwrap();
        behind.move_(origin_x, origin_y);
        wait_for(|| {
            let (_, x, y) = behind_native.origin();
            x == origin_x && y == origin_y
        });
        window.present();
        let display = window.display();
        display.sync();
        assert!(display.supports_input_shapes());
        let pointer = display.default_seat().unwrap().pointer().unwrap();
        let padding = geometry.padding;
        let middle_x = padding + geometry.width / 2;
        let middle_y = padding + geometry.height / 2;
        for (x, y, content) in [
            (middle_x, middle_y, true),
            (middle_x, padding, true),
            (middle_x, padding - 2, false),
            (padding - 2, middle_y, false),
            (padding + geometry.width + 2, middle_y, false),
            (middle_x, padding + geometry.height + 2, false),
            (padding, padding, false),
        ] {
            eprintln!("panel X11 pointer: ({x}, {y}), content={content}");
            pointer.warp(&native.screen(), origin_x + x, origin_y + y);
            display.sync();
            let expected = if content { &native } else { &behind_native };
            wait_for(|| {
                pointer
                    .window_at_position()
                    .0
                    .is_some_and(|target| target.toplevel() == *expected)
            });
        }
        window.set_transient_for(owner.as_ref());
        // SAFETY: the test owns this temporary input target.
        unsafe { behind.destroy() };
    }

    #[track_caller]
    fn wait_for(mut ready: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            while gtk::events_pending() {
                gtk::main_iteration_do(false);
            }
            if ready() {
                return;
            }
            assert!(Instant::now() < deadline, "isolated panel did not settle");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    #[ignore = "requires a disposable X11 or Wayland display; never run on the host desktop"]
    fn opaque_panel_keeps_viewport_clip_shadow_and_focus_across_resizes() {
        let backend = std::env::var("GDK_BACKEND").expect("explicit isolated GDK_BACKEND");
        assert!(matches!(backend.as_str(), "x11" | "wayland"));
        gtk::init().expect("isolated GTK display");
        let application = gtk::Application::new(
            Some("dev.zephium.opaque-panel-test"),
            gtk::gio::ApplicationFlags::NON_UNIQUE,
        );
        application
            .register(None::<&gtk::gio::Cancellable>)
            .unwrap();
        let window = gtk::ApplicationWindow::new(&application);
        if backend == "wayland" {
            window.set_titlebar(Some(&gtk::EventBox::new()));
        }
        window.set_decorated(false);
        window.set_resizable(false);
        window.set_app_paintable(true);
        if let Some(visual) = GtkWindowExt::screen(&window).and_then(|screen| screen.rgba_visual())
        {
            window.set_visual(Some(&visual));
        }
        window.set_default_size(1, 1);
        window.resize(680, 56);
        let vbox = gtk::Box::new(gtk::Orientation::Vertical, 0);
        window.add(&vbox);
        let context = webkit2gtk::WebContext::new_ephemeral();
        let view = webkit2gtk::WebView::with_context(&context);
        view.set_background_color(&gtk::gdk::RGBA::new(0.0, 0.0, 0.0, 0.0));
        vbox.pack_start(&view, true, true, 0);

        let wrong_parent = gtk::Box::new(gtk::Orientation::Vertical, 0);
        assert!(install_native(&window, &wrong_parent, &view).is_err());
        assert_eq!(view.parent().as_ref(), Some(vbox.upcast_ref()));
        let state = install_native(&window, &vbox, &view).unwrap();
        assert!(window.titlebar().is_none());
        let draws = Rc::new(Cell::new(0));
        let observed_draws = draws.clone();
        state.host.upgrade().unwrap().connect_draw(move |_, _| {
            observed_draws.set(observed_draws.get() + 1);
            Propagation::Proceed
        });
        assert_eq!(view.background_color().alpha(), 1.0);
        assert!(!window.is_visible());
        assert!(!window.is_mapped());
        assert!(install_native(&window, &vbox, &view).is_err());
        let loaded = Rc::new(Cell::new(false));
        let completed = loaded.clone();
        view.connect_load_changed(move |_, event| {
            if event == webkit2gtk::LoadEvent::Finished {
                completed.set(true);
            }
        });
        view.load_html(
            "<!doctype html><html style='background:#f00'><body></body></html>",
            None,
        );
        for (width, height, switcher) in [
            (680, 250, false),
            (852, 500, true),
            (228, 180, true),
            (680, 56, false),
        ] {
            eprintln!("panel {backend}: requesting {width}x{height}, switcher={switcher}");
            set_switcher(&window, switcher);
            super::super::place(
                &window,
                tauri::LogicalSize::new(f64::from(width), f64::from(height)),
                None,
                24.0,
            );
            window.show_all();
            let mut observed = None;
            wait_for(|| {
                let current = (
                    window.is_mapped(),
                    window.size(),
                    (view.allocated_width(), view.allocated_height()),
                    view.size_request(),
                    state
                        .host
                        .upgrade()
                        .map(|host| (host.allocated_width(), host.allocated_height())),
                    state.geometry.get(),
                    state.allocated.get(),
                    loaded.get(),
                );
                if observed != Some(current) {
                    eprintln!(
                        "panel geometry: mapped={}, window={:?}, view={:?}, request={:?}, host={:?}, desired={:?}, allocated={:?}, loaded={}",
                        current.0, current.1, current.2, current.3,
                        current.4, current.5, current.6, current.7,
                    );
                    observed = Some(current);
                }
                window.is_mapped()
                    && view.allocated_width() == width
                    && view.allocated_height() == height
            });
            assert_eq!(window.size(), (width + 40, height + 40));
            assert_eq!(state.allocated.get().padding, SHADOW_PADDING);
            assert_eq!(view.allocation().x(), SHADOW_PADDING);
            assert_eq!(view.allocation().y(), SHADOW_PADDING);
            assert_eq!(state.radius(), if switcher { SWITCHER_RADIUS } else { 20 });
            assert!(view.window().unwrap().is_shaped());
            assert_eq!(view.background_color().alpha(), 1.0);
            assert_eq!(view.window().unwrap().scale_factor(), window.scale_factor());
            window.set_focus(Some(&view));
            assert_eq!(window.focused_widget().as_ref(), Some(view.upcast_ref()));
        }
        eprintln!("panel {backend}: awaiting document load");
        wait_for(|| loaded.get());
        let native = window.window().unwrap();
        let scale = native.scale_factor();
        if let Ok(requested) = std::env::var("GDK_SCALE") {
            assert_eq!(scale, requested.parse::<i32>().expect("integer GDK_SCALE"));
        }
        let geometry = Geometry::new(180, 56, 20.0).unwrap();
        let (width, height) = geometry.window_size();
        let raster = native
            .create_similar_image_surface(cairo::Format::ARgb32, width * scale, height * scale, 0)
            .unwrap();
        assert_eq!(raster.device_scale(), (f64::from(scale), f64::from(scale)));
        let raster = cairo::ImageSurface::try_from(raster).unwrap();
        assert_raster_bounds(&raster, geometry, f64::from(scale));
        let mut last_draws = draws.get();
        let mut last_draw = Instant::now();
        eprintln!("panel {backend}: awaiting idle draws");
        wait_for(|| {
            if draws.get() != last_draws {
                last_draws = draws.get();
                last_draw = Instant::now();
            }
            last_draw.elapsed() >= Duration::from_millis(500)
        });
        drain_for(Duration::from_millis(500));
        assert_eq!(
            draws.get(),
            last_draws,
            "idle surface scheduled more Cairo draws"
        );
        eprintln!("opaque panel: GDK scale {scale}, idle host draws: 0 in 500 ms");
        if backend == "x11" {
            assert_x11_shadow_passes_pointer(&window, state.allocated.get());
        }
        let position = window.position();
        super::super::resize(&window, tauri::LogicalSize::new(680.0, 300.0));
        wait_for(|| view.allocated_width() == 680 && view.allocated_height() == 300);
        assert_eq!(window.position(), position);
        assert_eq!(window.size(), (720, 340));
        assert_eq!(state.geometry.get().padding, SHADOW_PADDING);
        assert_eq!(window.focused_widget().as_ref(), Some(view.upcast_ref()));
        super::super::resize(&window, tauri::LogicalSize::new(680.0, 56.0));
        wait_for(|| view.allocated_width() == 680 && view.allocated_height() == 56);
        assert!(install_native(&window, &vbox, &view).is_err());
        state.high_contrast.set(true);
        state.refresh();
        assert_eq!(state.radius(), 0);
        assert!(rounded_region(680, 56, state.radius()).contains_point(0, 0));
        window.hide();
        wait_for(|| !window.is_mapped());
        window.show_all();
        wait_for(|| window.is_mapped());
        assert_eq!(view.allocated_width(), 680);
        assert_eq!(view.allocated_height(), 56);
        for padding in [0, 8, SHADOW_PADDING] {
            super::super::place(
                &window,
                tauri::LogicalSize::new(680.0, 56.0),
                None,
                f64::from(padding),
            );
            wait_for(|| {
                window.size() == (680 + 2 * padding, 56 + 2 * padding)
                    && view.allocated_width() == 680
                    && view.allocated_height() == 56
                    && state.allocated.get().padding == padding
            });
            assert_eq!(view.allocation().x(), padding);
            assert_eq!(view.allocation().y(), padding);
        }
        window.hide();
        wait_for(|| !window.is_mapped());
        window.unrealize();
        super::super::place(
            &window,
            tauri::LogicalSize::new(680.0, 56.0),
            None,
            f64::from(SHADOW_PADDING),
        );
        window.show_all();
        wait_for(|| {
            window.is_mapped()
                && window.size() == (720, 96)
                && view.allocated_width() == 680
                && view.allocated_height() == 56
        });
        // SAFETY: the test owns the window and does not use it after destruction.
        unsafe { window.destroy() };
        assert!(find(&window).is_none());
    }
}

fn draw_shadow(cr: &cairo::Context, geometry: Geometry, radius: i32) {
    let spread = (geometry.padding - 4).max(0);
    if spread == 0 {
        return;
    }
    let mut previous = 0.0;
    for band in (0..=spread).rev() {
        let distance = f64::from(band) / f64::from(spread) * 3.0;
        let opacity = 0.28 * (-0.5 * distance * distance).exp();
        cr.set_source_rgba(0.0, 0.0, 0.0, (opacity - previous) / (1.0 - previous));
        previous = opacity;
        rounded_path(
            cr,
            f64::from(geometry.padding - band),
            f64::from(geometry.padding + 4 - band),
            f64::from(geometry.width + 2 * band),
            f64::from(geometry.height + 2 * band),
            f64::from(radius + band),
        );
        let _ = cr.fill();
    }
}

fn draw_webkit(view: &webkit2gtk::WebView, cr: &cairo::Context) {
    let class = view.class();
    let class = (class.as_ref() as *const webkit2gtk::ffi::WebKitWebViewClass)
        .cast::<gtk::ffi::GtkWidgetClass>();
    // SAFETY: the live WebKit widget and Cairo context belong to this draw
    // callback. Calling its vfunc preserves the clip across GTK handlers.
    unsafe {
        if let Some(draw) = (*class).draw {
            draw(view.as_ptr().cast(), cr.to_raw_none());
        }
    }
}

mod fixed {
    use gtk::glib;
    use gtk::subclass::prelude::*;

    mod imp {
        use super::*;

        #[derive(Default)]
        pub struct PanelFixed;

        #[glib::object_subclass]
        impl ObjectSubclass for PanelFixed {
            const NAME: &'static str = "ZephiumOpaquePanelFixed";
            type Type = super::PanelFixed;
            type ParentType = gtk::Fixed;
        }

        impl ObjectImpl for PanelFixed {}
        impl WidgetImpl for PanelFixed {
            // Zero would trigger GtkWindow's 200px empty-window fallback.
            fn preferred_width(&self) -> (i32, i32) {
                (1, 1)
            }
            fn preferred_height(&self) -> (i32, i32) {
                (1, 1)
            }
        }
        impl ContainerImpl for PanelFixed {}
        impl FixedImpl for PanelFixed {}
    }

    glib::wrapper! {
        pub struct PanelFixed(ObjectSubclass<imp::PanelFixed>)
            @extends gtk::Fixed, gtk::Container, gtk::Widget;
    }

    impl PanelFixed {
        pub fn new() -> Self {
            glib::Object::new()
        }
    }
}
