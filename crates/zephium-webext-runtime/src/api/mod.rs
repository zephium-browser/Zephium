//! Namespace implementations, one module per namespace, each exposing
//! `register(&mut ApiTable<S>)` and listed in [`register`].

use crate::dispatch::{ApiTable, RegistrationError};
pub mod action;
pub mod bookmarks;
pub mod commands;
pub mod declarative_net_request;
pub mod dom;
pub mod extension;
pub mod font_settings;
pub mod generated;
pub mod history;
pub mod idle;
pub mod management;
pub mod notifications;
pub mod permissions;
pub mod runtime;
pub mod scripting;
pub mod search;
pub mod sessions;
pub mod tabs;
pub mod top_sites;
pub mod web_navigation;
pub mod windows;

/// Registers every implemented namespace using generated schemas and feature metadata.
pub fn register<S>(table: &mut ApiTable<S>) -> Result<(), RegistrationError>
where
    S: crate::storage::StorageHost
        + crate::alarms::AlarmHost
        + crate::events::EventsHost
        + crate::ports::MessagingHost
        + crate::i18n::I18nHost
        + crate::scripting_registry::ScriptingHost
        + crate::browser::BrowserPort
        + declarative_net_request::DnrHost
        + 'static,
{
    crate::storage::register(table)?;
    crate::alarms::register(table)?;
    crate::events::register(table)?;
    crate::ports::register(table)?;
    crate::i18n::register(table)?;
    crate::scripting_registry::register(table)?;
    tabs::register(table)?;
    windows::register(table)?;
    runtime::register(table)?;
    action::register(table)?;
    commands::register(table)?;
    declarative_net_request::register(table)?;
    permissions::register(table)?;
    extension::register(table)?;
    management::register(table)?;
    idle::register(table)?;
    dom::register(table)?;
    font_settings::register(table)?;
    notifications::register(table)?;
    scripting::register(table)?;
    web_navigation::register(table)?;
    history::register(table)?;
    bookmarks::register(table)?;
    sessions::register(table)?;
    top_sites::register(table)?;
    search::register(table)?;
    Ok(())
}

#[cfg(test)]
mod tests;
