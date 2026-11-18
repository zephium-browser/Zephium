use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use rusqlite::config::DbConfig;
use rusqlite::types::ValueRef;
use rusqlite::{params, Connection, TransactionBehavior};

use super::{Area, Changes, Items, StorageError};

const SCHEMA_VERSION: i64 = 1;
const APPLICATION_ID: i64 = 0x5A_57_53_54;
const TABLE_SQL: &str = "CREATE TABLE storage (area TEXT NOT NULL CHECK (area IN ('local','sync')), key TEXT NOT NULL, value TEXT NOT NULL, PRIMARY KEY (area,key)) WITHOUT ROWID";

/// A backend belongs to one install. `apply` must read, validate and commit in
/// one atomic operation, invoking the trusted preparation function once.
pub trait StorageBackend {
    /// Whether this backend writes durable data. Incognito rejects it.
    fn is_persistent(&self) -> bool;
    /// Reads one durable area; session and managed must be rejected.
    fn read(&self, area: Area) -> Result<Items, StorageError>;
    /// Computes and commits changes atomically. A preparation failure leaves
    /// the backend unchanged; returned changes describe committed data only.
    fn apply(
        &mut self,
        area: Area,
        prepare: &mut dyn FnMut(&Items) -> Result<Changes, StorageError>,
    ) -> Result<Changes, StorageError>;
    /// Clears both durable areas atomically.
    fn erase(&mut self) -> Result<(), StorageError>;
    /// Retires the backend and removes its data files. The adapter must have
    /// retired all other handles before invoking this operation.
    fn destroy(self: Box<Self>) -> Result<(), StorageError>;
}

/// Fully ephemeral local and sync storage for private profiles and tests.
#[derive(Default)]
pub struct MemoryBackend {
    areas: BTreeMap<Area, Items>,
}

impl StorageBackend for MemoryBackend {
    fn is_persistent(&self) -> bool {
        false
    }

    fn read(&self, area: Area) -> Result<Items, StorageError> {
        durable(area)?;
        Ok(self.areas.get(&area).cloned().unwrap_or_default())
    }

    fn apply(
        &mut self,
        area: Area,
        prepare: &mut dyn FnMut(&Items) -> Result<Changes, StorageError>,
    ) -> Result<Changes, StorageError> {
        durable(area)?;
        let items = self.areas.entry(area).or_default();
        let changes = prepare(items)?;
        apply_changes(items, &changes);
        Ok(changes)
    }

    fn erase(&mut self) -> Result<(), StorageError> {
        self.areas.clear();
        Ok(())
    }

    fn destroy(self: Box<Self>) -> Result<(), StorageError> {
        Ok(())
    }
}

/// One SQLite file at an adapter-supplied install path. No implicit recovery,
/// file replacement or schema downgrade occurs on corrupt or newer databases.
pub struct SqliteBackend {
    connection: Connection,
    path: PathBuf,
}

impl SqliteBackend {
    /// Opens or creates a database. The adapter supplies an isolated, private
    /// parent directory and runs this synchronous work on its storage executor.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StorageError> {
        let path = path.as_ref();
        let mut connection = Connection::open(path).map_err(database_error)?;
        connection
            .set_db_config(DbConfig::SQLITE_DBCONFIG_DEFENSIVE, true)
            .map_err(database_error)?;
        connection
            .set_db_config(DbConfig::SQLITE_DBCONFIG_TRUSTED_SCHEMA, false)
            .map_err(database_error)?;
        connection
            .set_db_config(DbConfig::SQLITE_DBCONFIG_DQS_DDL, false)
            .map_err(database_error)?;
        connection
            .set_db_config(DbConfig::SQLITE_DBCONFIG_DQS_DML, false)
            .map_err(database_error)?;
        connection
            .execute_batch(
                "PRAGMA trusted_schema=OFF; PRAGMA cell_size_check=ON; PRAGMA busy_timeout=5000;",
            )
            .map_err(database_error)?;
        let integrity: String = connection
            .query_row("PRAGMA quick_check(1)", [], |row| row.get(0))
            .map_err(database_error)?;
        if integrity != "ok" {
            return Err(StorageError::Corrupt);
        }
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_error)?;
        let version: i64 = transaction
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(database_error)?;
        let application: i64 = transaction
            .query_row("PRAGMA application_id", [], |row| row.get(0))
            .map_err(database_error)?;
        let (empty, known_schema) = {
            let mut query = transaction
                .prepare(
                    "SELECT name, sql FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%' LIMIT 2",
                )
                .map_err(database_error)?;
            let mut rows = query.query([]).map_err(database_error)?;
            match rows.next().map_err(database_error)? {
                None => (true, false),
                Some(row) => {
                    let known = row.get_ref(0).map_err(database_error)?
                        == ValueRef::Text(b"storage")
                        && row.get_ref(1).map_err(database_error)?
                            == ValueRef::Text(TABLE_SQL.as_bytes());
                    (
                        false,
                        known && rows.next().map_err(database_error)?.is_none(),
                    )
                }
            }
        };
        if version == 0 && application == 0 && empty {
            transaction
                .execute_batch(TABLE_SQL)
                .map_err(database_error)?;
            transaction
                .pragma_update(None, "application_id", APPLICATION_ID)
                .map_err(database_error)?;
            transaction
                .pragma_update(None, "user_version", SCHEMA_VERSION)
                .map_err(database_error)?;
        } else if version != SCHEMA_VERSION || application != APPLICATION_ID || !known_schema {
            return Err(StorageError::UnsupportedSchema);
        }
        transaction.commit().map_err(database_error)?;
        connection.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA secure_delete=ON; PRAGMA foreign_keys=ON; PRAGMA temp_store=MEMORY; PRAGMA journal_size_limit=4194304; PRAGMA wal_autocheckpoint=1000;").map_err(database_error)?;
        Ok(Self {
            connection,
            path: path.to_owned(),
        })
    }

    /// Closes this handle and removes the database and WAL sidecars. All
    /// other install handles must already be retired, including other processes.
    pub fn erase_files(self) -> Result<(), StorageError> {
        let Self { connection, path } = self;
        connection
            .close()
            .map_err(|(_, error)| database_error(error))?;
        for suffix in ["-wal", "-shm", ""] {
            let mut name = path.as_os_str().to_os_string();
            name.push(suffix);
            match std::fs::remove_file(PathBuf::from(name)) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => return Err(StorageError::Backend),
            }
        }
        Ok(())
    }
}

impl StorageBackend for SqliteBackend {
    fn is_persistent(&self) -> bool {
        true
    }

    fn read(&self, area: Area) -> Result<Items, StorageError> {
        read_area(&self.connection, area)
    }

    fn apply(
        &mut self,
        area: Area,
        prepare: &mut dyn FnMut(&Items) -> Result<Changes, StorageError>,
    ) -> Result<Changes, StorageError> {
        durable(area)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_error)?;
        let changes = prepare(&read_area(&transaction, area)?)?;
        for (key, change) in &changes {
            match &change.new_value {
                Some(value) => {
                    let json = serde_json::to_string(value).map_err(|_| StorageError::Corrupt)?;
                    transaction.execute("INSERT INTO storage(area,key,value) VALUES (?1,?2,?3) ON CONFLICT(area,key) DO UPDATE SET value=excluded.value", params![area.as_str(), key, json]).map_err(database_error)?;
                }
                None => {
                    transaction
                        .execute(
                            "DELETE FROM storage WHERE area=?1 AND key=?2",
                            params![area.as_str(), key],
                        )
                        .map_err(database_error)?;
                }
            }
        }
        transaction.commit().map_err(database_error)?;
        Ok(changes)
    }

    fn erase(&mut self) -> Result<(), StorageError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_error)?;
        transaction
            .execute("DELETE FROM storage", [])
            .map_err(database_error)?;
        transaction.commit().map_err(database_error)
    }

    fn destroy(self: Box<Self>) -> Result<(), StorageError> {
        self.erase_files()
    }
}

fn read_area(connection: &Connection, area: Area) -> Result<Items, StorageError> {
    durable(area)?;
    let mut statement = connection
        .prepare("SELECT key,value FROM storage WHERE area=?1 ORDER BY key")
        .map_err(database_error)?;
    let mut rows = statement.query([area.as_str()]).map_err(database_error)?;
    let mut items = Items::new();
    let mut bytes = 0_u64;
    while let Some(row) = rows.next().map_err(database_error)? {
        let ValueRef::Text(key) = row.get_ref(0).map_err(database_error)? else {
            return Err(StorageError::Corrupt);
        };
        let ValueRef::Text(json) = row.get_ref(1).map_err(database_error)? else {
            return Err(StorageError::Corrupt);
        };
        bytes = bytes
            .saturating_add(key.len() as u64)
            .saturating_add(json.len() as u64);
        if bytes > crate::limits::STORAGE_LOCAL_UNLIMITED_QUOTA_BYTES {
            return Err(StorageError::Corrupt);
        }
        let key = std::str::from_utf8(key).map_err(|_| StorageError::Corrupt)?;
        let value = serde_json::from_slice(json).map_err(|_| StorageError::Corrupt)?;
        items.insert(key.to_owned(), value);
    }
    Ok(items)
}

fn durable(area: Area) -> Result<(), StorageError> {
    match area {
        Area::Local | Area::Sync => Ok(()),
        Area::Session | Area::Managed => Err(StorageError::Backend),
    }
}

fn database_error(error: rusqlite::Error) -> StorageError {
    match error {
        rusqlite::Error::SqliteFailure(code, _)
            if matches!(
                code.code,
                rusqlite::ErrorCode::DatabaseCorrupt | rusqlite::ErrorCode::NotADatabase
            ) =>
        {
            StorageError::Corrupt
        }
        _ => StorageError::Backend,
    }
}

pub(super) fn apply_changes(items: &mut Items, changes: &Changes) {
    for (key, change) in changes {
        match &change.new_value {
            Some(value) => {
                items.insert(key.clone(), value.clone());
            }
            None => {
                items.remove(key);
            }
        }
    }
}
