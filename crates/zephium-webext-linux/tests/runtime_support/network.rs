use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

pub struct Server {
    pub origin: String,
    pub requests: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Server {
    pub fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured = requests.clone();
        let thread = std::thread::spawn(move || {
            while !stopped.load(Ordering::Relaxed) {
                let (mut stream, _) = match listener.accept() {
                    Ok(value) => value,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(1));
                        continue;
                    }
                    Err(error) => panic!("fixture accept: {error}"),
                };
                stream
                    .set_read_timeout(Some(Duration::from_secs(1)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_secs(1)))
                    .unwrap();
                let mut bytes = Vec::new();
                let mut chunk = [0; 1024];
                while bytes.len() < 8192 && !bytes.windows(4).any(|part| part == b"\r\n\r\n") {
                    match stream.read(&mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(count) => bytes.extend_from_slice(&chunk[..count]),
                    }
                }
                let text = String::from_utf8_lossy(&bytes);
                let path = text.split_ascii_whitespace().nth(1).unwrap_or("");
                let mut requests = captured.lock().unwrap();
                assert!(requests.len() < 128);
                requests.push(path.to_owned());
                drop(requests);
                let body = "<!doctype html><p>runtime network fixture</p>";
                let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nCache-Control: no-store\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            }
        });
        Self {
            origin,
            requests,
            stop,
            thread: Some(thread),
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.thread.take().unwrap().join().unwrap();
    }
}

use zephium_core::{
    extensions::{ExtensionBrowserSurfaceGeneration, ExtensionBrowserTab},
    ids::{ExtensionInstallId, ItemId, ProfileId},
};
use zephium_webext_runtime::{
    api::action::ActionStore,
    browser::{
        BrowserPort, BrowserRequest, BrowserSnapshot, BrowserTab, BrowserWindow, ExtensionInfo,
    },
    ApiError, DocumentGeneration, Handled, Invocation, Sender,
};

pub struct Browser {
    snapshot: BrowserSnapshot,
    action: ActionStore,
}

impl Browser {
    pub fn new(origin: &str) -> Self {
        let url = url::Url::parse(origin).unwrap();
        let tab = ExtensionBrowserTab::from_snapshot(
            None,
            ItemId::from(1),
            true,
            "fixture",
            Some(&url),
            false,
            false,
        )
        .unwrap();
        Self {
            snapshot: BrowserSnapshot {
                profile: ProfileId::from(7),
                generation: ExtensionBrowserSurfaceGeneration::INITIAL,
                tabs: vec![BrowserTab {
                    id: 1,
                    window_id: 1,
                    index: 0,
                    tab,
                    active: true,
                    highlighted: true,
                    incognito: false,
                    document: DocumentGeneration::new(1).unwrap(),
                    document_id: "fixture-document".into(),
                    native_url: None,
                    pending_url: None,
                    fav_icon_url: None,
                    zoom: 1.0,
                    auto_discardable: true,
                    opener_tab_id: None,
                    last_accessed: 0.0,
                }],
                windows: vec![BrowserWindow {
                    id: 1,
                    native_id: 1,
                    incognito: false,
                    focused: true,
                    kind: "normal".into(),
                    state: "normal".into(),
                    bounds: [0, 0, 800, 600],
                    always_on_top: false,
                }],
                current_window: Some(1),
                last_focused_window: Some(1),
            },
            action: ActionStore::default(),
        }
    }
}

impl BrowserPort for Browser {
    fn snapshot(&self, _: &Sender) -> Result<BrowserSnapshot, ApiError> {
        Ok(self.snapshot.clone())
    }
    fn extension_info(&self, _: &Sender) -> Result<ExtensionInfo, ApiError> {
        Ok(ExtensionInfo::default())
    }
    fn request(&mut self, request: BrowserRequest, call: Invocation<'_>) -> Handled {
        call.reply(match request {
            BrowserRequest::Action(request) => self.action.handle(&request),
            _ => Err(zephium_webext_runtime::browser::unsupported()),
        })
    }
    fn set_dnr_badge(
        &mut self,
        _: ExtensionInstallId,
        tab_id: i32,
        text: Option<&str>,
    ) -> Result<(), ApiError> {
        self.action.set_dnr_badge(tab_id, text)
    }
}
