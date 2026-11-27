use crate::model::MiniApp;
use serde::{Deserialize, Serialize};

pub const REGISTRY_VERSION: u8 = 1;
pub const MAX_INSTALLED_APPS: usize = 64;
pub const MAX_REGISTRY_BYTES: usize = 512 * 1024;

/// The whole durable installation set. It is deliberately a plain JSON
/// document: one bounded write, one bounded read, and no schema migration
/// machinery for a list a person can count.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Registry {
    #[serde(default = "default_version")]
    pub(crate) version: u8,
    #[serde(default)]
    pub(crate) apps: Vec<MiniApp>,
}

fn default_version() -> u8 {
    REGISTRY_VERSION
}

impl Default for Registry {
    fn default() -> Self {
        Self {
            version: REGISTRY_VERSION,
            apps: Vec::new(),
        }
    }
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn apps(&self) -> &[MiniApp] {
        &self.apps
    }

    pub fn find(&self, id: &str) -> Option<&MiniApp> {
        self.apps.iter().find(|app| app.id == id)
    }
}
