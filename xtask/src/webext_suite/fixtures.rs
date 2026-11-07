//! Serves `suite-fixtures/` on a loopback port, so checks don't depend on
//! third-party pages.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};

/// Starts the server on a thread that lives as long as the process; returns
/// the origin pages are loaded from.
pub(super) fn serve(root: PathBuf) -> Result<String, String> {
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let root = root.clone();
            std::thread::spawn(move || respond(stream, &root));
        }
    });
    Ok(format!("http://localhost:{port}"))
}

fn respond(mut stream: TcpStream, root: &Path) {
    let mut request = String::new();
    if BufReader::new(&stream).read_line(&mut request).is_err() {
        return;
    }
    let path = request
        .split_whitespace()
        .nth(1)
        .and_then(|target| target.split(['?', '#']).next())
        .unwrap_or("/");
    let name = path.trim_start_matches('/');
    let name = if name.is_empty() { "blank.html" } else { name };
    let file = (!name.contains("..")).then(|| std::fs::read(root.join(name)));
    let (status, kind, body) = match file {
        Some(Ok(body)) => ("200 OK", content_type(name), body),
        _ => ("404 Not Found", "text/plain", b"not found".to_vec()),
    };
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(&body);
}

fn content_type(name: &str) -> &'static str {
    match name.rsplit('.').next() {
        Some("html") => "text/html; charset=utf-8",
        Some("json") => "application/json",
        Some("js") => "text/javascript",
        Some("css") => "text/css",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    fn get(origin: &str, path: &str) -> String {
        let address = origin.trim_start_matches("http://localhost:");
        let mut stream = TcpStream::connect(format!("127.0.0.1:{address}")).unwrap();
        write!(stream, "GET {path} HTTP/1.1\r\nHost: localhost\r\n\r\n").unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        response
    }

    #[test]
    fn serves_fixtures_and_nothing_outside_them() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("blank.html"), "<p>hi</p>").unwrap();
        std::fs::write(root.path().join("data.json"), "{}").unwrap();
        let origin = serve(root.path().to_path_buf()).unwrap();
        let page = get(&origin, "/?x=1");
        assert!(page.starts_with("HTTP/1.1 200 OK"), "{page}");
        assert!(page.contains("text/html") && page.ends_with("<p>hi</p>"));
        assert!(get(&origin, "/data.json").contains("application/json"));
        assert!(get(&origin, "/missing.html").starts_with("HTTP/1.1 404"));
        assert!(get(&origin, "/../secret").starts_with("HTTP/1.1 404"));
    }
}
