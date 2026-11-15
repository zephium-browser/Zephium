use gtk::glib::translate::{ToGlibPtr, ToGlibPtrMut};
use gtk::prelude::*;
use std::cell::RefCell;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::rc::Rc;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc,
};
use std::time::{Duration, Instant};
use webkit2gtk::{ContextMenuExt, ContextMenuItemExt, SettingsExt, WebContextExt, WebViewExt};
use wry::WebViewExtUnix;
use zephium_core::blocker::{ContentPolicyGeneration, ContentRuleDigest, ContentRules};
use zephium_core::geometry::Rect;
use zephium_core::ids::{ItemId, ProfileId};
use zephium_core::permissions::PageOrigin;
use zephium_core::ports::engine::{
    Engine, EngineEvent, NativeDispatch, Partition, UserContent, UserContentGeneration,
};
use zephium_core::split::Pane;

struct Fixture {
    origin: String,
    stop: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
    requests: mpsc::Receiver<String>,
}
impl Fixture {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let (send, requests) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            while !stopping.load(Ordering::Relaxed) {
                let (mut stream, _) = match listener.accept() {
                    Ok(connection) => connection,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    Err(error) => panic!("loopback accept: {error}"),
                };
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut request = Vec::new();
                let mut bytes = [0; 1024];
                while request.len() < 8192 && !request.windows(4).any(|v| v == b"\r\n\r\n") {
                    match stream.read(&mut bytes) {
                        Ok(0) | Err(_) => break,
                        Ok(count) => request.extend_from_slice(&bytes[..count]),
                    }
                }
                if let Some(end) = request.windows(4).position(|v| v == b"\r\n\r\n") {
                    let content_length = String::from_utf8_lossy(&request[..end])
                        .lines()
                        .filter_map(|line| line.split_once(':'))
                        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                        .map(|(_, value)| value.trim().parse::<usize>().unwrap())
                        .unwrap_or(0);
                    assert!(content_length < 8192);
                    while request.len() < end + 4 + content_length {
                        let count = stream.read(&mut bytes).unwrap();
                        assert_ne!(count, 0, "incomplete fixture request");
                        request.extend_from_slice(&bytes[..count]);
                    }
                }
                let request = String::from_utf8_lossy(&request).into_owned();
                let line = request.lines().next().unwrap_or_default().to_owned();
                if line.contains(" /auth ")
                    && !request
                        .lines()
                        .any(|line| line == "Authorization: Basic Zml4dHVyZTpmaXh0dXJl")
                {
                    let _ = stream.write_all(b"HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Basic realm=\"fixture\"\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                    continue;
                }
                let page = line.contains(" /page ");
                let body = if page {
                    "<!doctype html><title>ready</title><style>a,button{position:absolute;left:0;width:200px;height:40px}#blank{top:0}#link{top:60px}#script{top:120px}#post{top:180px}</style><a id=blank href='/blank' target=_blank>blank</a><a id=link href='/link'>link</a><button id=script onclick=\"window.open('/script')\">script</button><form action='/post' method=post target=_blank><input name=proof value=preserved><button id=post>post</button></form>"
                } else {
                    "<!doctype html><title>child</title><script>document.title=window.opener?'opener':'child'</script>"
                };
                let response = format!("HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                let _ = stream.write_all(response.as_bytes());
                let _ = send.send(request);
            }
        });
        Self {
            origin,
            stop,
            worker: Some(worker),
            requests,
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            worker.join().unwrap();
        }
    }
}

#[track_caller]
fn until(mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        while gtk::events_pending() {
            gtk::main_iteration_do(false);
        }
        if ready() {
            return;
        }
        assert!(Instant::now() < deadline, "native popup did not settle");
        std::thread::sleep(Duration::from_millis(2));
    }
}
fn has_view(id: ItemId) -> bool {
    let slot = Rc::new(std::cell::Cell::new(false));
    let result = slot.clone();
    assert!(super::super::try_with(
        move |host| result.set(host.views.contains_key(&id))
    ));
    slot.get()
}
fn native(id: ItemId) -> webkit2gtk::WebView {
    let slot = Rc::new(RefCell::new(None));
    let result = slot.clone();
    assert!(super::super::try_with(move |host| {
        *result.borrow_mut() = host.views.get(&id).map(|view| view.view.webview());
    }));
    slot.take().expect("owned native tab")
}
fn script(page: &webkit2gtk::WebView, source: &str) {
    page.evaluate_javascript(
        &format!("setTimeout(() => {{{source}}}, 1200)"),
        None,
        None,
        None::<&gtk::gio::Cancellable>,
        |_| {},
    );
}
fn click(page: &webkit2gtk::WebView, y: i32, button: u32, modifiers: gtk::gdk::ModifierType) {
    let window = page.window().unwrap();
    let pointer = window.display().default_seat().unwrap().pointer().unwrap();
    for (kind, signal) in [
        (gtk::gdk::EventType::ButtonPress, "button-press-event"),
        (gtk::gdk::EventType::ButtonRelease, "button-release-event"),
    ] {
        let mut event = gtk::gdk::Event::new(kind);
        event.set_device(Some(&pointer));
        event.set_source_device(Some(&pointer));
        let raw: *mut gtk::gdk::ffi::GdkEvent = event.to_glib_none_mut().0;
        // SAFETY: the live event owns this window reference; kind selects the button union.
        unsafe {
            (*raw).button.window = window.to_glib_full();
            static TIME: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1000);
            (*raw).button.time = TIME.fetch_add(20, Ordering::Relaxed);
            (*raw).button.x = 20.0;
            (*raw).button.y = f64::from(y);
            (*raw).button.button = button;
            (*raw).button.state = modifiers.bits();
            if kind == gtk::gdk::EventType::ButtonRelease {
                (*raw).button.state |= match button {
                    1 => gtk::gdk::ModifierType::BUTTON1_MASK.bits(),
                    2 => gtk::gdk::ModifierType::BUTTON2_MASK.bits(),
                    _ => 0,
                };
            }
        }
        page.emit_by_name::<bool>(signal, &[&event]);
    }
}

fn enter(page: &impl IsA<gtk::Widget>) {
    let window = page.window().unwrap();
    let keyboard = window.display().default_seat().unwrap().keyboard().unwrap();
    let keyval = 0xff0d;
    let keycode = gtk::gdk::Keymap::for_display(&window.display())
        .unwrap()
        .entries_for_keyval(keyval)[0]
        .keycode();
    for (kind, signal) in [
        (gtk::gdk::EventType::KeyPress, "key-press-event"),
        (gtk::gdk::EventType::KeyRelease, "key-release-event"),
    ] {
        let mut event = gtk::gdk::Event::new(kind);
        event.set_device(Some(&keyboard));
        event.set_source_device(Some(&keyboard));
        let raw: *mut gtk::gdk::ffi::GdkEvent = event.to_glib_none_mut().0;
        // SAFETY: the event owns the window reference; kind selects the key union.
        unsafe {
            (*raw).key.window = window.to_glib_full();
            (*raw).key.keyval = keyval;
            (*raw).key.hardware_keycode = u16::try_from(keycode).unwrap();
        }
        page.emit_by_name::<bool>(signal, &[&event]);
    }
}

struct PanicCleanup<'a>(&'a crate::WebviewEngine);
impl Drop for PanicCleanup<'_> {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            return;
        }
        let (send, done) = mpsc::channel();
        self.0.shutdown(Box::new(move |clean| {
            let _ = send.send(clean);
        }));
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline && done.try_recv().is_err() {
            gtk::main_iteration_do(false);
            std::thread::sleep(Duration::from_millis(2));
        }
        super::super::make_unavailable_for_test();
    }
}

#[test]
#[ignore = "requires isolated GTK display and admitted sandboxed WebKitGTK; localhost only"]
fn native_related_tabs_preserve_requests_and_enforce_popup_ownership() {
    let admission = crate::enforce_runtime_security_floor().unwrap();
    gtk::init().unwrap();
    let fixture = Fixture::new();
    let data = tempfile::tempdir().unwrap();
    let window = gtk::Window::new(gtk::WindowType::Toplevel);
    window.set_default_size(800, 600);
    let container = gtk::Fixed::new();
    window.add(&container);
    window.show_all();
    crate::install_container(container).unwrap();
    let (send, events) = mpsc::channel();
    let engine = crate::install(
        Arc::new(|job| {
            gtk::glib::idle_add_once(job);
            true
        }),
        data.path().to_owned(),
        admission,
        crate::InitialUserContent::new(
            UserContentGeneration::new(1).unwrap(),
            UserContent::default(),
        ),
        move |event| {
            send.send(event).unwrap();
        },
        |reason| panic!("native host sealed: {reason}"),
    )
    .unwrap();
    let _cleanup = PanicCleanup(&engine);
    let profile = ProfileId::generate();
    assert_eq!(
        engine.install_content_rules(
            profile,
            ContentPolicyGeneration::new(1).unwrap(),
            ContentRules::allow_all(ContentRuleDigest::from_bytes([1; 32]))
        ),
        NativeDispatch::Scheduled
    );
    let id = ItemId::generate();
    let window_id = 1;
    let bounds = Rect {
        x: 0.0,
        y: 0.0,
        width: 800.0,
        height: 600.0,
    };
    let url = format!("{}/page", fixture.origin);
    assert!(engine.create_view(id, Partition::Ephemeral(profile), &url, bounds));
    assert_eq!(
        engine.set_content(window_id, Some(Pane::leaf(id)), Some(bounds)),
        NativeDispatch::Scheduled
    );
    let mut opened = Vec::new();
    let mut closed = Vec::new();
    let drain = |opened: &mut Vec<(ItemId, bool)>, closed: &mut Vec<ItemId>| {
        for event in events.try_iter() {
            match event {
                EngineEvent::PresentationPending { id, navigation, .. }
                | EngineEvent::PresentationReady { id, navigation, .. } => {
                    assert_eq!(
                        engine.present_navigation(id, navigation),
                        NativeDispatch::Scheduled
                    );
                }
                EngineEvent::NativeTabOpened {
                    child,
                    foreground,
                    adoption,
                    ..
                } => {
                    adoption.finish(true);
                    opened.push((child, foreground));
                }
                EngineEvent::NativeTabCloseRequested { id } => {
                    closed.push(id);
                }
                EngineEvent::ViewCreationFailed { .. } => panic!("native tab creation failed"),
                _ => {}
            }
        }
    };
    until(|| {
        drain(&mut opened, &mut closed);
        has_view(id)
    });
    let page = native(id);
    page.connect_button_press_event(|_, event| {
        eprintln!(
            "popup fixture input: button={} position={:?}",
            event.button(),
            event.position()
        );
        gtk::glib::Propagation::Proceed
    });
    page.connect_create(|_, action| {
        eprintln!(
            "popup fixture denied create: gesture={} button={} modifiers={} type={:?} redirect={} method={:?}",
            action.is_user_gesture(),
            action.mouse_button(),
            action.modifiers(),
            action.navigation_type(),
            action.clone().is_redirect(),
            action.request().and_then(|r| webkit2gtk::URIRequestExt::http_method(&r))
        );
        None
    });
    until(|| {
        drain(&mut opened, &mut closed);
        page.title().as_deref() == Some("ready") && page.is_mapped()
    });
    window.present();
    window.window().unwrap().focus(0);
    until(|| window.is_active());
    assert!(page.context().unwrap().is_ephemeral());

    script(&page, "window.open('/denied');document.title='denied-done'");
    until(|| {
        drain(&mut opened, &mut closed);
        page.title().as_deref() == Some("denied-done")
    });
    assert!(opened.is_empty());

    for (y, button, modifiers, foreground) in [
        (20, 1, gtk::gdk::ModifierType::empty(), true),
        (80, 2, gtk::gdk::ModifierType::empty(), false),
        (80, 1, gtk::gdk::ModifierType::CONTROL_MASK, false),
        (
            80,
            1,
            gtk::gdk::ModifierType::CONTROL_MASK | gtk::gdk::ModifierType::SHIFT_MASK,
            true,
        ),
        (140, 1, gtk::gdk::ModifierType::empty(), true),
        (200, 1, gtk::gdk::ModifierType::empty(), true),
    ] {
        let count = opened.len();
        click(&page, y, button, modifiers);
        until(|| {
            drain(&mut opened, &mut closed);
            opened.len() == count + 1
        });
        let (child, actual_foreground) = opened[count];
        assert_eq!(actual_foreground, foreground);
        let child_page = native(child);
        until(|| {
            drain(&mut opened, &mut closed);
            child_page.title().is_some()
        });
        assert_eq!(child_page.context(), page.context());
        assert_eq!(page.uri().as_deref(), Some(url.as_str()));
        if y == 140 {
            assert_eq!(child_page.title().as_deref(), Some("opener"));
            script(&child_page, "window.close()");
            until(|| {
                drain(&mut opened, &mut closed);
                closed.contains(&child)
            });
            assert_eq!(engine.close(child), NativeDispatch::Scheduled);
        }
    }
    script(
        &page,
        "document.querySelector('#blank').focus();document.title=document.activeElement.id",
    );
    until(|| page.title().as_deref() == Some("blank"));
    let prior = opened.len();
    enter(&page);
    until(|| {
        drain(&mut opened, &mut closed);
        opened.len() == prior + 1
    });
    let menu_action = Rc::new(std::cell::Cell::new(None));
    let selected = menu_action.clone();
    page.connect_context_menu(move |_, menu, _, _| {
        assert!(!menu.items().iter().any(|item| {
            item.stock_action() == webkit2gtk::ContextMenuAction::OpenLinkInNewWindow
        }));
        selected.set(menu.items().iter().position(|item| {
            item.gaction()
                .is_some_and(|action| action.name() == "zephium-open-link-in-new-tab")
        }));
        false
    });
    click(&page, 80, 3, gtk::gdk::ModifierType::empty());
    until(|| menu_action.get().is_some() && gtk::grab_get_current().is_some());
    let prior = opened.len();
    let menu = gtk::grab_get_current()
        .unwrap()
        .downcast::<gtk::Menu>()
        .unwrap();
    menu.select_item(&menu.children()[menu_action.get().unwrap()]);
    enter(&menu);
    until(|| {
        drain(&mut opened, &mut closed);
        opened.len() == prior + 1
    });
    let menu_child = native(opened[prior].0);
    until(|| menu_child.title().as_deref() == Some("child"));
    let requests: Vec<_> = fixture.requests.try_iter().collect();
    assert!(requests
        .iter()
        .any(|request| request.starts_with("GET /link ")
            && !request
                .lines()
                .any(|line| line.to_ascii_lowercase().starts_with("referer:"))));
    assert!(requests
        .iter()
        .any(|request| request.starts_with("POST /post ")
            && request.ends_with("proof=preserved")
            && request
                .lines()
                .any(|line| line.eq_ignore_ascii_case(&format!("Referer: {url}")))));
    engine.set_popup_sites(
        profile,
        vec![PageOrigin::parse_exact(&fixture.origin).unwrap()],
    );
    until(|| {
        WebViewExt::settings(&page)
            .unwrap()
            .is_javascript_can_open_windows_automatically()
    });
    // The burst window elapsed while the trusted-input checks loaded children.
    std::thread::sleep(Duration::from_secs(1));
    let prior = opened.len();
    script(
        &page,
        "for(let i=0;i<12;i++) window.open('/burst');document.title='burst-done'",
    );
    until(|| {
        drain(&mut opened, &mut closed);
        page.title().as_deref() == Some("burst-done")
    });
    assert_eq!(opened.len() - prior, 8);
    assert_eq!(
        engine.set_content(window_id, None, None),
        NativeDispatch::Scheduled
    );
    until(|| !page.is_mapped());
    let prior = opened.len();
    script(&page, "window.open('/hidden');document.title='hidden-done'");
    until(|| {
        drain(&mut opened, &mut closed);
        page.title().as_deref() == Some("hidden-done")
    });
    assert_eq!(opened.len(), prior);
    for (child, _) in opened {
        if !closed.contains(&child) {
            assert_eq!(engine.close(child), NativeDispatch::Scheduled);
        }
    }
    assert_eq!(engine.close(id), NativeDispatch::Scheduled);
    until(|| !has_view(id));
    let (send, done) = mpsc::channel();
    engine.shutdown(Box::new(move |clean| send.send(clean).unwrap()));
    let mut settled = None;
    until(|| {
        settled = done.try_recv().ok();
        settled.is_some()
    });
    assert_eq!(settled, Some(true));
    super::super::make_unavailable_for_test();
    window.close();
}

#[test]
fn host_menu_admits_only_bounded_credential_free_web_urls() {
    for url in ["https://example.com/a", "http://localhost:8080/"] {
        assert!(super::linux_menu_url_allowed(url));
    }
    for url in [
        "javascript:alert(1)",
        "data:text/html,a",
        "file:///tmp/a",
        "https://user:password@example.com/",
        "not a URL",
    ] {
        assert!(!super::linux_menu_url_allowed(url));
    }
    assert!(!super::linux_menu_url_allowed(&format!(
        "https://example.com/{}",
        "a".repeat(16 * 1024)
    )));
}

#[test]
#[ignore = "requires isolated GTK display and admitted sandboxed WebKitGTK; localhost only"]
fn native_navigation_failure_keeps_first_tab_and_can_recover() {
    let admission = crate::enforce_runtime_security_floor().unwrap();
    gtk::init().unwrap();
    let fixture = Fixture::new();
    let data = tempfile::tempdir().unwrap();
    let window = gtk::Window::new(gtk::WindowType::Toplevel);
    let container = gtk::Fixed::new();
    window.add(&container);
    window.show_all();
    crate::install_container(container).unwrap();
    let (send, events) = mpsc::channel();
    let engine = crate::install(
        Arc::new(|job| {
            gtk::glib::idle_add_once(job);
            true
        }),
        data.path().to_owned(),
        admission,
        crate::InitialUserContent::new(
            UserContentGeneration::new(1).unwrap(),
            UserContent::default(),
        ),
        move |event| {
            let _ = send.send(event);
        },
        |reason| panic!("native host sealed: {reason}"),
    )
    .unwrap();
    let _cleanup = PanicCleanup(&engine);
    let profile = ProfileId::generate();
    assert_eq!(
        engine.install_content_rules(
            profile,
            ContentPolicyGeneration::new(1).unwrap(),
            ContentRules::allow_all(ContentRuleDigest::from_bytes([1; 32]))
        ),
        NativeDispatch::Scheduled
    );
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let unavailable = format!("http://{}/", listener.local_addr().unwrap());
    drop(listener);
    let id = ItemId::generate();
    assert!(engine.create_view(
        id,
        Partition::Ephemeral(profile),
        &unavailable,
        Rect::default()
    ));
    let mut reported = false;
    until(|| {
        for event in events.try_iter() {
            match event {
                EngineEvent::NavigationFailureReported {
                    id: failed,
                    url,
                    replaced_page,
                    ..
                } => {
                    assert_eq!(failed, id);
                    assert!(url.is_none());
                    assert!(!replaced_page);
                    reported = true;
                }
                EngineEvent::ViewCreationFailed { .. } => panic!("first load closed its tab"),
                _ => {}
            }
        }
        reported
    });
    assert!(has_view(id));
    let page = native(id);
    assert!(!page.is_mapped());
    page.load_uri(&format!("{}/page", fixture.origin));
    until(|| page.title().as_deref() == Some("ready"));
    assert!(has_view(id));
    let (send, done) = mpsc::channel();
    engine.shutdown(Box::new(move |clean| {
        let _ = send.send(clean);
    }));
    let mut settled = None;
    until(|| {
        settled = done.try_recv().ok();
        settled.is_some()
    });
    assert_eq!(settled, Some(true));
    super::super::make_unavailable_for_test();
    window.close();
}

struct NativeHarness {
    engine: crate::WebviewEngine,
    events: mpsc::Receiver<EngineEvent>,
    window: gtk::Window,
    profile: ProfileId,
    _data: tempfile::TempDir,
}
impl NativeHarness {
    fn new() -> Self {
        let admission = crate::enforce_runtime_security_floor().unwrap();
        gtk::init().unwrap();
        let data = tempfile::tempdir().unwrap();
        let window = gtk::Window::new(gtk::WindowType::Toplevel);
        window.set_default_size(800, 600);
        let container = gtk::Fixed::new();
        window.add(&container);
        window.show_all();
        crate::install_container(container).unwrap();
        let (send, events) = mpsc::channel();
        let engine = crate::install(
            Arc::new(|job| {
                gtk::glib::idle_add_once(job);
                true
            }),
            data.path().to_owned(),
            admission,
            crate::InitialUserContent::new(
                UserContentGeneration::new(1).unwrap(),
                UserContent::default(),
            ),
            move |event| {
                let _ = send.send(event);
            },
            |reason| panic!("native host sealed: {reason}"),
        )
        .unwrap();
        let profile = ProfileId::generate();
        assert_eq!(
            engine.install_content_rules(
                profile,
                ContentPolicyGeneration::new(1).unwrap(),
                ContentRules::allow_all(ContentRuleDigest::from_bytes([1; 32]))
            ),
            NativeDispatch::Scheduled
        );
        Self {
            engine,
            events,
            window,
            profile,
            _data: data,
        }
    }
    fn drain(&self) -> Vec<EngineEvent> {
        let events: Vec<_> = self.events.try_iter().collect();
        for event in &events {
            if let EngineEvent::PresentationPending { id, navigation, .. }
            | EngineEvent::PresentationReady { id, navigation, .. } = event
            {
                assert_eq!(
                    self.engine.present_navigation(*id, *navigation),
                    NativeDispatch::Scheduled
                );
            }
        }
        events
    }
    fn show(&self, url: &str) -> (ItemId, webkit2gtk::WebView) {
        let id = ItemId::generate();
        let bounds = Rect {
            x: 0.0,
            y: 0.0,
            width: 800.0,
            height: 600.0,
        };
        assert!(self
            .engine
            .create_view(id, Partition::Ephemeral(self.profile), url, bounds));
        assert_eq!(
            self.engine
                .set_content(1, Some(Pane::leaf(id)), Some(bounds)),
            NativeDispatch::Scheduled
        );
        until(|| {
            self.drain();
            has_view(id)
        });
        let page = native(id);
        until(|| {
            self.drain();
            page.is_mapped() && !page.is_loading()
        });
        self.window.present();
        self.window.window().unwrap().focus(0);
        until(|| self.window.is_active());
        (id, page)
    }
}
impl Drop for NativeHarness {
    fn drop(&mut self) {
        let (send, done) = mpsc::channel();
        self.engine.shutdown(Box::new(move |clean| {
            let _ = send.send(clean);
        }));
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut clean = None;
        while Instant::now() < deadline && clean.is_none() {
            gtk::main_iteration_do(false);
            clean = done.try_recv().ok();
            std::thread::sleep(Duration::from_millis(2));
        }
        super::super::make_unavailable_for_test();
        self.window.close();
        if !std::thread::panicking() {
            assert_eq!(clean, Some(true));
        }
    }
}

#[test]
#[ignore = "requires isolated GTK display and disposable XDG handler registry; localhost only"]
fn native_external_link_uses_shared_confirmation_with_unknown_source() {
    let data = std::path::PathBuf::from(std::env::var_os("XDG_DATA_HOME").unwrap());
    let applications = data.join("applications");
    std::fs::create_dir_all(&applications).unwrap();
    std::fs::write(applications.join("zephium-test-handler.desktop"),
        "[Desktop Entry]\nType=Application\nName=Fixture handler\nExec=/bin/true %u\nMimeType=x-scheme-handler/local-fixture;\nNoDisplay=true\n").unwrap();
    std::fs::write(
        applications.join("mimeapps.list"),
        "[Default Applications]\nx-scheme-handler/local-fixture=zephium-test-handler.desktop;\n",
    )
    .unwrap();
    let fixture = Fixture::new();
    let host = NativeHarness::new();
    let (id, page) = host.show(&format!("{}/page", fixture.origin));
    script(&page, "document.querySelector('#link').href='local-fixture:local-test';document.title='link-ready'");
    until(|| page.title().as_deref() == Some("link-ready"));
    click(&page, 80, 1, gtk::gdk::ModifierType::empty());
    let mut heard = false;
    until(|| {
        for event in host.drain() {
            if let EngineEvent::ExternalAppRequested {
                id: source,
                url,
                app,
                user_gesture,
                initiator,
            } = event
            {
                assert_eq!(source, id);
                assert_eq!(url, "local-fixture:local-test");
                assert_eq!(app.as_deref(), Some("Fixture handler"));
                assert!(user_gesture);
                assert!(matches!(
                    initiator,
                    zephium_core::navigation::AppLinkSource::Unknown
                ));
                heard = true;
            }
        }
        heard
    });
    assert_eq!(
        page.uri().as_deref(),
        Some(format!("{}/page", fixture.origin).as_str())
    );
    assert!(crate::platform::imp::open_external_app(
        "local-fixture:local-test"
    ));
    assert!(!crate::platform::imp::open_external_app(
        "javascript:alert(1)"
    ));
}

#[test]
#[ignore = "requires isolated GTK display; loopback fixture credentials only"]
fn native_http_auth_uses_shared_prompt_and_responder() {
    let fixture = Fixture::new();
    let host = NativeHarness::new();
    let (id, page) = host.show(&format!("{}/page", fixture.origin));
    page.load_uri(&format!("{}/auth", fixture.origin));
    let mut request = None;
    until(|| {
        for event in host.drain() {
            if let EngineEvent::HttpAuthRequested { id: owner, prompt } = event {
                assert_eq!(owner, id);
                assert!(prompt.insecure);
                assert!(!prompt.proxy);
                request = Some(prompt.request);
            }
        }
        request.is_some()
    });
    assert_eq!(
        host.engine.answer_http_auth(
            id,
            request.unwrap(),
            zephium_core::http_auth::HttpCredentials::new("fixture".into(), "fixture".into())
        ),
        NativeDispatch::Scheduled
    );
    until(|| {
        host.drain();
        page.title().as_deref() == Some("child") && !page.is_loading()
    });
    assert!(has_view(id));
}
