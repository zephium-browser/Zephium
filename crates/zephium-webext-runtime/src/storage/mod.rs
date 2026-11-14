//! Per-install Chrome storage semantics. Adapters run SQLite work on their
//! storage executor and deliver committed changes through [`StorageHost`].

mod api;
mod backend;
mod quota;
#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::fmt;

use serde::Serialize;
use serde_json::{Map, Value};
use zephium_core::ids::{ExtensionInstallId, ProfileId};

use crate::context::{ContextKind, Sender};
use crate::errors::ApiError;
use crate::limits::{
    STORAGE_LOCAL_QUOTA_BYTES, STORAGE_LOCAL_UNLIMITED_QUOTA_BYTES, STORAGE_SESSION_QUOTA_BYTES,
};

pub use api::{register, StorageHost};
pub use backend::{MemoryBackend, SqliteBackend, StorageBackend};
pub use quota::{
    item_bytes, MAX_ITEMS, MAX_SUSTAINED_WRITE_OPERATIONS_PER_MINUTE,
    MAX_WRITE_OPERATIONS_PER_HOUR, MAX_WRITE_OPERATIONS_PER_MINUTE, QUOTA_BYTES,
    QUOTA_BYTES_PER_ITEM,
};

/// JSON properties stored in one area.
pub type Items = Map<String, Value>;
/// The changed properties of one committed operation.
pub type Changes = BTreeMap<String, StorageChange>;

/// A storage area. Sync is durable local storage with Chrome sync quotas.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Area {
    /// Durable local values.
    Local,
    /// Durable local values, constrained by sync quotas; no cloud transport.
    Sync,
    /// Values retained across background restarts, until install retirement.
    Session,
    /// Read-only, empty enterprise policy values.
    Managed,
}

impl Area {
    /// Chrome's area name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Sync => "sync",
            Self::Session => "session",
            Self::Managed => "managed",
        }
    }

    fn index(self) -> usize {
        self as usize
    }
}

/// Whether web-renderer contexts may access an area.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccessLevel {
    /// Only runtime-owned extension contexts.
    TrustedContexts,
    /// Extension contexts and injected scripts, excluding external pages.
    TrustedAndUntrustedContexts,
}

/// The values before and after a change. Absence differs from JSON null.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageChange {
    /// Omitted when the key did not previously exist.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_value: Option<Value>,
    /// Omitted when the key was removed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_value: Option<Value>,
}

/// A committed mutation to fan out as `storage.onChanged` and area events.
#[derive(Clone, Debug, PartialEq)]
pub struct StorageChangeEvent {
    /// The affected area.
    pub area: Area,
    /// Changed properties; never includes unchanged writes.
    pub changes: Changes,
    /// Access policy captured at commit; recheck current policy at delivery.
    pub access_level: AccessLevel,
}

/// An explicit storage failure; database errors never reveal paths or values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StorageError {
    /// A total, item, or item-count quota was exceeded.
    QuotaExceeded(&'static str),
    /// A sync write-rate quota was exceeded.
    WriteRateExceeded(&'static str),
    /// Enterprise policy cannot be modified by extensions.
    ReadOnly,
    /// The caller may not access the requested area or install.
    AccessDenied,
    /// Only a trusted context may change an area's access level.
    CannotSetAccessLevel,
    /// A database or filesystem operation failed.
    Backend,
    /// The database contains an unknown or incompatible schema.
    UnsupportedSchema,
    /// Stored JSON or database integrity is invalid.
    Corrupt,
}

impl fmt::Display for StorageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::QuotaExceeded(resource) => write!(formatter, "{resource} quota exceeded"),
            Self::WriteRateExceeded(resource) => {
                write!(formatter, "This request exceeds the {resource} quota.")
            }
            Self::ReadOnly => formatter.write_str("This is a read-only store."),
            Self::AccessDenied => {
                formatter.write_str("Access to storage is not allowed from this context.")
            }
            Self::CannotSetAccessLevel => {
                formatter.write_str("Context cannot set the storage access level")
            }
            Self::Backend => formatter.write_str("Storage operation failed."),
            Self::UnsupportedSchema => formatter.write_str("Unsupported extension storage schema."),
            Self::Corrupt => formatter.write_str("Extension storage is corrupt."),
        }
    }
}

impl std::error::Error for StorageError {}

impl From<StorageError> for ApiError {
    fn from(error: StorageError) -> Self {
        Self::other(error.to_string())
    }
}

/// Selects all values, specific keys, or keys with fallback values.
#[derive(Clone, Debug, PartialEq)]
pub enum GetKeys {
    /// All properties (`null` or omitted argument).
    All,
    /// The requested keys; an empty list returns an empty object.
    Keys(Vec<String>),
    /// Defaults used only for missing top-level properties.
    Defaults(Items),
}

/// One atomic mutation.
#[derive(Clone, Debug, PartialEq)]
pub enum Mutation {
    /// Replace the named properties, retaining other properties.
    Set(Items),
    /// Remove the named keys, ignoring keys which do not exist.
    Remove(Vec<String>),
    /// Remove every property.
    Clear,
}

/// Storage state bound to one profile, install and incognito class.
/// Keep this alive when a background stops; drop it on disable/reload/update.
pub struct Storage {
    profile: ProfileId,
    install: ExtensionInstallId,
    incognito: bool,
    backend: Box<dyn StorageBackend>,
    session: Items,
    access: [AccessLevel; 4],
    rate: quota::WriteRate,
}

impl Storage {
    /// Constructs storage over a backend for exactly one install. Incognito
    /// instances must use [`MemoryBackend`] to keep their data ephemeral.
    pub fn new(
        profile: ProfileId,
        install: ExtensionInstallId,
        incognito: bool,
        backend: impl StorageBackend + 'static,
    ) -> Result<Self, StorageError> {
        if incognito && backend.is_persistent() {
            return Err(StorageError::AccessDenied);
        }
        Ok(Self {
            profile,
            install,
            incognito,
            backend: Box::new(backend),
            session: Items::new(),
            access: Self::default_access(),
            rate: quota::WriteRate::default(),
        })
    }

    /// Constructs fully ephemeral storage, including local and sync areas.
    pub fn memory(profile: ProfileId, install: ExtensionInstallId, incognito: bool) -> Self {
        Self {
            profile,
            install,
            incognito,
            backend: Box::new(MemoryBackend::default()),
            session: Items::new(),
            access: Self::default_access(),
            rate: quota::WriteRate::default(),
        }
    }

    fn default_access() -> [AccessLevel; 4] {
        [
            AccessLevel::TrustedAndUntrustedContexts,
            AccessLevel::TrustedAndUntrustedContexts,
            AccessLevel::TrustedContexts,
            AccessLevel::TrustedAndUntrustedContexts,
        ]
    }

    /// Checks identity and current access policy before a data operation or
    /// event delivery. Even a host returning the wrong store fails closed.
    pub fn check_access(&self, sender: &Sender, area: Area) -> Result<(), StorageError> {
        if sender.profile() != self.profile
            || sender.install() != self.install
            || sender.incognito() != self.incognito
            || sender.kind() == ContextKind::ExternalPage
            || (self.access[area.index()] == AccessLevel::TrustedContexts
                && !sender.kind().is_runtime_owned())
        {
            return Err(StorageError::AccessDenied);
        }
        Ok(())
    }

    /// The current access policy, initially trusted-only for session.
    pub fn access_level(&self, area: Area) -> AccessLevel {
        self.access[area.index()]
    }

    /// Changes access from a runtime-owned context. This never grants an
    /// untrusted context authority to change access itself.
    pub fn set_access_level(
        &mut self,
        sender: &Sender,
        area: Area,
        access: AccessLevel,
    ) -> Result<(), StorageError> {
        self.check_access(sender, area)?;
        if !sender.kind().is_runtime_owned() {
            return Err(StorageError::CannotSetAccessLevel);
        }
        self.access[area.index()] = access;
        Ok(())
    }

    fn read(&self, area: Area) -> Result<Items, StorageError> {
        match area {
            Area::Session => Ok(self.session.clone()),
            Area::Managed => Ok(Items::new()),
            Area::Local | Area::Sync => self.backend.read(area),
        }
    }

    /// Returns requested values, with top-level defaults for absent keys.
    /// Adapter calls must pass [`Self::check_access`] first.
    pub fn get(&self, area: Area, keys: &GetKeys) -> Result<Items, StorageError> {
        let stored = self.read(area)?;
        match keys {
            GetKeys::All => Ok(stored),
            GetKeys::Keys(keys) => Ok(keys
                .iter()
                .filter_map(|key| stored.get(key).map(|value| (key.clone(), value.clone())))
                .collect()),
            GetKeys::Defaults(defaults) => Ok(defaults
                .iter()
                .map(|(key, fallback)| (key.clone(), stored.get(key).unwrap_or(fallback).clone()))
                .collect()),
        }
    }

    /// Returns all keys in deterministic order.
    pub fn get_keys(&self, area: Area) -> Result<Vec<String>, StorageError> {
        Ok(self.read(area)?.into_iter().map(|(key, _)| key).collect())
    }

    /// Returns quota bytes for all keys (`None`) or the requested keys.
    /// Repeated keys count repeatedly, matching Chromium's implementation.
    pub fn get_bytes_in_use(
        &self,
        area: Area,
        keys: Option<&[String]>,
    ) -> Result<u64, StorageError> {
        let stored = self.read(area)?;
        match keys {
            None => Ok(stored
                .iter()
                .map(|(key, value)| item_bytes(key, value))
                .sum()),
            Some(keys) => Ok(keys
                .iter()
                .filter_map(|key| stored.get(key).map(|value| item_bytes(key, value)))
                .sum()),
        }
    }

    /// Commits an atomic mutation, returning changes only after success.
    /// Failed sync writes consume a rate slot after rate admission, as Chrome
    /// checks the write rate before quota/backend work. `now_nanos` is monotonic.
    pub fn mutate(
        &mut self,
        area: Area,
        mutation: &Mutation,
        unlimited_storage: bool,
        now_nanos: u64,
    ) -> Result<Option<StorageChangeEvent>, StorageError> {
        if area == Area::Managed {
            return Err(StorageError::ReadOnly);
        }
        if area == Area::Sync {
            self.rate.charge(now_nanos)?;
        }
        let maximum = match area {
            Area::Local if unlimited_storage => STORAGE_LOCAL_UNLIMITED_QUOTA_BYTES,
            Area::Local => STORAGE_LOCAL_QUOTA_BYTES,
            Area::Session => STORAGE_SESSION_QUOTA_BYTES,
            Area::Sync => QUOTA_BYTES,
            Area::Managed => return Err(StorageError::ReadOnly),
        };
        let mut prepare = |items: &Items| quota::prepare(items, mutation, area, maximum);
        let changes = if area == Area::Session {
            let changes = prepare(&self.session)?;
            backend::apply_changes(&mut self.session, &changes);
            changes
        } else {
            self.backend.apply(area, &mut prepare)?
        };
        Ok((!changes.is_empty()).then(|| StorageChangeEvent {
            area,
            changes,
            access_level: self.access[area.index()],
        }))
    }

    /// Erases all areas transactionally in the backend, then clears session,
    /// rate and access state. Uninstall must also retire handles and call
    /// [`SqliteBackend::erase_files`] to remove database files.
    pub fn erase(&mut self) -> Result<(), StorageError> {
        self.backend.erase()?;
        self.session.clear();
        self.access = Self::default_access();
        self.rate = quota::WriteRate::default();
        Ok(())
    }

    /// Consumes the retired install's storage and removes durable files.
    /// The adapter must first retire every other handle to the database.
    pub fn destroy(self) -> Result<(), StorageError> {
        self.backend.destroy()
    }
}
