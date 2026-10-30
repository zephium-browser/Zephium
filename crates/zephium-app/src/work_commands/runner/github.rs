//! The child can request one opaque local-socket route; only the captured broker
//! closure can decide the remote resource. No provider credential enters gh.
use super::{Failure, Output};
use std::{
    future::Future,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
pub(super) mod socket;
#[cfg(windows)]
use super::windows_github::Runtime;
use socket::Listener;
const BODY_LIMIT: usize = 60 * 1024;
const HEADER_LIMIT: usize = 8192;

pub(crate) fn available() -> bool {
    #[cfg(target_os = "macos")]
    {
        super::guard::github_installed() && super::macos_github::present()
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}
#[cfg(any(target_os = "macos", windows))]
fn slots() -> &'static std::sync::Arc<super::worker_slots::Slots> {
    static SLOTS: std::sync::OnceLock<std::sync::Arc<super::worker_slots::Slots>> =
        std::sync::OnceLock::new();
    SLOTS.get_or_init(|| std::sync::Arc::new(super::worker_slots::Slots::new(4)))
}
pub(crate) async fn run<C, CF, F, FF>(cancelled: &C, fetch: F) -> Result<Output, Failure>
where
    C: Fn() -> CF,
    CF: Future<Output = bool>,
    F: FnOnce() -> FF,
    FF: Future<Output = Result<Vec<u8>, Failure>>,
{
    if !available() && !cfg!(test) {
        return Err(Failure::GitHubUnavailable);
    }
    #[cfg(any(target_os = "macos", windows))]
    {
        let slots = slots();
        let (asked, request) = tokio::sync::oneshot::channel();
        let (reply, response) = tokio::sync::oneshot::channel();
        let observing = super::worker::run(
            slots,
            cancelled,
            super::worker::Limits::standard(Duration::from_secs(30)),
            move |control| async move {
                #[cfg(target_os = "macos")]
                {
                    control.native();
                    super::guard::github(&|| async { control.check().is_err() }, || async {
                        asked.send(()).map_err(|_| Failure::Stopped)?;
                        response.await.map_err(|_| Failure::Stopped)?
                    })
                    .await
                }
                #[cfg(windows)]
                {
                    let mut runtime = Runtime::prepare_checked(&|| control.check())?;
                    let outcome = exchange(&mut runtime, &control, || async {
                        asked.send(()).map_err(|_| Failure::Stopped)?;
                        response.await.map_err(|_| Failure::Stopped)?
                    })
                    .await;
                    control.filesystem();
                    super::worker::closed(outcome, runtime.close())
                }
            },
        );
        tokio::pin!(observing);
        let bridge = async {
            if request.await.is_ok() {
                let result = fetch().await;
                let _ = reply.send(result);
            }
        };
        // Only an exact validated child request starts the caller-owned fetch.
        // Dropping the caller cancels the worker; neither side can replay it.
        tokio::select! {result=&mut observing=>result,_=bridge=>observing.await}
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = (cancelled, fetch);
        Err(Failure::GitHubUnavailable)
    }
}
#[cfg(windows)]
async fn exchange<F, FF>(
    runtime: &mut Runtime,
    control: &super::worker::Control,
    fetch: F,
) -> Result<Output, Failure>
where
    F: FnOnce() -> FF,
    FF: Future<Output = Result<Vec<u8>, Failure>>,
{
    control.check()?;
    let listener = Listener::bind(&runtime.socket)?;
    runtime.grant_socket()?;
    control.check()?;
    control.native();
    let nonce = format!(
        "{:032x}",
        u128::from_be_bytes(zephium_core::work::WorkArtifactId::generate().bytes())
    );
    let halt = AtomicBool::new(false);
    let cancellation =
        || async { control.cancelled.load(Ordering::Acquire) || halt.load(Ordering::Acquire) };
    let child = async {
        let result = runtime.run(&nonce, &cancellation).await;
        if result.is_err() || result.as_ref().is_ok_and(|out| !out.success) {
            halt.store(true, Ordering::Release);
        }
        result
    };
    let broker = async {
        let result = serve(listener, 1, &nonce, &halt, fetch).await;
        if result.is_err() {
            halt.store(true, Ordering::Release);
        }
        result
    };
    let (child, broker) = tokio::join!(child, broker);
    if matches!(child, Err(Failure::StopUnconfirmed)) {
        return Err(Failure::StopUnconfirmed);
    }
    if control.cancelled.load(Ordering::Acquire) {
        return Err(Failure::Stopped);
    }
    let expected = broker?;
    let output = child?;
    if !output.success
        || serde_json::from_str::<serde_json::Value>(&output.text).ok() != Some(expected)
    {
        return Err(Failure::Failed);
    }
    Ok(output)
}

pub(super) async fn serve<F, FF>(
    listener: Listener,
    port: u16,
    nonce: &str,
    halted: &AtomicBool,
    fetch: F,
) -> Result<serde_json::Value, Failure>
where
    F: FnOnce() -> FF,
    FF: Future<Output = Result<Vec<u8>, Failure>>,
{
    let exchange = async {
        let mut stream = listener.accept().await?;
        let mut header = Vec::new();
        let mut chunk = [0u8; 1024];
        loop {
            let count = stream.read(&mut chunk).await.map_err(|_| Failure::Failed)?;
            if count == 0 || header.len() + count > HEADER_LIMIT {
                return Err(Failure::Failed);
            }
            header.extend_from_slice(&chunk[..count]);
            if header.windows(4).any(|part| part == b"\r\n\r\n") {
                break;
            }
        }
        if !exact_request(&header, port, nonce) {
            return Err(Failure::Failed);
        }
        let bytes = fetch().await?;
        if bytes.len() > BODY_LIMIT {
            return Err(Failure::Limit);
        }
        let text = std::str::from_utf8(&bytes).map_err(|_| Failure::Failed)?;
        if zephium_agentic::contains_recognized_secret(text) {
            return Err(Failure::Sensitive);
        }
        let value: serde_json::Value = serde_json::from_str(text).map_err(|_| Failure::Failed)?;
        if zephium_agentic::contains_recognized_secret(&value.to_string()) {
            return Err(Failure::Sensitive);
        }
        let header = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", bytes.len());
        stream
            .write_all(header.as_bytes())
            .await
            .map_err(|_| Failure::Failed)?;
        stream
            .write_all(&bytes)
            .await
            .map_err(|_| Failure::Failed)?;
        stream.shutdown().await.map_err(|_| Failure::Failed)?;
        Ok(value)
    };
    tokio::pin!(exchange);
    let deadline = tokio::time::sleep(Duration::from_secs(12));
    tokio::pin!(deadline);
    let mut tick = tokio::time::interval(Duration::from_millis(25));
    loop {
        tokio::select! {
            result = &mut exchange => return result,
            _ = &mut deadline => return Err(Failure::Limit),
            _ = tick.tick() => if halted.load(Ordering::Acquire) { return Err(Failure::Failed); }
        }
    }
}
fn exact_request(bytes: &[u8], port: u16, nonce: &str) -> bool {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return false;
    };
    if !text.ends_with("\r\n\r\n") || text[..text.len() - 4].contains("\r\n\r\n") {
        return false;
    }
    let mut lines = text[..text.len() - 4].split("\r\n");
    if lines.next() != Some(format!("GET /{nonce} HTTP/1.1").as_str()) {
        return false;
    }
    let mut host = false;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            return false;
        };
        if name.is_empty()
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            return false;
        }
        if name.eq_ignore_ascii_case("host") {
            if host || value.trim() != format!("127.0.0.1:{port}") {
                return false;
            }
            host = true;
        } else if name.eq_ignore_ascii_case("transfer-encoding")
            || name.eq_ignore_ascii_case("content-length") && value.trim() != "0"
        {
            return false;
        }
        if line.chars().any(|c| c.is_control() && c != '\t') {
            return false;
        }
    }
    host
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture_listener() -> (tempfile::TempDir, Listener, std::path::PathBuf) {
        #[cfg(unix)]
        let directory = tempfile::Builder::new()
            .prefix("zgh-")
            .tempdir_in("/private/tmp")
            .unwrap();
        #[cfg(windows)]
        // AF_UNIX connect is rejected under the user-profile temp tree on Windows.
        let directory = tempfile::Builder::new()
            .prefix("zgh-")
            .tempdir_in(
                std::env::var_os("PUBLIC")
                    .expect("PUBLIC must identify a writable local directory for AF_UNIX tests"),
            )
            .unwrap();
        #[cfg(not(any(unix, windows)))]
        let directory = tempfile::Builder::new().prefix("zgh-").tempdir().unwrap();
        let path = directory.path().canonicalize().unwrap().join("broker.sock");
        let listener = Listener::bind(&path).unwrap();
        (directory, listener, path)
    }
    #[test]
    fn broker_accepts_only_one_exact_bodyless_get() {
        let good = b"GET /nonce HTTP/1.1\r\nHost: 127.0.0.1:1234\r\nUser-Agent: fixture\r\n\r\n";
        assert!(exact_request(good, 1234, "nonce"));
        for request in [
            "POST /nonce HTTP/1.1\r\nHost: 127.0.0.1:1234\r\n\r\n",
            "GET /other HTTP/1.1\r\nHost: 127.0.0.1:1234\r\n\r\n",
            "GET /nonce?q=x HTTP/1.1\r\nHost: 127.0.0.1:1234\r\n\r\n",
            "GET /nonce HTTP/1.1\r\nHost: example.com\r\n\r\n",
            "GET /nonce HTTP/1.1\r\nHost: 127.0.0.1:1234\r\nHost: 127.0.0.1:1234\r\n\r\n",
            "GET /nonce HTTP/1.1\r\nHost: 127.0.0.1:1234\r\nContent-Length: 2\r\n\r\nxx",
            "GET /nonce HTTP/1.1\r\nHost: 127.0.0.1:1234\r\nTransfer-Encoding: chunked\r\n\r\n",
        ] {
            assert!(!exact_request(request.as_bytes(), 1234, "nonce"));
        }
    }
    #[tokio::test]
    async fn malformed_request_never_starts_remote_fetch() {
        let (_directory, listener, endpoint) = fixture_listener();
        let port = 1;
        let called = AtomicBool::new(false);
        let halt = AtomicBool::new(false);
        let broker = serve(listener, port, "nonce", &halt, || async {
            called.store(true, Ordering::Release);
            Ok(b"{}".to_vec())
        });
        let client = async {
            let mut socket = socket::Stream::connect(&endpoint);
            socket
                .write_all(
                    format!("POST /nonce HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n").as_bytes(),
                )
                .await
                .unwrap();
            let mut bytes = Vec::new();
            socket.read_to_end(&mut bytes).await.unwrap();
            bytes
        };
        let (reply, bytes) = tokio::join!(broker, client);
        assert!(reply.is_err());
        assert!(bytes.is_empty());
        assert!(!called.load(Ordering::Acquire));
    }
    #[tokio::test]
    async fn broker_withholds_raw_decoded_and_oversized_output_before_client_receives_it() {
        for content in [
            br#"{"text":"ghp_inertFixtureToken"}"#.to_vec(),
            br#"{"text":"\u0067hp_inertFixtureToken"}"#.to_vec(),
            vec![b'x'; BODY_LIMIT + 1],
        ] {
            let (_directory, listener, endpoint) = fixture_listener();
            let port = 1;
            let halt = AtomicBool::new(false);
            let broker = serve(listener, port, "nonce", &halt, || async { Ok(content) });
            let client = async {
                let mut socket = socket::Stream::connect(&endpoint);
                socket
                    .write_all(
                        format!("GET /nonce HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n").as_bytes(),
                    )
                    .await
                    .unwrap();
                let mut bytes = Vec::new();
                socket.read_to_end(&mut bytes).await.unwrap();
                bytes
            };
            let (reply, bytes) = tokio::join!(broker, client);
            assert!(matches!(reply, Err(Failure::Sensitive | Failure::Limit)));
            assert!(bytes.is_empty());
        }
    }
    #[cfg(target_os = "macos")]
    #[tokio::test]
    #[ignore = "Qualified gh class admission under pending worker debt; no service request"]
    async fn native_github_pending_workers_hold_capacity_before_fetch_or_launch() {
        assert!(available());
        super::super::worker::pending_capacity_fixture(slots().clone(), || async {
            let called = AtomicBool::new(false);
            let result = run(&|| async { false }, || async {
                called.store(true, Ordering::Release);
                Ok(b"{}".to_vec())
            })
            .await;
            !called.load(Ordering::Acquire) && matches!(result, Err(Failure::WorkerBusy))
        })
        .await;
    }
    #[cfg(target_os = "macos")]
    #[tokio::test]
    #[ignore = "Native pinned-gh qualification; inert local-socket broker only"]
    async fn native_github_fixed_get_returns_only_captured_public_fixture() {
        let output = run(&|| async { false }, || async {
            Ok(br#"{"number":3,"title":"Inert public fixture"}"#.to_vec())
        })
        .await
        .unwrap();
        assert!(output.success);
        let parsed: serde_json::Value = serde_json::from_str(&output.text).unwrap();
        assert_eq!(parsed["number"], 3);
        assert_eq!(parsed["title"], "Inert public fixture");
        let hidden = run(&|| async { false }, || async {
            Ok(br#"{"text":"ghp_inertFixtureToken"}"#.to_vec())
        })
        .await;
        assert!(matches!(hidden, Err(Failure::Sensitive)));
    }
    #[cfg(target_os = "macos")]
    #[tokio::test]
    #[ignore = "Native pinned-gh Stop qualification; pending inert broker, no remote request"]
    async fn native_github_stop_interrupts_pending_broker_with_owned_child_exit() {
        let started = AtomicBool::new(false);
        let requested = std::sync::Mutex::new(None::<std::time::Instant>);
        let cancelled = || async {
            if started.load(Ordering::Acquire) {
                requested
                    .lock()
                    .unwrap()
                    .get_or_insert_with(std::time::Instant::now);
                true
            } else {
                false
            }
        };
        let outcome = run(&cancelled, || async {
            started.store(true, Ordering::Release);
            std::future::pending::<Result<Vec<u8>, Failure>>().await
        })
        .await;
        assert!(matches!(outcome, Err(Failure::Stopped)));
        assert!(requested.lock().unwrap().unwrap().elapsed() < Duration::from_secs(1));
    }
}

pub(super) fn environment() -> Vec<(std::ffi::OsString, std::ffi::OsString)> {
    // Markers satisfy gh's CLI/auth preflight; neither is a provider credential.
    let result: Vec<_> = [
        ("GH_TOKEN", "work-local-broker-placeholder"),
        ("GH_ENTERPRISE_TOKEN", "work-local-broker-placeholder"),
        ("GH_PROMPT_DISABLED", "1"),
        ("GH_NO_UPDATE_NOTIFIER", "1"),
        ("GH_NO_EXTENSION_UPDATE_NOTIFIER", "1"),
        ("GH_TELEMETRY", "false"),
        ("DO_NOT_TRACK", "1"),
        ("NO_COLOR", "1"),
        ("LC_ALL", "C"),
        ("TZ", "UTC"),
    ]
    .into_iter()
    .map(|(key, value)| (key.into(), value.into()))
    .collect();
    #[cfg(unix)]
    let result = {
        let mut result = result;
        result.push(("PATH".into(), "/usr/bin:/bin".into()));
        result
    };
    result
}
pub(super) fn write_config(
    home: &std::path::Path,
    endpoint: &std::path::Path,
) -> Result<(), Failure> {
    use std::io::Write;
    let name = socket::endpoint_name(endpoint)?;
    let content = format!(
        "version: 1\nhttp_unix_socket: {}\n",
        serde_json::to_string(&name).map_err(|_| Failure::Input)?
    );
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(home.join("config.yml"))
        .map_err(|_| Failure::Storage)?;
    file.write_all(content.as_bytes())
        .and_then(|_| file.sync_all())
        .map_err(|_| Failure::Storage)
}

#[cfg(test)]
mod config_tests {
    use super::*;
    #[test]
    fn generated_config_is_exact_private_and_never_overwrites() {
        let root = tempfile::tempdir().unwrap();
        let socket = root.path().join("broker.sock");
        write_config(root.path(), &socket).unwrap();
        let content = std::fs::read_to_string(root.path().join("config.yml")).unwrap();
        assert_eq!(
            content,
            format!(
                "version: 1\nhttp_unix_socket: {}\n",
                serde_json::to_string(&socket::endpoint_name(&socket).unwrap()).unwrap()
            )
        );
        assert!(write_config(root.path(), &socket).is_err());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(root.path().join("config.yml"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }
}
