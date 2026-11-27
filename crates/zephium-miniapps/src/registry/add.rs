use crate::error::MiniAppError;
use crate::model::MiniApp;
use crate::registry::state::{Registry, MAX_INSTALLED_APPS};

impl Registry {
    /// Installs or replaces by start-address identity. Replacement preserves
    /// list position so a reinstall does not reshuffle the person's shelf.
    pub fn add(&mut self, app: MiniApp) -> Result<(), MiniAppError> {
        if let Some(existing) = self.apps.iter_mut().find(|existing| existing.id == app.id) {
            *existing = app;
            return Ok(());
        }
        if self.apps.len() >= MAX_INSTALLED_APPS {
            return Err(MiniAppError::RegistryFull);
        }
        self.apps.push(app);
        Ok(())
    }
}
