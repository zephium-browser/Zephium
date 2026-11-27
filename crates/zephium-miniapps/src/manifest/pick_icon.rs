use crate::policy::normalize_https_url::normalize_https_url;
use serde::Deserialize;
use url::Url;

#[derive(Deserialize)]
pub struct RawIcon {
    pub src: String,
    #[serde(default)]
    pub sizes: String,
    #[serde(default, rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub purpose: String,
}

/// The largest raster or vector icon at or below 1024 px. The bytes are not
/// fetched here; only a bounded HTTPS address is retained.
pub fn pick_icon(icons: &[RawIcon], base: &Url) -> Option<String> {
    let mut best: Option<(u32, String)> = None;
    for icon in icons.iter().take(32) {
        if icon
            .purpose
            .split_whitespace()
            .any(|value| value == "monochrome")
        {
            continue;
        }
        if !icon.kind.is_empty() && !icon.kind.starts_with("image/") {
            continue;
        }
        let Ok(url) = normalize_https_url(base, &icon.src) else {
            continue;
        };
        let size = largest_size(&icon.sizes);
        let area = size.0.saturating_mul(size.1);
        if area > 1_024 * 1_024 {
            continue;
        }
        let better = best.as_ref().is_none_or(|(current, _)| area > *current);
        if better {
            best = Some((area, url.to_string()));
        }
    }
    best.map(|(_, url)| url)
}

/// Accepts `192x192`, `any`, and the space-separated list form. A missing or
/// unparseable size is treated as one small icon so it never outranks a
/// genuinely large one.
fn largest_size(sizes: &str) -> (u32, u32) {
    sizes
        .split_whitespace()
        .filter_map(|entry| {
            let (width, height) = entry.split_once('x')?;
            let width: u32 = width.parse().ok()?;
            let height: u32 = height.parse().ok()?;
            Some((width, height))
        })
        .max_by_key(|(width, height)| width.saturating_mul(*height))
        .unwrap_or((1, 1))
}
