//! Saved page zoom, as Settings lists it.

use serde::{Deserialize, Serialize};
use specta::Type;

use zephium_core::time::MAX_SITE_BYTES;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ZoomCall {
    /// The sites the profile zoomed, by name.
    Levels,
    /// Shows one site at the default zoom again.
    Forget {
        site: String,
    },
    ForgetAll,
}

impl ZoomCall {
    pub fn validate(&self) -> bool {
        match self {
            Self::Forget { site } => !site.is_empty() && site.len() <= MAX_SITE_BYTES,
            Self::Levels | Self::ForgetAll => true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct ZoomLevelView {
    pub site: String,
    pub percent: u16,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ZoomResponse {
    /// The profile's levels after the call, by site.
    Levels {
        levels: Vec<ZoomLevelView>,
    },
    Error {
        error: ZoomError,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum ZoomError {
    Invalid,
    Unavailable,
    Capacity,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_forgotten_site_is_named_and_bounded() {
        assert!(ZoomCall::Levels.validate());
        assert!(ZoomCall::ForgetAll.validate());
        assert!(ZoomCall::Forget {
            site: "github.com".into()
        }
        .validate());
        assert!(!ZoomCall::Forget { site: "".into() }.validate());
        assert!(!ZoomCall::Forget {
            site: "a".repeat(MAX_SITE_BYTES + 1)
        }
        .validate());
    }
}
