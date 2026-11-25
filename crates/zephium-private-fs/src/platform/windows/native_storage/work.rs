//! Fixed-purpose Work storage. Arbitrary private namespaces remain gated.
use super::{
    admit_namespace_support, known_profile, NativeApplication, NativeSession, NativeStorageAnchor,
    NativeStorageFile,
};
use crate::{PrivateEntryName, PrivateFsError};
use std::path::Path;
use std::path::PathBuf;

const DATABASE: &str = "work.sqlite";
const WAL: &str = "work.sqlite-wal";
const SHM: &str = "work.sqlite-shm";

/// Exclusively leased fixed Work storage beneath the OS Profile KnownFolder.
/// No arbitrary root, generic filename, or extension namespace is exposed.
pub struct NativeWorkStorageAnchor {
    inner: NativeStorageAnchor,
}

/// Exact fixed Work database or sidecar identity pinned without delete sharing.
pub struct NativeWorkStorageFile {
    inner: NativeStorageFile,
}

impl NativeWorkStorageAnchor {
    /// Admits only the fixed closed application and optional QA session layout.
    /// Existing root and file permissions are inspected, never repaired.
    pub fn prepare(
        application: NativeApplication,
        session: Option<&NativeSession>,
    ) -> Result<Self, PrivateFsError> {
        validate_routing(application, session)?;
        let profile = known_profile()?;
        Ok(Self {
            inner: NativeStorageAnchor::prepare_under_impl(&profile, application, session)?,
        })
    }

    /// Disposable validation root; absent from shipping and optimized graphs.
    #[cfg(all(debug_assertions, feature = "windows-namespace-validation"))]
    #[doc(hidden)]
    pub fn prepare_validation_fixture(
        profile: &std::path::Path,
        application: NativeApplication,
        session: Option<&NativeSession>,
    ) -> Result<Self, PrivateFsError> {
        validate_routing(application, session)?;
        Ok(Self {
            inner: NativeStorageAnchor::prepare_under_impl(profile, application, session)?,
        })
    }

    /// Fixed SQLite path, valid only while original capability/file guards live.
    /// The returned path is an observation, never independent authority.
    #[must_use]
    pub fn database_path(&self) -> PathBuf {
        self.inner.directory().join(DATABASE)
    }

    /// Holds the exact existing database or exclusively creates a new empty one
    /// only when both fixed sidecars are absent. Nothing is truncated.
    pub fn create_or_hold_database(&self) -> Result<NativeWorkStorageFile, PrivateFsError> {
        match self.hold(DATABASE) {
            Ok(file) => Ok(file),
            Err(PrivateFsError::NotFound) => {
                for name in [WAL, SHM] {
                    match self.hold(name) {
                        Err(PrivateFsError::NotFound) => {}
                        Ok(_) => return Err(PrivateFsError::Unsafe),
                        Err(error) => return Err(error),
                    }
                }
                let name = PrivateEntryName::new(DATABASE).map_err(|_| PrivateFsError::Unsafe)?;
                let file = self.inner.create_file(&name)?;
                self.sync()?;
                Ok(NativeWorkStorageFile { inner: file })
            }
            Err(error) => Err(error),
        }
    }

    /// Holds the exact fixed WAL if present; absence is `NotFound`.
    pub fn hold_wal(&self) -> Result<NativeWorkStorageFile, PrivateFsError> {
        self.hold(WAL)
    }
    /// Holds the exact fixed shared-memory sidecar if present.
    pub fn hold_shm(&self) -> Result<NativeWorkStorageFile, PrivateFsError> {
        self.hold(SHM)
    }
    fn hold(&self, fixed_name: &str) -> Result<NativeWorkStorageFile, PrivateFsError> {
        let name = PrivateEntryName::new(fixed_name).map_err(|_| PrivateFsError::Unsafe)?;
        Ok(NativeWorkStorageFile {
            inner: self.inner.hold_file(&name)?,
        })
    }
    /// Revalidates original root identity, chain, ACL and exclusive lease.
    /// Disagreement stickily quarantines this original capability.
    pub fn verify(&self) -> Result<(), PrivateFsError> {
        self.inner.verify()
    }
    /// Executes the native directory barrier and revalidates original root.
    /// Required after SQLite creates sidecars, before durable acknowledgment.
    pub fn sync(&self) -> Result<(), PrivateFsError> {
        self.inner.sync()
    }
}

impl NativeWorkStorageFile {
    /// Revalidates this original file against the same held Work anchor.
    pub fn verify(&self, anchor: &NativeWorkStorageAnchor) -> Result<(), PrivateFsError> {
        self.inner.verify(&anchor.inner)
    }
}

fn validate_routing(
    application: NativeApplication,
    session: Option<&NativeSession>,
) -> Result<(), PrivateFsError> {
    if session.is_some() && application != NativeApplication::ExtensionQa {
        return Err(PrivateFsError::Unsafe);
    }
    Ok(())
}

#[cfg(all(test, feature = "windows-namespace-validation"))]
#[path = "work_acl_tests.rs"]
mod acl_tests;
#[cfg(test)]
#[path = "work_tests.rs"]
mod tests;
