//! Filesystem identity, database admission, and connection hardening.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};
use zephium_core::ids::ProfileId;

use crate::migrations;

use super::compatibility::clear_legacy_session_rows_once;
use super::history::enforce_history_budget;

pub(super) fn open_database(path: &Path) -> rusqlite::Result<Connection> {
    let path = canonical_child_path(path)?;
    let expected_identity = match std::fs::symlink_metadata(&path) {
        Ok(metadata) => verified_file_identity(&path, &metadata)
            .ok_or_else(|| rusqlite::Error::InvalidPath(path.clone()))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let mut options = std::fs::OpenOptions::new();
            options.read(true).write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            match options.open(&path) {
                Ok(file) => {
                    drop(file);
                    let metadata = std::fs::symlink_metadata(&path)
                        .map_err(|_| rusqlite::Error::InvalidPath(path.clone()))?;
                    verified_file_identity(&path, &metadata)
                        .ok_or_else(|| rusqlite::Error::InvalidPath(path.clone()))?
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    let metadata = std::fs::symlink_metadata(&path)
                        .map_err(|_| rusqlite::Error::InvalidPath(path.clone()))?;
                    verified_file_identity(&path, &metadata)
                        .ok_or_else(|| rusqlite::Error::InvalidPath(path.clone()))?
                }
                Err(error) => {
                    return Err(rusqlite::Error::ToSqlConversionFailure(Box::new(error)));
                }
            }
        }
        Err(_) => return Err(rusqlite::Error::InvalidPath(path)),
    };
    let connection = Connection::open_with_flags(
        &path,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )?;
    let current_metadata =
        std::fs::symlink_metadata(&path).map_err(|_| rusqlite::Error::InvalidPath(path.clone()))?;
    let Some(current_identity) = verified_file_identity(&path, &current_metadata) else {
        drop(connection);
        return Err(rusqlite::Error::InvalidPath(path));
    };
    if !same_file_identity(&expected_identity, &current_identity) {
        drop(connection);
        return Err(rusqlite::Error::InvalidPath(path));
    }
    Ok(connection)
}

/// Opens the authoritative metadata database without giving an unknown
/// on-disk schema a writable SQLite handle first. Existing files are fully
/// configured and fingerprinted through a verified read-only handle; only an
/// exact shipped migration boundary may then be reopened for migration.
pub(super) fn open_meta_database(path: &Path) -> rusqlite::Result<Connection> {
    let path = canonical_child_path(path)?;
    for _ in 0..4 {
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) => {
                if verified_file_identity(&path, &metadata).is_none() {
                    return Err(rusqlite::Error::InvalidPath(path));
                }
                let (validation, validation_metadata) = open_existing_database(
                    &path,
                    OpenFlags::SQLITE_OPEN_READ_ONLY
                        | OpenFlags::SQLITE_OPEN_NO_MUTEX
                        | OpenFlags::SQLITE_OPEN_NOFOLLOW,
                )?;
                configure_validation(&validation)?;
                migrations::validate_current(&validation, migrations::META)?;

                let (writable, writable_metadata) = open_existing_database(
                    &path,
                    OpenFlags::SQLITE_OPEN_READ_WRITE
                        | OpenFlags::SQLITE_OPEN_NO_MUTEX
                        | OpenFlags::SQLITE_OPEN_NOFOLLOW,
                )?;
                if !same_file_identity(&validation_metadata, &writable_metadata) {
                    return Err(rusqlite::Error::InvalidPath(path));
                }
                drop(validation);
                return Ok(writable);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let mut options = std::fs::OpenOptions::new();
                options.read(true).write(true).create_new(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.mode(0o600);
                }
                match options.open(&path) {
                    Ok(file) => {
                        drop(file);
                        let created_metadata = std::fs::symlink_metadata(&path)
                            .map_err(|_| rusqlite::Error::InvalidPath(path.clone()))?;
                        let created_identity = verified_file_identity(&path, &created_metadata)
                            .ok_or_else(|| rusqlite::Error::InvalidPath(path.clone()))?;
                        let (writable, writable_metadata) = open_existing_database(
                            &path,
                            OpenFlags::SQLITE_OPEN_READ_WRITE
                                | OpenFlags::SQLITE_OPEN_NO_MUTEX
                                | OpenFlags::SQLITE_OPEN_NOFOLLOW,
                        )?;
                        if !same_file_identity(&created_identity, &writable_metadata) {
                            return Err(rusqlite::Error::InvalidPath(path));
                        }
                        return Ok(writable);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                    Err(error) => {
                        return Err(rusqlite::Error::ToSqlConversionFailure(Box::new(error)));
                    }
                }
            }
            Err(_) => return Err(rusqlite::Error::InvalidPath(path)),
        }
    }
    Err(rusqlite::Error::InvalidPath(path))
}

/// Opens an already-existing regular, single-link database and returns the
/// verified filesystem identity captured after SQLite acquired its handle.
/// This never creates a replacement when the path disappears.
fn open_existing_database(
    path: &Path,
    flags: OpenFlags,
) -> rusqlite::Result<(Connection, FileIdentity)> {
    let path = canonical_child_path(path)?;
    let expected_identity = match std::fs::symlink_metadata(&path) {
        Ok(metadata) => verified_file_identity(&path, &metadata)
            .ok_or_else(|| rusqlite::Error::InvalidPath(path.clone()))?,
        Err(_) => return Err(rusqlite::Error::InvalidPath(path)),
    };
    let connection = Connection::open_with_flags(&path, flags)?;
    let current_metadata =
        std::fs::symlink_metadata(&path).map_err(|_| rusqlite::Error::InvalidPath(path.clone()))?;
    let Some(current_identity) = verified_file_identity(&path, &current_metadata) else {
        drop(connection);
        return Err(rusqlite::Error::InvalidPath(path));
    };
    if !same_file_identity(&expected_identity, &current_identity) {
        drop(connection);
        return Err(rusqlite::Error::InvalidPath(path));
    }
    Ok((connection, current_identity))
}

fn canonical_child_path(path: &Path) -> rusqlite::Result<PathBuf> {
    let parent = path
        .parent()
        .ok_or_else(|| rusqlite::Error::InvalidPath(path.into()))?;
    let file_name = path
        .file_name()
        .ok_or_else(|| rusqlite::Error::InvalidPath(path.into()))?;
    let parent =
        std::fs::canonicalize(parent).map_err(|_| rusqlite::Error::InvalidPath(parent.into()))?;
    Ok(parent.join(file_name))
}

pub(super) fn regular_file_exists(path: &Path) -> rusqlite::Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => verified_file_identity(path, &metadata)
            .map(|_| true)
            .ok_or_else(|| rusqlite::Error::InvalidPath(path.into())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err(rusqlite::Error::InvalidPath(path.into())),
    }
}

#[cfg(unix)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FileIdentity {
    device: u64,
    inode: u64,
}

#[cfg(target_os = "windows")]
type FileIdentity = crate::windows_file_identity::WindowsFileIdentity;

#[cfg(not(any(unix, target_os = "windows")))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FileIdentity;

/// Captures the identity of an exact regular, single-link file.
///
/// Windows' `std::os::windows::fs::MetadataExt` identity and link-count
/// accessors are still unstable. Querying the native handle directly also
/// makes the security property explicit: the link count and identity come
/// from one successfully opened non-reparse handle, rather than from path-only
/// metadata assembled by a different API. Identity uses `FileIdInfo`'s full
/// 128-bit file identifier; the legacy 64-bit index is not unique on ReFS.
#[cfg(unix)]
fn verified_file_identity(_path: &Path, metadata: &std::fs::Metadata) -> Option<FileIdentity> {
    use std::os::unix::fs::MetadataExt;

    (metadata.file_type().is_file() && metadata.nlink() == 1).then_some(FileIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    })
}

#[cfg(target_os = "windows")]
fn verified_file_identity(path: &Path, metadata: &std::fs::Metadata) -> Option<FileIdentity> {
    if !metadata.file_type().is_file() {
        return None;
    }
    crate::windows_file_identity::verified_file_identity(path)
}

#[cfg(not(any(unix, target_os = "windows")))]
fn verified_file_identity(_path: &Path, metadata: &std::fs::Metadata) -> Option<FileIdentity> {
    metadata.file_type().is_file().then_some(FileIdentity)
}

fn same_file_identity(before: &FileIdentity, after: &FileIdentity) -> bool {
    before == after
}

pub(super) fn profile_artifacts_exist(dir: &Path, profile: ProfileId) -> rusqlite::Result<bool> {
    let base = dir.join(format!("profile-{profile}.sqlite"));
    let notes = [
        dir.join("notes").join(profile.to_string()),
        dir.join("notes").join(format!(".erasing-{profile}")),
    ];
    for path in ["", "-wal", "-shm"]
        .into_iter()
        .map(|suffix| {
            let mut path = base.as_os_str().to_owned();
            path.push(suffix);
            PathBuf::from(path)
        })
        .chain(notes)
    {
        match std::fs::symlink_metadata(&path) {
            Ok(_) => return Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(rusqlite::Error::ToSqlConversionFailure(Box::new(error)));
            }
        }
    }
    Ok(false)
}

pub(super) fn profile_artifacts_absent(dir: &Path, profile: ProfileId) -> rusqlite::Result<bool> {
    Ok(!profile_artifacts_exist(dir, profile)?)
}

pub(super) fn configure(conn: &Connection) -> rusqlite::Result<()> {
    use rusqlite::config::DbConfig;

    // Treat an on-disk schema as untrusted input. Zephium never needs the
    // legacy double-quoted-string compatibility mode or SQLite operations
    // that can deliberately corrupt a database file. Apply these flags before
    // migrations or any schema-owned trigger can execute.
    conn.set_db_config(DbConfig::SQLITE_DBCONFIG_DEFENSIVE, true)?;
    conn.set_db_config(DbConfig::SQLITE_DBCONFIG_TRUSTED_SCHEMA, false)?;
    conn.set_db_config(DbConfig::SQLITE_DBCONFIG_DQS_DDL, false)?;
    conn.set_db_config(DbConfig::SQLITE_DBCONFIG_DQS_DML, false)?;
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA synchronous=FULL;
         PRAGMA secure_delete=ON;
         PRAGMA foreign_keys=ON;
         PRAGMA trusted_schema=OFF;
         PRAGMA cell_size_check=ON;
         PRAGMA temp_store=MEMORY;
         PRAGMA journal_size_limit=4194304;
         PRAGMA wal_autocheckpoint=1000;
         PRAGMA busy_timeout=5000;",
    )
}

/// A profile database (history, icons, time, tasks) commits often. In WAL
/// mode NORMAL still survives an app crash intact; a power loss can drop only
/// the last commits, never corrupt the file. The meta database, which holds
/// the session, keeps FULL.
pub(super) fn configure_profile(conn: &Connection) -> rusqlite::Result<()> {
    configure(conn)?;
    conn.execute_batch("PRAGMA synchronous=NORMAL;")
}

pub(super) fn configure_validation(conn: &Connection) -> rusqlite::Result<()> {
    use rusqlite::config::DbConfig;

    conn.set_db_config(DbConfig::SQLITE_DBCONFIG_DEFENSIVE, true)?;
    conn.set_db_config(DbConfig::SQLITE_DBCONFIG_TRUSTED_SCHEMA, false)?;
    conn.set_db_config(DbConfig::SQLITE_DBCONFIG_DQS_DDL, false)?;
    conn.set_db_config(DbConfig::SQLITE_DBCONFIG_DQS_DML, false)?;
    conn.execute_batch(
        "PRAGMA query_only=ON;
         PRAGMA foreign_keys=ON;
         PRAGMA trusted_schema=OFF;
         PRAGMA cell_size_check=ON;
         PRAGMA temp_store=MEMORY;
         PRAGMA busy_timeout=5000;",
    )
}

pub(super) fn harden_registered_profile_files(
    dir: &Path,
    registry: &HashSet<ProfileId>,
    purge_session: bool,
    allow_degraded: bool,
) -> rusqlite::Result<HashSet<ProfileId>> {
    let mut profiles: Vec<ProfileId> = registry.iter().copied().collect();
    profiles.sort_unstable_by_key(ToString::to_string);
    let mut degraded = HashSet::new();
    for profile in profiles {
        let path = dir.join(format!("profile-{profile}.sqlite"));
        if !regular_file_exists(&path)? {
            continue;
        }

        // Validate through a securely opened read-only handle first. A future
        // schema, corrupt sqlite_schema, or malformed SQLite file must be
        // classified before journal-mode setup or migrations can modify it.
        // Path/symlink/link violations occur before this handle exists and
        // remain global fail-closed startup errors.
        let (validation, validation_metadata) = open_existing_database(
            &path,
            OpenFlags::SQLITE_OPEN_READ_ONLY
                | OpenFlags::SQLITE_OPEN_NO_MUTEX
                | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )?;
        let validation_result = configure_validation(&validation)
            .and_then(|()| migrations::validate_current(&validation, migrations::PROFILE))
            .map(|_| ());
        if let Err(error) = validation_result {
            if !allow_degraded {
                return Err(error);
            }
            eprintln!(
                "store: preserving degraded ancillary database for profile {profile}: {error}"
            );
            degraded.insert(profile);
            continue;
        }

        let (mut conn, writable_metadata) = open_existing_database(
            &path,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX
                | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )?;
        if !same_file_identity(&validation_metadata, &writable_metadata) {
            return Err(rusqlite::Error::InvalidPath(path));
        }
        drop(validation);

        let result = configure_profile(&conn)
            .and_then(|()| migrations::apply(&mut conn, migrations::PROFILE))
            .and_then(|()| enforce_history_budget(&conn))
            .and_then(|()| {
                if purge_session {
                    clear_legacy_session_rows_once(&mut conn)
                } else {
                    Ok(())
                }
            });
        if let Err(error) = result {
            if !allow_degraded {
                return Err(error);
            }
            eprintln!("store: disabling failed ancillary database for profile {profile}: {error}");
            degraded.insert(profile);
        }
    }
    Ok(degraded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::config::DbConfig;

    #[test]
    fn authoritative_meta_creation_and_read_only_reopen_reach_exact_boundary() {
        for precreate_empty_file in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("meta.sqlite");
            if precreate_empty_file {
                std::fs::File::create(&path).unwrap();
            }

            let mut connection = open_meta_database(&path).unwrap();
            configure(&connection).unwrap();
            migrations::apply(&mut connection, migrations::META).unwrap();
            drop(connection);
            let first_metadata = std::fs::symlink_metadata(&path).unwrap();
            let first_identity = verified_file_identity(&path, &first_metadata).unwrap();

            let mut reopened = open_meta_database(&path).unwrap();
            configure(&reopened).unwrap();
            migrations::apply(&mut reopened, migrations::META).unwrap();
            let version: i64 = reopened
                .query_row("PRAGMA user_version", [], |row| row.get(0))
                .unwrap();
            assert_eq!(version, migrations::META.last().unwrap().version);
            drop(reopened);

            let second_metadata = std::fs::symlink_metadata(&path).unwrap();
            let second_identity = verified_file_identity(&path, &second_metadata).unwrap();
            assert!(same_file_identity(&first_identity, &second_identity));
        }
    }

    #[test]
    fn configured_connections_fail_closed_on_schema_and_temp_storage() {
        let connection = Connection::open_in_memory().unwrap();
        configure(&connection).unwrap();

        assert!(connection
            .db_config(DbConfig::SQLITE_DBCONFIG_DEFENSIVE)
            .unwrap());
        assert!(!connection
            .db_config(DbConfig::SQLITE_DBCONFIG_TRUSTED_SCHEMA)
            .unwrap());
        assert!(!connection
            .db_config(DbConfig::SQLITE_DBCONFIG_DQS_DDL)
            .unwrap());
        assert!(!connection
            .db_config(DbConfig::SQLITE_DBCONFIG_DQS_DML)
            .unwrap());
        assert_eq!(
            connection
                .query_row("PRAGMA cell_size_check", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            connection
                .query_row("PRAGMA temp_store", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            2
        );
    }
}
