//! Fixed-shape favicon values crossing from an untrusted page renderer into
//! privileged browser chrome.
//!
//! The site renderer owns all network fetching and image decoding. Rust and
//! the chrome renderer accept only a 32x32 RGBA raster, so malformed PNG,
//! ICO, SVG, font and animation parsers are never exposed in the privileged
//! process for a page-controlled favicon. The one exception is the Work
//! origin probe, which fetches an icon anonymously (no page, no session) and
//! decodes it with a bounded, memory-safe decoder into this same raster.

use base64::Engine as _;

mod candidates;
pub use candidates::*;

pub const ICON_SIDE: usize = 32;
pub const RGBA32_BYTES: usize = ICON_SIDE * ICON_SIDE * 4;
pub const RGBA32_MIME: &str = "application/x-zephium-rgba32";
// Four-byte RGBA input always has this exact canonical padded length.
pub const RGBA32_BASE64_BYTES: usize = RGBA32_BYTES.div_ceil(3) * 4;
/// Longest page address a page's own icon is kept under; a longer path falls
/// back to the icon of its origin.
pub const MAX_PAGE_KEY_BYTES: usize = 1024;

pub fn validated_rgba32(bytes: &[u8]) -> Option<&[u8]> {
    (bytes.len() == RGBA32_BYTES).then_some(bytes)
}

pub fn decode_rgba32(encoded: &str) -> Option<Vec<u8>> {
    if encoded.len() != RGBA32_BASE64_BYTES {
        return None;
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .ok()?;
    validated_rgba32(&bytes)?;
    // Reject alternate/non-canonical encodings before this value becomes a
    // cache key or crosses another process boundary.
    (base64::engine::general_purpose::STANDARD.encode(&bytes) == encoded).then_some(bytes)
}

pub fn encode_rgba32(bytes: &[u8]) -> Option<String> {
    validated_rgba32(bytes)?;
    Some(base64::engine::general_purpose::STANDARD.encode(bytes))
}

/// Identifies one raster's contents so chrome can cache by origin and repaint
/// only when the pixels actually change. A cache tag, not a security boundary.
pub fn revision(bytes: &[u8]) -> Option<String> {
    validated_rgba32(bytes)?;
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    Some(format!("{hash:016x}"))
}

/// The address a page's own icon is keyed by: its origin and path, without
/// query, fragment or credentials, so every address of one page shares an icon.
/// An origin has no path, which keeps the two kinds of key apart.
pub fn page_key(url: &url::Url) -> Option<String> {
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    let url::Origin::Tuple(..) = url.origin() else {
        return None;
    };
    let key = format!("{}{}", url.origin().ascii_serialization(), url.path());
    (key.len() <= MAX_PAGE_KEY_BYTES).then_some(key)
}

/// Whether `key` is exactly what [`page_key`] would write for the page it names.
pub fn is_page_key(key: &str) -> bool {
    names_page(key)
        && url::Url::parse(key)
            .ok()
            .and_then(|url| page_key(&url))
            .is_some_and(|canonical| canonical == key)
}

/// Whether a favicon key names a page rather than an origin. A shape check
/// only, cheap enough for hot paths; stores validate with [`is_page_key`].
pub fn names_page(key: &str) -> bool {
    key.split_once("://")
        .is_some_and(|(_, rest)| rest.contains('/'))
}

/// The origin a favicon key belongs to: a page key's origin, or the key itself.
pub fn origin_of_key(key: &str) -> &str {
    let Some((scheme, rest)) = key.split_once("://") else {
        return key;
    };
    match rest.find('/') {
        Some(end) => &key[..scheme.len() + 3 + end],
        None => key,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgba_round_trip_is_exact_and_canonical() {
        let bytes: Vec<u8> = (0..RGBA32_BYTES).map(|n| (n % 251) as u8).collect();
        let encoded = encode_rgba32(&bytes).unwrap();
        assert_eq!(encoded.len(), RGBA32_BASE64_BYTES);
        assert_eq!(decode_rgba32(&encoded), Some(bytes));
    }

    #[test]
    fn revision_tracks_pixels_and_rejects_malformed_rasters() {
        let bytes = vec![7; RGBA32_BYTES];
        let mut changed = bytes.clone();
        changed[RGBA32_BYTES - 1] = 8;
        assert_eq!(revision(&bytes), revision(&vec![7; RGBA32_BYTES]));
        assert_ne!(revision(&bytes), revision(&changed));
        assert_eq!(revision(&vec![7; RGBA32_BYTES - 1]), None);
    }

    #[test]
    fn rejects_encoded_containers_and_wrong_sized_rasters() {
        assert_eq!(validated_rgba32(b"<svg><script/></svg>"), None);
        assert_eq!(validated_rgba32(b"\x89PNG\r\n\x1a\n"), None);
        assert_eq!(validated_rgba32(&vec![0; RGBA32_BYTES - 1]), None);
        assert_eq!(validated_rgba32(&vec![0; RGBA32_BYTES + 1]), None);
        assert_eq!(decode_rgba32("AAAA"), None);

        let bytes = vec![7; RGBA32_BYTES];
        let mut encoded = encode_rgba32(&bytes).unwrap();
        encoded.pop();
        assert_eq!(decode_rgba32(&encoded), None);
    }

    #[test]
    fn a_page_key_drops_query_fragment_and_credentials_and_stays_apart_from_origins() {
        let key = |value: &str| page_key(&url::Url::parse(value).unwrap());
        assert_eq!(
            key("https://User:pw@Docs.Example.com:443/sheets/d/1?x=2#gid=3").as_deref(),
            Some("https://docs.example.com/sheets/d/1")
        );
        assert_eq!(
            key("http://example.com:8080").as_deref(),
            Some("http://example.com:8080/")
        );
        assert_eq!(key("ftp://example.com/a"), None);
        assert_eq!(key("data:text/plain,hi"), None);
        assert_eq!(
            key(&format!("https://example.com/{}", "a".repeat(2048))),
            None
        );

        assert!(is_page_key("https://example.com/a/b"));
        assert!(is_page_key("https://example.com/"));
        for refused in [
            "https://example.com",
            "https://example.com/a?b=1",
            "https://example.com/a#b",
            "https://user@example.com/a",
            "https://EXAMPLE.com/a",
            "ftp://example.com/a",
            "not a url",
        ] {
            assert!(!is_page_key(refused), "{refused}");
        }
        assert!(names_page("https://example.com/") && !names_page("https://example.com"));
    }

    #[test]
    fn every_page_key_written_is_one_the_store_accepts() {
        for address in [
            "https://example.com//a",
            "https://example.com/a%20b/c d",
            "https://example.com/\u{fc}/\u{65e5}\u{672c}",
            "https://\u{4f8b}\u{3048}.jp/\u{30d1}\u{30b9}",
            "https://example.com/a/../b/./c",
            "https://example.com/%2e%2e/x",
            "https://example.com/a\\b",
            "http://[::1]:8080/p?q#f",
            "https://example.com:8443",
        ] {
            let key = page_key(&url::Url::parse(address).unwrap()).unwrap();
            assert!(is_page_key(&key), "{address} -> {key}");
            assert!(names_page(&key));
            assert_eq!(
                url::Url::parse(origin_of_key(&key)).unwrap().origin(),
                url::Url::parse(address).unwrap().origin()
            );
        }
    }

    #[test]
    fn the_origin_of_a_key_is_the_key_up_to_its_path() {
        assert_eq!(
            origin_of_key("https://a.example:8443/x/y"),
            "https://a.example:8443"
        );
        assert_eq!(origin_of_key("https://a.example/"), "https://a.example");
        assert_eq!(origin_of_key("https://a.example"), "https://a.example");
        assert_eq!(origin_of_key("garbage"), "garbage");
    }
}
