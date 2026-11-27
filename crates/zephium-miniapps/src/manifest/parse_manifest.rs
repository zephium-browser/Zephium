use crate::error::MiniAppError;
use crate::manifest::normalize_text::normalize_text;
use crate::manifest::pick_color::pick_color;
use crate::manifest::pick_icon::{pick_icon, RawIcon};
use crate::manifest::pick_name::pick_name;
use crate::model::{DisplayMode, MiniApp};
use crate::policy::app_id::app_id;
use crate::policy::is_in_scope::is_in_scope;
use crate::policy::normalize_https_url::normalize_https_url;
use crate::policy::resolve_scope::resolve_scope;
use serde::Deserialize;
use url::Url;

/// A manifest is metadata, never a payload. The cap keeps a hostile site from
/// forcing the installer to buffer an arbitrary document.
pub const MAX_MANIFEST_BYTES: usize = 256 * 1024;

#[derive(Deserialize)]
struct RawManifest {
    name: Option<String>,
    short_name: Option<String>,
    start_url: Option<String>,
    scope: Option<String>,
    display: Option<String>,
    theme_color: Option<String>,
    background_color: Option<String>,
    #[serde(default)]
    icons: Vec<serde_json::Value>,
}

/// Parses one Web App Manifest into an installation descriptor. Relative
/// addresses resolve against the manifest's own URL, as the specification
/// requires, and every accepted address is re-validated after resolution.
pub fn parse_manifest(
    bytes: &[u8],
    manifest_url: &Url,
    installed_at: u64,
) -> Result<MiniApp, MiniAppError> {
    if bytes.len() > MAX_MANIFEST_BYTES {
        return Err(MiniAppError::ManifestTooLarge);
    }
    let raw: RawManifest = serde_json::from_slice(bytes).map_err(|_| MiniAppError::InvalidJson)?;
    let icons = raw
        .icons
        .iter()
        .take(32)
        .filter_map(|icon| serde_json::from_value::<RawIcon>(icon.clone()).ok())
        .collect::<Vec<_>>();
    let start = raw
        .start_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or(MiniAppError::MissingStartUrl)?;
    let mut start_url = normalize_https_url(manifest_url, start)?;
    let query = start_url
        .query()
        .filter(|query| !query.is_empty())
        .map(str::to_owned);
    start_url.set_query(query.as_deref());
    let scope = resolve_scope(manifest_url, &start_url, raw.scope.as_deref())?;
    if !is_in_scope(&start_url, &scope) {
        return Err(MiniAppError::StartOutsideScope);
    }
    let name = pick_name(
        raw.name.as_deref(),
        raw.short_name.as_deref(),
        start_url.host_str(),
    );
    if name.is_empty() || normalize_text(&name, 80).is_none() {
        return Err(MiniAppError::InvalidName);
    }
    let canonical_start = start_url.to_string();
    Ok(MiniApp {
        id: app_id(&canonical_start),
        storage_id: String::new(),
        name,
        start_url: canonical_start,
        scope: scope.to_string(),
        display: DisplayMode::from_manifest(raw.display.as_deref()),
        theme_color: pick_color(raw.theme_color.as_deref()),
        background_color: pick_color(raw.background_color.as_deref()),
        icon: pick_icon(&icons, manifest_url),
        installed_at,
    })
}
