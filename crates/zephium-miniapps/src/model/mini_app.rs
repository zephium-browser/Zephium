use crate::model::DisplayMode;
use serde::{Deserialize, Serialize};

/// The durable identity of one installed mini app. Icon bytes are never kept
/// here: the engine fetches the icon through its ordinary, protected network
/// path, which keeps the registry small and removes an image decoder from the
/// installation boundary.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MiniApp {
    pub id: String,
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
