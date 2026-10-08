//! Anonymous origin icon probe for Work sources the agent did not read.
//!
//! One cookie-free client: HTTPS to public hosts only, at most two redirects,
//! a fixed timeout and byte caps. `/favicon.ico` first, then the icon links in
//! the root document's head. Icon bytes are decoded by a bounded, memory-safe
//! decoder and leave this module only as the fixed 32x32 RGBA raster the
//! renderer path produces. Diagnostics carry origin, bytes and outcome class.

use std::io::Cursor;
use std::time::Duration;

use reqwest::{redirect::Policy, Client, Url};
use zephium_app::{CallbackHandle, Command};
use zephium_core::icon::{ICON_SIDE, RGBA32_BYTES};
use zephium_core::ids::ProfileId;

pub(crate) const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
pub(crate) const MAX_ICON_BYTES: usize = 256 * 1024;
pub(crate) const MAX_HEAD_BYTES: usize = 64 * 1024;
pub(crate) const MAX_REDIRECTS: usize = 2;
/// Largest declared icon side the head parser picks.
pub(crate) const MAX_CANDIDATE_SIDE: u32 = 256;
const MAX_CANDIDATES_TRIED: usize = 3;
const MAX_LINKS: usize = 64;
const MAX_DECODED_SIDE: u32 = 1024;
const MAX_DECODE_ALLOC: u64 = 16 * 1024 * 1024;
/// The agent browser's product user agent.
const WORK_USER_AGENT: &str = "Zephium-Agent-Browser/0.1";
const REPLY_ATTEMPTS: usize = 20;
const REPLY_RETRY: Duration = Duration::from_millis(250);

/// Why a probe produced no icon. Never carries response content.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProbeOutcome {
    Found,
    Refused,
    Unavailable,
    TooLarge,
    NotAnImage,
    NoIcon,
}

#[derive(Clone, Copy)]
struct ProbePolicy {
    public_https: bool,
}

impl ProbePolicy {
    const PRODUCT: Self = Self { public_https: true };

    fn admits(self, url: &Url) -> bool {
        if self.public_https {
            zephium_agentic::public_asset::public_https(url)
        } else {
            matches!(url.scheme(), "http" | "https")
        }
    }
}

fn client(policy: ProbePolicy) -> Option<Client> {
    Client::builder()
        .https_only(policy.public_https)
        .redirect(Policy::custom(move |attempt| {
            if attempt.previous().len() > MAX_REDIRECTS || !policy.admits(attempt.url()) {
                attempt.stop()
            } else {
                attempt.follow()
            }
        }))
        .referer(false)
        .retry(reqwest::retry::never())
        .timeout(PROBE_TIMEOUT)
        .connect_timeout(PROBE_TIMEOUT)
        .pool_max_idle_per_host(0)
        .user_agent(WORK_USER_AGENT)
        .build()
        .ok()
}

/// Attaches the prober to the shell. Each request answers exactly once.
pub(crate) fn install(shell: &zephium_app::Handle) {
    let Some(client) = client(ProbePolicy::PRODUCT) else {
        crate::write_diagnostic(format_args!("favicon: probe client did not build"));
        return;
    };
    let reply = shell.callback_handle();
    let prober: zephium_app::FaviconProber = std::sync::Arc::new(move |profile, origin| {
        let client = client.clone();
        let reply = reply.clone();
        tauri::async_runtime::spawn(async move {
            let (rgba, bytes, outcome) = probe(&client, ProbePolicy::PRODUCT, &origin).await;
            crate::work_provider::record_diagnostic(format_args!(
                "favicon: phase=probe bytes={bytes} outcome={outcome:?}"
            ));
            answer(&reply, profile, origin, rgba).await;
        });
    });
    if !shell.dispatch(Command::AttachFaviconProber(
        zephium_app::FaviconProberAttachment(prober),
    )) {
        crate::write_diagnostic(format_args!("favicon: the shell refused its prober"));
    }
}

/// The shell holds the origin in flight until it hears back, so a full
/// queue is retried briefly rather than dropped.
async fn answer(reply: &CallbackHandle, profile: ProfileId, origin: String, rgba: Option<Vec<u8>>) {
    for _ in 0..REPLY_ATTEMPTS {
        if reply.dispatch(Command::FaviconProbed {
            profile,
            origin: origin.clone(),
            rgba: rgba.clone(),
        }) {
            return;
        }
        tokio::time::sleep(REPLY_RETRY).await;
    }
}

async fn probe(
    client: &Client,
    policy: ProbePolicy,
    origin: &str,
) -> (Option<Vec<u8>>, usize, ProbeOutcome) {
    let Some(base) = Url::parse(origin).ok().filter(|url| policy.admits(url)) else {
        return (None, 0, ProbeOutcome::Refused);
    };
    let mut total = 0;
    if let Ok(ico) = base.join("/favicon.ico") {
        match fetch(client, ico, MAX_ICON_BYTES, false).await {
            Ok((bytes, _)) => {
                total += bytes.len();
                if let Some(rgba) = rasterize(&bytes) {
                    return (Some(rgba), total, ProbeOutcome::Found);
                }
            }
            Err(ProbeOutcome::TooLarge) => {}
            Err(_) => {}
        }
    }
    let (html, landed) = match fetch(client, base.clone(), MAX_HEAD_BYTES, true).await {
        Ok(page) => page,
        Err(outcome) => return (None, total, outcome),
    };
    total += html.len();
    let candidates = head_icons(&String::from_utf8_lossy(&html), &landed);
    if candidates.is_empty() {
        return (None, total, ProbeOutcome::NoIcon);
    }
    let mut outcome = ProbeOutcome::NotAnImage;
    for candidate in candidates
        .into_iter()
        .filter(|url| policy.admits(url))
        .take(MAX_CANDIDATES_TRIED)
    {
        match fetch(client, candidate, MAX_ICON_BYTES, false).await {
            Ok((bytes, _)) => {
                total += bytes.len();
                if let Some(rgba) = rasterize(&bytes) {
                    return (Some(rgba), total, ProbeOutcome::Found);
                }
                outcome = ProbeOutcome::NotAnImage;
            }
            Err(error) => outcome = error,
        }
    }
    (None, total, outcome)
}

/// One GET under a byte cap. A document may be cut at the cap, since only its
/// head is read; an icon over the cap is refused.
async fn fetch(
    client: &Client,
    url: Url,
    cap: usize,
    truncate: bool,
) -> Result<(Vec<u8>, Url), ProbeOutcome> {
    let mut response = client
        .get(url)
        .send()
        .await
        .map_err(|_| ProbeOutcome::Unavailable)?;
    if response.status() != reqwest::StatusCode::OK {
        return Err(ProbeOutcome::Unavailable);
    }
    let landed = response.url().clone();
    if !truncate
        && response
            .content_length()
            .is_some_and(|length| length > cap as u64)
    {
        return Err(ProbeOutcome::TooLarge);
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| ProbeOutcome::Unavailable)?
    {
        let room = cap - bytes.len();
        if chunk.len() > room {
            if !truncate {
                return Err(ProbeOutcome::TooLarge);
            }
            bytes.extend_from_slice(&chunk[..room]);
            break;
        }
        bytes.extend_from_slice(&chunk);
    }
    if bytes.is_empty() {
        return Err(ProbeOutcome::NotAnImage);
    }
    Ok((bytes, landed))
}

/// Decodes one icon and fits it, centred, into the fixed transparent 32x32
/// raster, as the renderer's canvas path does.
pub(crate) fn rasterize(bytes: &[u8]) -> Option<Vec<u8>> {
    use image::{ImageFormat, ImageReader};
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    if !matches!(
        reader.format()?,
        ImageFormat::Png
            | ImageFormat::Ico
            | ImageFormat::Gif
            | ImageFormat::WebP
            | ImageFormat::Jpeg
            | ImageFormat::Bmp
    ) {
        return None;
    }
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_DECODED_SIDE);
    limits.max_image_height = Some(MAX_DECODED_SIDE);
    limits.max_alloc = Some(MAX_DECODE_ALLOC);
    reader.limits(limits);
    let decoded = reader.decode().ok()?.into_rgba8();
    let (width, height) = decoded.dimensions();
    if width == 0 || height == 0 {
        return None;
    }
    let side = ICON_SIDE as f64;
    let scale = (side / f64::from(width)).min(side / f64::from(height));
    let draw_width = ((f64::from(width) * scale).round() as u32).clamp(1, ICON_SIDE as u32);
    let draw_height = ((f64::from(height) * scale).round() as u32).clamp(1, ICON_SIDE as u32);
    let scaled = image::imageops::resize(
        &decoded,
        draw_width,
        draw_height,
        image::imageops::FilterType::Triangle,
    );
    let mut canvas = image::RgbaImage::new(ICON_SIDE as u32, ICON_SIDE as u32);
    image::imageops::overlay(
        &mut canvas,
        &scaled,
        i64::from((ICON_SIDE as u32 - draw_width) / 2),
        i64::from((ICON_SIDE as u32 - draw_height) / 2),
    );
    let raw = canvas.into_raw();
    (raw.len() == RGBA32_BYTES).then_some(raw)
}

#[derive(Debug, PartialEq, Eq)]
struct HeadIcon {
    href: String,
    /// Largest declared square side, if any.
    side: Option<u32>,
    touch: bool,
}

/// Icon links in a document head, best first: the largest declared square
/// side at most [`MAX_CANDIDATE_SIDE`], then touch icons, then undeclared.
/// A closed tag scan; nothing is executed and no markup is trusted.
fn head_icons(html: &str, base: &Url) -> Vec<Url> {
    let mut icons = parse_head_icons(html);
    icons.sort_by_key(|icon| (std::cmp::Reverse(icon.side.unwrap_or(0)), !icon.touch));
    let mut seen = Vec::new();
    for icon in icons {
        let Ok(url) = base.join(&icon.href) else {
            continue;
        };
        if !seen.contains(&url) {
            seen.push(url);
        }
    }
    seen
}

fn parse_head_icons(html: &str) -> Vec<HeadIcon> {
    let lower = html.to_ascii_lowercase();
    let end = ["</head", "<body"]
        .iter()
        .filter_map(|marker| lower.find(marker))
        .min()
        .unwrap_or(lower.len());
    let mut icons = Vec::new();
    let mut cursor = 0;
    while let Some(offset) = lower[cursor..end].find("<link") {
        let start = cursor + offset + "<link".len();
        let Some(close) = lower[start..end].find('>') else {
            break;
        };
        cursor = start + close + 1;
        if !lower[start..].starts_with(|c: char| c.is_ascii_whitespace() || c == '/') {
            continue;
        }
        let attributes = attributes(&html[start..start + close]);
        let value = |name: &str| {
            attributes
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.as_str())
        };
        let rel = value("rel").unwrap_or("").to_ascii_lowercase();
        let tokens: Vec<&str> = rel.split_ascii_whitespace().collect();
        let touch = tokens
            .iter()
            .any(|token| matches!(*token, "apple-touch-icon" | "apple-touch-icon-precomposed"));
        if !touch && !tokens.contains(&"icon") {
            continue;
        }
        let Some(href) = value("href")
            .map(str::trim)
            .filter(|href| !href.is_empty() && href.len() <= 2048 && !href.starts_with("data:"))
        else {
            continue;
        };
        let svg = value("type").is_some_and(|kind| kind.eq_ignore_ascii_case("image/svg+xml"))
            || href
                .to_ascii_lowercase()
                .split(['?', '#'])
                .next()
                .is_some_and(|path| path.ends_with(".svg"));
        if svg {
            continue;
        }
        let sizes = value("sizes").unwrap_or("");
        let side = square_side(sizes);
        if side.is_none() && squares(sizes).next().is_some() {
            continue;
        }
        icons.push(HeadIcon {
            href: href.to_owned(),
            side,
            touch,
        });
        if icons.len() == MAX_LINKS {
            break;
        }
    }
    icons
}

fn squares(sizes: &str) -> impl Iterator<Item = u32> + '_ {
    sizes.split_ascii_whitespace().filter_map(|size| {
        let (width, height) = size.split_once(['x', 'X'])?;
        let width: u32 = width.parse().ok()?;
        (width > 0 && height.parse::<u32>().ok()? == width).then_some(width)
    })
}

/// The largest declared square side at most [`MAX_CANDIDATE_SIDE`].
fn square_side(sizes: &str) -> Option<u32> {
    squares(sizes)
        .filter(|side| *side <= MAX_CANDIDATE_SIDE)
        .max()
}

/// Attribute pairs of one tag: quoted, single-quoted or bare values; names
/// are lowercased, entities are not expanded beyond `&amp;`.
fn attributes(tag: &str) -> Vec<(String, String)> {
    let bytes = tag.as_bytes();
    let mut pairs = Vec::new();
    let mut index = 0;
    while index < bytes.len() && pairs.len() < 32 {
        while index < bytes.len() && (bytes[index].is_ascii_whitespace() || bytes[index] == b'/') {
            index += 1;
        }
        let name_start = index;
        while index < bytes.len()
            && !bytes[index].is_ascii_whitespace()
            && !matches!(bytes[index], b'=' | b'/')
        {
            index += 1;
        }
        if index == name_start {
            break;
        }
        let name = tag[name_start..index].to_ascii_lowercase();
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        if bytes.get(index) != Some(&b'=') {
            pairs.push((name, String::new()));
            continue;
        }
        index += 1;
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        let value = match bytes.get(index) {
            Some(&quote @ (b'"' | b'\'')) => {
                let start = index + 1;
                let end = tag[start..]
                    .find(quote as char)
                    .map_or(tag.len(), |offset| start + offset);
                index = end + 1;
                &tag[start..end]
            }
            _ => {
                let start = index;
                while index < bytes.len() && !bytes[index].is_ascii_whitespace() {
                    index += 1;
                }
                &tag[start..index]
            }
        };
        pairs.push((name, value.replace("&amp;", "&")));
    }
    pairs
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    fn base() -> Url {
        Url::parse("https://example.com/").unwrap()
    }

    fn icons(html: &str) -> Vec<String> {
        head_icons(html, &base())
            .into_iter()
            .map(String::from)
            .collect()
    }

    #[test]
    fn head_parser_reads_the_five_shapes() {
        // Only /favicon.ico: the head names nothing.
        assert!(icons("<html><head><title>x</title></head></html>").is_empty());
        // A plain icon link, attribute order and case free.
        assert_eq!(
            icons(r#"<HEAD><Link HREF="/static/icon.png" REL="Shortcut Icon"></HEAD>"#),
            ["https://example.com/static/icon.png"]
        );
        // A touch icon is taken, and a larger declared square wins.
        assert_eq!(
            icons(
                r#"<head><link rel="icon" href="/16.png" sizes="16x16">
                <link rel=apple-touch-icon href='/touch.png' sizes=180x180></head>"#
            ),
            [
                "https://example.com/touch.png",
                "https://example.com/16.png"
            ]
        );
        // A relative href resolves against the page, and too-large or
        // non-square icons are skipped.
        assert_eq!(
            icons(
                r#"<head><base href="/ignored/"><link rel="icon" href="img/a.ico?v=2&amp;x=1">
                <link rel="icon" href="/huge.png" sizes="512x512">
                <link rel="icon" href="/wide.png" sizes="64x32"></head>"#
            ),
            [
                "https://example.com/img/a.ico?v=2&x=1",
                "https://example.com/wide.png"
            ]
        );
        // No icon: stylesheets, SVG, data URIs and links after the head.
        assert!(icons(
            r#"<head><link rel="stylesheet" href="/a.css"><link rel="icon" href="/a.svg">
            <link rel="icon" type="image/svg+xml" href="/b"><link rel="icon" href="data:image/png;base64,AA=="></head>
            <body><link rel="icon" href="/late.png"></body>"#
        )
        .is_empty());
    }

    #[test]
    fn square_sizes_pick_the_largest_admitted_side() {
        assert_eq!(square_side("16x16 32x32 512x512"), Some(32));
        assert_eq!(square_side("any"), None);
        assert_eq!(square_side("48X48"), Some(48));
        assert_eq!(square_side("0x0 20x21"), None);
    }

    #[test]
    fn product_policy_admits_only_public_https() {
        let policy = ProbePolicy::PRODUCT;
        assert!(policy.admits(&base()));
        for refused in [
            "http://example.com/",
            "https://127.0.0.1/",
            "https://localhost/",
            "https://router.local/",
        ] {
            assert!(!policy.admits(&Url::parse(refused).unwrap()), "{refused}");
        }
    }

    fn png(width: u32, height: u32) -> Vec<u8> {
        let image = image::RgbaImage::from_pixel(width, height, image::Rgba([200, 10, 10, 255]));
        let mut bytes = Vec::new();
        image
            .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
            .unwrap();
        bytes
    }

    #[test]
    fn icons_decode_to_the_fixed_centred_raster() {
        let rgba = rasterize(&png(64, 32)).unwrap();
        assert_eq!(rgba.len(), RGBA32_BYTES);
        let alpha = |x: usize, y: usize| rgba[(y * ICON_SIDE + x) * 4 + 3];
        assert_eq!(alpha(16, 0), 0);
        assert_eq!(alpha(16, 16), 255);
        assert!(rasterize(b"<svg xmlns='http://www.w3.org/2000/svg'/>").is_none());
        assert!(rasterize(&png(2048, 2048)).is_none());
    }

    /// Serves each connection the next canned response, and records paths.
    fn serve(responses: Vec<Vec<u8>>) -> (String, std::sync::mpsc::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let (paths, seen) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for response in responses {
                let Ok((mut stream, _)) = listener.accept() else {
                    return;
                };
                let mut request = [0u8; 2048];
                let read = stream.read(&mut request).unwrap_or(0);
                let line = String::from_utf8_lossy(&request[..read]);
                let path = line.split_whitespace().nth(1).unwrap_or("").to_owned();
                let _ = paths.send(path);
                let _ = stream.write_all(&response);
            }
        });
        (origin, seen)
    }

    fn ok(content_type: &str, body: &[u8]) -> Vec<u8> {
        let mut response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .into_bytes();
        response.extend_from_slice(body);
        response
    }

    fn redirect(location: &str) -> Vec<u8> {
        format!("HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .into_bytes()
    }

    const LOCAL: ProbePolicy = ProbePolicy {
        public_https: false,
    };

    fn run<T>(future: impl std::future::Future<Output = T>) -> T {
        tauri::async_runtime::block_on(future)
    }

    #[test]
    fn a_missing_ico_falls_back_to_the_head_link() {
        let icon = png(16, 16);
        let (origin, paths) = serve(vec![
            b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
            ok(
                "text/html",
                br#"<head><link rel="icon" href="/brand.png"></head>"#,
            ),
            ok("image/png", &icon),
        ]);
        let client = client(LOCAL).unwrap();
        let (rgba, bytes, outcome) = run(probe(&client, LOCAL, &origin));
        assert_eq!(outcome, ProbeOutcome::Found);
        assert_eq!(rgba.map(|rgba| rgba.len()), Some(RGBA32_BYTES));
        assert!(bytes > icon.len());
        let seen: Vec<String> = paths.try_iter().collect();
        assert_eq!(seen, ["/favicon.ico", "/", "/brand.png"]);
    }

    #[test]
    fn an_icon_over_the_cap_is_refused_and_a_document_is_cut_at_its_cap() {
        let (origin, _) = serve(vec![
            ok("image/png", &vec![0; MAX_ICON_BYTES + 1]),
            ok("text/html", &vec![b' '; MAX_HEAD_BYTES * 2]),
        ]);
        let client = client(LOCAL).unwrap();
        let base = Url::parse(&origin).unwrap();
        assert_eq!(
            run(fetch(
                &client,
                base.join("/favicon.ico").unwrap(),
                MAX_ICON_BYTES,
                false
            )),
            Err(ProbeOutcome::TooLarge)
        );
        let (document, _) = run(fetch(&client, base, MAX_HEAD_BYTES, true)).unwrap();
        assert_eq!(document.len(), MAX_HEAD_BYTES);
    }

    #[test]
    fn at_most_two_redirects_are_followed() {
        let icon = png(8, 8);
        let (origin, _) = serve(vec![
            redirect("/one"),
            redirect("/two"),
            ok("image/png", &icon),
        ]);
        let client = client(LOCAL).unwrap();
        let url = Url::parse(&origin).unwrap().join("/favicon.ico").unwrap();
        let (bytes, landed) = run(fetch(&client, url, MAX_ICON_BYTES, false)).unwrap();
        assert_eq!(bytes, icon);
        assert_eq!(landed.path(), "/two");

        let (origin, _) = serve(vec![
            redirect("/one"),
            redirect("/two"),
            redirect("/three"),
            ok("image/png", &icon),
        ]);
        let url = Url::parse(&origin).unwrap().join("/favicon.ico").unwrap();
        assert_eq!(
            run(fetch(&client, url, MAX_ICON_BYTES, false)),
            Err(ProbeOutcome::Unavailable)
        );
    }

    #[test]
    fn a_redirect_off_https_is_not_followed_by_the_product_client() {
        let policy = ProbePolicy::PRODUCT;
        assert!(!policy.admits(&Url::parse("http://example.com/favicon.ico").unwrap()));
        assert!(client(policy).is_some());
    }
}
