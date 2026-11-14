//! Namespace implementations, one module per namespace, each exposing
//! `register(&mut ApiTable<S>)` and listed in [`register`].

use crate::dispatch::{ApiTable, RegistrationError};
pub mod generated;

/// Registers every implemented namespace using generated schemas and feature metadata.
pub fn register<S>(table: &mut ApiTable<S>) -> Result<(), RegistrationError>
where
    S: crate::storage::StorageHost
        + crate::alarms::AlarmHost
        + crate::events::EventsHost
        + crate::ports::MessagingHost
        + crate::i18n::I18nHost
        + crate::scripting_registry::ScriptingHost
        + 'static,
{
    crate::storage::register(table)?;
    crate::alarms::register(table)?;
    crate::events::register(table)?;
    crate::ports::register(table)?;
    crate::i18n::register(table)?;
    crate::scripting_registry::register(table)?;
    Ok(())
}

#[cfg(test)]
mod tests;
