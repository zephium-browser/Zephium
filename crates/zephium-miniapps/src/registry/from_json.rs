use crate::error::MiniAppError;
use crate::policy::app_id::app_id;
use crate::policy::is_in_scope::is_in_scope;
use crate::policy::same_origin::same_origin;
use crate::registry::state::{Registry, MAX_INSTALLED_APPS, MAX_REGISTRY_BYTES, REGISTRY_VERSION};
use url::Url;

impl Registry {
    /// Reads the one durable document. An empty value is a fresh browser, not
    /// corruption. Every entry is re-validated because the bytes may have been
    /// edited, truncated, or produced by a future build.
    pub fn from_json(raw: &str) -> Result<Self, MiniAppError> {
        if raw.trim().is_empty() {
            return Ok(Self::default());
        }
        if raw.len() > MAX_REGISTRY_BYTES {
            return Err(MiniAppError::RegistryTooLarge);
        }
        let registry: Self =
            serde_json::from_str(raw).map_err(|_| MiniAppError::InvalidRegistry)?;
        if registry.version != REGISTRY_VERSION || registry.apps.len() > MAX_INSTALLED_APPS {
            return Err(MiniAppError::InvalidRegistry);
        }
        let mut seen = std::collections::HashSet::with_capacity(registry.apps.len());
        for app in &registry.apps {
            if app.id.len() != 32
                || !app.id.bytes().all(|byte| byte.is_ascii_hexdigit())
                || (!app.storage_id.is_empty()
                    && (app.storage_id.len() != 32
                        || !app.storage_id.bytes().all(|byte| byte.is_ascii_hexdigit())))
                || app.name.is_empty()
                || app.name.len() > 320
                || !seen.insert(app.id.as_str())
            {
                return Err(MiniAppError::InvalidRegistry);
            }
            let start = Url::parse(&app.start_url).map_err(|_| MiniAppError::InvalidRegistry)?;
            let scope = Url::parse(&app.scope).map_err(|_| MiniAppError::InvalidRegistry)?;
            if app.id != app_id(&app.start_url)
                || start.scheme() != "https"
                || !start.username().is_empty()
                || start.password().is_some()
                || scope.scheme() != "https"
                || !same_origin(&start, &scope)
                || !is_in_scope(&start, &scope)
            {
                return Err(MiniAppError::InvalidRegistry);
            }
        }
        Ok(registry)
    }
}
