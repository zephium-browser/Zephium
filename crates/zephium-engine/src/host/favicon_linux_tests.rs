use super::supported::{rasterize, watch};
use gtk::prelude::*;
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    time::{Duration, Instant},
};
use webkit2gtk::WebViewExt;
use wry::{WebViewBuilder, WebViewBuilderExtUnix, WebViewExtUnix};

struct Fixture {
    address: String,
    stop: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl Fixture {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let worker = std::thread::spawn(move || {
            while !stopping.load(Ordering::Relaxed) {
                let (mut stream, _) = match listener.accept() {
                    Ok(connection) => connection,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    Err(error) => panic!("local favicon fixture failed: {error}"),
                };
                stream
                    .set_read_timeout(Some(Duration::from_secs(1)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_secs(1)))
                    .unwrap();
                let mut request = Vec::new();
                let mut bytes = [0; 1024];
                while request.len() < 8192 && !request.windows(4).any(|end| end == b"\r\n\r\n") {
                    match stream.read(&mut bytes) {
                        Ok(0) | Err(_) => break,
                        Ok(count) => request.extend_from_slice(&bytes[..count]),
                    }
                }
                let request = String::from_utf8_lossy(&request);
                let path = request.split_ascii_whitespace().nth(1).unwrap_or("");
                let (kind, body) = match path {
                    "/icon.svg" | "/updated.svg" => (
                        "image/svg+xml",
                        "<svg xmlns='http://www.w3.org/2000/svg' width='32' height='16'><path fill='#20a040' d='M0 0h32v16H0z'/></svg>",
                    ),
                    "/next" => ("text/html", "<!doctype html><title>next</title>"),
                    _ => (
                        "text/html",
                        "<!doctype html><title>icons</title><link rel='icon' href='/icon.svg' type='image/svg+xml' sizes='any'>",
                    ),
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nCache-Control: no-store\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
            }
        });
        Self {
            address,
            stop,
            worker: Some(worker),
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

fn receive<T>(receiver: &mpsc::Receiver<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        while gtk::events_pending() {
            gtk::main_iteration_do(false);
        }
        match receiver.try_recv() {
            Ok(result) => return result,
            Err(mpsc::TryRecvError::Empty) => {}
            Err(error) => panic!("native favicon channel failed: {error}"),
        }
        assert!(Instant::now() < deadline, "native favicon did not settle");
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
#[ignore = "requires an isolated display and admitted sandboxed WebKitGTK"]
fn native_favicon_watch_updates_and_svg_raster_is_fixed_rgba32() {
    crate::platform::linux::enforce_runtime_security_floor().expect("admitted WebKitGTK");
    gtk::init().expect("isolated GTK display");
    let fixture = Fixture::new();
    let window = gtk::Window::new(gtk::WindowType::Toplevel);
    let container = gtk::Fixed::new();
    window.add(&container);
    window.realize();
    let mut context = wry::WebContext::new_ephemeral().unwrap();
    let view = WebViewBuilder::new_with_web_context(&mut context)
        .with_incognito(true)
        .with_url(format!("{}/page", fixture.address))
        .build_gtk(&container)
        .unwrap();
    window.show_all();
    let page = view.webview();
    let deadline = Instant::now() + Duration::from_secs(10);
    while page.title().as_deref() != Some("icons") {
        while gtk::events_pending() {
            gtk::main_iteration_do(false);
        }
        assert!(Instant::now() < deadline, "favicon document did not load");
        std::thread::sleep(Duration::from_millis(2));
    }

    let (sender, receiver) = mpsc::channel();
    assert!(watch(&view, None, move |report| sender
        .send(report)
        .unwrap()));
    let report = receive(&receiver).expect("declared icon report");
    assert_eq!(
        report.candidates[0].url,
        format!("{}/icon.svg", fixture.address)
    );

    let (sender, receiver) = mpsc::channel();
    assert!(watch(&view, Some(&report.token), move |report| sender
        .send(report)
        .unwrap()));
    view.evaluate_script("document.querySelector('link').href='/updated.svg';")
        .unwrap();
    let updated = receive(&receiver).expect("dynamic icon report");
    let icon = format!("{}/updated.svg", fixture.address);
    assert_eq!(updated.candidates[0].url, icon);
    assert_ne!(updated.token, report.token);

    let (sender, receiver) = mpsc::channel();
    assert!(rasterize(&view, &icon, move |rgba| sender
        .send(rgba)
        .unwrap()));
    let rgba = receive(&receiver).expect("native SVG raster");
    assert_eq!(rgba.len(), 32 * 32 * 4);
    assert_eq!(rgba[3], 0, "non-square sources retain transparent padding");
    let center = (16 * 32 + 16) * 4;
    assert_eq!(&rgba[center..center + 4], &[32, 160, 64, 255]);

    let (sender, receiver) = mpsc::channel();
    assert!(rasterize(
        &view,
        "data:image/svg+xml,hostile",
        move |rgba| sender.send(rgba).unwrap()
    ));
    assert_eq!(receive(&receiver), None);

    let (sender, receiver) = mpsc::channel();
    assert!(watch(&view, Some(&updated.token), move |report| sender
        .send(report)
        .unwrap()));
    view.load_url(&format!("{}/next", fixture.address)).unwrap();
    assert_eq!(receive(&receiver), None, "navigation retires the old watch");
    window.close();
}
