//! Lazy, separately protected Windows Work persistence; never a WebView2 UDF.

use std::sync::Arc;

use rusqlite::{Connection, OpenFlags, OptionalExtension};
use zephium_core::ids::ProfileId;
use zephium_private_fs::{
    NativeApplication, NativeSession, NativeWorkStorageAnchor, NativeWorkStorageFile,
    PrivateFsError,
};

use super::{filesystem, Hub};
use crate::migrations;

/// Closed native application/session routing for protected Work state.
/// Installing this selector does no filesystem I/O; admission remains lazy.
#[derive(Clone)]
pub struct WindowsWorkStorage {
    application: NativeApplication,
    session: Option<NativeSession>,
    #[cfg(all(debug_assertions, feature = "windows-namespace-validation"))]
    fixture: Option<std::path::PathBuf>,
}

impl WindowsWorkStorage {
    /// Selects only a recognized native application identifier. QA sessions
    /// remain exclusive to the extension QA application.
    pub fn for_application(
        identifier: &str,
        qa_session: Option<&str>,
    ) -> Result<Self, PrivateFsError> {
        let application = match identifier {
            "app.zephium" => NativeApplication::Product,
            "app.zephium.dev" => NativeApplication::Development,
            "app.zephium.review-qa" => NativeApplication::ReviewQa,
            "app.zephium.webext-qa" => NativeApplication::ExtensionQa,
            "app.zephium.files-integration-qa" => NativeApplication::FilesIntegrationQa,
            "app.zephium.performance" => NativeApplication::Performance,
            "app.zephium.protection-qa" => NativeApplication::ProtectionQa,
            "app.zephium.work-integration" => NativeApplication::WorkIntegration,
            "app.zephium.work-navigation-probe" => NativeApplication::WorkNavigationProbe,
            "app.zephium.work-rendering-probe" => NativeApplication::WorkRenderingProbe,
            _ => return Err(PrivateFsError::Unsafe),
        };
        if qa_session.is_some() && application != NativeApplication::ExtensionQa {
            return Err(PrivateFsError::Unsafe);
        }
        Ok(Self {
            application,
            session: qa_session.map(NativeSession::new).transpose()?,
            #[cfg(all(debug_assertions, feature = "windows-namespace-validation"))]
            fixture: None,
        })
    }

    /// Disposable protected-fixture routing, absent from shipping builds.
    #[cfg(all(debug_assertions, feature = "windows-namespace-validation"))]
    #[doc(hidden)]
    pub fn for_validation_fixture(
        profile: &std::path::Path,
        identifier: &str,
        qa_session: Option<&str>,
    ) -> Result<Self, PrivateFsError> {
        let mut selector = Self::for_application(identifier, qa_session)?;
        selector.fixture = Some(profile.to_path_buf());
        Ok(selector)
    }

    fn prepare(&self) -> Result<NativeWorkStorageAnchor, PrivateFsError> {
        #[cfg(all(debug_assertions, feature = "windows-namespace-validation"))]
        if let Some(profile) = &self.fixture {
            return NativeWorkStorageAnchor::prepare_validation_fixture(
                profile,
                self.application,
                self.session.as_ref(),
            );
        }
        NativeWorkStorageAnchor::prepare(self.application, self.session.as_ref())
    }
}

pub(super) struct WindowsWorkDatabase {
    // Drop SQLite first, then its canonical file guard and original root lease.
    connection: Connection,
    database: NativeWorkStorageFile,
    pub(super) anchor: Arc<NativeWorkStorageAnchor>,
    quarantined: bool,
}

fn unavailable() -> rusqlite::Error {
    super::invalid_data("protected Windows Work storage admission failed")
}

fn sidecars(anchor: &NativeWorkStorageAnchor) -> rusqlite::Result<Vec<NativeWorkStorageFile>> {
    [anchor.hold_wal(), anchor.hold_shm()]
        .into_iter()
        .map(|result| match result {
            Ok(file) => Ok(Some(file)),
            Err(PrivateFsError::NotFound) => Ok(None),
            Err(_) => Err(unavailable()),
        })
        .collect::<rusqlite::Result<Vec<_>>>()
        .map(|files| files.into_iter().flatten().collect())
}

impl WindowsWorkDatabase {
    /// Raw persisted-fact access for fixtures; never runtime authority.
    #[cfg(test)]
    pub(super) fn test_connection(&mut self) -> &mut Connection {
        &mut self.connection
    }

    fn open(selector: &WindowsWorkStorage) -> rusqlite::Result<Self> {
        let anchor = Arc::new(selector.prepare().map_err(|_| unavailable())?);
        let database = anchor
            .create_or_hold_database()
            .map_err(|_| unavailable())?;
        let path = anchor.database_path();
        let flags = OpenFlags::SQLITE_OPEN_NO_MUTEX | OpenFlags::SQLITE_OPEN_NOFOLLOW;
        let held = sidecars(&anchor)?;
        {
            let validation =
                Connection::open_with_flags(&path, flags | OpenFlags::SQLITE_OPEN_READ_ONLY)?;
            filesystem::configure_validation(&validation)?;
            migrations::validate_current(&validation, migrations::WORK)?;
            database.verify(&anchor).map_err(|_| unavailable())?;
            for file in &held {
                file.verify(&anchor).map_err(|_| unavailable())?;
            }
        }
        let mut connection =
            Connection::open_with_flags(&path, flags | OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        filesystem::configure(&connection)?;
        migrations::apply(&mut connection, migrations::WORK)?;
        for file in &held {
            file.verify(&anchor).map_err(|_| unavailable())?;
        }
        let mut result = Self {
            connection,
            database,
            anchor,
            quarantined: false,
        };
        result.with_connection(|_| Ok(()))?;
        result.anchor.sync().map_err(|_| unavailable())?;
        Ok(result)
    }

    pub(super) fn with_connection<T>(
        &mut self,
        operation: impl FnOnce(&mut Connection) -> rusqlite::Result<T>,
    ) -> rusqlite::Result<T> {
        if self.quarantined {
            return Err(unavailable());
        }
        let admission = self
            .database
            .verify(&self.anchor)
            .map_err(|_| unavailable())
            .and_then(|()| sidecars(&self.anchor));
        let held = match admission {
            Ok(held) => held,
            Err(error) => {
                self.quarantined = true;
                return Err(error);
            }
        };
        let result = operation(&mut self.connection);
        let settlement = (|| {
            self.database
                .verify(&self.anchor)
                .map_err(|_| unavailable())?;
            for file in &held {
                file.verify(&self.anchor).map_err(|_| unavailable())?;
            }
            let after = sidecars(&self.anchor)?;
            // Every old fixed slot is still held without delete sharing and
            // verified above. An increased inventory therefore proves a newly
            // published namespace entry that needs a native directory barrier.
            if after.len() < held.len() {
                return Err(unavailable());
            }
            if after.len() > held.len() {
                self.anchor.sync().map_err(|_| unavailable())?;
            }
            Ok(())
        })();
        if settlement.is_err() {
            self.quarantined = true;
        }
        settlement?;
        result
    }
}

pub(super) fn profile_retired(
    connection: &Connection,
    profile: ProfileId,
) -> rusqlite::Result<bool> {
    connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM agent_work_profile_deletion WHERE profile_id = ?1)",
        [profile.to_string()],
        |row| row.get(0),
    )
}

impl Hub {
    pub(super) fn windows_work_database(&mut self) -> rusqlite::Result<&mut WindowsWorkDatabase> {
        if self.windows_work_database.is_none() {
            let selector = self.windows_work_storage.as_ref().ok_or_else(unavailable)?;
            self.windows_work_database = Some(WindowsWorkDatabase::open(selector)?);
            self.prune_agent_work(0, super::agent_work::AGENT_WORK_PRUNE_AT_OPEN);
        }
        self.windows_work_database.as_mut().ok_or_else(unavailable)
    }

    /// Records protected deletion authority before the ordinary meta commit.
    /// An ambiguous meta commit leaves a safe fence, never body deletion.
    pub(super) fn authorize_windows_work_artifact_deletion(
        &mut self,
        profile: ProfileId,
    ) -> rusqlite::Result<()> {
        if self.windows_work_storage.is_none() {
            return Ok(());
        }
        if !self.registry.contains(&profile) {
            return Err(unavailable());
        }
        self.windows_work_database()?.with_connection(|connection| {
            let transaction = connection.transaction()?;
            let referenced: bool = transaction.query_row("SELECT EXISTS(SELECT 1 FROM agent_work_runs WHERE result_profile = ?1) OR EXISTS(SELECT 1 FROM agent_work_artifacts WHERE profile_id = ?1)", [profile.to_string()], |row| row.get(0))?;
            if referenced && !profile_retired(&transaction, profile)? {
                transaction.execute("INSERT INTO agent_work_profile_deletion(profile_id, purged) VALUES (?1, 0)", [profile.to_string()])?;
            }
            transaction.commit()
        })
    }

    /// Idempotent protected body purge. The caller additionally owns the exact
    /// ordinary-meta deletion row; no untrusted tombstone alone is authority.
    pub(super) fn purge_windows_work_artifacts(
        &mut self,
        profile: ProfileId,
    ) -> rusqlite::Result<()> {
        if self.windows_work_storage.is_none() {
            return Ok(());
        }
        if self.registry.contains(&profile) {
            return Err(unavailable());
        }
        self.windows_work_database()?.with_connection(|connection| {
            let transaction = connection.transaction()?;
            let intent: Option<bool> = transaction
                .query_row(
                    "SELECT purged FROM agent_work_profile_deletion WHERE profile_id = ?1",
                    [profile.to_string()],
                    |row| row.get(0),
                )
                .optional()?;
            let has_bodies: bool = transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM agent_work_artifacts WHERE profile_id = ?1)",
                [profile.to_string()],
                |row| row.get(0),
            )?;
            if intent.is_none() {
                return if has_bodies {
                    Err(unavailable())
                } else {
                    Ok(())
                };
            }
            transaction.execute(
                "DELETE FROM agent_work_artifacts WHERE profile_id = ?1",
                [profile.to_string()],
            )?;
            transaction.execute(
                "UPDATE agent_work_profile_deletion SET purged = 1 WHERE profile_id = ?1",
                [profile.to_string()],
            )?;
            transaction.commit()?;
            // secure_delete clears freed main pages; truncating the dedicated
            // WAL retires prior body frames before cleanup is acknowledged.
            // A busy/failing checkpoint leaves the durable intent for retry.
            let checkpoint: (i64, i64, i64) =
                connection.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                })?;
            if checkpoint != (0, 0, 0) {
                return Err(unavailable());
            }
            Ok(())
        })
    }
}

#[cfg(test)]
#[path = "windows_work_storage_tests.rs"]
mod tests;
