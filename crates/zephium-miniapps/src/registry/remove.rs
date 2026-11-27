use crate::error::MiniAppError;
use crate::model::MiniApp;
use crate::registry::state::Registry;

impl Registry {
    pub fn remove(&mut self, id: &str) -> Result<MiniApp, MiniAppError> {
        let at = self
            .apps
            .iter()
            .position(|app| app.id == id)
            .ok_or(MiniAppError::NotFound)?;
        Ok(self.apps.remove(at))
    }
}
