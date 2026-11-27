use crate::model::DisplayMode;
use serde::{Deserialize, Serialize};

/// The durable identity of one installed mini app. The optional icon is a
/// bounded 32x32 RGBA raster for the settings UI. Windows taskbar icon files
/// are derived separately from the fetched source and kept outside the registry.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MiniApp {
    pub id: String,
    /// Per-install storage identity. Empty is the legacy id-derived path.
    #[serde(default)]
    pub storage_id: String,
    pub name: String,
    pub start_url: String,
    pub scope: String,
    pub display: DisplayMode,
    pub theme_color: Option<String>,
    pub background_color: Option<String>,
    pub icon: Option<String>,
    /// Seconds since the Unix epoch. Supplied by the caller so the crate stays
    /// free of a clock and deterministic in tests.
    pub installed_at: u64,
}
