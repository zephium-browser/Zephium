use serde::{Deserialize, Serialize};

/// How much browser chrome an installed app asks for. The engine honours the
/// request only as a hint; fullscreen never bypasses the operating system's own
/// permission prompts.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DisplayMode {
    Browser,
    MinimalUi,
    #[default]
    Standalone,
    Fullscreen,
}

impl DisplayMode {
    pub fn from_manifest(value: Option<&str>) -> Self {
        match value
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str()
        {
            "fullscreen" => Self::Fullscreen,
            "standalone" => Self::Standalone,
            "minimal-ui" => Self::MinimalUi,
            _ => Self::Browser,
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Self::Browser => "browser",
            Self::MinimalUi => "minimal-ui",
            Self::Standalone => "standalone",
            Self::Fullscreen => "fullscreen",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_display_values_fall_back_to_in_page_browsing() {
        assert_eq!(DisplayMode::from_manifest(None), DisplayMode::Browser);
        assert_eq!(
            DisplayMode::from_manifest(Some(" STANDALONE ")),
            DisplayMode::Standalone
        );
        assert_eq!(
            DisplayMode::from_manifest(Some("minimal-ui")),
            DisplayMode::MinimalUi
        );
        assert_eq!(
            DisplayMode::from_manifest(Some("window-controls-overlay")),
            DisplayMode::Browser
        );
    }
}
