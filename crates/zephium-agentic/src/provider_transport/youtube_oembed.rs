//! A pasted YouTube link's real title and channel, from YouTube's public
//! oEmbed endpoint. The address asked is built from a video ID parsed here,
//! never from the pasted string, and only two plain-text fields are kept.
use crate::public_asset::{
    lookup_client, with_direct_route, PublicAssetError, PublicRoute, RouteSource,
};
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(5);
const MAX_BODY_BYTES: usize = 64 * 1024;
const MAX_TITLE_CHARS: usize = 200;
const MAX_CHANNEL_CHARS: usize = 100;

/// What a video is called and who published it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct YoutubeDetails {
    /// The video's title, plain text.
    pub title: String,
    /// The publishing channel's name, plain text.
    pub channel: String,
}

/// The video a watch, youtu.be or shorts address plays: exactly 11 of
/// `[A-Za-z0-9_-]`.
pub fn video_id(raw: &str) -> Option<String> {
    let parsed = url::Url::parse(raw).ok()?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return None;
    }
    let host = parsed.host_str()?.to_ascii_lowercase();
    let host = host
        .strip_prefix("www.")
        .or_else(|| host.strip_prefix("m."))
        .unwrap_or(&host);
    let mut path = parsed.path_segments()?.filter(|part| !part.is_empty());
    let id = match (host, path.next()) {
        ("youtu.be", Some(id)) => id.to_owned(),
        ("youtube.com", Some("watch")) => parsed
            .query_pairs()
            .find(|(key, _)| key == "v")
            .map(|(_, value)| value.into_owned())?,
        ("youtube.com", Some("shorts")) => path.next()?.to_owned(),
        _ => return None,
    };
    (id.len() == 11
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-'))
    .then_some(id)
}

/// The one address that is ever asked, built from a parsed ID.
pub fn oembed_url(id: &str) -> Option<url::Url> {
    if id.len() != 11
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    {
        return None;
    }
    let mut endpoint = url::Url::parse("https://www.youtube.com/oembed").ok()?;
    endpoint
        .query_pairs_mut()
        .append_pair("url", &format!("https://www.youtube.com/watch?v={id}"))
        .append_pair("format", "json");
    Some(endpoint)
}

fn plain(value: &serde_json::Value, key: &str, limit: usize) -> Option<String> {
    let text: String = value
        .get(key)?
        .as_str()?
        .chars()
        .map(|c| {
            let invisible = matches!(c, '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}');
            if c.is_control() || invisible {
                ' '
            } else {
                c
            }
        })
        .collect();
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let text: String = text.chars().take(limit).collect();
    (!text.is_empty()).then_some(text)
}

/// Reads `title` and `author_name` only; every other field, `html` included,
/// is ignored.
pub fn details(body: &[u8]) -> Option<YoutubeDetails> {
    if body.len() > MAX_BODY_BYTES {
        return None;
    }
    let value: serde_json::Value = serde_json::from_slice(body).ok()?;
    Some(YoutubeDetails {
        title: plain(&value, "title", MAX_TITLE_CHARS)?,
        channel: plain(&value, "author_name", MAX_CHANNEL_CHARS)?,
    })
}

/// One anonymous request, refused unless `routes` is direct; every other
/// failure is `Unavailable`.
pub async fn fetch(routes: &RouteSource, raw: &str) -> Result<YoutubeDetails, PublicAssetError> {
    let endpoint = video_id(raw)
        .and_then(|id| oembed_url(&id))
        .ok_or(PublicAssetError::InvalidUrl)?;
    with_direct_route(routes, async {
        let client = lookup_client(PublicRoute::Direct, TIMEOUT)?;
        let response = client
            .get(endpoint)
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await
            .map_err(|_| PublicAssetError::Unavailable)?;
        read_details(response).await
    })
    .await
}

async fn read_details(mut response: reqwest::Response) -> Result<YoutubeDetails, PublicAssetError> {
    if response.status() != reqwest::StatusCode::OK
        || response
            .content_length()
            .is_some_and(|length| length > MAX_BODY_BYTES as u64)
    {
        return Err(PublicAssetError::Unavailable);
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| PublicAssetError::Unavailable)?
    {
        if body.len() + chunk.len() > MAX_BODY_BYTES {
            return Err(PublicAssetError::Unavailable);
        }
        body.extend_from_slice(&chunk);
    }
    details(&body).ok_or(PublicAssetError::Unavailable)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_come_from_watch_short_and_shortened_addresses_only() {
        for raw in [
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ&t=9",
            "https://m.youtube.com/watch?v=dQw4w9WgXcQ",
            "http://youtube.com/shorts/dQw4w9WgXcQ?feature=share",
            "https://youtu.be/dQw4w9WgXcQ?si=abc",
        ] {
            assert_eq!(video_id(raw).as_deref(), Some("dQw4w9WgXcQ"), "{raw}");
        }
        for raw in [
            "https://youtube.com.evil.example/watch?v=dQw4w9WgXcQ",
            "https://evil.example/watch?v=dQw4w9WgXcQ",
            "https://www.youtube.com/watch?v=short",
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ%00x",
            "https://www.youtube.com/watch?v=dQw4w9WgXc!",
            "https://www.youtube.com/playlist?list=PLabcdefghijk",
            "ftp://youtu.be/dQw4w9WgXcQ",
            "not a url",
        ] {
            assert_eq!(video_id(raw), None, "{raw}");
        }
    }

    #[test]
    fn the_request_is_built_from_the_id_and_pinned_to_youtube() {
        let url = oembed_url("dQw4w9WgXcQ").unwrap();
        assert_eq!(
            url.as_str(),
            "https://www.youtube.com/oembed?url=https%3A%2F%2Fwww.youtube.com%2Fwatch%3Fv%3DdQw4w9WgXcQ&format=json"
        );
        assert_eq!(url.host_str(), Some("www.youtube.com"));
        assert!(oembed_url("dQw4w9WgXc&").is_none());
        assert!(oembed_url("toolongtoolong").is_none());
    }

    #[test]
    fn a_response_keeps_a_clean_title_and_channel_and_nothing_else() {
        let body = serde_json::json!({
            "title": "  Calm\u{0}  canvases\n\u{202e}talk ",
            "author_name": "Some Channel",
            "html": "<iframe src=\"https://evil.example\"></iframe>",
            "thumbnail_url": "https://evil.example/x.jpg"
        });
        let got = details(body.to_string().as_bytes()).unwrap();
        assert_eq!(got.channel, "Some Channel");
        assert!(!got.title.chars().any(|c| c.is_control() || c == '\u{202e}'));
        assert!(got.title.starts_with("Calm canvases"));
    }

    #[test]
    fn a_malformed_missing_empty_or_oversized_response_is_refused() {
        assert!(details(b"not json").is_none());
        assert!(details(br#"{"title":"a"}"#).is_none());
        assert!(details(br#"{"title":"  ","author_name":"b"}"#).is_none());
        assert!(details(br#"{"title":1,"author_name":"b"}"#).is_none());
        let big = format!(r#"{{"title":"a","author_name":"{}"}}"#, "b".repeat(70_000));
        assert!(details(big.as_bytes()).is_none());
        let long = serde_json::json!({"title": "x".repeat(900), "author_name": "y".repeat(900)});
        let got = details(long.to_string().as_bytes()).unwrap();
        assert_eq!(got.title.chars().count(), MAX_TITLE_CHARS);
        assert_eq!(got.channel.chars().count(), MAX_CHANNEL_CHARS);
    }
}

#[cfg(test)]
#[tokio::test]
async fn fetch_refuses_invalid_input_before_route_admission() {
    let routes = RouteSource::new(&tokio::runtime::Handle::current());
    assert_eq!(
        fetch(&routes, "https://example.com/not-a-video").await,
        Err(PublicAssetError::InvalidUrl)
    );
}

#[cfg(test)]
#[tokio::test]
async fn oembed_response_maps_status_size_and_json_failures() {
    use std::io::{Read, Write};
    let valid = br#"{"title":"Title","author_name":"Channel"}"#;
    let cases = [
        (
            "200 OK",
            format!("Content-Length: {}\r\n", valid.len()),
            valid.to_vec(),
            true,
        ),
        (
            "503 Service Unavailable",
            "Content-Length: 0\r\n".into(),
            Vec::new(),
            false,
        ),
        (
            "200 OK",
            format!("Content-Length: {}\r\n", MAX_BODY_BYTES + 1),
            Vec::new(),
            false,
        ),
        (
            "200 OK",
            "Content-Length: 1\r\n".into(),
            b"{".to_vec(),
            false,
        ),
        (
            "200 OK",
            "Transfer-Encoding: chunked\r\n".into(),
            {
                let mut encoded = format!("{:x}\r\n", MAX_BODY_BYTES + 1).into_bytes();
                encoded.resize(encoded.len() + MAX_BODY_BYTES + 1, b'x');
                encoded.extend_from_slice(b"\r\n0\r\n\r\n");
                encoded
            },
            false,
        ),
    ];
    for (status, header, body, accepted) in cases {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let worker = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(TIMEOUT)).unwrap();
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                stream.read_exact(&mut byte).unwrap();
                request.push(byte[0]);
            }
            write!(
                stream,
                "HTTP/1.1 {status}\r\n{header}Connection: close\r\n\r\n"
            )
            .unwrap();
            let _ = stream.write_all(&body);
        });
        let response = reqwest::Client::builder()
            .no_proxy()
            .timeout(TIMEOUT)
            .build()
            .unwrap()
            .get(format!("http://{address}/"))
            .send()
            .await
            .unwrap();
        let expected = if accepted {
            Ok(YoutubeDetails {
                title: "Title".into(),
                channel: "Channel".into(),
            })
        } else {
            Err(PublicAssetError::Unavailable)
        };
        assert_eq!(read_details(response).await, expected);
        worker.join().unwrap();
    }
}
