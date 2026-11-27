use crate::error::MiniAppError;
use crate::registry::state::{Registry, MAX_REGISTRY_BYTES};

impl Registry {
    pub fn to_json(&self) -> Result<String, MiniAppError> {
        let json = serde_json::to_string(self).map_err(|_| MiniAppError::InvalidRegistry)?;
        if json.len() > MAX_REGISTRY_BYTES {
            return Err(MiniAppError::RegistryTooLarge);
        }
        Ok(json)
    }
}
