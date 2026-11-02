//! The icon addresses a page declares, as the page reports them and as the
//! browser ranks them. The page only names addresses; it never supplies an
//! image, and every field is validated here before anything is fetched.

use url::Url;

/// Longest report the page may send: sixteen candidates of a few hundred
/// bytes each, with room for long query strings.
pub const MAX_REPORT_BYTES: usize = 64 * 1024;
pub const MAX_CANDIDATES: usize = 16;
/// Links the page may name before the report is refused as hostile.
const MAX_LINKS: usize = 64;
pub const MAX_CANDIDATE_URL_BYTES: usize = 2048;
/// The raster is 32 px and drawn at up to 2x on a small row, so a source of
/// this side or twice it is drawn without upscaling or much loss.
pub const WANTED_SIDE: u32 = 32;
const SHARP_SIDE: u32 = WANTED_SIDE * 2;
const MAX_DECLARED_SIDE: u32 = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum IconKind {
    /// `rel` icon or shortcut icon.
    Icon,
    /// `rel` apple-touch-icon, with or without precomposed.
    Touch,
    /// `rel` mask-icon, a monochrome vector made for pinned tabs.
    Mask,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IconCandidate {
    pub url: String,
    pub kind: IconKind,
    /// The largest declared square side, if the link declares one.
    pub side: Option<u32>,
    /// `sizes="any"` or a vector format: it fits any side.
    pub scalable: bool,
    pub svg: bool,
}

impl IconCandidate {
    /// Whether the privileged side may fetch and decode it: a bitmap format
    /// its bounded decoders handle. Vectors are rasterized by the page.
    pub fn bitmap(&self) -> bool {
        !self.svg && self.kind != IconKind::Mask
    }

    fn fit(&self) -> (u8, u32) {
        if self.scalable {
            return (0, 0);
        }
        match self.side {
            Some(side) if (WANTED_SIDE..=SHARP_SIDE).contains(&side) => (0, side - WANTED_SIDE),
            None => (1, 0),
            Some(side) if side > SHARP_SIDE => (2, side - SHARP_SIDE),
            Some(side) => (3, WANTED_SIDE - side),
        }
    }
}

/// One report from a page: the address it was made for, whether the page is
/// done changing its icon links, and what they name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CandidateReport {
    pub page_url: String,
    pub complete: bool,
    /// Names this exact set of links; a watch resumes from it.
    pub token: String,
    pub candidates: Vec<IconCandidate>,
}

/// Reads the page's report: `{"u": address, "d": 0|1, "k": token, "c": [[rel,
/// href, sizes, type], ...]}`. Anything else is refused whole.
pub fn parse_report(json: &str) -> Option<CandidateReport> {
    #[derive(serde::Deserialize)]
    struct Wire {
        u: String,
        d: u8,
        k: String,
        c: Vec<(String, String, String, String)>,
    }
    if json.len() > MAX_REPORT_BYTES {
        return None;
    }
    let wire: Wire = serde_json::from_str(json).ok()?;
    if wire.u.len() > MAX_CANDIDATE_URL_BYTES * 4
        || wire.d > 1
        || wire.c.len() > MAX_LINKS
        || wire.k.is_empty()
        || wire.k.len() > 16
        || !wire.k.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return None;
    }
    let mut candidates: Vec<IconCandidate> = Vec::new();
    for (rel, href, sizes, kind) in wire.c {
        let Some(candidate) = candidate(&rel, &href, &sizes, &kind) else {
            continue;
        };
        if candidates.len() < MAX_CANDIDATES && !candidates.iter().any(|c| c.url == candidate.url) {
            candidates.push(candidate);
        }
    }
    Some(CandidateReport {
        page_url: wire.u,
        complete: wire.d == 1,
        token: wire.k,
        candidates,
    })
}

fn candidate(rel: &str, href: &str, sizes: &str, media_type: &str) -> Option<IconCandidate> {
    let kind = match rel {
        "i" => IconKind::Icon,
        "t" => IconKind::Touch,
        "m" => IconKind::Mask,
        _ => return None,
    };
    if href.len() > MAX_CANDIDATE_URL_BYTES {
        return None;
    }
    let mut url = Url::parse(href).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return None;
    }
    url.set_fragment(None);
    let svg = media_type
        .split(';')
        .next()
        .is_some_and(|kind| kind.trim().eq_ignore_ascii_case("image/svg+xml"))
        || url.path().to_ascii_lowercase().ends_with(".svg");
    let (side, any) = declared_sides(sizes);
    Some(IconCandidate {
        url: url.into(),
        kind,
        side,
        scalable: svg || any,
        svg,
    })
}

/// The largest declared square side, and whether `any` was declared. Sides
/// that are not square, zero or absurd are not declarations.
fn declared_sides(sizes: &str) -> (Option<u32>, bool) {
    let mut side = None;
    let mut any = false;
    for token in sizes.split_ascii_whitespace().take(16) {
        if token.eq_ignore_ascii_case("any") {
            any = true;
            continue;
        }
        let Some((width, height)) = token.split_once(['x', 'X']) else {
            continue;
        };
        let (Ok(width), Ok(height)) = (width.parse::<u32>(), height.parse::<u32>()) else {
            continue;
        };
        if width > 0 && width == height && width <= MAX_DECLARED_SIDE {
            side = side.max(Some(width));
        }
    }
    (side, any)
}

/// What a page's links name, best first: icons before touch icons before
/// masks, and within a kind the side closest to the wanted one without
/// upscaling. The site's own `/favicon.ico` comes last, as the one icon a
/// browser asks for without being told.
pub fn rank(page: &Url, candidates: &[IconCandidate]) -> Vec<IconCandidate> {
    let mut ranked: Vec<IconCandidate> = candidates.to_vec();
    ranked.sort_by_key(|candidate| (candidate.kind, candidate.fit()));
    if let Some(fallback) = default_icon(page) {
        if !ranked.iter().any(|candidate| candidate.url == fallback.url) {
            ranked.push(fallback);
        }
    }
    ranked
}

fn default_icon(page: &Url) -> Option<IconCandidate> {
    let mut url = page.join("/favicon.ico").ok()?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return None;
    }
    url.set_fragment(None);
    let _ = url.set_username("");
    let _ = url.set_password(None);
    Some(IconCandidate {
        url: url.into(),
        kind: IconKind::Icon,
        side: None,
        scalable: false,
        svg: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(candidates: &[(&str, &str, &str, &str)]) -> String {
        serde_json::json!({
            "u": "https://example.com/a",
            "d": 1,
            "k": "1a2b3c4d",
            "c": candidates,
        })
        .to_string()
    }

    fn urls(ranked: &[IconCandidate]) -> Vec<&str> {
        ranked.iter().map(|c| c.url.as_str()).collect()
    }

    fn page() -> Url {
        Url::parse("https://example.com/a/b?q=1#f").unwrap()
    }

    #[test]
    fn a_report_keeps_cross_origin_and_touch_icons_and_drops_what_cannot_be_fetched() {
        let parsed = parse_report(&report(&[
            (
                "i",
                "https://ssl.gstatic.com/docs/doc_32.png",
                "32x32",
                "image/png",
            ),
            ("t", "https://example.com/touch.png", "180x180", ""),
            ("m", "https://example.com/pin.svg", "", ""),
            ("i", "http://example.com/plain.ico", "", ""),
            ("i", "data:image/png;base64,AAAA", "", ""),
            ("i", "javascript:alert(1)", "", ""),
            ("i", "https://user:pw@example.com/a.png", "", ""),
            ("i", "not a url", "", ""),
            ("x", "https://example.com/other.png", "", ""),
            (
                "i",
                "https://ssl.gstatic.com/docs/doc_32.png#again",
                "32x32",
                "",
            ),
        ]))
        .unwrap();
        assert_eq!(parsed.page_url, "https://example.com/a");
        assert_eq!(parsed.token, "1a2b3c4d");
        assert!(parsed.complete);
        assert_eq!(
            urls(&parsed.candidates),
            [
                "https://ssl.gstatic.com/docs/doc_32.png",
                "https://example.com/touch.png",
                "https://example.com/pin.svg",
                "http://example.com/plain.ico",
            ]
        );
        let kinds: Vec<_> = parsed.candidates.iter().map(|c| c.kind).collect();
        assert_eq!(
            kinds,
            [
                IconKind::Icon,
                IconKind::Touch,
                IconKind::Mask,
                IconKind::Icon
            ]
        );
        assert_eq!(parsed.candidates[0].side, Some(32));
        assert!(parsed.candidates[2].svg && parsed.candidates[2].scalable);
    }

    #[test]
    fn a_malformed_or_oversized_report_is_refused_whole() {
        assert!(parse_report("not json").is_none());
        assert!(parse_report(r#"{"u":"https://a.example/","d":2,"k":"a","c":[]}"#).is_none());
        assert!(
            parse_report(r#"{"u":"https://a.example/","d":1,"k":"a","c":[["i","x"]]}"#).is_none()
        );
        assert!(parse_report(r#"{"u":"https://a.example/","d":1,"k":"a"}"#).is_none());
        assert!(parse_report(r#"{"u":"https://a.example/","d":1,"c":[]}"#).is_none());
        for token in ["", "xyz", "0123456789abcdef0", "\");alert(1)"] {
            let json = serde_json::json!({"u": "https://a.example/", "d": 1, "k": token, "c": []});
            assert!(parse_report(&json.to_string()).is_none(), "{token}");
        }
        assert!(parse_report(&" ".repeat(MAX_REPORT_BYTES + 1)).is_none());
        let long = format!(
            "https://example.com/{}",
            "a".repeat(MAX_CANDIDATE_URL_BYTES)
        );
        let parsed = parse_report(&report(&[("i", &long, "", "")])).unwrap();
        assert!(parsed.candidates.is_empty());
        let many: Vec<(&str, String, &str, &str)> = (0..=MAX_LINKS)
            .map(|n| ("i", format!("https://example.com/{n}.png"), "", ""))
            .collect();
        let json = serde_json::json!({"u": "https://example.com/", "d": 0, "k": "a", "c": many})
            .to_string();
        assert!(parse_report(&json).is_none());
        let some: Vec<(&str, String, &str, &str)> = (0..40)
            .map(|n| ("i", format!("https://example.com/{n}.png"), "", ""))
            .collect();
        let json = serde_json::json!({"u": "https://example.com/", "d": 0, "k": "a", "c": some})
            .to_string();
        let parsed = parse_report(&json).unwrap();
        assert_eq!(parsed.candidates.len(), MAX_CANDIDATES);
        assert!(!parsed.complete);
    }

    #[test]
    fn sizes_declare_squares_only_and_any_means_scalable() {
        assert_eq!(declared_sides("16x16 32x32 512x512"), (Some(512), false));
        assert_eq!(declared_sides("48X48"), (Some(48), false));
        assert_eq!(declared_sides("any"), (None, true));
        assert_eq!(declared_sides("0x0 20x21 64x32 axb"), (None, false));
        assert_eq!(declared_sides("99999x99999"), (None, false));
        assert_eq!(declared_sides(""), (None, false));
    }

    #[test]
    fn the_size_closest_to_the_wanted_one_without_upscaling_ranks_first() {
        let link = |url: &str, sizes: &str| ("i", url.to_owned(), sizes.to_owned(), "".to_owned());
        let owned = [
            link("https://example.com/16.png", "16x16"),
            link("https://example.com/512.png", "512x512"),
            link("https://example.com/none.ico", ""),
            link("https://example.com/180.png", "180x180"),
            link("https://example.com/32.png", "32x32"),
            link("https://example.com/48.png", "48x48"),
            link("https://example.com/64.png", "64x64"),
            link("https://example.com/8.png", "8x8"),
        ];
        let json = serde_json::json!({"u": "https://example.com/", "d": 1, "k": "a", "c": owned})
            .to_string();
        let ranked = rank(&page(), &parse_report(&json).unwrap().candidates);
        assert_eq!(
            urls(&ranked),
            [
                "https://example.com/32.png",
                "https://example.com/48.png",
                "https://example.com/64.png",
                "https://example.com/none.ico",
                "https://example.com/180.png",
                "https://example.com/512.png",
                "https://example.com/16.png",
                "https://example.com/8.png",
                "https://example.com/favicon.ico",
            ]
        );
    }

    #[test]
    fn kind_outranks_size_and_a_vector_fits_any_side() {
        let parsed = parse_report(&report(&[
            ("t", "https://example.com/touch.png", "180x180", ""),
            ("m", "https://example.com/mask.svg", "", ""),
            ("i", "https://example.com/small.png", "16x16", ""),
            ("i", "https://example.com/logo.svg", "", "image/svg+xml"),
            ("i", "https://example.com/any.ico", "any", ""),
        ]))
        .unwrap();
        let ranked = rank(&page(), &parsed.candidates);
        assert_eq!(ranked[0].url, "https://example.com/logo.svg");
        assert!(ranked[0].svg && !ranked[0].bitmap());
        assert_eq!(ranked[1].url, "https://example.com/any.ico");
        assert!(ranked[1].scalable && ranked[1].bitmap());
        assert_eq!(ranked[2].url, "https://example.com/small.png");
        assert_eq!(ranked[3].url, "https://example.com/touch.png");
        assert_eq!(ranked[4].url, "https://example.com/mask.svg");
        assert!(!ranked[4].bitmap());
        assert_eq!(
            ranked.last().unwrap().url,
            "https://example.com/favicon.ico"
        );
    }

    #[test]
    fn the_default_icon_is_the_pages_own_origin_and_never_listed_twice() {
        let ranked = rank(&Url::parse("https://app.example.com:8443/x").unwrap(), &[]);
        assert_eq!(urls(&ranked), ["https://app.example.com:8443/favicon.ico"]);
        let declared =
            parse_report(&report(&[("i", "https://example.com/favicon.ico", "", "")])).unwrap();
        assert_eq!(rank(&page(), &declared.candidates).len(), 1);
        assert!(rank(&Url::parse("file:///tmp/a.html").unwrap(), &[]).is_empty());
    }
}
