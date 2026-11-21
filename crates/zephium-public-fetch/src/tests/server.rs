use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    thread::JoinHandle,
    time::Duration,
};

#[derive(Clone, Debug)]
pub(crate) struct Seen {
    pub(crate) method: String,
    pub(crate) target: String,
    pub(crate) headers: Vec<(String, String)>,
}

impl Seen {
    pub(crate) fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    pub(crate) fn path(&self) -> &str {
        &self.target
    }
}

pub(crate) enum Step {
    Write(Vec<u8>),
    Sleep(Duration),
}

pub(crate) enum Reply {
    Keep(Vec<u8>),
    Close(Vec<u8>),
    Steps(Vec<Step>),
    Hang,
    Endless { head: Vec<u8>, chunk: Vec<u8> },
}

#[derive(Default)]
pub(crate) struct Gauge {
    now: AtomicUsize,
    pub(crate) max: AtomicUsize,
}

impl Gauge {
    fn enter(&self) {
        let now = self.now.fetch_add(1, Ordering::AcqRel) + 1;
        self.max.fetch_max(now, Ordering::AcqRel);
    }

    fn leave(&self) {
        self.now.fetch_sub(1, Ordering::AcqRel);
    }
}

const ACCEPT_POLL: Duration = Duration::from_millis(5);

type Handler = dyn Fn(&Seen) -> Reply + Send + Sync;

pub(crate) struct Server {
    pub(crate) port: u16,
    pub(crate) seen: Arc<Mutex<Vec<Seen>>>,
    pub(crate) connections: Arc<AtomicUsize>,
    pub(crate) closed: Arc<AtomicUsize>,
    pub(crate) in_flight: Arc<Gauge>,
    stopped: Arc<AtomicBool>,
    accept: Option<JoinHandle<()>>,
}

impl Server {
    pub(crate) fn start(handler: impl Fn(&Seen) -> Reply + Send + Sync + 'static) -> Server {
        Self::start_with(Arc::new(Gauge::default()), handler)
    }

    pub(crate) fn start_with(
        shared: Arc<Gauge>,
        handler: impl Fn(&Seen) -> Reply + Send + Sync + 'static,
    ) -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).unwrap();
        let mut server = Server {
            port,
            seen: Arc::default(),
            connections: Arc::default(),
            closed: Arc::default(),
            in_flight: Arc::default(),
            stopped: Arc::default(),
            accept: None,
        };
        let handler: Arc<Handler> = Arc::new(handler);
        let seen = server.seen.clone();
        let connections = server.connections.clone();
        let closed = server.closed.clone();
        let own = server.in_flight.clone();
        let stopped = server.stopped.clone();
        server.accept = Some(std::thread::spawn(move || {
            while !stopped.load(Ordering::Acquire) {
                let stream = match listener.accept() {
                    Ok((stream, _)) => stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::park_timeout(ACCEPT_POLL);
                        continue;
                    }
                    Err(_) => return,
                };
                stream.set_nonblocking(false).unwrap();
                connections.fetch_add(1, Ordering::AcqRel);
                let (handler, seen, closed, own, shared) = (
                    handler.clone(),
                    seen.clone(),
                    closed.clone(),
                    own.clone(),
                    shared.clone(),
                );
                std::thread::spawn(move || {
                    serve(stream, &*handler, &seen, &own, &shared);
                    closed.fetch_add(1, Ordering::AcqRel);
                });
            }
        }));
        server
    }

    pub(crate) fn url(&self, path: &str) -> url::Url {
        url::Url::parse(&format!("http://127.0.0.1:{}{path}", self.port)).unwrap()
    }

    pub(crate) fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        if let Some(accept) = self.accept.take() {
            accept.thread().unpark();
            let _ = accept.join();
        }
    }
}

#[test]
fn dropping_server_joins_its_accepting_thread() {
    let server = Server::start(|_| Reply::Hang);
    let connections = Arc::downgrade(&server.connections);
    drop(server);
    assert!(connections.upgrade().is_none());
}

fn read_head(stream: &mut TcpStream) -> Option<Seen> {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        if head.len() > 256 * 1024 || stream.read(&mut byte).ok()? == 0 {
            return None;
        }
        head.push(byte[0]);
    }
    let text = String::from_utf8_lossy(&head);
    let mut lines = text.split("\r\n");
    let mut request = lines.next()?.split(' ');
    let method = request.next()?.to_owned();
    let target = request.next()?.to_owned();
    let headers = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_owned()))
        .collect();
    Some(Seen {
        method,
        target,
        headers,
    })
}

fn serve(
    mut stream: TcpStream,
    handler: &Handler,
    seen: &Mutex<Vec<Seen>>,
    own: &Gauge,
    shared: &Gauge,
) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(20)));
    while let Some(request) = read_head(&mut stream) {
        seen.lock().unwrap().push(request.clone());
        own.enter();
        shared.enter();
        let keep = match handler(&request) {
            Reply::Keep(bytes) => stream.write_all(&bytes).is_ok(),
            Reply::Close(bytes) => {
                let _ = stream.write_all(&bytes);
                false
            }
            Reply::Steps(steps) => {
                for step in steps {
                    match step {
                        Step::Write(bytes) => {
                            if stream
                                .write_all(&bytes)
                                .and_then(|()| stream.flush())
                                .is_err()
                            {
                                break;
                            }
                        }
                        Step::Sleep(duration) => std::thread::sleep(duration),
                    }
                }
                false
            }
            Reply::Hang => {
                let mut sink = [0u8; 1024];
                while matches!(stream.read(&mut sink), Ok(read) if read > 0) {}
                false
            }
            Reply::Endless { head, chunk } => {
                if stream.write_all(&head).is_ok() {
                    while stream.write_all(&chunk).is_ok() {}
                }
                false
            }
        };
        own.leave();
        shared.leave();
        if !keep {
            break;
        }
    }
}

pub(crate) fn response(status: &str, headers: &[(&str, &str)], body: &[u8]) -> Vec<u8> {
    let mut head = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\n", body.len());
    for (name, value) in headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    let mut bytes = head.into_bytes();
    bytes.extend_from_slice(body);
    bytes
}

pub(crate) fn ok(content_type: &str, body: &[u8]) -> Reply {
    Reply::Keep(response("200 OK", &[("Content-Type", content_type)], body))
}

pub(crate) fn redirect(location: &str) -> Reply {
    Reply::Keep(response("302 Found", &[("Location", location)], b""))
}

pub(crate) fn close_delimited(headers: &[(&str, &str)], body: &[u8]) -> Reply {
    let mut head = String::from("HTTP/1.1 200 OK\r\nConnection: close\r\n");
    for (name, value) in headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    let mut bytes = head.into_bytes();
    bytes.extend_from_slice(body);
    Reply::Close(bytes)
}
