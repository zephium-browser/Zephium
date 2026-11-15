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
                if let Some(capability) = line
                    .split(" /permission-child?")
                    .nth(1)
                    .and_then(|s| s.split(' ').next())
                {
                    let body = format!(
                        "<!doctype html><body><script>{}</script>",
                        permission_fixture_script(capability)
                    );
                    let response = format!("HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                    let _ = stream.write_all(response.as_bytes());
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
        self.show_partition(Partition::Ephemeral(self.profile), url)
    }
    fn show_partition(&self, partition: Partition, url: &str) -> (ItemId, webkit2gtk::WebView) {
        let id = ItemId::generate();
        let bounds = Rect {
            x: 0.0,
            y: 0.0,
            width: 800.0,
            height: 600.0,
        };
        assert!(self.engine.create_view(id, partition, url, bounds));
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

#[test]
#[ignore = "requires isolated GTK display; localhost alert and confirm only"]
fn native_script_dialogs_settle_and_cancel_with_the_tab() {
    let fixture = Fixture::new();
    let host = NativeHarness::new();
    let (_, page) = host.show(&format!("{}/page", fixture.origin));
    script(&page, "alert('fixture');document.title='alert-returned'");
    let mut panel = None;
    until(|| {
        panel = gtk::Window::list_toplevels()
            .into_iter()
            .filter_map(|widget| widget.downcast::<gtk::Dialog>().ok())
            .find(|dialog| dialog.is_visible());
        panel.is_some()
    });
    let panel = panel.unwrap();
    assert_eq!(panel.transient_for().as_ref(), Some(&host.window));
    panel.response(gtk::ResponseType::Ok);
    until(|| page.title().as_deref() == Some("alert-returned"));
    host.window.window().unwrap().focus(0);
    until(|| host.window.is_active());
    script(
        &page,
        "document.title=confirm('fixture')?'accepted':'cancelled'",
    );
    let mut panel = None;
    until(|| {
        panel = gtk::Window::list_toplevels()
            .into_iter()
            .filter_map(|widget| widget.downcast::<gtk::Dialog>().ok())
            .find(|dialog| dialog.is_visible());
        panel.is_some()
    });
    panel.unwrap().response(gtk::ResponseType::Cancel);
    until(|| page.title().as_deref() == Some("cancelled"));
}

#[test]
#[ignore = "requires isolated GTK display; localhost element fullscreen only"]
fn native_element_fullscreen_uses_shared_owner_and_escape() {
    let fixture = Fixture::new();
    let host = NativeHarness::new();
    let (id, page) = host.show(&format!("{}/page", fixture.origin));
    script(&page, "document.querySelector('#script').onclick=()=>document.documentElement.requestFullscreen();document.title='fullscreen-ready'");
    until(|| page.title().as_deref() == Some("fullscreen-ready"));
    click(&page, 140, 1, gtk::gdk::ModifierType::empty());
    until(|| {
        host.drain().iter().any(|event| matches!(event, EngineEvent::FullscreenChanged { id: owner, active: true } if *owner == id))
    });
    assert_eq!(
        host.engine.fullscreen_presentation(),
        zephium_core::ports::engine::FullscreenPresentation::FillHostWindow
    );
    let window = host.window.window().unwrap();
    let keyboard = window.display().default_seat().unwrap().keyboard().unwrap();
    let mut event = gtk::gdk::Event::new(gtk::gdk::EventType::KeyPress);
    event.set_device(Some(&keyboard));
    let raw: *mut gtk::gdk::ffi::GdkEvent = event.to_glib_none_mut().0;
    // SAFETY: the event owns this window reference and uses the KeyPress union.
    unsafe {
        (*raw).key.window = window.to_glib_full();
        (*raw).key.keyval = 0xff1b;
    }
    assert!(host.window.emit_by_name::<bool>("event", &[&event]));
    until(|| {
        host.drain().iter().any(|event| matches!(event, EngineEvent::FullscreenChanged { id: owner, active: false } if *owner == id))
    });
}

#[test]
#[ignore = "requires isolated GTK display; localhost history only"]
fn native_history_lists_and_revalidates_jump_targets() {
    let fixture = Fixture::new();
    let host = NativeHarness::new();
    let (id, page) = host.show(&format!("{}/page", fixture.origin));
    for path in ["second", "third"] {
        let url = format!("{}/{path}", fixture.origin);
        page.load_uri(&url);
        until(|| {
            host.drain();
            page.uri().as_deref() == Some(url.as_str()) && !page.is_loading()
        });
    }
    let snapshot = Rc::new(RefCell::new(None));
    let result = snapshot.clone();
    assert!(super::super::try_with(move |host| {
        assert!(crate::platform::imp::history_session(
            &host.views[&id].view,
            |value| {
                *result.borrow_mut() = value;
            }
        ));
    }));
    let (entries, current) = snapshot.take().unwrap();
    assert_eq!(entries[current].1, format!("{}/third", fixture.origin));
    assert_eq!(entries[current - 1].1, format!("{}/second", fixture.origin));
    assert_eq!(entries[current - 2].1, format!("{}/page", fixture.origin));
    let second = entries[current - 1].1.clone();
    assert!(super::super::try_with(move |host| {
        let view = &host.views[&id].view;
        assert!(!crate::platform::imp::go_to_history_entry(
            view,
            -1,
            "https://wrong.invalid/"
        ));
        assert!(crate::platform::imp::go_to_history_entry(view, -1, &second));
    }));
    until(|| {
        host.drain();
        page.uri().as_deref() == Some(format!("{}/second", fixture.origin).as_str())
            && !page.is_loading()
    });
}

#[test]
#[ignore = "requires isolated GTK display; localhost muted audio only"]
fn native_audio_mute_preserves_person_and_focus_intent() {
    let fixture = Fixture::new();
    let host = NativeHarness::new();
    let (id, page) = host.show(&format!("{}/page", fixture.origin));
    assert!(crate::page_mute_supported());
    assert_eq!(host.engine.set_muted(id, true), NativeDispatch::Scheduled);
    until(|| page.is_muted());
    host.engine.set_media_suspended(id, true);
    assert_eq!(host.engine.set_muted(id, false), NativeDispatch::Scheduled);
    // A host barrier proves both queued mute intents settled.
    let settled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let done = settled.clone();
    assert!(host
        .engine
        .run(move || done.store(true, std::sync::atomic::Ordering::Release)));
    until(|| settled.load(std::sync::atomic::Ordering::Acquire));
    assert!(page.is_muted());
    host.engine.set_media_suspended(id, false);
    until(|| !page.is_muted());
    assert_eq!(host.engine.set_muted(id, true), NativeDispatch::Scheduled);
    until(|| page.is_muted());
    script(&page, "document.querySelector('#script').onclick=()=>{window.audio=new AudioContext();window.tone=audio.createOscillator();tone.connect(audio.destination);tone.start();document.title='audio-started'};document.title='audio-ready'");
    until(|| page.title().as_deref() == Some("audio-ready"));
    click(&page, 140, 1, gtk::gdk::ModifierType::empty());
    until(|| {
        host.drain().iter().any(|event| matches!(event, EngineEvent::AudioChanged { id: owner, audible: true } if *owner == id))
    });
    script(&page, "tone.stop();audio.close()");
    until(|| {
        host.drain().iter().any(|event| matches!(event, EngineEvent::AudioChanged { id: owner, audible: false } if *owner == id))
    });
    page.load_uri(&format!("{}/second", fixture.origin));
    until(|| {
        host.drain();
        !page.is_loading()
    });
    assert!(page.is_muted());
}

#[test]
fn context_menu_policy_refuses_unowned_actions_and_unsafe_urls() {
    use webkit2gtk::ContextMenuAction as A;
    for action in [
        A::Custom,
        A::DownloadLinkToDisk,
        A::DownloadImageToDisk,
        A::DownloadVideoToDisk,
        A::DownloadAudioToDisk,
        A::OpenLinkInNewWindow,
        A::OpenImageInNewWindow,
        A::OpenFrameInNewWindow,
        A::OpenVideoInNewWindow,
        A::OpenAudioInNewWindow,
        A::__Unknown(123456),
    ] {
        assert!(!super::linux_stock_action_allowed(action, true));
    }
    assert!(super::linux_stock_action_allowed(
        A::CopyLinkToClipboard,
        false
    ));
    assert!(!super::linux_stock_action_allowed(A::InspectElement, false));
    assert!(super::linux_stock_action_allowed(A::InspectElement, true));
    for url in [
        "javascript:alert(1)",
        "file:///tmp/file",
        "data:text/plain,x",
        "https://user:password@example.com/",
    ] {
        assert!(!super::linux_menu_url_allowed(url));
    }
    assert!(!super::linux_menu_url_allowed(&format!(
        "https://example.com/{}",
        "x".repeat(16384)
    )));
}

#[test]
#[ignore = "requires isolated GTK display; localhost native context menu only"]
fn native_context_menu_filters_unowned_actions() {
    use webkit2gtk::{ContextMenuExt, ContextMenuItemExt, SettingsExt};
    let fixture = Fixture::new();
    let host = NativeHarness::new();
    let (_, page) = host.show(&format!("{}/page", fixture.origin));
    webkit2gtk::WebViewExt::settings(&page)
        .unwrap()
        .set_enable_developer_extras(false);
    let observed = Rc::new(RefCell::new(None));
    let delivered = observed.clone();
    page.connect_context_menu(move |_, menu, _, _| {
        *delivered.borrow_mut() = Some(
            menu.items()
                .iter()
                .map(|item| {
                    (
                        item.stock_action(),
                        item.is_separator(),
                        item.gaction().map(|action| action.name().to_string()),
                    )
                })
                .collect::<Vec<_>>(),
        );
        true
    });
    click(&page, 80, 3, gtk::gdk::ModifierType::empty());
    until(|| observed.borrow().is_some());
    let items = observed.take().unwrap();
    assert!(items
        .iter()
        .any(|item| item.0 == webkit2gtk::ContextMenuAction::CopyLinkToClipboard));
    assert!(items
        .iter()
        .any(|item| item.2.as_deref() == Some("zephium-open-link-in-new-tab")));
    for (action, separator, _) in items {
        if action != webkit2gtk::ContextMenuAction::Custom && !separator {
            assert!(super::linux_stock_action_allowed(action, false));
        }
    }
}

#[test]
#[ignore = "requires isolated GTK display; loopback proxy only"]
fn native_proxy_applies_to_new_and_existing_profile_managers() {
    let proxy = Fixture::new();
    let direct = Fixture::new();
    let host = NativeHarness::new();
    host.engine.set_proxy(zephium_core::proxy::from_settings(
        "http",
        proxy.origin.strip_prefix("http://").unwrap(),
    ));
    let target = format!("{}/through-proxy", direct.origin);
    let (_, page) = host.show(&target);
    until(|| {
        host.drain();
        page.title().as_deref() == Some("child")
    });
    let mut seen = false;
    until(|| {
        seen |= proxy
            .requests
            .try_iter()
            .any(|request| request.starts_with(&format!("GET {target} ")));
        seen
    });
    host.engine.set_proxy(None);
    let settled = Arc::new(AtomicBool::new(false));
    let done = settled.clone();
    assert!(host.engine.run(move || done.store(true, Ordering::Release)));
    until(|| settled.load(Ordering::Acquire));
    page.load_uri(&format!("{}/page", direct.origin));
    until(|| {
        host.drain();
        page.title().as_deref() == Some("ready") && !page.is_loading()
    });
    assert!(direct
        .requests
        .try_iter()
        .any(|request| request.starts_with("GET /page ")));
}

fn data_call(
    host: &NativeHarness,
    partition: Partition,
    call: zephium_core::browsing_data::EngineDataCall,
) -> zephium_core::browsing_data::EngineDataReply {
    let (send, receive) = mpsc::channel();
    host.engine.browsing_data_call(
        partition,
        call,
        zephium_core::browsing_data::EngineDataCompletion::new(move |reply| {
            let _ = send.send(reply);
        }),
    );
    let mut reply = None;
    until(|| {
        host.drain();
        reply = receive.try_recv().ok();
        reply.is_some()
    });
    reply.unwrap()
}

#[test]
#[ignore = "requires isolated GTK display; disposable persistent profiles and localhost data only"]
fn native_site_data_clearing_preserves_other_profiles() {
    use webkit2gtk::WebsiteDataManagerExt;
    use zephium_core::browsing_data::{
        EngineDataCall as Call, EngineDataFailure as Failure, EngineDataKinds,
        EngineDataReply as Reply,
    };
    let fixture = Fixture::new();
    let host = NativeHarness::new();
    let first = Partition::Persistent(host.profile);
    let (_, page) = host.show_partition(first, &format!("{}/page", fixture.origin));
    let manager = page.website_data_manager().unwrap();
    assert!(manager.is_itp_enabled());
    assert!(!manager.is_persistent_credential_storage_enabled());
    script(&page, "localStorage.setItem('proof','first');document.cookie='proof=first; SameSite=Lax';document.title='stored'");
    until(|| page.title().as_deref() == Some("stored"));
    let other = ProfileId::generate();
    assert_eq!(
        host.engine.install_content_rules(
            other,
            ContentPolicyGeneration::new(1).unwrap(),
            ContentRules::allow_all(ContentRuleDigest::from_bytes([1; 32]))
        ),
        NativeDispatch::Scheduled
    );
    let (_, second) = host.show_partition(
        Partition::Persistent(other),
        &format!("{}/page", fixture.origin),
    );
    script(&second, "localStorage.setItem('proof','second');document.cookie='proof=second; SameSite=Lax';document.title='stored'");
    until(|| second.title().as_deref() == Some("stored"));
    let Reply::Sites { sites, partial } = data_call(&host, first, Call::Sites) else {
        panic!("expected native sites");
    };
    assert!(partial, "GTK cannot enumerate file-system-only sites");
    assert!(sites
        .iter()
        .any(|site| site.name == "127.0.0.1" && site.cookies && site.storage));
    assert_eq!(
        data_call(&host, Partition::Ephemeral(host.profile), Call::Sites),
        Reply::Failed(Failure::Unsupported)
    );
    assert_eq!(
        data_call(
            &host,
            first,
            Call::Clear {
                kinds: EngineDataKinds {
                    site_data: true,
                    ..Default::default()
                },
                since: Some(1)
            }
        ),
        Reply::Failed(Failure::Unsupported)
    );
    assert_eq!(
        data_call(
            &host,
            first,
            Call::ForgetSite {
                site: "127.0.0.1".into(),
                origins: vec![]
            }
        ),
        Reply::PartlyCleared
    );
    script(&page, "document.title=(localStorage.getItem('proof')===null && !document.cookie.includes('proof='))?'cleared':'retained'");
    until(|| page.title().as_deref() == Some("cleared"));
    script(&second, "document.title=(localStorage.getItem('proof')==='second' && document.cookie.includes('proof=second'))?'isolated':'lost'");
    until(|| second.title().as_deref() == Some("isolated"));
}

struct DownloadFixtureStore;
impl zephium_core::ports::store::Store for DownloadFixtureStore {
    fn save_session(&self, _: zephium_core::session::SessionState) {}
    fn flush(&self) -> bool {
        true
    }
    fn load_session(&self) -> zephium_core::ports::store::SessionLoad {
        zephium_core::ports::store::SessionLoad::Absent
    }
    fn record_visit(&self, _: ProfileId, _: String, _: String) {}
    fn app_setting(&self, _: &str) -> Option<String> {
        None
    }
    fn set_app_setting(&self, _: String, _: String) -> bool {
        false
    }
    fn search_history(
        &self,
        _: ProfileId,
        _: &str,
        _: u32,
    ) -> Vec<zephium_core::ports::store::HistoryHit> {
        vec![]
    }
    fn history_page(
        &self,
        _: ProfileId,
        _: &str,
        _: Option<i64>,
        _: Option<i64>,
        _: u32,
    ) -> Vec<zephium_core::ports::store::HistoryVisit> {
        vec![]
    }
    fn forget_history_urls(&self, _: ProfileId, _: &[String]) -> u32 {
        0
    }
    fn clear_history(&self, _: ProfileId, _: Option<i64>) -> u32 {
        0
    }
    fn amend_visit_title(&self, _: ProfileId, _: String, _: String) -> bool {
        false
    }
    fn favicon_age(&self, _: ProfileId, _: &str) -> Option<i64> {
        None
    }
    fn save_favicon(&self, _: ProfileId, _: String, _: Option<String>, _: Vec<u8>) {}
    fn favicon_bytes(&self, _: ProfileId, _: &str) -> Option<(Option<String>, Vec<u8>)> {
        None
    }
    fn download_recovery_profiles(
        &self,
        done: Box<dyn FnOnce(zephium_core::downloads::DownloadStoreReply) + Send>,
    ) -> bool {
        done(zephium_core::downloads::DownloadStoreReply::Profiles(
            vec![],
        ));
        true
    }
    fn download_call(
        &self,
        _: ProfileId,
        _: zephium_core::downloads::DownloadStoreCall,
        _: Box<dyn FnOnce(zephium_core::downloads::DownloadStoreReply) + Send>,
    ) -> bool {
        panic!("private download history must not reach the durable store")
    }
}

#[test]
#[ignore = "requires isolated GTK/WebKitGTK host"]
fn native_download_uses_shared_confirmation_and_secure_publication() {
    use std::os::unix::fs::PermissionsExt;
    use zephium_core::downloads::{DownloadCall, DownloadCompletion, DownloadResponse};
    let host = NativeHarness::new();
    let fixture = Fixture::new();
    assert!(host
        .engine
        .initialize_downloads(Arc::new(DownloadFixtureStore), |_| {}));
    let (id, page) = host.show(&format!("{}/page", fixture.origin));
    let output = tempfile::tempdir().unwrap();
    let directory = output.path().to_owned();
    let profile = host.profile;
    assert!(super::super::try_with(move |host| {
        host.downloads
            .as_ref()
            .unwrap()
            .test_directory(profile, directory);
    }));
    script(&page, "document.querySelector('#script').onclick=()=>{const a=document.createElement('a');a.href=URL.createObjectURL(new Blob(['download fixture'],{type:'text/plain'}));a.download='fixture.txt';document.body.append(a);a.click()};document.title='download-ready'");
    until(|| page.title().as_deref() == Some("download-ready"));
    click(&page, 140, 1, gtk::gdk::ModifierType::empty());
    let mut asked = None;
    until(|| {
        for event in host.drain() {
            if let EngineEvent::DownloadPermissionRequested { id: owner, prompt } = event {
                assert_eq!(owner, id);
                asked = Some(prompt.request);
            }
        }
        asked.is_some()
    });
    assert_eq!(
        host.engine
            .answer_download_permission(id, asked.unwrap(), true),
        NativeDispatch::Scheduled
    );
    until(|| output.path().join("fixture.txt").exists());
    let path = output.path().join("fixture.txt");
    assert_eq!(std::fs::read(&path).unwrap(), b"download fixture");
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o111,
        0
    );
    let file = std::fs::File::open(&path).unwrap();
    use std::os::fd::AsRawFd;
    assert_eq!(
        unsafe {
            libc::fgetxattr(
                file.as_raw_fd(),
                c"user.xdg.origin.url".as_ptr(),
                std::ptr::null_mut(),
                0,
            )
        },
        -1
    );
    let (send, response) = mpsc::channel();
    assert!(host.engine.download_call(
        Partition::Ephemeral(host.profile),
        DownloadCall::Updates,
        DownloadCompletion::new(Box::new(move |result| {
            send.send(result).unwrap();
        }))
    ));
    let mut result = None;
    until(|| {
        result = response.try_recv().ok();
        result.is_some()
    });
    assert!(matches!(result, Some(DownloadResponse::Updates { entries, .. }) if entries.len()==1));
}

struct RawNativeHarness {
    _data: tempfile::TempDir,
    page: webkit2gtk::WebView,
    window: gtk::Window,
}
impl RawNativeHarness {
    fn new() -> Self {
        Self::create(true)
    }
    fn create(private: bool) -> Self {
        crate::enforce_runtime_security_floor().unwrap();
        gtk::init().unwrap();
        let data = tempfile::tempdir().unwrap();
        let manager = if private {
            webkit2gtk::WebsiteDataManager::new_ephemeral()
        } else {
            webkit2gtk::WebsiteDataManager::builder()
                .base_data_directory(data.path().to_str().unwrap())
                .base_cache_directory(data.path().to_str().unwrap())
                .build()
        };
        let context = webkit2gtk::WebContext::builder()
            .website_data_manager(&manager)
            .process_swap_on_cross_site_navigation_enabled(true)
            .build();
        context.set_sandbox_enabled(true);
        assert!(context.is_sandbox_enabled());
        let page = webkit2gtk::WebView::with_context(&context);
        let settings = webkit2gtk::WebViewExt::settings(&page).unwrap();
        settings.set_enable_media_stream(true);
        settings.set_enable_mock_capture_devices(true);
        let window = gtk::Window::new(gtk::WindowType::Toplevel);
        window.set_default_size(800, 600);
        window.add(&page);
        window.show_all();
        window.present();
        window.window().unwrap().focus(0);
        until(|| window.is_active());
        Self {
            page,
            window,
            _data: data,
        }
    }
}
impl Drop for RawNativeHarness {
    fn drop(&mut self) {
        self.page.stop_loading();
        self.window.close();
    }
}

#[test]
#[ignore = "isolated GTK; verifies missing directory-request metadata without disclosing files"]
fn native_file_and_directory_requests_have_indistinguishable_public_metadata() {
    use webkit2gtk::FileChooserRequestExt;
    let host = RawNativeHarness::new();
    let fixture = Fixture::new();
    let requests = Rc::new(RefCell::new(Vec::new()));
    let observed = requests.clone();
    host.page.connect_run_file_chooser(move |_, request| {
        observed.borrow_mut().push((
            request.selects_multiple(),
            request.mime_types(),
            request.selected_files(),
        ));
        request.cancel();
        true
    });
    host.page.load_uri(&format!("{}/page", fixture.origin));
    until(|| host.page.title().as_deref() == Some("ready"));
    script(&host.page, "document.body.innerHTML='<input type=file accept=\"text/plain\" style=\"position:absolute;top:0;left:0;width:200px;height:40px\"><input type=file webkitdirectory accept=\"text/plain\" style=\"position:absolute;top:60px;left:0;width:200px;height:40px\">';document.title='inputs-ready'");
    until(|| host.page.title().as_deref() == Some("inputs-ready"));
    click(&host.page, 20, 1, gtk::gdk::ModifierType::empty());
    until(|| requests.borrow().len() == 1);
    click(&host.page, 80, 1, gtk::gdk::ModifierType::empty());
    until(|| requests.borrow().len() == 2);
    assert_eq!(requests.borrow()[0], requests.borrow()[1]);
}

#[test]
#[ignore = "isolated GTK; real WebKit Permissions Policy, mock capture, no device grants"]
fn native_permission_policy_requires_explicit_cross_origin_delegation() {
    use webkit2gtk::{PermissionRequestExt, UserMediaPermissionRequestExt};
    let first = Fixture::new();
    let second = Fixture::new();
    let mut failures = Vec::new();
    for capability in ["camera", "microphone", "geolocation", "notifications"] {
        for frame in [
            "cross-deny",
            "cross-allow",
            "same",
            "srcdoc",
            "blank",
            "top",
        ] {
            let host = RawNativeHarness::create(capability != "notifications");
            let count = Rc::new(std::cell::Cell::new(0));
            let seen = count.clone();
            host.page.connect_permission_request(move |_, request| {
                let relevant = match capability {
                    "camera" => request
                        .downcast_ref::<webkit2gtk::UserMediaPermissionRequest>()
                        .is_some_and(|r| r.is_for_video_device()),
                    "microphone" => request
                        .downcast_ref::<webkit2gtk::UserMediaPermissionRequest>()
                        .is_some_and(|r| r.is_for_audio_device()),
                    "geolocation" => request.is::<webkit2gtk::GeolocationPermissionRequest>(),
                    "notifications" => request.is::<webkit2gtk::NotificationPermissionRequest>(),
                    _ => false,
                };
                if relevant {
                    seen.set(seen.get() + 1);
                }
                request.deny();
                true
            });
            host.page.load_uri(&format!("{}/page", first.origin));
            until(|| host.page.title().as_deref() == Some("ready"));
            let child = permission_fixture_script(capability);
            let source = match frame {
                "cross-deny" | "cross-allow" | "same" => {
                    let origin = if frame=="same" { &first.origin } else { &second.origin };
                    let allow = if frame=="cross-allow" { "camera; microphone; geolocation" } else { "" };
                    format!("const f=document.createElement('iframe');f.allow={};f.src={};document.body.append(f);", serde_json::to_string(allow).unwrap(), serde_json::to_string(&format!("{origin}/permission-child?{capability}")).unwrap())
                },
                "srcdoc" => format!("const f=document.createElement('iframe');f.srcdoc={};document.body.append(f);", serde_json::to_string(&format!("<body><script>{child}</script>")).unwrap()),
                "top" => child.clone(),
                "blank" => format!("const f=document.createElement('iframe');document.body.append(f);f.contentDocument.write({});f.contentDocument.close();", serde_json::to_string(&format!("<body><script>{child}</script>")).unwrap()),
                _ => unreachable!(),
            };
            let finished = Rc::new(std::cell::Cell::new(false));
            let done = finished.clone();
            host.page.evaluate_javascript(&format!("window.addEventListener('message',e=>{{if(e.data==='permission-failed'||e.data==='notification-ready')document.title=e.data}});{source};document.querySelector('iframe')?.setAttribute('style','position:fixed;left:0;top:0;width:300px;height:100px;z-index:999999')"), None,None,None::<&gtk::gio::Cancellable>,move |result| { result.unwrap(); done.set(true); });
            if capability == "notifications" {
                until(|| {
                    finished.get() && host.page.title().as_deref() == Some("notification-ready")
                });
                click(&host.page, 20, 1, gtk::gdk::ModifierType::empty());
            }
            until(|| finished.get() && host.page.title().as_deref() == Some("permission-failed"));
            let actual = count.get();
            let expected = if frame == "cross-deny"
                || (capability == "notifications" && frame == "cross-allow")
            {
                0
            } else {
                1
            };
            eprintln!(
                "policy {capability} {frame}: signals={actual}, page=refused, expected={expected}"
            );
            if actual != expected {
                failures.push(format!(
                    "{capability}/{frame}: {actual}, expected {expected}"
                ));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "native policy qualification failed: {failures:?}"
    );
}

fn permission_fixture_script(capability: &str) -> String {
    let operation = match capability {
        "camera" => "navigator.mediaDevices.getUserMedia({video:true}).then(s=>{s.getTracks().forEach(t=>t.stop());parent.postMessage('unexpected-grant','*')},fail)",
        "microphone" => "navigator.mediaDevices.getUserMedia({audio:true}).then(s=>{s.getTracks().forEach(t=>t.stop());parent.postMessage('unexpected-grant','*')},fail)",
        "geolocation" => "navigator.geolocation.getCurrentPosition(()=>parent.postMessage('unexpected-grant','*'),fail,{timeout:3000})",
        "notifications" => "const b=document.createElement('button');b.style='position:fixed;top:0;left:0;width:200px;height:40px';b.textContent='Request';b.onclick=()=>Notification.requestPermission().then(fail,fail);document.body.append(b);parent.postMessage('notification-ready','*')",
        _ => "fail()",
    };
    format!("const fail=()=>parent.postMessage('permission-failed','*');try{{{operation}}}catch(e){{fail()}}")
}

#[test]
#[ignore = "isolated GTK; shared consent, mock camera, capture observation and native stop"]
fn native_permissions_use_shared_consent_and_revoke_capture() {
    use zephium_core::permissions::{
        PagePermissionKind, PagePermissionRequestKind, PagePermissionRequestSettlement,
    };
    let host = NativeHarness::new();
    let fixture = Fixture::new();
    let (id, page) = host.show(&format!("{}/page", fixture.origin));
    let settings = webkit2gtk::WebViewExt::settings(&page).unwrap();
    settings.set_enable_media_stream(true);
    settings.set_enable_mock_capture_devices(true);
    script(&page, "navigator.mediaDevices.getUserMedia({video:true}).then(s=>{window.stream=s;s.getTracks()[0].onended=()=>document.title='stopped';document.title='capturing'},e=>document.title='denied')");
    let mut request = None;
    until(|| {
        for event in host.drain() {
            if let EngineEvent::PermissionRequested {
                id: owner,
                request: r,
                ..
            } = event
            {
                assert_eq!(owner, id);
                request = Some(r);
            }
        }
        request.is_some()
    });
    let request = request.unwrap();
    assert_eq!(request.origin.as_str(), fixture.origin);
    assert_eq!(
        request.kind,
        PagePermissionRequestKind::Single(PagePermissionKind::Camera)
    );
    assert_eq!(
        host.engine.settle_page_permission_request(
            host.profile,
            id,
            request.id,
            PagePermissionRequestSettlement::Allow
        ),
        NativeDispatch::Scheduled
    );
    let mut capture = None;
    until(|| {
        for event in host.drain() {
            if let EngineEvent::MediaCaptureChanged {
                id: owner,
                navigation,
                state,
            } = event
            {
                if owner == id && state.is_capturing() {
                    capture = Some(navigation);
                }
            }
        }
        capture.is_some() && page.title().as_deref() == Some("capturing")
    });
    assert_eq!(
        host.engine.stop_media_capture(id, capture.unwrap()),
        NativeDispatch::Scheduled
    );
    until(|| {
        page.title().as_deref() == Some("stopped")
            && page.camera_capture_state() == webkit2gtk::MediaCaptureState::None
    });
    assert_eq!(
        page.camera_capture_state(),
        webkit2gtk::MediaCaptureState::None
    );
    page.load_uri(&format!("{}/again", fixture.origin));
    until(|| {
        host.drain();
        !page.is_loading() && page.is_mapped()
    });
    script(&page, "navigator.mediaDevices.getUserMedia({video:true}).then(s=>{s.getTracks().forEach(t=>t.stop());document.title='unexpected-cached-grant'},()=>document.title='denied')");
    let mut second = None;
    until(|| {
        for event in host.drain() {
            if let EngineEvent::PermissionRequested { request, .. } = event {
                second = Some(request.id);
            }
        }
        second.is_some() || page.title().as_deref() == Some("unexpected-cached-grant")
    });
    assert!(
        second.is_some(),
        "native permission cache bypassed the next document's broker"
    );
    page.load_uri(&format!("{}/next", fixture.origin));
    until(|| {
        host.drain();
        !page.is_loading()
    });
    assert_eq!(
        host.engine.settle_page_permission_request(
            host.profile,
            id,
            second.unwrap(),
            PagePermissionRequestSettlement::Allow
        ),
        NativeDispatch::Scheduled
    );
    assert_eq!(
        page.camera_capture_state(),
        webkit2gtk::MediaCaptureState::None
    );
}

#[test]
#[ignore = "isolated GTK; display-capture activation policy with denied mock requests"]
fn native_display_capture_requires_activation_before_permission() {
    use webkit2gtk::PermissionRequestExt;
    let host = RawNativeHarness::new();
    let fixture = Fixture::new();
    let requests = Rc::new(std::cell::Cell::new(0));
    let observed = requests.clone();
    host.page.connect_permission_request(move |_, request| {
        observed.set(observed.get() + 1);
        request.deny();
        true
    });
    host.page.load_uri(&format!("{}/page", fixture.origin));
    until(|| host.page.title().as_deref() == Some("ready"));
    script(&host.page,"navigator.mediaDevices.getDisplayMedia({video:true}).then(s=>{s.getTracks().forEach(t=>t.stop());document.title='unexpected'},e=>document.title=e.name)");
    until(|| host.page.title().as_deref() != Some("ready"));
    assert_eq!(host.page.title().as_deref(), Some("InvalidStateError"));
    assert_eq!(requests.get(), 0);
    script(&host.page,"document.querySelector('#script').onclick=()=>navigator.mediaDevices.getDisplayMedia({video:true}).then(s=>{s.getTracks().forEach(t=>t.stop());document.title='unexpected'},e=>document.title='display-failed');document.title='display-ready'");
    until(|| host.page.title().as_deref() == Some("display-ready"));
    click(&host.page, 140, 1, gtk::gdk::ModifierType::empty());
    until(|| host.page.title().as_deref() == Some("display-failed"));
    assert_eq!(requests.get(), 1, "no positive native activation control");
}

#[test]
#[ignore = "isolated GTK and private D-Bus without a desktop portal; no real capture"]
fn native_display_capture_fails_cleanly_without_portal() {
    let host = NativeHarness::new();
    let fixture = Fixture::new();
    let (_, page) = host.show(&format!("{}/page", fixture.origin));
    script(&page, "document.querySelector('#script').onclick=()=>navigator.mediaDevices.getDisplayMedia({video:true}).then(s=>{s.getTracks().forEach(t=>t.stop());document.title='unexpected'},e=>document.title='display-failed');document.title='display-ready'");
    until(|| page.title().as_deref() == Some("display-ready"));
    click(&page, 140, 1, gtk::gdk::ModifierType::empty());
    until(|| page.title().as_deref() == Some("display-failed"));
    assert!(!host
        .drain()
        .iter()
        .any(|event| matches!(event, EngineEvent::PermissionRequested { .. })));
    assert_eq!(
        page.camera_capture_state(),
        webkit2gtk::MediaCaptureState::None
    );
}

fn visible_dialog() -> gtk::Dialog {
    let mut panel = None;
    until(|| {
        panel = gtk::Window::list_toplevels()
            .into_iter()
            .filter_map(|widget| widget.downcast::<gtk::Dialog>().ok())
            .find(|dialog| dialog.is_visible());
        panel.is_some()
    });
    panel.unwrap()
}

#[test]
#[ignore = "isolated GTK; localhost prompt input and cancellation"]
fn native_prompt_returns_chosen_text_and_null_on_cancel() {
    let fixture = Fixture::new();
    let host = NativeHarness::new();
    let (_, page) = host.show(&format!("{}/page", fixture.origin));
    script(&page, "document.title=String(prompt('fixture','initial'))");
    let panel = visible_dialog();
    let entry = panel
        .content_area()
        .children()
        .into_iter()
        .find_map(|widget| widget.downcast::<gtk::Entry>().ok())
        .unwrap();
    assert_eq!(entry.text(), "initial");
    entry.set_text("chosen <literal>");
    panel.response(gtk::ResponseType::Ok);
    until(|| page.title().as_deref() == Some("chosen <literal>"));
    host.window.window().unwrap().focus(0);
    until(|| host.window.is_active());
    script(
        &page,
        "document.title=String(prompt('fixture','not-returned'))",
    );
    visible_dialog().response(gtk::ResponseType::Cancel);
    until(|| page.title().as_deref() == Some("null"));
}

#[test]
#[ignore = "isolated GTK; trusted activation and beforeunload cancel/leave"]
fn native_beforeunload_can_keep_or_leave_the_document() {
    let fixture = Fixture::new();
    let host = NativeHarness::new();
    let (_, page) = host.show(&format!("{}/page", fixture.origin));
    script(&page, "document.querySelector('#script').onclick=()=>{window.onbeforeunload=e=>{e.preventDefault();e.returnValue='fixture';return 'fixture'};document.title='armed'};document.title='activation-ready'");
    until(|| page.title().as_deref() == Some("activation-ready"));
    click(&page, 140, 1, gtk::gdk::ModifierType::empty());
    until(|| page.title().as_deref() == Some("armed"));
    page.load_uri(&format!("{}/leave", fixture.origin));
    visible_dialog().response(gtk::ResponseType::Cancel);
    script(
        &page,
        "document.title=window.onbeforeunload && location.pathname==='/page'?'kept':'lost-document'",
    );
    until(|| page.title().as_deref() == Some("kept"));
    host.window.window().unwrap().focus(0);
    until(|| host.window.is_active());
    page.load_uri(&format!("{}/leave", fixture.origin));
    visible_dialog().response(gtk::ResponseType::Ok);
    until(|| !page.is_loading() && page.title().as_deref() == Some("child"));
}
