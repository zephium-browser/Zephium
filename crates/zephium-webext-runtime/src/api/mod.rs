//! Namespace implementations, one module per namespace, each exposing
//! `register(&mut ApiTable<S>)` and listed in [`register`].

use crate::dispatch::{ApiTable, RegistrationError};
use crate::ApiError;
use serde_json::Value;

/// Replaceable validation boundary for handwritten and generated schemas.
pub trait ArgumentValidator {
    /// Rejects malformed arguments before invoking a service or adapter.
    fn validate(&self, method: &str, args: &[Value]) -> Result<(), ApiError>;
}

/// Registers every implemented namespace.
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
