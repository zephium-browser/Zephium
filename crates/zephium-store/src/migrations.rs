//! Forward-only, versioned migrations tracked by `PRAGMA user_version`.
//! Never edit a shipped migration; append a new one.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use rusqlite::{Connection, Transaction};

// Keep META v16's fixed audit payload and append ceiling tied to the adapter.
const _: [(); 128] = [(); zephium_agentic::AGENT_AUDIT_RECORD_V1_BYTES];
const _: [(); 16] = [(); zephium_agentic::MAX_AGENT_AUDIT_DELIVERY_EVENTS];
const _: [(); 262_144] = [(); crate::hub::MAX_DURABLE_AGENT_AUDIT_EVENTS];
const _: [(); 128] = [(); crate::hub::MAX_TIME_BATCH_RECEIPTS];

pub struct Migration {
    pub version: i64,
    pub up: fn(&Transaction) -> rusqlite::Result<()>,
}

// Bound both the merged schema and historical Work QA schemas during upgrade.
const MAX_SCHEMA_OBJECTS: i64 = 192;
const MAX_SCHEMA_IDENTIFIER_BYTES: i64 = 256;
const MAX_SCHEMA_SQL_BYTES: i64 = 256 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
struct SchemaObject {
    kind: String,
    name: String,
    table: String,
    sql: Option<String>,
}

type ManifestCache = HashMap<(u8, i64), Vec<SchemaObject>>;

static EXPECTED_MANIFESTS: OnceLock<Mutex<ManifestCache>> = OnceLock::new();

pub fn apply(conn: &mut Connection, migrations: &[Migration]) -> rusqlite::Result<()> {
    let current = validate_current(conn, migrations)?;
    if legacy_work_profile(conn, migrations, current)? {
        integrate_legacy_work_profile(conn, current)?;
        return apply(conn, migrations);
    }
    if accepts_legacy_profile_v14(migrations)
        && current == 14
        && schema_manifest(conn)? == expected_legacy_profile_v14_manifest()?
    {
        integrate_legacy_profile_v14(conn)?;
        return apply(conn, migrations);
    }
    for m in migrations.iter().filter(|m| m.version > current) {
        let tx = conn.transaction()?;
        (m.up)(&tx)?;
        tx.pragma_update(None, "user_version", m.version)?;
        tx.commit()?;
        validate_manifest(conn, migrations, m.version)?;
    }
    Ok(())
}

/// Validates a database's claimed migration boundary and exact schema without
/// issuing migration DDL/DML. Startup uses this through a securely opened
/// read-only connection before deciding whether an ancillary profile file is
/// safe to mutate or must be preserved in degraded mode.
pub(crate) fn validate_current(
    conn: &Connection,
    migrations: &[Migration],
) -> rusqlite::Result<i64> {
    let current: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    let latest = migrations.last().map_or(0, |migration| migration.version);
    if current > latest {
        // Opening a database written by a newer binary and then issuing writes
        // against an older schema is not a supported rollback path. It can
        // corrupt state even when every individual SQL statement succeeds.
        return Err(rusqlite::Error::InvalidParameterName(format!(
            "database schema version {current} is newer than supported version {latest}"
        )));
    }
    if current != 0
        && !migrations
            .iter()
            .any(|migration| migration.version == current)
    {
        return Err(invalid_schema(&format!(
            "database schema version {current} is not a shipped migration boundary"
        )));
    }

    // A version number alone is not a schema identity. Validate the exact
    // claimed prefix before any migration DML can fire a replaced or injected
    // trigger, then validate again after every committed step. The trusted
    // reference is generated once from these same immutable migrations in a
    // fresh in-memory database, including FTS shadow objects and triggers.
    if accepts_legacy_profile_v14(migrations)
        && current == 14
        && schema_manifest(conn)? == expected_legacy_profile_v14_manifest()?
    {
        return Ok(current);
    }
    if legacy_work_profile(conn, migrations, current)? {
        return Ok(current);
    }
    validate_manifest(conn, migrations, current)?;
    Ok(current)
}

fn accepts_legacy_profile_v14(migrations: &[Migration]) -> bool {
    std::ptr::eq(migrations.as_ptr(), PROFILE.as_ptr())
        && migrations
            .last()
            .is_some_and(|migration| migration.version >= 22)
}

// The extension branch shipped a different PROFILE v14. Identify it by the
// complete schema, not its ambiguous version number or a few table names.
fn expected_legacy_profile_v14_manifest() -> rusqlite::Result<Vec<SchemaObject>> {
    let cache = EXPECTED_MANIFESTS.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(manifest) = cache
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(&(3, 14))
        .cloned()
    {
        return Ok(manifest);
    }

    let mut reference = Connection::open_in_memory()?;
    for migration in &PROFILE[..13] {
        let tx = reference.transaction()?;
        (migration.up)(&tx)?;
        tx.commit()?;
    }
    let tx = reference.transaction()?;
    create_extension_profile_provenance(&tx)?;
    tx.commit()?;
    let manifest = schema_manifest(&reference)?;
    cache
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert((3, 14), manifest.clone());
    Ok(manifest)
}

// Apply the main lineage and extension provenance in one transaction. No
// intermediate user_version can claim one lineage's schema with the other's
// tables after a crash or a failed migration.
fn integrate_legacy_profile_v14(conn: &mut Connection) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    for migration in PROFILE
        .iter()
        .filter(|migration| (14..=22).contains(&migration.version))
    {
        (migration.up)(&tx)?;
    }
    tx.pragma_update(None, "user_version", 22)?;
    validate_manifest(&tx, PROFILE, 22)?;
    tx.commit()
}

// Work QA used 22..=29 before integration. Match the complete historical
// schema before applying the missing main migrations, in one transaction.
fn legacy_work_profile(
    conn: &Connection,
    migrations: &[Migration],
    current: i64,
) -> rusqlite::Result<bool> {
    if !std::ptr::eq(migrations.as_ptr(), PROFILE.as_ptr())
        || migrations.len() != PROFILE.len()
        || !(22..=29).contains(&current)
    {
        return Ok(false);
    }
    let mut reference = Connection::open_in_memory()?;
    for migration in PROFILE[..21]
        .iter()
        .chain(PROFILE[24..(current + 3) as usize].iter())
    {
        let tx = reference.transaction()?;
        (migration.up)(&tx)?;
        tx.commit()?;
    }
    Ok(schema_manifest(conn)? == schema_manifest(&reference)?)
}

fn integrate_legacy_work_profile(conn: &mut Connection, current: i64) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    for migration in &PROFILE[21..24] {
        (migration.up)(&tx)?;
    }
    tx.pragma_update(None, "user_version", current + 3)?;
    validate_manifest(&tx, PROFILE, current + 3)?;
    tx.commit()
}

fn validate_manifest(
    conn: &Connection,
    migrations: &[Migration],
    version: i64,
) -> rusqlite::Result<()> {
    let actual = schema_manifest(conn)?;
    let expected = expected_manifest(migrations, version)?;
    if actual != expected {
        return Err(invalid_schema(
            "database sqlite_schema does not match the claimed migration version",
        ));
    }
    Ok(())
}

fn expected_manifest(
    migrations: &[Migration],
    version: i64,
) -> rusqlite::Result<Vec<SchemaObject>> {
    let family = if std::ptr::eq(migrations.as_ptr(), META.as_ptr()) {
        1
    } else if std::ptr::eq(migrations.as_ptr(), PROFILE.as_ptr()) {
        2
    } else if std::ptr::eq(migrations.as_ptr(), WORK.as_ptr()) {
        4
    } else {
        return Err(invalid_schema("unknown migration family"));
    };
    let cache = EXPECTED_MANIFESTS.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(manifest) = cache
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(&(family, version))
        .cloned()
    {
        return Ok(manifest);
    }

    let mut reference = Connection::open_in_memory()?;
    for migration in migrations
        .iter()
        .filter(|migration| migration.version <= version)
    {
        let tx = reference.transaction()?;
        (migration.up)(&tx)?;
        tx.pragma_update(None, "user_version", migration.version)?;
        tx.commit()?;
    }
    let manifest = schema_manifest(&reference)?;
    cache
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert((family, version), manifest.clone());
    Ok(manifest)
}

/// Separate protected Work lineage. No ordinary AppData tables are imported.
pub(crate) static WORK: &[Migration] = &[Migration {
    version: 1,
    up: |tx| {
        (META[16].up)(tx)?;
        tx.execute_batch(
            "ALTER TABLE agent_work_runs ADD COLUMN result_profile TEXT
                 CHECK (result_profile IS NULL OR length(result_profile) = 26);
             CREATE TRIGGER agent_work_result_intent_immutable BEFORE UPDATE ON agent_work_runs
             WHEN NEW.result_profile IS NOT OLD.result_profile
             BEGIN SELECT RAISE(ABORT, 'work result intent is immutable'); END;
             CREATE TABLE agent_work_artifacts (
                 run_key BLOB PRIMARY KEY REFERENCES agent_work_runs(run_key),
                 profile_id TEXT NOT NULL CHECK (length(CAST(profile_id AS BLOB)) = 26),
                 artifact_id BLOB NOT NULL UNIQUE CHECK (length(artifact_id) = 16),
                 digest BLOB NOT NULL CHECK (length(digest) = 32),
                 body BLOB NOT NULL CHECK (length(body) BETWEEN 1 AND 262144)
             ) STRICT, WITHOUT ROWID;
             CREATE TRIGGER agent_work_artifact_capacity BEFORE INSERT ON agent_work_artifacts
             WHEN (SELECT coalesce(sum(length(body)), 0) FROM agent_work_artifacts) + length(NEW.body) > 33554432
             BEGIN SELECT RAISE(ABORT, 'work result capacity exceeded'); END;
             CREATE TRIGGER agent_work_artifact_immutable BEFORE UPDATE ON agent_work_artifacts
             BEGIN SELECT RAISE(ABORT, 'work result is immutable'); END;
             CREATE TABLE agent_work_profile_deletion (
                 profile_id TEXT PRIMARY KEY CHECK (length(CAST(profile_id AS BLOB)) = 26),
                 purged INTEGER NOT NULL CHECK (purged IN (0, 1))
             ) STRICT, WITHOUT ROWID;
             CREATE TRIGGER agent_work_deletion_capacity BEFORE INSERT ON agent_work_profile_deletion
             WHEN (SELECT count(*) FROM agent_work_profile_deletion) >= 1024
             BEGIN SELECT RAISE(ABORT, 'work deletion capacity exceeded'); END;
             CREATE TRIGGER agent_work_deletion_retained BEFORE DELETE ON agent_work_profile_deletion
             BEGIN SELECT RAISE(ABORT, 'work deletion authority is retained'); END;
             CREATE TRIGGER agent_work_deletion_immutable BEFORE UPDATE ON agent_work_profile_deletion
             WHEN NEW.profile_id != OLD.profile_id OR NEW.purged < OLD.purged
             BEGIN SELECT RAISE(ABORT, 'work deletion authority is immutable'); END;
             CREATE TRIGGER agent_work_deleted_artifact BEFORE INSERT ON agent_work_artifacts
             WHEN EXISTS(SELECT 1 FROM agent_work_profile_deletion WHERE profile_id = NEW.profile_id)
             BEGIN SELECT RAISE(ABORT, 'work result profile is retired'); END;
             CREATE TRIGGER agent_work_deleted_run BEFORE INSERT ON agent_work_runs
             WHEN EXISTS(SELECT 1 FROM agent_work_profile_deletion WHERE profile_id = NEW.result_profile)
             BEGIN SELECT RAISE(ABORT, 'work result profile is retired'); END;",
        )
    },
}];

fn schema_manifest(conn: &Connection) -> rusqlite::Result<Vec<SchemaObject>> {
    let count = conn.query_row("SELECT count(*) FROM sqlite_schema", [], |row| {
        row.get::<_, i64>(0)
    })?;
    if !(0..=MAX_SCHEMA_OBJECTS).contains(&count) {
        return Err(invalid_schema("database schema object count exceeds limit"));
    }

    let mut statement = conn.prepare(
        "SELECT
             CASE WHEN length(CAST(type AS BLOB)) <= 16 THEN type END,
             CASE WHEN length(CAST(name AS BLOB)) <= ?1 THEN name END,
             CASE WHEN length(CAST(tbl_name AS BLOB)) <= ?1 THEN tbl_name END,
             sql IS NULL,
             CASE WHEN length(CAST(sql AS BLOB)) <= ?2 THEN sql END
         FROM sqlite_schema
         ORDER BY type, name, tbl_name",
    )?;
    let rows = statement.query_map([MAX_SCHEMA_IDENTIFIER_BYTES, MAX_SCHEMA_SQL_BYTES], |row| {
        Ok((
            row.get::<_, Option<String>>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, bool>(3)?,
            row.get::<_, Option<String>>(4)?,
        ))
    })?;
    let mut manifest = Vec::with_capacity(count as usize);
    for row in rows {
        let (kind, name, table, sql_is_null, sql) = row?;
        let kind = kind.ok_or_else(|| invalid_schema("schema object type exceeds limit"))?;
        let name = name.ok_or_else(|| invalid_schema("schema object name exceeds limit"))?;
        let table = table.ok_or_else(|| invalid_schema("schema table name exceeds limit"))?;
        let sql = match (sql_is_null, sql) {
            (true, None) => None,
            (false, Some(sql)) => Some(normalize_schema_sql(&sql)),
            _ => {
                return Err(invalid_schema(
                    "schema SQL exceeds limit or has invalid type",
                ))
            }
        };
        manifest.push(SchemaObject {
            kind: kind.to_ascii_lowercase(),
            name,
            table,
            sql,
        });
    }
    if manifest.len() != count as usize {
        return Err(invalid_schema("database schema changed while validating"));
    }
    Ok(manifest)
}

/// SQLite preserves much of the original DDL formatting. Compare semantics
/// across harmless keyword whitespace/case changes while preserving quoted
/// identifier and string-literal bytes exactly.
fn normalize_schema_sql(sql: &str) -> String {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Mode {
        Normal,
        Single,
        Double,
        Backtick,
        Bracket,
        LineComment,
        BlockComment,
    }

    let mut normalized = String::with_capacity(sql.len());
    let mut chars = sql.chars().peekable();
    let mut mode = Mode::Normal;
    let mut pending_space = false;
    while let Some(character) = chars.next() {
        match mode {
            Mode::Single => {
                normalized.push(character);
                if character == '\'' {
                    if chars.peek() == Some(&'\'') {
                        normalized.push(chars.next().unwrap_or('\''));
                    } else {
                        mode = Mode::Normal;
                    }
                }
            }
            Mode::Double => {
                normalized.push(character);
                if character == '"' {
                    if chars.peek() == Some(&'"') {
                        normalized.push(chars.next().unwrap_or('"'));
                    } else {
                        mode = Mode::Normal;
                    }
                }
            }
            Mode::Backtick => {
                normalized.push(character);
                if character == '`' {
                    if chars.peek() == Some(&'`') {
                        normalized.push(chars.next().unwrap_or('`'));
                    } else {
                        mode = Mode::Normal;
                    }
                }
            }
            Mode::Bracket => {
                normalized.push(character);
                if character == ']' {
                    mode = Mode::Normal;
                }
            }
            Mode::LineComment => {
                normalized.push(character);
                if matches!(character, '\n' | '\r') {
                    mode = Mode::Normal;
                }
            }
            Mode::BlockComment => {
                normalized.push(character);
                if character == '*' && chars.peek() == Some(&'/') {
                    normalized.push(chars.next().unwrap_or('/'));
                    mode = Mode::Normal;
                }
            }
            Mode::Normal if character.is_whitespace() => pending_space = true,
            Mode::Normal => {
                if pending_space && !normalized.is_empty() {
                    normalized.push(' ');
                }
                pending_space = false;
                match character {
                    '\'' => {
                        normalized.push(character);
                        mode = Mode::Single;
                    }
                    '"' => {
                        normalized.push(character);
                        mode = Mode::Double;
                    }
                    '`' => {
                        normalized.push(character);
                        mode = Mode::Backtick;
                    }
                    '[' => {
                        normalized.push(character);
                        mode = Mode::Bracket;
                    }
                    '-' if chars.peek() == Some(&'-') => {
                        normalized.push('-');
                        normalized.push(chars.next().unwrap_or('-'));
                        mode = Mode::LineComment;
                    }
                    '/' if chars.peek() == Some(&'*') => {
                        normalized.push('/');
                        normalized.push(chars.next().unwrap_or('*'));
                        mode = Mode::BlockComment;
                    }
                    _ => normalized.extend(character.to_lowercase()),
                }
            }
        }
    }
    normalized
}

fn invalid_schema(message: &str) -> rusqlite::Error {
    rusqlite::Error::InvalidParameterName(message.to_owned())
}

fn preflight_macos_native_namespace_seeds(tx: &Transaction<'_>) -> rusqlite::Result<()> {
    // The v11 schema caps this table at 1,024 rows, but migrations must not
    // trust historical row contents merely because the schema manifest is
    // exact. A damaged database can otherwise make DISTINCT retain arbitrary
    // strings before any Rust-side bounded decoder sees them.
    let journal_rows = tx.query_row(
        "SELECT count(*) FROM extension_native_ownership_journal",
        [],
        |row| row.get::<_, i64>(0),
    )?;
    if !(0..=1024).contains(&journal_rows) {
        return Err(invalid_schema(
            "native-ownership journal exceeds migration capacity",
        ));
    }

    // This query returns only a Boolean. SQLite examines byte lengths and the
    // canonical alphabet without materializing profile_id in Rust. It must run
    // before count(DISTINCT profile_id) or the seed INSERT below.
    let invalid_candidate = tx.query_row(
        "SELECT EXISTS(
             SELECT 1
             FROM extension_native_ownership_journal
             WHERE runtime_backend = 'macos_native'
               AND browsing_context = 'regular'
               AND (
                   phase IN ('native_may_own', 'native_owned')
                   OR (phase = 'native_absent_release_pending' AND revision > 2)
               )
               AND (
                   typeof(profile_id) != 'text'
                   OR length(CAST(profile_id AS BLOB)) != 26
                   OR instr(CAST(profile_id AS BLOB), X'00') != 0
                   OR substr(profile_id, 1, 1) NOT BETWEEN '0' AND '7'
                   OR profile_id GLOB '*[^0123456789ABCDEFGHJKMNPQRSTVWXYZ]*'
               )
         )",
        [],
        |row| row.get::<_, bool>(0),
    )?;
    if invalid_candidate {
        return Err(invalid_schema(
            "macOS native namespace migration has an invalid profile identity",
        ));
    }

    let seed_count = tx.query_row(
        "SELECT count(DISTINCT profile_id)
         FROM extension_native_ownership_journal
         WHERE runtime_backend = 'macos_native'
           AND browsing_context = 'regular'
           AND (
               phase IN ('native_may_own', 'native_owned')
               OR (phase = 'native_absent_release_pending' AND revision > 2)
           )",
        [],
        |row| row.get::<_, i64>(0),
    )?;
    if !(0..=128).contains(&seed_count) {
        return Err(invalid_schema(
            "macOS native namespace migration exceeds obligation capacity",
        ));
    }
    Ok(())
}

// Rebuild from the immutable v18 *program-generated reference*, never SQL
// supplied by the database being migrated. Preserve every column and every
// related trigger, including cross-table history guards. Migration framework
// verifies the exact v18 schema before this function and v19 afterwards.
fn migrate_beta_native_source(tx: &Transaction) -> rusqlite::Result<()> {
    const TABLE: &str = "extension_native_ownership_journal";
    const NEXT: &str = "extension_native_ownership_journal_beta_v19";
    const OLD_ROLE: &str = "check (catalog_role in ('active', 'rollback'))";
    let reference = expected_manifest(META, 18)?;
    let table = reference
        .iter()
        .find(|object| object.kind == "table" && object.name == TABLE)
        .and_then(|object| object.sql.as_deref())
        .ok_or_else(|| invalid_schema("missing v18 native journal template"))?;
    if table.matches(OLD_ROLE).count() != 1 {
        return Err(invalid_schema("native journal role template changed"));
    }
    let table = table.replacen(TABLE, NEXT, 1).replace(
        OLD_ROLE,
        "CHECK (catalog_role IN ('active', 'rollback', 'beta'))",
    );
    let end = table
        .rfind(')')
        .ok_or_else(|| invalid_schema("native journal template has no closing boundary"))?;
    if table[end + 1..].trim() != "strict, without rowid" {
        return Err(invalid_schema("native journal template suffix changed"));
    }
    // These are the immutable V1 domains. Future domains require a new
    // migration; deriving SQL from a future mutable runtime policy is unsafe.
    const MAC: &str = "X'e3da979db873ec00b4f1b8be4496b5428f961db187b5c4588f3d5b30d60426da',X'4e86c0e24ae61e3cc7078300975eaff4a296bcf1192df0285140e21715fc9239'";
    const WIN: &str = "X'9378c48279f1efef79cd8fb2a8c0227214e13961084dd2c5f64585248c3d1368',X'b8d3da2d3adb78aaa3a4c44ae0402664bd6e9a2600de328b192bd7116076a71e'";
    let checks = format!(
        ", CHECK ((catalog_role = 'beta') = (authority IN ({MAC},{WIN}))),
        CHECK (catalog_role != 'beta' OR (browsing_context = 'regular' AND payload_kind = 2 AND
          ((authority IN ({MAC}) AND runtime_backend = 'macos_native') OR
           (authority IN ({WIN}) AND runtime_backend = 'windows_native')))),
        CHECK (catalog_role != 'beta' OR phase = 'native_absent_preparing'
          OR (phase = 'native_absent_release_pending' AND revision = 2)
          OR expected_native_identity IS NOT NULL),
        CHECK (catalog_role != 'beta' OR phase != 'native_absent_release_pending'
          OR revision != 2 OR expected_native_identity IS NULL)"
    );
    let create = format!("{}{}{};", &table[..end], checks, &table[end..]);
    rebuild_native_source_table(tx, &reference, &create, NEXT)
}

fn rebuild_native_source_table(
    tx: &Transaction,
    reference: &[SchemaObject],
    create: &str,
    next: &str,
) -> rusqlite::Result<()> {
    const TABLE: &str = "extension_native_ownership_journal";
    if !next
        .bytes()
        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    {
        return Err(invalid_schema(
            "native migration identifier is not canonical",
        ));
    }
    let count: i64 = tx.query_row(
        "SELECT count(*) FROM (SELECT 1 FROM extension_native_ownership_journal LIMIT 1025)",
        [],
        |row| row.get(0),
    )?;
    if count > 1024 {
        return Err(invalid_schema("native journal exceeds migration capacity"));
    }
    let triggers = reference
        .iter()
        .filter(|object| {
            object.kind == "trigger" && object.sql.as_ref().is_some_and(|sql| sql.contains(TABLE))
        })
        .collect::<Vec<_>>();
    for trigger in &triggers {
        if !trigger
            .name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        {
            return Err(invalid_schema("native trigger identifier is not canonical"));
        }
        tx.execute_batch(&format!(r#"DROP TRIGGER "{}";"#, trigger.name))?;
    }
    tx.execute_batch(create)?;
    // Exact v18 schema and unchanged column order make this a complete copy;
    // no native identity or source authority is synthesized from other fields.
    tx.execute_batch(&format!(
        "INSERT INTO {next} SELECT * FROM extension_native_ownership_journal;"
    ))?;
    let copied: i64 = tx.query_row(&format!("SELECT count(*) FROM {next}"), [], |row| {
        row.get(0)
    })?;
    if copied != count {
        return Err(invalid_schema("native journal migration lost rows"));
    }
    tx.execute_batch(&format!("DROP TABLE extension_native_ownership_journal; ALTER TABLE {next} RENAME TO extension_native_ownership_journal;"))?;
    for trigger in triggers {
        tx.execute_batch(
            trigger
                .sql
                .as_deref()
                .ok_or_else(|| invalid_schema("missing native trigger template"))?,
        )?;
    }
    Ok(())
}

// Local external admission is a distinct namespace. Existing signed-policy
// objects retain their historical identities and requirements.
fn migrate_local_external_native_source(tx: &Transaction) -> rusqlite::Result<()> {
    let reference = expected_manifest(META, 19)?;
    let table = reference
        .iter()
        .find(|object| {
            object.kind == "table" && object.name == "extension_native_ownership_journal"
        })
        .and_then(|object| object.sql.as_deref())
        .ok_or_else(|| invalid_schema("missing v19 native journal template"))?;
    let mut create = table.replacen(
        "extension_native_ownership_journal",
        "extension_native_ownership_journal_local_v20",
        1,
    );
    for (existing, local) in [
        ("x'e3da979db873ec00b4f1b8be4496b5428f961db187b5c4588f3d5b30d60426da',x'4e86c0e24ae61e3cc7078300975eaff4a296bcf1192df0285140e21715fc9239'", "56ffb419362f319ef825913f3727022f7c12c7e69890dc91324a2b7e8cd7b5aa"),
        ("x'9378c48279f1efef79cd8fb2a8c0227214e13961084dd2c5f64585248c3d1368',x'b8d3da2d3adb78aaa3a4c44ae0402664bd6e9a2600de328b192bd7116076a71e'", "64340b026ee26f834b57ac14d459f241ce92ad2d60a86a6c887288ae98b9a750"),
    ] {
        if create.matches(existing).count() != 2 { return Err(invalid_schema("native v19 domain template changed")); }
        create = create.replace(existing, &format!("{existing},x'{local}'"));
    }
    rebuild_native_source_table(
        tx,
        &reference,
        &create,
        "extension_native_ownership_journal_local_v20",
    )
}

pub static META: &[Migration] = &[
    Migration {
        version: 1,
        up: |tx| {
            tx.execute_batch(
                "CREATE TABLE profiles (
                     id TEXT PRIMARY KEY,
                     name TEXT NOT NULL,
                     kind TEXT NOT NULL CHECK (kind IN ('default', 'named')),
                     position INTEGER NOT NULL
                 ) STRICT;
                 CREATE TABLE state (
                     id INTEGER PRIMARY KEY CHECK (id = 1),
                     last_profile TEXT
                 ) STRICT;",
            )
        },
    },
    Migration {
        version: 2,
        up: |tx| {
            tx.execute_batch(
                "CREATE TABLE settings (
                     key TEXT PRIMARY KEY,
                     value TEXT NOT NULL
                 ) STRICT;",
            )
        },
    },
    Migration {
        version: 3,
        up: |tx| {
            tx.execute_batch(
                "CREATE TABLE session_snapshot (
                     id INTEGER PRIMARY KEY CHECK (id = 1),
                     schema_version INTEGER NOT NULL,
                     data TEXT NOT NULL
                 ) STRICT;",
            )
        },
    },
    Migration {
        version: 4,
        up: |tx| {
            tx.execute_batch(
                "DELETE FROM settings
                 WHERE length(CAST(key AS BLOB)) = 0
                    OR length(CAST(key AS BLOB)) > 256
                    OR length(CAST(value AS BLOB)) > 65536;",
            )
        },
    },
    Migration {
        version: 5,
        up: |tx| {
            tx.execute_batch(
                "CREATE TABLE session_recovery (
                     id INTEGER PRIMARY KEY CHECK (id = 1),
                     detected_at INTEGER NOT NULL,
                     reason TEXT NOT NULL,
                     schema_version INTEGER,
                     data BLOB
                 ) STRICT;",
            )
        },
    },
    Migration {
        version: 6,
        up: |tx| {
            tx.execute_batch(
                "CREATE TABLE profile_deletion_journal (
                     profile_id TEXT PRIMARY KEY,
                     authorized_at INTEGER NOT NULL
                 ) STRICT;",
            )
        },
    },
    Migration {
        version: 7,
        up: |tx| {
            tx.execute_batch(
                "ALTER TABLE profile_deletion_journal
                 ADD COLUMN native_erasure_verified INTEGER NOT NULL DEFAULT 0
                 CHECK (native_erasure_verified IN (0, 1));",
            )
        },
    },
    Migration {
        version: 8,
        up: |tx| {
            tx.execute_batch(
                "ALTER TABLE profile_deletion_journal
                 ADD COLUMN local_unlink_completed INTEGER NOT NULL DEFAULT 0
                 CHECK (local_unlink_completed IN (0, 1));",
            )
        },
    },
    Migration {
        version: 9,
        up: |tx| {
            let invalid_completion = tx.query_row(
                "SELECT EXISTS(
                     SELECT 1 FROM profile_deletion_journal
                     WHERE local_unlink_completed = 1
                       AND native_erasure_verified != 1
                 )",
                [],
                |row| row.get::<_, bool>(0),
            )?;
            if invalid_completion {
                return Err(invalid_schema(
                    "profile deletion completed local unlink without native proof",
                ));
            }
            tx.execute_batch(
                // Version 8 was exercised by development builds and is an
                // immutable on-disk boundary. Preserve its completion bit as
                // a prior-process tombstone while introducing the
                // generation-bearing representation used by the durable
                // Windows deletion protocol. The all-zero ULID is a valid,
                // reserved legacy generation that can never equal the
                // nonzero current-process token. Keep the legacy column: an
                // ADD-only migration avoids SQLite-version-dependent table
                // rewriting and makes this boundary stable across upgrades.
                "ALTER TABLE profile_deletion_journal
                 ADD COLUMN local_unlink_process TEXT
                 CHECK (local_unlink_process IS NULL OR
                        length(CAST(local_unlink_process AS BLOB)) = 26);
                 UPDATE profile_deletion_journal
                 SET local_unlink_process = '00000000000000000000000000'
                 WHERE local_unlink_completed = 1;",
            )
        },
    },
    Migration {
        version: 10,
        up: |tx| {
            tx.execute_batch(
                // Profile blocker preferences are independent authoritative
                // state, not part of the serialized session payload. Existing
                // profiles start disabled: migration must never manufacture
                // an enabled preference before a real bundled policy exists.
                "CREATE TABLE profile_blocker_settings (
                     profile_id TEXT PRIMARY KEY
                         CHECK (length(CAST(profile_id AS BLOB)) = 26),
                     revision INTEGER NOT NULL
                         CHECK (revision BETWEEN 1 AND 9223372036854775807),
                     enabled INTEGER NOT NULL
                         CHECK (enabled IN (0, 1))
                 ) STRICT;
                 INSERT INTO profile_blocker_settings(profile_id, revision, enabled)
                 SELECT id, 1, 0 FROM profiles;",
            )
        },
    },
    Migration {
        version: 11,
        up: |tx| {
            tx.execute_batch(
                // This global meta journal is the durable ordering barrier
                // around platform-native extension ownership. Rows have no
                // foreign key to profiles: retirement and restart cleanup
                // must survive profile removal and ancillary DB degradation.
                // The application-level loader additionally validates the
                // complete cohort without filtering it; SQL ordering uses
                // bounded projections rather than raw durable sort keys.
                "CREATE TABLE extension_native_ownership_journal_state (
                     id INTEGER PRIMARY KEY CHECK (id = 1),
                     revision INTEGER NOT NULL
                         CHECK (revision BETWEEN 1 AND 9223372036854775807),
                     operation_high_water INTEGER NOT NULL
                         CHECK (operation_high_water BETWEEN 0 AND 9223372036854775807),
                     native_incarnation_high_water INTEGER NOT NULL
                         CHECK (native_incarnation_high_water BETWEEN 0 AND 9223372036854775807),
                     CHECK (
                         (revision = 1
                          AND operation_high_water = 0
                          AND native_incarnation_high_water = 0)
                         OR
                         (revision > 1
                          AND operation_high_water = native_incarnation_high_water
                          AND operation_high_water BETWEEN 1 AND revision - 1)
                     )
                 ) STRICT;
                 INSERT INTO extension_native_ownership_journal_state(
                     id, revision, operation_high_water, native_incarnation_high_water
                 ) VALUES (1, 1, 0, 0);
                 CREATE TABLE extension_native_ownership_journal (
                     profile_id TEXT NOT NULL
                         CHECK (length(CAST(profile_id AS BLOB)) = 26
                                AND instr(CAST(profile_id AS BLOB), X'00') = 0),
                     install_id BLOB NOT NULL
                         CHECK (typeof(install_id) = 'blob' AND length(install_id) = 16),
                     browsing_context TEXT NOT NULL
                         CHECK (browsing_context IN ('regular', 'private')),
                     operation INTEGER NOT NULL UNIQUE
                         CHECK (operation BETWEEN 1 AND 9223372036854775807),
                     revision INTEGER NOT NULL
                         CHECK (revision BETWEEN 1 AND 9223372036854775807),
                     authority BLOB NOT NULL
                         CHECK (typeof(authority) = 'blob' AND length(authority) = 32),
                     package_key BLOB NOT NULL
                         CHECK (typeof(package_key) = 'blob' AND length(package_key) = 32),
                     package_revision INTEGER NOT NULL
                         CHECK (package_revision BETWEEN 1 AND 9223372036854775807),
                     payload_kind INTEGER NOT NULL CHECK (payload_kind IN (1, 2)),
                     archive_length INTEGER,
                     archive_sha256 BLOB,
                     manifest_sha256 BLOB NOT NULL
                         CHECK (typeof(manifest_sha256) = 'blob' AND length(manifest_sha256) = 32),
                     tree_sha256 BLOB NOT NULL
                         CHECK (typeof(tree_sha256) = 'blob' AND length(tree_sha256) = 32),
                     catalog_set_sha256 BLOB NOT NULL
                         CHECK (typeof(catalog_set_sha256) = 'blob' AND length(catalog_set_sha256) = 32),
                     catalog_role TEXT NOT NULL
                         CHECK (catalog_role IN ('active', 'rollback')),
                     store_catalog_revision INTEGER NOT NULL
                         CHECK (store_catalog_revision BETWEEN 1 AND 9223372036854775807),
                     store_install_revision INTEGER NOT NULL
                         CHECK (store_install_revision BETWEEN 1 AND 9223372036854775807),
                     store_grant_revision INTEGER NOT NULL
                         CHECK (store_grant_revision BETWEEN 1 AND 9223372036854775807),
                     grant_sha256 BLOB NOT NULL
                         CHECK (typeof(grant_sha256) = 'blob' AND length(grant_sha256) = 32),
                     runtime_backend TEXT NOT NULL CHECK (runtime_backend IN (
                         'macos_native', 'macos_compatibility',
                         'linux_compatibility', 'windows_native'
                     )),
                     native_incarnation INTEGER NOT NULL UNIQUE
                         CHECK (native_incarnation BETWEEN 1 AND 9223372036854775807),
                     intent TEXT NOT NULL CHECK (intent IN ('acquire', 'release')),
                     phase TEXT NOT NULL CHECK (phase IN (
                         'native_absent_preparing', 'native_may_own',
                         'native_owned', 'native_absent_release_pending'
                     )),
                     PRIMARY KEY (profile_id, install_id, browsing_context),
                     CHECK (
                         (payload_kind = 1
                          AND archive_length IS NULL
                          AND archive_sha256 IS NULL)
                         OR
                         (payload_kind = 2
                          AND typeof(archive_length) = 'integer'
                          AND archive_length BETWEEN 1 AND 67108864
                          AND typeof(archive_sha256) = 'blob'
                          AND length(archive_sha256) = 32)
                     ),
                     CHECK (
                         operation = native_incarnation
                     ),
                     CHECK (
                         (intent = 'acquire'
                          AND phase = 'native_absent_preparing'
                          AND revision = 1)
                         OR
                         (intent = 'acquire'
                          AND phase = 'native_may_own'
                          AND revision = 2)
                         OR
                         (intent = 'acquire'
                          AND phase = 'native_owned'
                          AND revision = 3)
                         OR
                         (intent = 'release'
                          AND phase = 'native_may_own'
                          AND revision BETWEEN 3 AND 4)
                         OR
                         (intent = 'release'
                          AND phase = 'native_absent_release_pending'
                          AND revision BETWEEN 2 AND 5)
                     )
                 ) STRICT, WITHOUT ROWID;
                 CREATE TRIGGER extension_native_ownership_journal_capacity
                 BEFORE INSERT ON extension_native_ownership_journal
                 WHEN (SELECT count(*) FROM extension_native_ownership_journal) >= 1024
                 BEGIN
                     SELECT RAISE(ABORT, 'native-ownership journal capacity exceeded');
                 END;
                 CREATE TRIGGER extension_native_ownership_journal_state_reachable
                 BEFORE UPDATE OF revision, operation_high_water,
                                  native_incarnation_high_water
                 ON extension_native_ownership_journal_state
                 BEGIN
                     SELECT CASE
                         WHEN live_count > NEW.operation_high_water
                           OR EXISTS (
                               SELECT 1
                               FROM extension_native_ownership_journal
                               WHERE operation > NEW.operation_high_water
                                  OR native_incarnation > NEW.native_incarnation_high_water
                                  OR operation > NEW.revision - revision
                           )
                         THEN RAISE(ABORT, 'native-ownership row exceeds journal authority')
                         WHEN NEW.revision - (1 + live_revision_sum) < 0
                         THEN RAISE(ABORT, 'native-ownership journal history is impossible')
                         WHEN NEW.operation_high_water - live_count
                              > (NEW.revision - (1 + live_revision_sum)) / 3
                         THEN RAISE(ABORT, 'native-ownership journal history is too short')
                         WHEN NEW.operation_high_water - live_count
                              < (NEW.revision - (1 + live_revision_sum)) / 6
                           OR (
                               NEW.operation_high_water - live_count
                               = (NEW.revision - (1 + live_revision_sum)) / 6
                               AND (NEW.revision - (1 + live_revision_sum)) % 6 != 0
                           )
                         THEN RAISE(ABORT, 'native-ownership journal history is too long')
                     END
                     FROM (
                         SELECT count(*) AS live_count,
                                coalesce(sum(revision), 0) AS live_revision_sum
                         FROM extension_native_ownership_journal
                     );
                 END;",
            )
        },
    },
    Migration {
        version: 12,
        up: |tx| {
            tx.execute_batch(
                // Native owner identifiers are optional until positively
                // observed, but once present they are exact, backend-bound,
                // fixed-width authority. Rebuild instead of weakening v11's
                // immutable schema boundary. Any legacy native-owned row that
                // lacks an identifier makes the transaction fail closed; an
                // identity cannot be inferred during migration.
                "DROP TRIGGER extension_native_ownership_journal_capacity;
                 DROP TRIGGER extension_native_ownership_journal_state_reachable;
                 ALTER TABLE extension_native_ownership_journal
                 RENAME TO extension_native_ownership_journal_v11;
                 CREATE TABLE extension_native_ownership_journal (
                     profile_id TEXT NOT NULL
                         CHECK (length(CAST(profile_id AS BLOB)) = 26
                                AND instr(CAST(profile_id AS BLOB), X'00') = 0),
                     install_id BLOB NOT NULL
                         CHECK (typeof(install_id) = 'blob' AND length(install_id) = 16),
                     browsing_context TEXT NOT NULL
                         CHECK (browsing_context IN ('regular', 'private')),
                     operation INTEGER NOT NULL UNIQUE
                         CHECK (operation BETWEEN 1 AND 9223372036854775807),
                     revision INTEGER NOT NULL
                         CHECK (revision BETWEEN 1 AND 9223372036854775807),
                     authority BLOB NOT NULL
                         CHECK (typeof(authority) = 'blob' AND length(authority) = 32),
                     package_key BLOB NOT NULL
                         CHECK (typeof(package_key) = 'blob' AND length(package_key) = 32),
                     package_revision INTEGER NOT NULL
                         CHECK (package_revision BETWEEN 1 AND 9223372036854775807),
                     payload_kind INTEGER NOT NULL CHECK (payload_kind IN (1, 2)),
                     archive_length INTEGER,
                     archive_sha256 BLOB,
                     manifest_sha256 BLOB NOT NULL
                         CHECK (typeof(manifest_sha256) = 'blob' AND length(manifest_sha256) = 32),
                     tree_sha256 BLOB NOT NULL
                         CHECK (typeof(tree_sha256) = 'blob' AND length(tree_sha256) = 32),
                     catalog_set_sha256 BLOB NOT NULL
                         CHECK (typeof(catalog_set_sha256) = 'blob' AND length(catalog_set_sha256) = 32),
                     catalog_role TEXT NOT NULL
                         CHECK (catalog_role IN ('active', 'rollback')),
                     store_catalog_revision INTEGER NOT NULL
                         CHECK (store_catalog_revision BETWEEN 1 AND 9223372036854775807),
                     store_install_revision INTEGER NOT NULL
                         CHECK (store_install_revision BETWEEN 1 AND 9223372036854775807),
                     store_grant_revision INTEGER NOT NULL
                         CHECK (store_grant_revision BETWEEN 1 AND 9223372036854775807),
                     grant_sha256 BLOB NOT NULL
                         CHECK (typeof(grant_sha256) = 'blob' AND length(grant_sha256) = 32),
                     runtime_backend TEXT NOT NULL CHECK (runtime_backend IN (
                         'macos_native', 'macos_compatibility',
                         'linux_compatibility', 'windows_native'
                     )),
                     native_identity_kind INTEGER,
                     native_identity BLOB,
                     native_incarnation INTEGER NOT NULL UNIQUE
                         CHECK (native_incarnation BETWEEN 1 AND 9223372036854775807),
                     intent TEXT NOT NULL CHECK (intent IN ('acquire', 'release')),
                     phase TEXT NOT NULL CHECK (phase IN (
                         'native_absent_preparing', 'native_may_own',
                         'native_owned', 'native_absent_release_pending'
                     )),
                     PRIMARY KEY (profile_id, install_id, browsing_context),
                     CHECK (
                         (payload_kind = 1
                          AND archive_length IS NULL
                          AND archive_sha256 IS NULL)
                         OR
                         (payload_kind = 2
                          AND typeof(archive_length) = 'integer'
                          AND archive_length BETWEEN 1 AND 67108864
                          AND typeof(archive_sha256) = 'blob'
                          AND length(archive_sha256) = 32)
                     ),
                     CHECK (operation = native_incarnation),
                     CHECK (
                         (native_identity_kind IS NULL) = (native_identity IS NULL)
                     ),
                     CHECK (
                         (native_identity_kind IS NULL AND native_identity IS NULL)
                         OR
                         (native_identity_kind IS NOT NULL
                          AND native_identity IS NOT NULL
                          AND (
                              (native_identity_kind = 1
                               AND runtime_backend = 'macos_native'
                               AND typeof(native_identity) = 'blob'
                               AND length(native_identity) = 32
                               AND length(CAST(native_identity AS TEXT)) = 32
                               AND CAST(native_identity AS TEXT) NOT GLOB '*[^a-p]*')
                              OR
                              (native_identity_kind = 2
                               AND runtime_backend = 'windows_native'
                               AND typeof(native_identity) = 'blob'
                               AND length(native_identity) = 32
                               AND length(CAST(native_identity AS TEXT)) = 32
                               AND CAST(native_identity AS TEXT) NOT GLOB '*[^a-p]*')
                          ))
                     ),
                     CHECK (
                         (intent = 'acquire'
                          AND phase = 'native_absent_preparing'
                          AND revision = 1
                          AND native_identity IS NULL)
                         OR
                         (intent = 'acquire'
                          AND phase = 'native_may_own'
                          AND ((revision = 2 AND native_identity IS NULL)
                               OR (revision = 3 AND native_identity IS NOT NULL)))
                         OR
                         (intent = 'acquire'
                          AND phase = 'native_owned'
                          AND ((runtime_backend IN ('macos_native', 'windows_native')
                                AND revision BETWEEN 3 AND 4
                                AND native_identity IS NOT NULL)
                               OR
                               (runtime_backend IN ('macos_compatibility', 'linux_compatibility')
                                AND revision = 3
                                AND native_identity IS NULL)))
                         OR
                         (intent = 'release'
                          AND phase = 'native_may_own'
                          AND ((revision = 3)
                               OR revision = 4
                               OR (revision = 5 AND native_identity IS NOT NULL)))
                         OR
                         (intent = 'release'
                          AND phase = 'native_absent_release_pending'
                          AND ((revision BETWEEN 2 AND 3 AND native_identity IS NULL)
                               OR revision = 4
                               OR revision = 5
                               OR (revision = 6 AND native_identity IS NOT NULL)))
                     )
                 ) STRICT, WITHOUT ROWID;
                 INSERT INTO extension_native_ownership_journal(
                     profile_id, install_id, browsing_context, operation, revision,
                     authority, package_key, package_revision,
                     payload_kind, archive_length, archive_sha256,
                     manifest_sha256, tree_sha256,
                     catalog_set_sha256, catalog_role,
                     store_catalog_revision, store_install_revision, store_grant_revision,
                     grant_sha256, runtime_backend, native_identity_kind, native_identity,
                     native_incarnation, intent, phase
                 )
                 SELECT profile_id, install_id, browsing_context, operation, revision,
                        authority, package_key, package_revision,
                        payload_kind, archive_length, archive_sha256,
                        manifest_sha256, tree_sha256,
                        catalog_set_sha256, catalog_role,
                        store_catalog_revision, store_install_revision, store_grant_revision,
                        grant_sha256, runtime_backend, NULL, NULL,
                        native_incarnation, intent, phase
                 FROM extension_native_ownership_journal_v11;
                 DROP TABLE extension_native_ownership_journal_v11;
                 CREATE TRIGGER extension_native_ownership_journal_capacity
                 BEFORE INSERT ON extension_native_ownership_journal
                 WHEN (SELECT count(*) FROM extension_native_ownership_journal) >= 1024
                 BEGIN
                     SELECT RAISE(ABORT, 'native-ownership journal capacity exceeded');
                 END;
                 CREATE TRIGGER extension_native_ownership_journal_state_reachable
                 BEFORE UPDATE OF revision, operation_high_water,
                                  native_incarnation_high_water
                 ON extension_native_ownership_journal_state
                 BEGIN
                     SELECT CASE
                         WHEN live_count > NEW.operation_high_water
                           OR EXISTS (
                               SELECT 1
                               FROM extension_native_ownership_journal
                               WHERE operation > NEW.operation_high_water
                                  OR native_incarnation > NEW.native_incarnation_high_water
                                  OR operation > NEW.revision - revision
                           )
                         THEN RAISE(ABORT, 'native-ownership row exceeds journal authority')
                         WHEN NEW.revision - (1 + live_revision_sum) < 0
                         THEN RAISE(ABORT, 'native-ownership journal history is impossible')
                         WHEN NEW.operation_high_water - live_count
                              > (NEW.revision - (1 + live_revision_sum)) / 3
                         THEN RAISE(ABORT, 'native-ownership journal history is too short')
                         WHEN NEW.operation_high_water - live_count
                              < (NEW.revision - (1 + live_revision_sum)) / 7
                           OR (
                               NEW.operation_high_water - live_count
                               = (NEW.revision - (1 + live_revision_sum)) / 7
                               AND (NEW.revision - (1 + live_revision_sum)) % 7 != 0
                           )
                         THEN RAISE(ABORT, 'native-ownership journal history is too long')
                     END
                     FROM (
                         SELECT count(*) AS live_count,
                                coalesce(sum(revision), 0) AS live_revision_sum
                         FROM extension_native_ownership_journal
                     );
                 END;",
            )
        },
    },
    Migration {
        version: 13,
        up: |tx| {
            tx.execute_batch(
                // The package-authenticated expectation and adapter-observed
                // identity are independent facts. Keep every v12 expectation
                // NULL: migration cannot infer trust from an observed owner.
                // Legacy observed-only rows remain representable so startup
                // recovery can conservatively clean them up.
                "ALTER TABLE extension_native_ownership_journal
                     ADD COLUMN expected_native_identity_kind INTEGER;
                 ALTER TABLE extension_native_ownership_journal
                     ADD COLUMN expected_native_identity BLOB
                     CHECK (
                         (expected_native_identity_kind IS NULL
                          AND expected_native_identity IS NULL)
                         OR
                         (expected_native_identity_kind IS NOT NULL
                          AND expected_native_identity IS NOT NULL
                          AND (
                              (expected_native_identity_kind = 1
                               AND runtime_backend = 'macos_native'
                               AND typeof(expected_native_identity) = 'blob'
                               AND length(expected_native_identity) = 32
                               AND length(CAST(expected_native_identity AS TEXT)) = 32
                               AND CAST(expected_native_identity AS TEXT) NOT GLOB '*[^a-p]*')
                              OR
                              (expected_native_identity_kind = 2
                               AND runtime_backend = 'windows_native'
                               AND typeof(expected_native_identity) = 'blob'
                               AND length(expected_native_identity) = 32
                               AND length(CAST(expected_native_identity AS TEXT)) = 32
                               AND CAST(expected_native_identity AS TEXT) NOT GLOB '*[^a-p]*')
                          ))
                     )
                     CHECK (
                         phase != 'native_absent_preparing'
                         OR expected_native_identity IS NULL
                     )
                     CHECK (
                         expected_native_identity IS NULL
                         OR intent != 'acquire'
                         OR phase != 'native_owned'
                         OR (native_identity_kind IS NOT NULL
                             AND native_identity IS NOT NULL
                             AND native_identity_kind = expected_native_identity_kind
                             AND native_identity = expected_native_identity)
                     );",
            )
        },
    },
    Migration {
        version: 14,
        up: |tx| {
            // META v13 shipped before product code could create a native
            // WKWebExtensionController. Consequently, a cleared historical
            // row cannot hide a product-created namespace at this migration
            // boundary. Extant regular macOS-native possible-owner rows are
            // nevertheless exact backend evidence and must be seeded. This
            // release invariant is the migration witness; do not broaden this
            // query by guessing from unrelated profile state.
            preflight_macos_native_namespace_seeds(tx)?;
            let unanchored_seed = tx.query_row(
                "SELECT EXISTS(
                     SELECT 1
                     FROM extension_native_ownership_journal AS ownership
                     WHERE ownership.runtime_backend = 'macos_native'
                       AND ownership.browsing_context = 'regular'
                       AND (
                           ownership.phase IN ('native_may_own', 'native_owned')
                           OR (ownership.phase = 'native_absent_release_pending'
                               AND ownership.revision > 2)
                       )
                       AND NOT EXISTS (
                           SELECT 1 FROM profiles
                           WHERE profiles.id = ownership.profile_id
                       )
                       AND NOT EXISTS (
                           SELECT 1 FROM profile_deletion_journal
                           WHERE profile_deletion_journal.profile_id = ownership.profile_id
                       )
                 )",
                [],
                |row| row.get::<_, bool>(0),
            )?;
            if unanchored_seed {
                return Err(invalid_schema(
                    "macOS native namespace migration has no profile or deletion anchor",
                ));
            }
            let seed_after_native_proof = tx.query_row(
                "SELECT EXISTS(
                     SELECT 1
                     FROM extension_native_ownership_journal AS ownership
                     JOIN profile_deletion_journal AS deletion
                       ON deletion.profile_id = ownership.profile_id
                     WHERE ownership.runtime_backend = 'macos_native'
                       AND ownership.browsing_context = 'regular'
                       AND (
                           ownership.phase IN ('native_may_own', 'native_owned')
                           OR (ownership.phase = 'native_absent_release_pending'
                               AND ownership.revision > 2)
                       )
                       AND deletion.native_erasure_verified = 1
                 )",
                [],
                |row| row.get::<_, bool>(0),
            )?;
            if seed_after_native_proof {
                return Err(invalid_schema(
                    "macOS native namespace migration contradicts native-erasure proof",
                ));
            }

            tx.execute_batch(
                // A row means only that the deterministic V1 namespace may
                // exist and therefore must be included in profile erasure.
                // It deliberately survives runtime-row retirement, disable,
                // uninstall, and package-pin release.
                "CREATE TABLE extension_native_namespace_obligations (
                     profile_id TEXT NOT NULL
                         CHECK (
                             length(CAST(profile_id AS BLOB)) = 26
                             AND instr(CAST(profile_id AS BLOB), X'00') = 0
                             AND substr(profile_id, 1, 1) BETWEEN '0' AND '7'
                             AND profile_id NOT GLOB '*[^0123456789ABCDEFGHJKMNPQRSTVWXYZ]*'
                         ),
                     namespace_version INTEGER NOT NULL
                         CHECK (namespace_version = 1),
                     PRIMARY KEY (profile_id, namespace_version)
                 ) STRICT, WITHOUT ROWID;

                 CREATE TRIGGER extension_native_namespace_obligation_capacity
                 BEFORE INSERT ON extension_native_namespace_obligations
                 WHEN (SELECT count(*)
                       FROM extension_native_namespace_obligations) >= 128
                 BEGIN
                     SELECT RAISE(ABORT,
                         'native extension namespace obligation capacity exceeded');
                 END;

                 CREATE TRIGGER extension_native_namespace_obligation_insert_anchor
                 BEFORE INSERT ON extension_native_namespace_obligations
                 WHEN NOT EXISTS (
                          SELECT 1 FROM profiles WHERE id = NEW.profile_id
                      )
                      AND NOT EXISTS (
                          SELECT 1 FROM profile_deletion_journal
                          WHERE profile_id = NEW.profile_id
                      )
                 BEGIN
                     SELECT RAISE(ABORT,
                         'native extension namespace obligation has no durable anchor');
                 END;

                 CREATE TRIGGER extension_native_namespace_obligation_immutable
                 BEFORE UPDATE ON extension_native_namespace_obligations
                 BEGIN
                     SELECT RAISE(ABORT,
                         'native extension namespace obligation identity is immutable');
                 END;

                 CREATE TRIGGER extension_native_namespace_profile_anchor_delete
                 BEFORE DELETE ON profiles
                 WHEN EXISTS (
                          SELECT 1 FROM extension_native_namespace_obligations
                          WHERE profile_id = OLD.id
                      )
                      AND NOT EXISTS (
                          SELECT 1 FROM profile_deletion_journal
                          WHERE profile_id = OLD.id
                      )
                 BEGIN
                     SELECT RAISE(ABORT,
                         'native extension namespace obligation requires deletion journal');
                 END;

                 CREATE TRIGGER extension_native_namespace_profile_anchor_update
                 BEFORE UPDATE OF id ON profiles
                 WHEN NEW.id != OLD.id
                      AND EXISTS (
                          SELECT 1 FROM extension_native_namespace_obligations
                          WHERE profile_id = OLD.id
                      )
                 BEGIN
                     SELECT RAISE(ABORT,
                         'native extension namespace obligation blocks profile identity change');
                 END;

                 CREATE TRIGGER extension_native_namespace_deletion_anchor_delete
                 BEFORE DELETE ON profile_deletion_journal
                 WHEN EXISTS (
                     SELECT 1 FROM extension_native_namespace_obligations
                     WHERE profile_id = OLD.profile_id
                 )
                 BEGIN
                     SELECT RAISE(ABORT,
                         'native extension namespace obligation blocks deletion completion');
                 END;

                 CREATE TRIGGER extension_native_namespace_deletion_anchor_update
                 BEFORE UPDATE OF profile_id ON profile_deletion_journal
                 WHEN NEW.profile_id != OLD.profile_id
                      AND EXISTS (
                          SELECT 1 FROM extension_native_namespace_obligations
                          WHERE profile_id = OLD.profile_id
                      )
                 BEGIN
                     SELECT RAISE(ABORT,
                         'native extension namespace obligation blocks deletion identity change');
                 END;

                 CREATE TRIGGER extension_native_namespace_proof_requires_absence
                 BEFORE UPDATE OF native_erasure_verified
                 ON profile_deletion_journal
                 WHEN NEW.native_erasure_verified = 1
                      AND EXISTS (
                          SELECT 1 FROM extension_native_namespace_obligations
                          WHERE profile_id = NEW.profile_id
                      )
                 BEGIN
                     SELECT RAISE(ABORT,
                         'native extension namespace obligation lacks erasure settlement');
                 END;

                 CREATE TRIGGER extension_native_namespace_delete_requires_pending_proof
                 BEFORE DELETE ON extension_native_namespace_obligations
                 WHEN NOT EXISTS (
                     SELECT 1 FROM profile_deletion_journal
                     WHERE profile_id = OLD.profile_id
                       AND native_erasure_verified = 0
                 )
                 BEGIN
                     SELECT RAISE(ABORT,
                         'native extension namespace erasure has no pending deletion proof');
                 END;

                 CREATE TRIGGER extension_native_namespace_delete_records_proof
                 AFTER DELETE ON extension_native_namespace_obligations
                 BEGIN
                     UPDATE profile_deletion_journal
                     SET native_erasure_verified = 1
                     WHERE profile_id = OLD.profile_id
                       AND native_erasure_verified = 0;
                     SELECT CASE WHEN changes() != 1 THEN RAISE(ABORT,
                         'native extension namespace proof was not recorded exactly once') END;
                 END;

                 INSERT INTO extension_native_namespace_obligations(
                     profile_id, namespace_version
                 )
                 SELECT DISTINCT profile_id, 1
                 FROM extension_native_ownership_journal
                 WHERE runtime_backend = 'macos_native'
                   AND browsing_context = 'regular'
                   AND (
                       phase IN ('native_may_own', 'native_owned')
                       OR (phase = 'native_absent_release_pending' AND revision > 2)
                   );",
            )
        },
    },
    Migration {
        version: 15,
        up: |tx| {
            tx.execute_batch(
                // A live native owner can adopt a newly committed optional
                // grant without changing its lifecycle phase or incarnation.
                // Record those global-CAS advances independently so the
                // original lifecycle-history proof remains exact rather than
                // treating a grant change as a fictional native transition.
                "ALTER TABLE extension_native_ownership_journal_state
                 ADD COLUMN grant_rebind_count INTEGER NOT NULL DEFAULT 0
                     CHECK (grant_rebind_count BETWEEN 0 AND 9223372036854775807);

                 DROP TRIGGER extension_native_ownership_journal_state_reachable;
                 CREATE TRIGGER extension_native_ownership_journal_state_reachable
                 BEFORE UPDATE OF revision, operation_high_water,
                                  native_incarnation_high_water, grant_rebind_count
                 ON extension_native_ownership_journal_state
                 BEGIN
                     SELECT CASE
                         WHEN NEW.grant_rebind_count > NEW.revision - 1
                         THEN RAISE(ABORT,
                             'native-ownership grant rebind history is impossible')
                         WHEN live_count > NEW.operation_high_water
                           OR EXISTS (
                               SELECT 1
                               FROM extension_native_ownership_journal
                               WHERE operation > NEW.operation_high_water
                                  OR native_incarnation > NEW.native_incarnation_high_water
                                  OR operation >
                                     (NEW.revision - NEW.grant_rebind_count) - revision
                           )
                         THEN RAISE(ABORT,
                             'native-ownership row exceeds journal authority')
                         WHEN (NEW.revision - NEW.grant_rebind_count)
                              - (1 + live_revision_sum) < 0
                         THEN RAISE(ABORT,
                             'native-ownership journal history is impossible')
                         WHEN NEW.operation_high_water - live_count
                              > ((NEW.revision - NEW.grant_rebind_count)
                                 - (1 + live_revision_sum)) / 3
                         THEN RAISE(ABORT,
                             'native-ownership journal history is too short')
                         WHEN NEW.operation_high_water - live_count
                              < ((NEW.revision - NEW.grant_rebind_count)
                                 - (1 + live_revision_sum)) / 7
                           OR (
                               NEW.operation_high_water - live_count
                               = ((NEW.revision - NEW.grant_rebind_count)
                                  - (1 + live_revision_sum)) / 7
                               AND ((NEW.revision - NEW.grant_rebind_count)
                                    - (1 + live_revision_sum)) % 7 != 0
                           )
                         THEN RAISE(ABORT,
                             'native-ownership journal history is too long')
                     END
                     FROM (
                         SELECT count(*) AS live_count,
                                coalesce(sum(revision), 0) AS live_revision_sum
                         FROM extension_native_ownership_journal
                     );
                 END;",
            )
        },
    },
    Migration {
        version: 16,
        up: |tx| {
            tx.execute_batch(
                // Agent audit is deliberately app-global: one run may span
                // several profiles, while these records contain neither a
                // profile identity nor page/provider content. The state row
                // enforces a fail-closed durable ceiling without an O(n)
                // count query on every append. Rows are immutable; retention
                // requires a future explicit product migration, never silent
                // eviction from the append path.
                "CREATE TABLE agent_audit_state (
                     id INTEGER PRIMARY KEY CHECK (id = 1),
                     delivery_count INTEGER NOT NULL
                         CHECK (delivery_count BETWEEN 0 AND 262144),
                     event_count INTEGER NOT NULL
                         CHECK (event_count BETWEEN 0 AND 262144)
                 ) STRICT;
                 INSERT INTO agent_audit_state(id, delivery_count, event_count)
                 VALUES (1, 0, 0);

                 CREATE TABLE agent_audit_deliveries (
                     manifest_id BLOB NOT NULL
                         CHECK (length(manifest_id) = 16),
                     supervisor_id BLOB NOT NULL
                         CHECK (length(supervisor_id) = 8
                                AND supervisor_id != X'0000000000000000'),
                     delivery_id BLOB NOT NULL
                         CHECK (length(delivery_id) = 8
                                AND delivery_id != X'0000000000000000'),
                     first_event_id BLOB NOT NULL
                         CHECK (length(first_event_id) = 8
                                AND first_event_id != X'0000000000000000'),
                     last_event_id BLOB NOT NULL
                         CHECK (length(last_event_id) = 8
                                AND last_event_id != X'0000000000000000'),
                     event_count INTEGER NOT NULL
                         CHECK (event_count BETWEEN 1 AND 16),
                     CHECK (first_event_id <= last_event_id),
                     PRIMARY KEY (manifest_id, supervisor_id, delivery_id)
                 ) STRICT, WITHOUT ROWID;

                 CREATE TABLE agent_audit_events (
                     manifest_id BLOB NOT NULL
                         CHECK (length(manifest_id) = 16),
                     supervisor_id BLOB NOT NULL
                         CHECK (length(supervisor_id) = 8
                                AND supervisor_id != X'0000000000000000'),
                     event_id BLOB NOT NULL
                         CHECK (length(event_id) = 8
                                AND event_id != X'0000000000000000'),
                     delivery_id BLOB NOT NULL
                         CHECK (length(delivery_id) = 8
                                AND delivery_id != X'0000000000000000'),
                     batch_index INTEGER NOT NULL
                         CHECK (batch_index BETWEEN 0 AND 15),
                     recorded_at BLOB NOT NULL
                         CHECK (length(recorded_at) = 8),
                     record_version INTEGER NOT NULL
                         CHECK (record_version = 1),
                     record BLOB NOT NULL
                         CHECK (length(record) = 128
                                AND substr(record, 1, 1) = X'01'),
                     PRIMARY KEY (manifest_id, supervisor_id, event_id),
                     UNIQUE (manifest_id, supervisor_id, delivery_id, batch_index),
                     FOREIGN KEY (manifest_id, supervisor_id, delivery_id)
                         REFERENCES agent_audit_deliveries(
                             manifest_id, supervisor_id, delivery_id
                         ) ON DELETE RESTRICT ON UPDATE RESTRICT
                 ) STRICT, WITHOUT ROWID;

                 CREATE TRIGGER agent_audit_delivery_capacity
                 BEFORE INSERT ON agent_audit_deliveries
                 WHEN (SELECT delivery_count FROM agent_audit_state WHERE id = 1)
                      >= 262144
                 BEGIN
                     SELECT RAISE(ABORT, 'agent audit delivery capacity exceeded');
                 END;
                 CREATE TRIGGER agent_audit_event_capacity
                 BEFORE INSERT ON agent_audit_events
                 WHEN (SELECT event_count FROM agent_audit_state WHERE id = 1)
                      >= 262144
                 BEGIN
                     SELECT RAISE(ABORT, 'agent audit event capacity exceeded');
                 END;
                 CREATE TRIGGER agent_audit_delivery_count
                 AFTER INSERT ON agent_audit_deliveries
                 BEGIN
                     UPDATE agent_audit_state
                     SET delivery_count = delivery_count + 1 WHERE id = 1;
                     SELECT CASE WHEN changes() != 1 THEN RAISE(ABORT,
                         'agent audit delivery count is unavailable') END;
                 END;
                 CREATE TRIGGER agent_audit_event_count
                 AFTER INSERT ON agent_audit_events
                 BEGIN
                     UPDATE agent_audit_state
                     SET event_count = event_count + 1 WHERE id = 1;
                     SELECT CASE WHEN changes() != 1 THEN RAISE(ABORT,
                         'agent audit event count is unavailable') END;
                 END;
                 CREATE TRIGGER agent_audit_state_update_exact
                 BEFORE UPDATE ON agent_audit_state
                 WHEN NOT (
                     (NEW.delivery_count = OLD.delivery_count + 1
                      AND NEW.event_count = OLD.event_count)
                     OR
                     (NEW.delivery_count = OLD.delivery_count
                      AND NEW.event_count = OLD.event_count + 1)
                 )
                 BEGIN
                     SELECT RAISE(ABORT, 'agent audit count transition is invalid');
                 END;
                 CREATE TRIGGER agent_audit_state_immutable
                 BEFORE DELETE ON agent_audit_state
                 BEGIN
                     SELECT RAISE(ABORT, 'agent audit state is immutable');
                 END;
                 CREATE TRIGGER agent_audit_delivery_immutable
                 BEFORE UPDATE ON agent_audit_deliveries
                 BEGIN
                     SELECT RAISE(ABORT, 'agent audit delivery is immutable');
                 END;
                 CREATE TRIGGER agent_audit_delivery_retained
                 BEFORE DELETE ON agent_audit_deliveries
                 BEGIN
                     SELECT RAISE(ABORT, 'agent audit delivery retention is explicit');
                 END;
                 CREATE TRIGGER agent_audit_event_immutable
                 BEFORE UPDATE ON agent_audit_events
                 BEGIN
                     SELECT RAISE(ABORT, 'agent audit event is immutable');
                 END;
                 CREATE TRIGGER agent_audit_event_retained
                 BEFORE DELETE ON agent_audit_events
                 BEGIN
                     SELECT RAISE(ABORT, 'agent audit event retention is explicit');
                 END;",
            )
        },
    },
    Migration {
        version: 17,
        up: |tx| {
            tx.execute_batch(
                "CREATE TABLE agent_work_owner (
                 id INTEGER PRIMARY KEY CHECK (id = 1),
                 incarnation BLOB NOT NULL CHECK (length(incarnation) = 16)
             ) STRICT;
             CREATE TABLE agent_work_runs (
                 run_key BLOB PRIMARY KEY CHECK (length(run_key) = 32),
                 record BLOB NOT NULL CHECK (length(record) = 96
                     AND substr(record, 1, 1) = X'01'
                     AND substr(record, 33, 32) = run_key),
                 terminal INTEGER NOT NULL CHECK (terminal IN (0, 1))
             ) STRICT, WITHOUT ROWID;
             CREATE UNIQUE INDEX agent_work_run_identity
                 ON agent_work_runs(substr(run_key, 17, 16));
             CREATE TRIGGER agent_work_capacity BEFORE INSERT ON agent_work_runs
             WHEN (SELECT count(*) FROM agent_work_runs) >= 1024
             BEGIN SELECT RAISE(ABORT, 'work capacity exceeded'); END;
             CREATE TRIGGER agent_work_terminal_immutable BEFORE UPDATE ON agent_work_runs
             WHEN OLD.terminal = 1 OR NEW.run_key != OLD.run_key
             BEGIN SELECT RAISE(ABORT, 'work terminal is immutable'); END;
             CREATE TRIGGER agent_work_retained BEFORE DELETE ON agent_work_runs
             BEGIN SELECT RAISE(ABORT, 'work retention is explicit'); END;",
            )
        },
    },
    Migration {
        version: 18,
        up: |tx| {
            tx.execute_batch(
                "ALTER TABLE agent_work_runs ADD COLUMN result_profile TEXT
                     CHECK (result_profile IS NULL OR length(result_profile) = 26);
                 CREATE TRIGGER agent_work_result_intent_immutable BEFORE UPDATE ON agent_work_runs
                 WHEN NEW.result_profile IS NOT OLD.result_profile
                 BEGIN SELECT RAISE(ABORT, 'work result intent is immutable'); END;
                 CREATE TABLE agent_work_artifacts (
                     run_key BLOB PRIMARY KEY REFERENCES agent_work_runs(run_key),
                     profile_id TEXT NOT NULL REFERENCES profiles(id) ON DELETE CASCADE,
                     artifact_id BLOB NOT NULL UNIQUE CHECK (length(artifact_id) = 16),
                     digest BLOB NOT NULL CHECK (length(digest) = 32),
                     body BLOB NOT NULL CHECK (length(body) BETWEEN 1 AND 262144)
                 ) STRICT, WITHOUT ROWID;
                 CREATE TRIGGER agent_work_artifact_capacity BEFORE INSERT ON agent_work_artifacts
                 WHEN (SELECT coalesce(sum(length(body)), 0) FROM agent_work_artifacts) + length(NEW.body) > 33554432
                 BEGIN SELECT RAISE(ABORT, 'work result capacity exceeded'); END;
                 CREATE TRIGGER agent_work_artifact_immutable BEFORE UPDATE ON agent_work_artifacts
                 BEGIN SELECT RAISE(ABORT, 'work result is immutable'); END;",
            )
        },
    },
    Migration {
        version: 19,
        up: migrate_beta_native_source,
    },
    Migration {
        version: 20,
        up: migrate_local_external_native_source,
    },
    Migration {
        version: 21,
        up: |tx| {
            // The previous extension stack's native ownership records. Its
            // anchors live on tables that stay, so they go explicitly.
            tx.execute_batch(
                "DROP TRIGGER extension_native_namespace_profile_anchor_delete;
                 DROP TRIGGER extension_native_namespace_profile_anchor_update;
                 DROP TRIGGER extension_native_namespace_deletion_anchor_delete;
                 DROP TRIGGER extension_native_namespace_deletion_anchor_update;
                 DROP TRIGGER extension_native_namespace_proof_requires_absence;
                 DROP TABLE extension_native_namespace_obligations;
                 DROP TABLE extension_native_ownership_journal;
                 DROP TABLE extension_native_ownership_journal_state;",
            )
        },
    },
    Migration {
        version: 22,
        up: |tx| {
            tx.execute_batch(
                "CREATE TABLE profile_blocker_sites (
                    profile_id TEXT PRIMARY KEY REFERENCES profiles(id) ON DELETE CASCADE
                        CHECK (length(CAST(profile_id AS BLOB)) = 26),
                    revision INTEGER NOT NULL CHECK (revision BETWEEN 1 AND 9223372036854775807),
                    payload TEXT NOT NULL CHECK (length(CAST(payload AS BLOB)) BETWEEN 1 AND 2097152)
                 ) STRICT;
                 INSERT INTO profile_blocker_sites(profile_id, revision, payload)
                 SELECT id, 1, '{\"version\":1,\"revision\":1,\"next_hide_id\":1,\"paused\":[],\"hides\":[]}' FROM profiles;",
            )
        },
    },
    Migration {
        version: 23,
        up: |tx| {
            // One release transition, never a startup override. Subsequent
            // explicit opt-outs survive reopening. Do not reset revision or
            // change site pauses/personal hides. Exhaustion aborts atomically.
            tx.execute_batch(
                "UPDATE profile_blocker_settings
                 SET enabled = 1, revision = revision + 1
                 WHERE enabled = 0;",
            )
        },
    },
    Migration {
        version: 24,
        up: |tx| {
            // Focus belongs to the person, not to a profile. `day` is the
            // local day a session began, fixed when it is written.
            tx.execute_batch(
                "CREATE TABLE focus_sessions (
                     started_ms INTEGER PRIMARY KEY,
                     ended_ms INTEGER NOT NULL,
                     day INTEGER NOT NULL,
                     focused_ms INTEGER NOT NULL CHECK (focused_ms >= 0),
                     rounds INTEGER NOT NULL CHECK (rounds >= 0),
                     completed INTEGER NOT NULL CHECK (completed IN (0, 1))
                 ) STRICT;
                 CREATE INDEX idx_focus_sessions_day ON focus_sessions(day);",
            )
        },
    },
];

// These statements are the exact extension-branch PROFILE v14 artifact. The
// schema manifest uses their stored CREATE text to recognize existing files.
fn create_extension_profile_provenance(tx: &Transaction) -> rusqlite::Result<()> {
    tx.execute_batch(
        // No source authority is inferred for existing reviewed installs.
        // History deliberately has no install FK: uninstall is not a
        // rollback-protection reset. Profile deletion removes both tables.
        "CREATE TABLE extension_upstream_history (
                 publisher BLOB PRIMARY KEY CHECK (typeof(publisher) = 'blob' AND length(publisher) = 32),
                 checkpoint BLOB NOT NULL CHECK (typeof(checkpoint) = 'blob' AND length(checkpoint) = 105)
             ) STRICT, WITHOUT ROWID;
             CREATE TRIGGER extension_upstream_history_capacity BEFORE INSERT ON extension_upstream_history
             WHEN NOT EXISTS (SELECT 1 FROM extension_upstream_history WHERE publisher = NEW.publisher)
                  AND (SELECT count(*) FROM extension_upstream_history) >= 128
             BEGIN SELECT RAISE(ABORT, 'extension upstream history capacity exceeded'); END;
             CREATE TABLE extension_install_provenance (
                 install_id BLOB PRIMARY KEY REFERENCES extension_installs(id) ON DELETE CASCADE
                    CHECK (typeof(install_id) = 'blob' AND length(install_id) = 16),
                 provenance BLOB NOT NULL CHECK (typeof(provenance) = 'blob' AND length(provenance) BETWEEN 1 AND 1024)
             ) STRICT, WITHOUT ROWID;",
    )
}

fn migrate_extension_profile_provenance(tx: &Transaction) -> rusqlite::Result<()> {
    let already_created: i64 = tx.query_row(
        "SELECT count(*) FROM sqlite_schema WHERE name='extension_upstream_history'",
        [],
        |row| row.get(0),
    )?;
    if already_created == 0 {
        create_extension_profile_provenance(tx)?;
    }
    Ok(())
}

pub static PROFILE: &[Migration] = &[
    Migration {
        version: 1,
        up: |tx| {
            tx.execute_batch(
            "CREATE TABLE spaces (
                 id TEXT PRIMARY KEY,
                 name TEXT NOT NULL,
                 position INTEGER NOT NULL
             ) STRICT;
             CREATE TABLE items (
                 id TEXT PRIMARY KEY,
                 parent_id TEXT REFERENCES items(id) ON DELETE CASCADE,
                 space_id TEXT REFERENCES spaces(id) ON DELETE CASCADE,
                 section TEXT NOT NULL CHECK (section IN ('favorites', 'pinned', 'today')),
                 position INTEGER NOT NULL,
                 kind TEXT NOT NULL CHECK (kind IN ('folder', 'tab')),
                 name TEXT,
                 url TEXT,
                 title TEXT,
                 CHECK ((space_id IS NULL) = (section = 'favorites'))
             ) STRICT;
             CREATE INDEX idx_items_container ON items(space_id, section, position);
             CREATE TABLE focus (
                 id INTEGER PRIMARY KEY CHECK (id = 1),
                 active_space TEXT,
                 active_item TEXT,
                 splits TEXT
             ) STRICT;
             CREATE TABLE history (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 url TEXT NOT NULL,
                 title TEXT NOT NULL,
                 visited_at INTEGER NOT NULL
             ) STRICT;
             CREATE INDEX idx_history_visited_at ON history(visited_at);
             CREATE TABLE settings (
                 key TEXT PRIMARY KEY,
                 value TEXT NOT NULL
             ) STRICT;
             CREATE VIRTUAL TABLE history_fts USING fts5(url, title, content='history', content_rowid='id');
             CREATE TRIGGER history_ai AFTER INSERT ON history BEGIN
                 INSERT INTO history_fts(rowid, url, title) VALUES (new.id, new.url, new.title);
             END;
             CREATE TRIGGER history_ad AFTER DELETE ON history BEGIN
                 INSERT INTO history_fts(history_fts, rowid, url, title)
                 VALUES ('delete', old.id, old.url, old.title);
             END;
             CREATE TRIGGER history_au AFTER UPDATE ON history BEGIN
                 INSERT INTO history_fts(history_fts, rowid, url, title)
                 VALUES ('delete', old.id, old.url, old.title);
                 INSERT INTO history_fts(rowid, url, title) VALUES (new.id, new.url, new.title);
             END;",
            )
        },
    },
    Migration {
        version: 2,
        up: |tx| {
            tx.execute_batch(
                "CREATE TABLE favicons (
                     origin TEXT PRIMARY KEY,
                     content_type TEXT,
                     icon BLOB NOT NULL,
                     fetched_at INTEGER NOT NULL
                 ) STRICT;",
            )
        },
    },
    Migration {
        version: 3,
        up: |tx| tx.execute_batch("ALTER TABLE items ADD COLUMN zoom REAL NOT NULL DEFAULT 1;"),
    },
    Migration {
        version: 4,
        up: |tx| {
            tx.execute_batch(
                // Enforce quotas for databases created before write-time
                // pruning existed. Legacy session tables are cleared only
                // after an authoritative meta snapshot has committed.
                "DELETE FROM history WHERE id IN (
                     SELECT id FROM history
                     ORDER BY visited_at DESC, id DESC
                     LIMIT -1 OFFSET 50000
                 );
                 DELETE FROM favicons WHERE origin IN (
                     SELECT origin FROM favicons
                     ORDER BY fetched_at DESC, origin
                     LIMIT -1 OFFSET 512
                 );",
            )
        },
    },
    Migration {
        version: 5,
        up: |tx| {
            tx.execute_batch(
                // Bound individual legacy rows as well as table cardinality.
                // Runtime parsing performs the stronger URL/icon validation.
                "DELETE FROM history
                 WHERE length(CAST(url AS BLOB)) > 8192
                    OR length(CAST(title AS BLOB)) > 2048;
                 DELETE FROM favicons
                 WHERE length(CAST(origin AS BLOB)) > 8192
                    OR length(CAST(content_type AS BLOB)) > 128
                    OR length(icon) = 0
                    OR length(icon) > 262144;
                 DELETE FROM settings
                 WHERE length(CAST(key AS BLOB)) = 0
                    OR length(CAST(key AS BLOB)) > 256
                    OR length(CAST(value AS BLOB)) > 65536;",
            )
        },
    },
    Migration {
        version: 6,
        up: |tx| {
            tx.execute_batch(
                "CREATE TABLE history_usage (
                     id INTEGER PRIMARY KEY CHECK (id = 1),
                     bytes INTEGER NOT NULL CHECK (bytes >= 0)
                 ) STRICT;
                 INSERT INTO history_usage(id, bytes)
                 SELECT 1, COALESCE(SUM(
                     length(CAST(url AS BLOB)) + length(CAST(title AS BLOB))
                 ), 0)
                 FROM history;
                 CREATE TRIGGER history_usage_ai AFTER INSERT ON history BEGIN
                     UPDATE history_usage
                     SET bytes = bytes
                         + length(CAST(new.url AS BLOB))
                         + length(CAST(new.title AS BLOB))
                     WHERE id = 1;
                 END;
                 CREATE TRIGGER history_usage_ad AFTER DELETE ON history BEGIN
                     UPDATE history_usage
                     SET bytes = MAX(0, bytes
                         - length(CAST(old.url AS BLOB))
                         - length(CAST(old.title AS BLOB)))
                     WHERE id = 1;
                 END;
                 CREATE TRIGGER history_usage_au AFTER UPDATE OF url, title ON history BEGIN
                     UPDATE history_usage
                     SET bytes = MAX(0, bytes
                         - length(CAST(old.url AS BLOB))
                         - length(CAST(old.title AS BLOB))
                         + length(CAST(new.url AS BLOB))
                         + length(CAST(new.title AS BLOB)))
                     WHERE id = 1;
                 END;",
            )
        },
    },
    Migration {
        version: 7,
        up: |tx| {
            tx.execute_batch(
                // Source is the only durable metadata authority. Names,
                // matches, grants and compatibility status are derived by
                // the bounded core parser on every load, avoiding a second
                // representation that could drift across runtime upgrades.
                "CREATE TABLE userscript_catalog (
                     id INTEGER PRIMARY KEY CHECK (id = 1),
                     revision INTEGER NOT NULL
                         CHECK (revision BETWEEN 1 AND 9223372036854775807)
                 ) STRICT;
                 INSERT INTO userscript_catalog(id, revision) VALUES (1, 1);
                 CREATE TABLE userscripts (
                     id TEXT PRIMARY KEY
                         CHECK (length(CAST(id AS BLOB)) = 26),
                     revision INTEGER NOT NULL
                         CHECK (revision BETWEEN 1 AND 9223372036854775807),
                     enabled INTEGER NOT NULL
                         CHECK (enabled IN (0, 1)),
                     metadata_format INTEGER NOT NULL
                         CHECK (metadata_format BETWEEN 1 AND 2147483647),
                     source TEXT NOT NULL
                         CHECK (length(CAST(source AS BLOB)) BETWEEN 1 AND 2097152
                                AND instr(CAST(source AS BLOB), X'00') = 0),
                     -- SHA-256(\"zephium-userscript-source-v1\" || UTF-8 source).
                     -- The versioned name prevents a future digest change
                     -- from silently reinterpreting durable bytes.
                     source_sha256_v1 BLOB NOT NULL
                         CHECK (length(source_sha256_v1) = 32)
                 ) STRICT;",
            )
        },
    },
    Migration {
        version: 8,
        up: |tx| {
            tx.execute_batch(
                // Absence is Ask. Remembered page permissions are an exact
                // per-profile authority domain and intentionally share no
                // table with extension API, host, temporary, or scheme grants.
                "CREATE TABLE page_permission_catalog (
                     id INTEGER PRIMARY KEY CHECK (id = 1),
                     revision INTEGER NOT NULL
                         CHECK (revision BETWEEN 1 AND 9223372036854775807)
                 ) STRICT;
                 INSERT INTO page_permission_catalog(id, revision) VALUES (1, 1);
                 CREATE TABLE page_permission_grants (
                     id TEXT PRIMARY KEY
                         CHECK (length(CAST(id AS BLOB)) = 26),
                     revision INTEGER NOT NULL
                         CHECK (revision BETWEEN 1 AND 9223372036854775807),
                     origin TEXT NOT NULL
                         CHECK (length(CAST(origin AS BLOB)) BETWEEN 1 AND 512
                                AND instr(CAST(origin AS BLOB), X'00') = 0),
                     kind TEXT NOT NULL
                         CHECK (kind IN (
                             'geolocation', 'camera', 'microphone',
                             'notifications', 'clipboard_read'
                         )),
                     decision TEXT NOT NULL
                         CHECK (decision IN ('allow', 'deny')),
                     UNIQUE(origin, kind)
                 ) STRICT;",
            )
        },
    },
    Migration {
        version: 9,
        up: |tx| {
            tx.execute_batch(
                // This catalog records per-profile installation intent only.
                // Every package field is an exact structural identity value;
                // neither presence here nor desired_enabled authenticates
                // package bytes, grants permissions, or proves activation.
                "CREATE TABLE extension_install_catalog (
                     id INTEGER PRIMARY KEY CHECK (id = 1),
                     revision INTEGER NOT NULL
                         CHECK (revision BETWEEN 1 AND 9223372036854775807)
                 ) STRICT;
                 INSERT INTO extension_install_catalog(id, revision) VALUES (1, 1);
                 CREATE TABLE extension_installs (
                     id BLOB PRIMARY KEY
                         CHECK (typeof(id) = 'blob' AND length(id) = 16),
                     revision INTEGER NOT NULL
                         CHECK (revision BETWEEN 1 AND 9223372036854775807),
                     authority BLOB NOT NULL
                         CHECK (typeof(authority) = 'blob' AND length(authority) = 32),
                     package_key BLOB NOT NULL
                         CHECK (typeof(package_key) = 'blob' AND length(package_key) = 32),
                     package_revision INTEGER NOT NULL
                         CHECK (package_revision BETWEEN 1 AND 9223372036854775807),
                     archive_sha256 BLOB NOT NULL
                         CHECK (typeof(archive_sha256) = 'blob' AND length(archive_sha256) = 32),
                     manifest_sha256 BLOB NOT NULL
                         CHECK (typeof(manifest_sha256) = 'blob' AND length(manifest_sha256) = 32),
                     tree_sha256 BLOB NOT NULL
                         CHECK (typeof(tree_sha256) = 'blob' AND length(tree_sha256) = 32),
                     desired_enabled INTEGER NOT NULL
                         CHECK (desired_enabled IN (0, 1)),
                     UNIQUE(authority, package_key)
                 ) STRICT;",
            )
        },
    },
    Migration {
        version: 10,
        up: |tx| {
            tx.execute_batch(
                // Grant authority is subordinate to one exact install and is
                // deliberately not a profile catalog. Absence means
                // uninitialized/deny, so this migration must not manufacture
                // rows for existing installs. The duplicated package identity
                // and digest are revalidated by the bounded durable codec.
                "CREATE TABLE extension_grants (
                     install_id BLOB PRIMARY KEY
                         REFERENCES extension_installs(id) ON DELETE CASCADE
                         CHECK (typeof(install_id) = 'blob' AND length(install_id) = 16),
                     revision INTEGER NOT NULL
                         CHECK (revision BETWEEN 1 AND 9223372036854775807),
                     authority BLOB NOT NULL
                         CHECK (typeof(authority) = 'blob' AND length(authority) = 32),
                     package_key BLOB NOT NULL
                         CHECK (typeof(package_key) = 'blob' AND length(package_key) = 32),
                     package_revision INTEGER NOT NULL
                         CHECK (package_revision BETWEEN 1 AND 9223372036854775807),
                     archive_sha256 BLOB NOT NULL
                         CHECK (typeof(archive_sha256) = 'blob' AND length(archive_sha256) = 32),
                     manifest_sha256 BLOB NOT NULL
                         CHECK (typeof(manifest_sha256) = 'blob' AND length(manifest_sha256) = 32),
                     tree_sha256 BLOB NOT NULL
                         CHECK (typeof(tree_sha256) = 'blob' AND length(tree_sha256) = 32),
                     grant_sha256 BLOB NOT NULL
                         CHECK (typeof(grant_sha256) = 'blob' AND length(grant_sha256) = 32),
                     file_access INTEGER NOT NULL CHECK (file_access IN (0, 1)),
                     private_access INTEGER NOT NULL CHECK (private_access IN (0, 1))
                 ) STRICT;
                 CREATE TABLE extension_grant_api_permissions (
                     install_id BLOB NOT NULL
                         REFERENCES extension_grants(install_id) ON DELETE CASCADE
                         CHECK (typeof(install_id) = 'blob' AND length(install_id) = 16),
                     name TEXT NOT NULL
                         CHECK (length(CAST(name AS BLOB)) BETWEEN 1 AND 96
                                AND instr(CAST(name AS BLOB), X'00') = 0),
                     PRIMARY KEY (install_id, name)
                 ) STRICT, WITHOUT ROWID;
                 CREATE TABLE extension_grant_host_permissions (
                     install_id BLOB NOT NULL
                         REFERENCES extension_grants(install_id) ON DELETE CASCADE
                         CHECK (typeof(install_id) = 'blob' AND length(install_id) = 16),
                     pattern TEXT NOT NULL
                         CHECK (length(CAST(pattern AS BLOB)) BETWEEN 1 AND 2048
                                AND instr(CAST(pattern AS BLOB), X'00') = 0),
                     PRIMARY KEY (install_id, pattern)
                 ) STRICT, WITHOUT ROWID;",
            )
        },
    },
    Migration {
        version: 11,
        up: |tx| {
            tx.execute_batch(
                // A single monotonic identity floor prevents identities
                // deleted after this migration from ever being reused without
                // retaining an unbounded tombstone set. Profile schemas 9 and
                // 10 predate every native-extension reconciliation journal,
                // so they cannot leave durable native work for an already
                // deleted row. Big-endian ULID bytes preserve numeric order,
                // making max(id) the exact recoverable upgrade floor.
                "ALTER TABLE extension_install_catalog
                 ADD COLUMN install_id_high_water BLOB
                     CHECK (
                         install_id_high_water IS NULL OR (
                             typeof(install_id_high_water) = 'blob'
                             AND length(install_id_high_water) = 16
                         )
                     );
                 UPDATE extension_install_catalog
                 SET install_id_high_water = (
                     SELECT max(id) FROM extension_installs
                 )
                 WHERE id = 1;",
            )
        },
    },
    Migration {
        version: 12,
        up: |tx| {
            tx.execute_batch(
                // Versions 9-11 were an unreleased structural foundation and
                // stored only an archive digest. They cannot truthfully be
                // upgraded to exact {length, digest} ZIP evidence, nor can an
                // archive digest represent a bundled release tree. Preserve
                // the catalog clock and monotonic install-id floor, but
                // invalidate every legacy install and subordinate grant. A
                // package must be reinstalled through the exact v12 authority
                // before it can activate.
                "UPDATE extension_install_catalog
                 SET install_id_high_water = CASE
                     WHEN (SELECT max(id) FROM extension_installs) IS NULL
                         THEN install_id_high_water
                     WHEN install_id_high_water IS NULL
                          OR install_id_high_water < (SELECT max(id) FROM extension_installs)
                         THEN (SELECT max(id) FROM extension_installs)
                     ELSE install_id_high_water
                 END
                 WHERE id = 1;

                 DELETE FROM extension_grant_api_permissions;
                 DELETE FROM extension_grant_host_permissions;
                 DELETE FROM extension_grants;
                 DELETE FROM extension_installs;

                 DROP TABLE extension_grant_api_permissions;
                 DROP TABLE extension_grant_host_permissions;
                 DROP TABLE extension_grants;
                 DROP TABLE extension_installs;

                 CREATE TABLE extension_installs (
                     id BLOB PRIMARY KEY
                         CHECK (typeof(id) = 'blob' AND length(id) = 16),
                     revision INTEGER NOT NULL
                         CHECK (revision BETWEEN 1 AND 9223372036854775807),
                     authority BLOB NOT NULL
                         CHECK (typeof(authority) = 'blob' AND length(authority) = 32),
                     package_key BLOB NOT NULL
                         CHECK (typeof(package_key) = 'blob' AND length(package_key) = 32),
                     package_revision INTEGER NOT NULL
                         CHECK (package_revision BETWEEN 1 AND 9223372036854775807),
                     payload_kind INTEGER NOT NULL
                         CHECK (payload_kind IN (1, 2)),
                     archive_length INTEGER,
                     archive_sha256 BLOB,
                     manifest_sha256 BLOB NOT NULL
                         CHECK (typeof(manifest_sha256) = 'blob' AND length(manifest_sha256) = 32),
                     tree_sha256 BLOB NOT NULL
                         CHECK (typeof(tree_sha256) = 'blob' AND length(tree_sha256) = 32),
                     desired_enabled INTEGER NOT NULL
                         CHECK (desired_enabled IN (0, 1)),
                     CHECK (
                         (payload_kind = 1
                          AND archive_length IS NULL
                          AND archive_sha256 IS NULL)
                         OR
                         (payload_kind = 2
                          AND typeof(archive_length) = 'integer'
                          AND archive_length BETWEEN 1 AND 67108864
                          AND typeof(archive_sha256) = 'blob'
                          AND length(archive_sha256) = 32)
                     ),
                     UNIQUE(authority, package_key)
                 ) STRICT;

                 CREATE TABLE extension_grants (
                     install_id BLOB PRIMARY KEY
                         REFERENCES extension_installs(id) ON DELETE CASCADE
                         CHECK (typeof(install_id) = 'blob' AND length(install_id) = 16),
                     revision INTEGER NOT NULL
                         CHECK (revision BETWEEN 1 AND 9223372036854775807),
                     authority BLOB NOT NULL
                         CHECK (typeof(authority) = 'blob' AND length(authority) = 32),
                     package_key BLOB NOT NULL
                         CHECK (typeof(package_key) = 'blob' AND length(package_key) = 32),
                     package_revision INTEGER NOT NULL
                         CHECK (package_revision BETWEEN 1 AND 9223372036854775807),
                     payload_kind INTEGER NOT NULL
                         CHECK (payload_kind IN (1, 2)),
                     archive_length INTEGER,
                     archive_sha256 BLOB,
                     manifest_sha256 BLOB NOT NULL
                         CHECK (typeof(manifest_sha256) = 'blob' AND length(manifest_sha256) = 32),
                     tree_sha256 BLOB NOT NULL
                         CHECK (typeof(tree_sha256) = 'blob' AND length(tree_sha256) = 32),
                     grant_sha256 BLOB NOT NULL
                         CHECK (typeof(grant_sha256) = 'blob' AND length(grant_sha256) = 32),
                     file_access INTEGER NOT NULL CHECK (file_access IN (0, 1)),
                     private_access INTEGER NOT NULL CHECK (private_access IN (0, 1)),
                     CHECK (
                         (payload_kind = 1
                          AND archive_length IS NULL
                          AND archive_sha256 IS NULL)
                         OR
                         (payload_kind = 2
                          AND typeof(archive_length) = 'integer'
                          AND archive_length BETWEEN 1 AND 67108864
                          AND typeof(archive_sha256) = 'blob'
                          AND length(archive_sha256) = 32)
                     )
                 ) STRICT;
                 CREATE TABLE extension_grant_api_permissions (
                     install_id BLOB NOT NULL
                         REFERENCES extension_grants(install_id) ON DELETE CASCADE
                         CHECK (typeof(install_id) = 'blob' AND length(install_id) = 16),
                     name TEXT NOT NULL
                         CHECK (length(CAST(name AS BLOB)) BETWEEN 1 AND 96
                                AND instr(CAST(name AS BLOB), X'00') = 0),
                     PRIMARY KEY (install_id, name)
                 ) STRICT, WITHOUT ROWID;
                 CREATE TABLE extension_grant_host_permissions (
                     install_id BLOB NOT NULL
                         REFERENCES extension_grants(install_id) ON DELETE CASCADE
                         CHECK (typeof(install_id) = 'blob' AND length(install_id) = 16),
                     pattern TEXT NOT NULL
                         CHECK (length(CAST(pattern AS BLOB)) BETWEEN 1 AND 2048
                                AND instr(CAST(pattern AS BLOB), X'00') = 0),
                     PRIMARY KEY (install_id, pattern)
                 ) STRICT, WITHOUT ROWID;",
            )
        },
    },
    Migration {
        version: 13,
        up: |tx| {
            tx.execute_batch(
                // Profile-wide extension pause/site policy is a separate
                // authority domain from per-install grants. The singleton
                // revision orders complete replacement, while child rows are
                // canonical exact web-host match patterns.
                "CREATE TABLE extension_profile_policy (
                     id INTEGER PRIMARY KEY CHECK (id = 1),
                     revision INTEGER NOT NULL
                         CHECK (revision BETWEEN 1 AND 9223372036854775807),
                     paused INTEGER NOT NULL CHECK (paused IN (0, 1))
                 ) STRICT;
                 INSERT INTO extension_profile_policy(id, revision, paused)
                 VALUES (1, 1, 0);
                 CREATE TABLE extension_profile_site_denials (
                     policy_id INTEGER NOT NULL
                         REFERENCES extension_profile_policy(id) ON DELETE CASCADE
                         CHECK (policy_id = 1),
                     pattern TEXT NOT NULL
                         CHECK (length(CAST(pattern AS BLOB)) BETWEEN 1 AND 2048
                                AND instr(CAST(pattern AS BLOB), X'00') = 0),
                     PRIMARY KEY (policy_id, pattern)
                 ) STRICT, WITHOUT ROWID;
                 CREATE TRIGGER extension_profile_site_denials_capacity
                 BEFORE INSERT ON extension_profile_site_denials
                 WHEN (SELECT count(*) FROM extension_profile_site_denials) >= 128
                 BEGIN
                     SELECT RAISE(ABORT, 'extension site-denial capacity exceeded');
                 END;",
            )
        },
    },
    Migration {
        version: 14,
        up: |tx| {
            tx.execute_batch(
            "CREATE TABLE user_resources (
                id TEXT PRIMARY KEY NOT NULL CHECK(length(id)=26),
                kind TEXT NOT NULL CHECK(kind IN ('note','task')),
                revision INTEGER NOT NULL CHECK(revision>0),
                title TEXT NOT NULL,
                completed INTEGER CHECK(completed IS NULL OR completed IN (0,1)),
                due_date TEXT,
                pinned INTEGER NOT NULL CHECK(pinned IN (0,1)),
                trashed INTEGER NOT NULL CHECK(trashed IN (0,1)),
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                body TEXT NOT NULL CHECK(length(CAST(body AS BLOB))<=524288),
                search_text TEXT NOT NULL
            ) STRICT;
            CREATE TABLE user_resource_usage (id INTEGER PRIMARY KEY CHECK(id=1),bytes INTEGER NOT NULL CHECK(bytes>=0)) STRICT;
            INSERT INTO user_resource_usage VALUES(1,0);
            CREATE TRIGGER user_resource_usage_insert AFTER INSERT ON user_resources BEGIN
                UPDATE user_resource_usage SET bytes=bytes+length(CAST(NEW.body AS BLOB)) WHERE id=1; END;
            CREATE TRIGGER user_resource_usage_update AFTER UPDATE OF body ON user_resources BEGIN
                UPDATE user_resource_usage SET bytes=bytes-length(CAST(OLD.body AS BLOB))+length(CAST(NEW.body AS BLOB)) WHERE id=1; END;
            CREATE TRIGGER user_resource_usage_delete AFTER DELETE ON user_resources BEGIN
                UPDATE user_resource_usage SET bytes=bytes-length(CAST(OLD.body AS BLOB)) WHERE id=1; END;
            CREATE INDEX user_resources_listing ON user_resources(kind,trashed,pinned DESC,id DESC);
            CREATE TABLE user_resource_receipts (
                request_id TEXT PRIMARY KEY NOT NULL,
                digest BLOB NOT NULL CHECK(length(digest)=32),
                retained INTEGER NOT NULL CHECK(retained IN (0,1)),
                resource_id TEXT NOT NULL REFERENCES user_resources(id),
                revision INTEGER NOT NULL CHECK(revision>0)
            ) STRICT;
            CREATE TRIGGER user_resources_capacity BEFORE INSERT ON user_resources
            WHEN (SELECT count(*) FROM user_resources)>=10000
            BEGIN SELECT RAISE(ABORT,'resource capacity'); END;
            CREATE TRIGGER user_resource_receipts_capacity BEFORE INSERT ON user_resource_receipts
            WHEN (SELECT count(*) FROM user_resource_receipts)>=100000
            BEGIN SELECT RAISE(ABORT,'resource receipt capacity'); END;"
        )
        },
    },
    Migration {
        version: 15,
        up: |tx| {
            // Titles only. Note and task bodies are deliberately outside the
            // index so search never reaches document contents.
            //
            // SQLite stores a CREATE statement verbatim in sqlite_schema, and
            // the migration manifest compares that text. Reformatting any
            // statement below would fail the manifest on every database that
            // already applied this version, degrading it permanently. Treat
            // this SQL as a released artifact, not as source to tidy.
            tx.execute_batch(
                "CREATE VIRTUAL TABLE resource_titles_fts USING fts5(title, content='user_resources', content_rowid='rowid', prefix='2 3');
                 INSERT INTO resource_titles_fts(resource_titles_fts) VALUES ('rebuild');
                 INSERT INTO resource_titles_fts(resource_titles_fts, rank) VALUES ('secure-delete', 1);
                 CREATE TRIGGER resource_titles_insert AFTER INSERT ON user_resources BEGIN INSERT INTO resource_titles_fts(rowid,title) VALUES(NEW.rowid,NEW.title); END;
                 CREATE TRIGGER resource_titles_delete AFTER DELETE ON user_resources BEGIN INSERT INTO resource_titles_fts(resource_titles_fts,rowid,title) VALUES('delete',OLD.rowid,OLD.title); END;
                 CREATE TRIGGER resource_titles_update AFTER UPDATE OF title ON user_resources WHEN OLD.title IS NOT NEW.title BEGIN
        INSERT INTO resource_titles_fts(resource_titles_fts,rowid,title) VALUES('delete',OLD.rowid,OLD.title);
        INSERT INTO resource_titles_fts(rowid,title) VALUES(NEW.rowid,NEW.title); END;
                 CREATE TABLE search_queries (query_key TEXT PRIMARY KEY NOT NULL, query TEXT NOT NULL CHECK(length(CAST(query AS BLOB))<=512), url TEXT NOT NULL CHECK(length(CAST(url AS BLOB))<=8192), last_used INTEGER NOT NULL, use_count INTEGER NOT NULL CHECK(use_count>0)) STRICT;",
            )
        },
    },
    Migration {
        version: 16,
        up: |tx| {
            // A task list row draws status, who holds it, where it came from
            // and its manual position. Projecting those out of the JSON body
            // keeps a populated list one query instead of one query plus a
            // fetch per row. The body stays authoritative; these are an index.
            //
            // Existing tasks predate the lifecycle, so they adopt the state
            // their stored `completed` already implies, in the body as well as
            // the columns: a task whose two representations of done-ness
            // disagreed would no longer validate, and so would stop loading.
            // A body that is not JSON was already unreadable and is left alone
            // rather than aborting every other profile's migration.
            //
            // `work` is reserved for the Work runtime track and carries no
            // reference constraint here.
            tx.execute_batch(
                "ALTER TABLE user_resources ADD COLUMN status TEXT;
                 ALTER TABLE user_resources ADD COLUMN assignee TEXT;
                 ALTER TABLE user_resources ADD COLUMN origin TEXT;
                 ALTER TABLE user_resources ADD COLUMN context_url TEXT;
                 ALTER TABLE user_resources ADD COLUMN context_title TEXT;
                 ALTER TABLE user_resources ADD COLUMN sort_key TEXT;
                 ALTER TABLE user_resources ADD COLUMN work TEXT;
                 UPDATE user_resources SET status=CASE WHEN completed=1 THEN 'done' ELSE 'open' END, assignee='user', origin='user' WHERE kind='task';
                 UPDATE user_resources SET body=json_set(body,'$.content.status',CASE WHEN completed=1 THEN 'done' ELSE 'open' END,'$.content.assignee','user','$.content.origin','user') WHERE kind='task' AND json_valid(body);",
            )
        },
    },
    Migration {
        version: 17,
        up: |tx| {
            // A due time is optional and only ever set alongside a day, so there
            // is nothing to backfill: every existing task simply has none.
            tx.execute_batch("ALTER TABLE user_resources ADD COLUMN due_time TEXT;")
        },
    },
    Migration {
        version: 18,
        up: |tx| {
            tx.execute_batch(
            "CREATE TABLE task_lists(id TEXT PRIMARY KEY NOT NULL CHECK(length(id)=26),title TEXT NOT NULL,revision INTEGER NOT NULL CHECK(revision>0),deleted INTEGER NOT NULL DEFAULT 0 CHECK(deleted IN (0,1))) STRICT;
             CREATE TABLE task_list_receipts(request_id TEXT PRIMARY KEY NOT NULL,digest BLOB NOT NULL CHECK(length(digest)=32),list_id TEXT NOT NULL REFERENCES task_lists(id),retained INTEGER NOT NULL CHECK(retained IN (0,1))) STRICT;
             ALTER TABLE user_resources ADD COLUMN task_list TEXT;
             ALTER TABLE user_resources ADD COLUMN task_inbox INTEGER NOT NULL DEFAULT 0 CHECK(task_inbox IN (0,1));
             ALTER TABLE user_resources ADD COLUMN task_priority TEXT NOT NULL DEFAULT 'none';
             ALTER TABLE user_resources ADD COLUMN task_steps INTEGER NOT NULL DEFAULT 0;
             ALTER TABLE user_resources ADD COLUMN task_steps_done INTEGER NOT NULL DEFAULT 0;
             ALTER TABLE user_resources ADD COLUMN task_completed_at TEXT;
             CREATE INDEX user_tasks_list ON user_resources(task_list,trashed,completed);
             CREATE INDEX user_tasks_date ON user_resources(kind,trashed,completed,due_date);")
        },
    },
    Migration {
        version: 19,
        up: |tx| {
            // Deadline and estimate are new and optional; no task has either yet.
            tx.execute_batch(
                "ALTER TABLE user_resources ADD COLUMN task_deadline TEXT;
                 ALTER TABLE user_resources ADD COLUMN task_duration INTEGER;
                 CREATE INDEX user_tasks_deadline ON user_resources(kind,trashed,completed,task_deadline);",
            )
        },
    },
    Migration {
        version: 20,
        up: |tx| {
            tx.execute_batch(
            "CREATE TABLE downloads (id TEXT PRIMARY KEY NOT NULL CHECK(length(id)=26), session TEXT NOT NULL CHECK(length(session)=26), revision INTEGER NOT NULL CHECK(revision>0), terminal INTEGER NOT NULL CHECK(terminal IN (0,1)), payload TEXT NOT NULL CHECK(length(CAST(payload AS BLOB))<=24576 AND json_valid(payload))) STRICT;
             CREATE TABLE download_preferences (id INTEGER PRIMARY KEY CHECK(id=1), payload TEXT NOT NULL CHECK(length(CAST(payload AS BLOB))<=8192 AND json_valid(payload))) STRICT;
             CREATE TRIGGER downloads_capacity BEFORE INSERT ON downloads WHEN NOT EXISTS(SELECT 1 FROM downloads WHERE id=NEW.id) AND (SELECT count(*) FROM downloads)>=10000 BEGIN SELECT RAISE(ABORT,'download history capacity'); END;"
        )
        },
    },
    Migration {
        version: 21,
        up: |tx| {
            tx.execute_batch(
            "CREATE TABLE download_cleanup (id TEXT PRIMARY KEY NOT NULL CHECK(length(id)=26), session TEXT NOT NULL CHECK(length(session)=26), terminal INTEGER NOT NULL CHECK(terminal IN (0,1)), payload TEXT NOT NULL CHECK(length(CAST(payload AS BLOB))<=24576 AND json_valid(payload))) STRICT;
             INSERT INTO download_cleanup(id,session,terminal,payload) SELECT id,session,terminal,payload FROM downloads WHERE json_type(payload,'$.staging')='text' AND json_type(payload,'$.staging_identity')='object';
             CREATE TRIGGER download_cleanup_capacity BEFORE INSERT ON download_cleanup WHEN NOT EXISTS(SELECT 1 FROM download_cleanup WHERE id=NEW.id) AND (SELECT count(*) FROM download_cleanup)>=10000 BEGIN SELECT RAISE(ABORT,'download cleanup capacity'); END;"
        )
        },
    },
    Migration {
        version: 22,
        up: migrate_extension_profile_provenance,
    },
    Migration {
        version: 23,
        up: |tx| {
            // The previous extension stack's installs and grants; the current
            // runtime keeps its registry outside the database.
            tx.execute_batch(
                "DROP TABLE extension_install_provenance;
                 DROP TABLE extension_upstream_history;
                 DROP TABLE extension_grant_api_permissions;
                 DROP TABLE extension_grant_host_permissions;
                 DROP TABLE extension_grants;
                 DROP TABLE extension_profile_site_denials;
                 DROP TABLE extension_profile_policy;
                 DROP TABLE extension_installs;
                 DROP TABLE extension_install_catalog;",
            )
        },
    },
    Migration {
        version: 24,
        up: |tx| {
            tx.execute_batch(
                "CREATE TABLE blocker_statistics (
                id INTEGER PRIMARY KEY CHECK(id = 1),
                payload TEXT NOT NULL CHECK(length(CAST(payload AS BLOB)) BETWEEN 1 AND 512)
             ) STRICT;",
            )
        },
    },
    // Work lineage. Appended after the release lineage's 14-21.
    Migration {
        version: 25,
        up: |tx| tx.execute_batch(include_str!("work_schema_v1.sql")),
    },
    Migration {
        version: 26,
        up: crate::work_migration_v2::migrate,
    },
    Migration {
        version: 27,
        up: |tx| tx.execute_batch(include_str!("work_runtime_schema_v1.sql")),
    },
    Migration {
        version: 28,
        up: |tx| tx.execute_batch(include_str!("work_authoring_commands_v1.sql")),
    },
    Migration {
        version: 29,
        up: |tx| tx.execute_batch(include_str!("work_environment_schema_v1.sql")),
    },
    Migration {
        version: 30,
        up: |tx| tx.execute_batch(include_str!("work_environment_checkpoints_v1.sql")),
    },
    Migration {
        version: 31,
        up: |tx| tx.execute_batch(include_str!("user_resources_v28.sql")),
    },
    Migration {
        version: 32,
        up: |tx| tx.execute_batch(include_str!("work_personal_v29.sql")),
    },
    Migration {
        version: 33,
        up: |tx| {
            // Bookmarks, apart from the sidebar's live tabs. Bounds mirror
            // zephium_core::bookmarks. Count and depth are enforced by the
            // writes: a counting trigger would make an import quadratic.
            tx.execute_batch(
                "CREATE TABLE bookmarks (
                     id INTEGER PRIMARY KEY,
                     parent_id INTEGER REFERENCES bookmarks(id) ON DELETE CASCADE,
                     position INTEGER NOT NULL,
                     title TEXT NOT NULL CHECK (length(CAST(title AS BLOB)) <= 512),
                     url TEXT CHECK (url IS NULL OR length(CAST(url AS BLOB)) <= 8192),
                     added_at INTEGER NOT NULL
                 ) STRICT;
                 CREATE INDEX idx_bookmarks_parent ON bookmarks(parent_id, position);
                 CREATE INDEX idx_bookmarks_url ON bookmarks(url) WHERE url IS NOT NULL;",
            )
        },
    },
    Migration {
        version: 34,
        up: |tx| {
            // Time per local hour and place. A place is a registrable domain,
            // or '' for Work; a domain always holds a dot, so they never meet.
            tx.execute_batch(
                "CREATE TABLE time_spent (
                     hour INTEGER NOT NULL,
                     place TEXT NOT NULL CHECK (length(CAST(place AS BLOB)) <= 253),
                     spent_ms INTEGER NOT NULL CHECK (spent_ms >= 0),
                     opens INTEGER NOT NULL CHECK (opens >= 0),
                     PRIMARY KEY (hour, place)
                 ) STRICT, WITHOUT ROWID;",
            )
        },
    },
    Migration {
        version: 35,
        up: |tx| {
            tx.execute_batch(
                "CREATE TABLE time_batch_receipts (
                     sequence INTEGER PRIMARY KEY AUTOINCREMENT,
                     batch_id BLOB NOT NULL UNIQUE CHECK (typeof(batch_id) = 'blob' AND length(batch_id) = 16),
                     digest BLOB NOT NULL CHECK (typeof(digest) = 'blob' AND length(digest) = 32)
                 ) STRICT;
                 CREATE TRIGGER time_batch_receipt_retention AFTER INSERT ON time_batch_receipts
                 BEGIN
                     DELETE FROM time_batch_receipts WHERE sequence IN (
                         SELECT sequence FROM time_batch_receipts
                         ORDER BY sequence DESC LIMIT -1 OFFSET 128
                     );
                 END;",
            )
        },
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    /// Beta authority bytes as the previous extension stack derived them, so
    /// its shipped v19/v20 constraints stay exercised.
    fn beta_authority(channel: &str, target: &str) -> [u8; 32] {
        use sha2::{Digest, Sha256};
        let mut hash = Sha256::new();
        hash.update(b"zephium:beta-source-authority:v1\0");
        hash.update(channel.as_bytes());
        hash.update(b"\0");
        hash.update(target.as_bytes());
        hash.finalize().into()
    }

    /// Fingerprint of the schema each shipped version produces.
    ///
    /// SQLite stores a CREATE statement verbatim, and `validate_current`
    /// compares that text, so editing an already-released migration -- even
    /// its whitespace -- fails the manifest on every database that applied the
    /// old text and degrades it permanently. These constants exist so that
    /// mistake breaks this test instead of a user's profile.
    ///
    /// A new migration appends one line. An existing line never changes.
    const META_SCHEMA_FINGERPRINTS: &[(i64, u64)] = &[
        (1, 0xa62c_2e08_66e8_0dd2),
        (2, 0x4e66_f2fb_22b8_70d5),
        (3, 0x2910_38e3_a32e_7be3),
        (4, 0x2910_38e3_a32e_7be3),
        (5, 0x0284_7c35_b4c0_e297),
        (6, 0xac6c_b5d7_b66c_6da2),
        (7, 0x0816_09d3_99f0_7c45),
        (8, 0xea74_9bb2_69f4_5506),
        (9, 0x000a_33fa_6b52_c5f2),
        (10, 0xe187_684a_35da_7d8e),
        (11, 0xfe6f_079b_1c21_7cde),
        (12, 0x7367_0d0b_3f1f_9c96),
        (13, 0xbf7c_627a_b150_22ae),
        (14, 0x3a07_c3c5_e4cf_25fc),
        (15, 0x8c12_efd7_c942_f404),
        (16, 0x2dc6_d332_0299_d29b),
        (17, 0xc06b_3cdd_2a6e_9fc6),
        (18, 0xf8c3_1606_d301_f467),
        (19, 0xb30f_9566_ea44_65bf),
        (20, 0xeb32_555e_6d72_1ded),
        (21, 0xfe22_09ee_a55b_a3f5),
        (22, 0x6a156db401738842),
        (23, 0x6a156db401738842),
        (24, 0xb38257a2cbe5bad8),
    ];
    const PROFILE_SCHEMA_FINGERPRINTS: &[(i64, u64)] = &[
        (1, 0x10b8_b7a3_094f_23d7),
        (2, 0xd36a_6ccd_26cd_8ab3),
        (3, 0xfa38_77ec_ded0_391e),
        (4, 0xfa38_77ec_ded0_391e),
        (5, 0xfa38_77ec_ded0_391e),
        (6, 0x6133_6034_2097_64ef),
        (7, 0xc545_b0e1_65b2_9c87),
        (8, 0x441d_75af_3edb_d1fb),
        (9, 0x52da_c9e7_ce8a_4d9b),
        (10, 0x163b_4df0_050f_285e),
        (11, 0x1831_eb77_6cbd_8ea1),
        (12, 0x0d0b_cc00_2fd0_8fc9),
        (13, 0x128f_f07d_8b37_6bc9),
        (14, 0x0bb9_e89a_39c2_fb9f),
        (15, 0x2f78_2c71_a646_acaf),
        (16, 0x119a_472e_4564_25df),
        (17, 0x8b55_d4bb_445e_7756),
        (18, 0xa412_5523_e2ac_aef7),
        (19, 0x321a_2e79_d8d2_77da),
        (20, 0x4b37_b9cf_91e8_b507),
        (21, 0x3483_1796_92c9_33a6),
        (22, 0xd5d5_eec8_ddc1_ca18),
        (23, 0xfa14_edb0_3a39_f586),
        (24, 0x7f905292c3b461b8),
        (25, 0x030c3ec9027a4538),
        (26, 0xb9c104121d4d71db),
        (27, 0x6289d36e7af61775),
        (28, 0xa326a261b19a6901),
        (29, 0xf08b7bd4e9b17709),
        (30, 0x6f827893a13c3ccb),
        (31, 0x55b58f4944aa33b6),
        (32, 0x6323d1c7efd84e2c),
        (33, 0x101002bc7ceb482a),
        (34, 0x1a93a832df64a4a1),
        (35, 0x3c4c67becd54e02a),
    ];

    #[test]
    fn upgrades_drop_every_previous_extension_object_and_keep_user_data() {
        let extension_objects = |conn: &Connection| -> i64 {
            conn.query_row(
                "SELECT count(*) FROM sqlite_schema WHERE name LIKE '%extension%'",
                [],
                |row| row.get(0),
            )
            .unwrap()
        };

        let mut meta = Connection::open_in_memory().unwrap();
        apply(&mut meta, &META[..20]).unwrap();
        meta.execute(
            "INSERT INTO profiles(id, name, kind, position)
             VALUES ('01J00000000000000000000000', 'Profile', 'default', 0)",
            [],
        )
        .unwrap();
        insert_native_ownership_test_row(&meta, 1, 1, 1, "acquire", "native_absent_preparing")
            .unwrap();
        assert!(extension_objects(&meta) > 0);
        apply(&mut meta, META).unwrap();
        assert_eq!(extension_objects(&meta), 0);
        let profiles: i64 = meta
            .query_row("SELECT count(*) FROM profiles", [], |row| row.get(0))
            .unwrap();
        assert_eq!(profiles, 1);
        meta.execute("DELETE FROM profiles", []).unwrap();

        let mut profile = Connection::open_in_memory().unwrap();
        apply(&mut profile, &PROFILE[..22]).unwrap();
        profile
            .execute(
                "INSERT INTO history(url, title, visited_at) VALUES ('https://example.com/', 'Kept', 1)",
                [],
            )
            .unwrap();
        assert!(extension_objects(&profile) > 0);
        apply(&mut profile, PROFILE).unwrap();
        assert_eq!(extension_objects(&profile), 0);
        let title: String = profile
            .query_row("SELECT title FROM history", [], |row| row.get(0))
            .unwrap();
        assert_eq!(title, "Kept");
    }

    #[test]
    fn meta_v19_preserves_native_rows_and_history_without_inventing_beta_authority() {
        let mut conn = Connection::open_in_memory().unwrap();
        apply(&mut conn, &META[..18]).unwrap();
        insert_native_ownership_test_row(&conn, 1, 1, 1, "acquire", "native_absent_preparing")
            .unwrap();
        let native_id = b"abcdefghijklmnopabcdefghijklmnop";
        conn.execute(
            "UPDATE extension_native_ownership_journal SET revision=3,phase='native_owned',
             expected_native_identity_kind=1,expected_native_identity=?1,
             native_identity_kind=1,native_identity=?1",
            [&native_id[..]],
        )
        .unwrap();
        conn.execute("UPDATE extension_native_ownership_journal_state SET revision=4,operation_high_water=1,native_incarnation_high_water=1 WHERE id=1",[]).unwrap();
        let before: (i64,i64,i64,i64) = conn.query_row("SELECT revision,operation_high_water,native_incarnation_high_water,grant_rebind_count FROM extension_native_ownership_journal_state",[],|r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).unwrap();
        apply(&mut conn, &META[..19]).unwrap();
        apply(&mut conn, &META[..19]).unwrap();
        assert_eq!(
            conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            19
        );
        assert_eq!(conn.query_row("SELECT revision,operation_high_water,native_incarnation_high_water,grant_rebind_count FROM extension_native_ownership_journal_state",[],|r| Ok((r.get::<_,i64>(0)?,r.get::<_,i64>(1)?,r.get::<_,i64>(2)?,r.get::<_,i64>(3)?))).unwrap(),before);
        assert_eq!(conn.query_row("SELECT catalog_role,expected_native_identity,native_identity FROM extension_native_ownership_journal",[],|r| Ok((r.get::<_,String>(0)?,r.get::<_,Option<Vec<u8>>>(1)?,r.get::<_,Option<Vec<u8>>>(2)?))).unwrap(),("active".into(),Some(native_id.to_vec()),Some(native_id.to_vec())));
        assert_eq!(conn.query_row("SELECT phase,revision,expected_native_identity_kind,native_identity_kind FROM extension_native_ownership_journal",[],|r| Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?,r.get::<_,i64>(2)?,r.get::<_,i64>(3)?))).unwrap(),("native_owned".into(),3,1,1));
        assert!(conn
            .execute(
                "UPDATE extension_native_ownership_journal_state SET revision=1 WHERE id=1",
                []
            )
            .is_err());
    }

    #[test]
    fn meta_v19_rejects_beta_catalog_confusion_atomically_and_preserves_v18() {
        let mut conn = Connection::open_in_memory().unwrap();
        apply(&mut conn, &META[..18]).unwrap();
        insert_native_ownership_test_row(&conn, 1, 1, 1, "acquire", "native_absent_preparing")
            .unwrap();
        let authority = beta_authority("stable", "macos.wkwebextension.v1");
        conn.execute(
            "UPDATE extension_native_ownership_journal SET authority=?1",
            [&authority[..]],
        )
        .unwrap();
        let error = apply(&mut conn, &META[..19]).unwrap_err();
        assert_eq!(
            error.sqlite_error_code(),
            Some(rusqlite::ErrorCode::ConstraintViolation)
        );
        assert_eq!(
            conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            18
        );
        validate_manifest(&conn, META, 18).unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT catalog_role FROM extension_native_ownership_journal",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            "active"
        );
    }

    #[test]
    fn meta_v19_beta_source_constraints_preserve_native_frontiers() {
        let mut conn = Connection::open_in_memory().unwrap();
        apply(&mut conn, &META[..19]).unwrap();
        insert_native_ownership_test_row(&conn, 1, 1, 1, "acquire", "native_absent_preparing")
            .unwrap();
        let authority = beta_authority("staging", "macos.wkwebextension.v1");
        assert!(conn
            .execute(
                "UPDATE extension_native_ownership_journal SET catalog_role='beta'",
                []
            )
            .is_err());
        assert!(conn
            .execute(
                "UPDATE extension_native_ownership_journal SET authority=?1",
                [&authority[..]]
            )
            .is_err());
        conn.execute(
            "UPDATE extension_native_ownership_journal SET authority=?1,catalog_role='beta',payload_kind=2,archive_length=17,archive_sha256=zeroblob(32)",
            [&authority[..]],
        )
        .unwrap();
        assert!(conn
            .execute(
                "UPDATE extension_native_ownership_journal SET runtime_backend='windows_native'",
                []
            )
            .is_err());
        assert!(conn
            .execute(
                "UPDATE extension_native_ownership_journal SET browsing_context='private'",
                []
            )
            .is_err());
        assert!(conn
            .execute(
                "UPDATE extension_native_ownership_journal SET phase='native_may_own',revision=2",
                []
            )
            .is_err());
        conn.execute("UPDATE extension_native_ownership_journal SET phase='native_may_own',revision=2,expected_native_identity_kind=1,expected_native_identity=?1",[&b"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"[..]]).unwrap();
        assert!(conn.execute("UPDATE extension_native_ownership_journal SET expected_native_identity=NULL,expected_native_identity_kind=NULL",[]).is_err());
    }

    fn extension_profile_v14_fixture() -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", true).unwrap();
        apply(&mut conn, &PROFILE[..13]).unwrap();
        let tx = conn.transaction().unwrap();
        create_extension_profile_provenance(&tx).unwrap();
        tx.pragma_update(None, "user_version", 14).unwrap();
        tx.commit().unwrap();
        conn
    }

    #[test]
    fn extension_profile_v14_with_an_unexpected_trigger_is_preserved() {
        let mut conn = extension_profile_v14_fixture();
        conn.execute_batch(
            "CREATE TRIGGER unexpected_extension_trigger BEFORE INSERT ON extension_upstream_history
             BEGIN SELECT RAISE(ABORT,'unexpected'); END;",
        )
        .unwrap();
        assert!(validate_current(&conn, PROFILE).is_err());
        assert!(apply(&mut conn, PROFILE).is_err());
        assert_eq!(
            conn.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            14
        );
        assert_eq!(
            conn.query_row(
                "SELECT count(*) FROM sqlite_schema WHERE name='user_resources'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
    }

    #[test]
    fn both_release_and_work_qa_lineages_upgrade_without_losing_data() {
        for version in [13, 24, 29] {
            let mut conn = Connection::open_in_memory().unwrap();
            if version == 29 {
                for migration in PROFILE[..21].iter().chain(PROFILE[24..32].iter()) {
                    let tx = conn.transaction().unwrap();
                    (migration.up)(&tx).unwrap();
                    tx.commit().unwrap();
                }
                conn.pragma_update(None, "user_version", 29).unwrap();
                conn.execute("INSERT INTO works(id, schema_version, revision, status, objective, created_unix_ms, updated_unix_ms, lifecycle, objective_revision, context_revision, objective_author) VALUES ('00000000000000000000000001', 2, 1, 'draft', 'Keep my work', 1, 1, 'active', 1, 1, 'user')", []).unwrap();
                conn.execute("INSERT INTO work_memories(id,text,kind,work,execution,created_ms) VALUES ('0000000000000000000000000A','Keep my memory','fact','00000000000000000000000001',NULL,1)", []).unwrap();
            } else {
                apply(&mut conn, &PROFILE[..version]).unwrap();
            }
            conn.execute("INSERT INTO history(url,title,visited_at) VALUES ('https://example.com/','Kept',1)", []).unwrap();
            assert_eq!(validate_current(&conn, PROFILE).unwrap(), version as i64);
            apply(&mut conn, PROFILE).unwrap();
            apply(&mut conn, PROFILE).unwrap();
            assert_eq!(validate_current(&conn, PROFILE).unwrap(), 35);
            assert_eq!(
                conn.query_row("SELECT title FROM history", [], |r| r.get::<_, String>(0))
                    .unwrap(),
                "Kept"
            );
            assert_eq!(
                conn.query_row(
                    "SELECT count(*) FROM sqlite_schema WHERE name LIKE '%extension%'",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
                0
            );
            if version == 29 {
                assert_eq!(
                    conn.query_row("SELECT objective FROM works", [], |r| r.get::<_, String>(0))
                        .unwrap(),
                    "Keep my work"
                );
                assert_eq!(
                    conn.query_row("SELECT text FROM work_memories", [], |r| r
                        .get::<_, String>(0))
                        .unwrap(),
                    "Keep my memory"
                );
            }
        }
    }

    #[test]
    fn profile_v35_preserves_existing_time_and_adds_empty_bounded_receipts() {
        let mut conn = Connection::open_in_memory().unwrap();
        apply(&mut conn, &PROFILE[..34]).unwrap();
        conn.execute(
            "INSERT INTO time_spent(hour,place,spent_ms,opens) VALUES(100,'fixture.test',1234,2)",
            [],
        )
        .unwrap();
        apply(&mut conn, PROFILE).unwrap();
        apply(&mut conn, PROFILE).unwrap();
        assert_eq!(
            conn.query_row("SELECT spent_ms FROM time_spent", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            1234
        );
        assert_eq!(
            conn.query_row("SELECT count(*) FROM time_batch_receipts", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert!(conn
            .execute(
                "INSERT INTO time_batch_receipts(batch_id,digest) VALUES(x'00',zeroblob(32))",
                []
            )
            .is_err());
        assert!(conn
            .execute(
                "INSERT INTO time_batch_receipts(batch_id,digest) VALUES(zeroblob(16),x'00')",
                []
            )
            .is_err());
    }

    #[test]
    fn work_qa_bridge_refuses_an_injected_schema_before_writing() {
        let mut conn = Connection::open_in_memory().unwrap();
        for migration in PROFILE[..21].iter().chain(PROFILE[24..32].iter()) {
            let tx = conn.transaction().unwrap();
            (migration.up)(&tx).unwrap();
            tx.commit().unwrap();
        }
        conn.pragma_update(None, "user_version", 29).unwrap();
        conn.execute_batch("CREATE TABLE injected(value TEXT);")
            .unwrap();
        assert!(apply(&mut conn, PROFILE).is_err());
        assert_eq!(
            conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            29
        );
    }

    fn schema_fingerprint(migrations: &[Migration], version: i64) -> u64 {
        let manifest = expected_manifest(migrations, version).expect("shipped migrations apply");
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for object in &manifest {
            for part in [
                object.kind.as_str(),
                object.name.as_str(),
                object.table.as_str(),
                object.sql.as_deref().unwrap_or("\u{0}"),
            ] {
                for byte in part.as_bytes().iter().chain(std::iter::once(&0x1f)) {
                    hash ^= u64::from(*byte);
                    hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
                }
            }
        }
        hash
    }

    #[test]
    #[ignore]
    fn print_schema_fingerprints() {
        for (migrations, family) in [(META, "META"), (PROFILE, "PROFILE")] {
            println!("const {family}_SCHEMA_FINGERPRINTS: &[(i64, u64)] = &[");
            for migration in migrations {
                println!(
                    "        ({}, {:#018x}),",
                    migration.version,
                    schema_fingerprint(migrations, migration.version)
                );
            }
            println!("    ];");
        }
    }

    #[test]
    fn standalone_download_qa_v16_is_not_mistaken_for_the_task_schema() {
        let mut conn = Connection::open_in_memory().unwrap();
        apply(&mut conn, &PROFILE[..15]).unwrap();
        let transaction = conn.transaction().unwrap();
        (PROFILE
            .iter()
            .find(|migration| migration.version == 20)
            .unwrap()
            .up)(&transaction)
        .unwrap();
        transaction.pragma_update(None, "user_version", 16).unwrap();
        transaction.commit().unwrap();
        // The old standalone QA assigned download tables to version 16.
        // The release lineage assigned Tasks to that version. Never rewrite
        // the version or partially migrate a foreign schema into this lineage.
        assert!(apply(&mut conn, PROFILE).is_err());
        assert_eq!(
            conn.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            16
        );
        assert_eq!(
            conn.query_row(
                "SELECT count(*) FROM pragma_table_info('user_resources') WHERE name='status'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
        assert_eq!(
            conn.query_row(
                "SELECT count(*) FROM sqlite_schema WHERE name='downloads'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            1
        );
    }

    #[test]
    fn profile_v20_adds_downloads_without_changing_existing_tasks_or_receipts() {
        let mut conn = Connection::open_in_memory().unwrap();
        let previous = PROFILE
            .iter()
            .position(|migration| migration.version == 20)
            .unwrap();
        apply(&mut conn, &PROFILE[..previous]).unwrap();
        let id = "00000000000000000000000001";
        let list = "00000000000000000000000002";
        let body = r#"{"title":"Keep this task","content":{"kind":"task","description":"Original description","details":{"list":"00000000000000000000000002"}}}"#;
        conn.execute(
            "INSERT INTO task_lists(id,title,revision) VALUES(?1,'Release',3)",
            [list],
        )
        .unwrap();
        conn.execute("INSERT INTO user_resources(id,kind,revision,title,completed,pinned,trashed,created_at,updated_at,body,search_text,task_list,task_deadline,task_duration) VALUES(?1,'task',7,'Keep this task',0,0,0,10,20,?2,'keep this task',?3,'2026-10-01',45)", rusqlite::params![id,body,list]).unwrap();
        conn.execute("INSERT INTO user_resource_receipts(request_id,digest,retained,resource_id,revision) VALUES('request-1',zeroblob(32),1,?1,7)", [id]).unwrap();
        conn.execute("INSERT INTO task_list_receipts(request_id,digest,list_id,retained) VALUES('list-request-1',zeroblob(32),?1,1)", [list]).unwrap();
        apply(&mut conn, PROFILE).unwrap();
        let saved: (String, i64, String, i64) = conn
            .query_row(
                "SELECT body,revision,task_deadline,task_duration FROM user_resources WHERE id=?1",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(saved, (body.into(), 7, "2026-10-01".into(), 45));
        assert_eq!(
            conn.query_row(
                "SELECT bytes FROM user_resource_usage WHERE id=1",
                [],
                |row| row.get::<_, usize>(0)
            )
            .unwrap(),
            body.len()
        );
        for table in ["task_lists", "task_list_receipts", "user_resource_receipts"] {
            assert_eq!(
                conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| row
                    .get::<_, i64>(0))
                    .unwrap(),
                1
            );
        }
        for table in ["downloads", "download_preferences"] {
            assert_eq!(
                conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| row
                    .get::<_, i64>(0))
                    .unwrap(),
                0
            );
        }
        apply(&mut conn, PROFILE).unwrap();
        assert_eq!(
            conn.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            PROFILE.last().unwrap().version
        );
    }

    #[test]
    fn protection_release_migration_enables_once_and_preserves_site_preferences() {
        let mut conn = Connection::open_in_memory().unwrap();
        apply(&mut conn, &META[..22]).unwrap();
        let off = "00000000000000000000000001";
        let on = "00000000000000000000000002";
        for (id, enabled, revision) in [(off, 0, 7), (on, 1, 9)] {
            conn.execute(
                "INSERT INTO profiles(id,name,kind,position) VALUES(?1,'Test','named',?2)",
                rusqlite::params![id, enabled],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO profile_blocker_settings VALUES(?1,?2,?3)",
                rusqlite::params![id, revision, enabled],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO profile_blocker_sites VALUES(?1,5,'unchanged-site-preferences')",
                [id],
            )
            .unwrap();
        }
        apply(&mut conn, META).unwrap();
        let settings = |conn: &Connection, id: &str| {
            conn.query_row(
                "SELECT enabled,revision FROM profile_blocker_settings WHERE profile_id=?1",
                [id],
                |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
            )
            .unwrap()
        };
        assert_eq!(settings(&conn, off), (1, 8));
        assert_eq!(settings(&conn, on), (1, 9));
        assert_eq!(conn.query_row("SELECT count(*) FROM profile_blocker_sites WHERE revision=5 AND payload='unchanged-site-preferences'", [], |r| r.get::<_,i64>(0)).unwrap(), 2);
        conn.execute(
            "UPDATE profile_blocker_settings SET enabled=0,revision=9 WHERE profile_id=?1",
            [off],
        )
        .unwrap();
        apply(&mut conn, META).unwrap();
        assert_eq!(
            settings(&conn, off),
            (0, 9),
            "later explicit opt-out survives reopening"
        );
    }

    #[test]
    fn protection_release_migration_rolls_back_revision_exhaustion() {
        let mut conn = Connection::open_in_memory().unwrap();
        apply(&mut conn, &META[..22]).unwrap();
        conn.execute_batch("INSERT INTO profile_blocker_settings VALUES('00000000000000000000000001',7,0),('00000000000000000000000002',9223372036854775807,0)").unwrap();
        assert!(apply(&mut conn, META).is_err());
        assert_eq!(
            conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            22
        );
        assert_eq!(
            conn.query_row(
                "SELECT sum(enabled) FROM profile_blocker_settings",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
        assert_eq!(conn.query_row("SELECT revision FROM profile_blocker_settings WHERE profile_id='00000000000000000000000001'", [], |r| r.get::<_,i64>(0)).unwrap(), 7);
    }

    #[test]
    fn released_migrations_keep_the_exact_schema_text_they_shipped_with() {
        for (migrations, expected, family) in [
            (META, META_SCHEMA_FINGERPRINTS, "meta"),
            (PROFILE, PROFILE_SCHEMA_FINGERPRINTS, "profile"),
        ] {
            let versions: Vec<i64> = migrations.iter().map(|m| m.version).collect();
            assert_eq!(
                expected
                    .iter()
                    .map(|(version, _)| *version)
                    .collect::<Vec<_>>(),
                versions,
                "{family}: every shipped version needs a recorded fingerprint"
            );
            for (version, fingerprint) in expected {
                assert_eq!(
                    schema_fingerprint(migrations, *version),
                    *fingerprint,
                    "{family} migration {version} no longer produces the schema it shipped with; \
                     a released migration's SQL text is an artifact, not source to reformat"
                );
            }
        }
    }

    fn insert_native_ownership_test_row(
        conn: &Connection,
        operation: i64,
        entry_revision: i64,
        incarnation: i64,
        intent: &str,
        phase: &str,
    ) -> rusqlite::Result<usize> {
        conn.execute(
            "INSERT INTO extension_native_ownership_journal(
                 profile_id, install_id, browsing_context, operation, revision,
                 authority, package_key, package_revision,
                 payload_kind, archive_length, archive_sha256,
                 manifest_sha256, tree_sha256,
                 catalog_set_sha256, catalog_role,
                 store_catalog_revision, store_install_revision, store_grant_revision,
                 grant_sha256, runtime_backend, native_incarnation, intent, phase
             ) VALUES (
                 '00000000000000000000000001', ?1, 'regular', ?2, ?3,
                 ?4, ?5, 1, 1, NULL, NULL, ?6, ?7, ?8, 'active',
                 1, 1, 1, ?9, 'macos_native', ?10, ?11, ?12
             )",
            rusqlite::params![
                vec![operation as u8; 16],
                operation,
                entry_revision,
                vec![2_u8; 32],
                vec![3_u8; 32],
                vec![4_u8; 32],
                vec![5_u8; 32],
                vec![6_u8; 32],
                vec![7_u8; 32],
                incarnation,
                intent,
                phase,
            ],
        )
    }

    #[test]
    fn apply_is_idempotent_and_versioned() {
        let mut conn = Connection::open_in_memory().unwrap();
        apply(&mut conn, PROFILE).unwrap();
        apply(&mut conn, PROFILE).unwrap();
        let v: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, PROFILE.last().unwrap().version);
    }

    #[test]
    fn grant_migration_preserves_uninitialized_absence_and_cascades_with_install() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", true).unwrap();
        apply(&mut conn, &PROFILE[..9]).unwrap();
        conn.execute(
            "INSERT INTO extension_installs(
                 id, revision, authority, package_key, package_revision,
                 archive_sha256, manifest_sha256, tree_sha256, desired_enabled
             ) VALUES (?1, 1, ?2, ?3, 1, ?4, ?5, ?6, 0)",
            rusqlite::params![
                vec![1_u8; 16],
                vec![2_u8; 32],
                vec![3_u8; 32],
                vec![4_u8; 32],
                vec![5_u8; 32],
                vec![6_u8; 32],
            ],
        )
        .unwrap();

        apply(&mut conn, &PROFILE[..10]).unwrap();
        let roots: i64 = conn
            .query_row("SELECT count(*) FROM extension_grants", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(roots, 0, "migration must not grant existing installs");

        conn.execute(
            "INSERT INTO extension_grants(
                 install_id, revision, authority, package_key, package_revision,
                 archive_sha256, manifest_sha256, tree_sha256, grant_sha256,
                 file_access, private_access
             ) VALUES (?1, 1, ?2, ?3, 1, ?4, ?5, ?6, ?7, 0, 0)",
            rusqlite::params![
                vec![1_u8; 16],
                vec![2_u8; 32],
                vec![3_u8; 32],
                vec![4_u8; 32],
                vec![5_u8; 32],
                vec![6_u8; 32],
                vec![7_u8; 32],
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO extension_grant_api_permissions(install_id, name)
             VALUES (?1, 'storage')",
            [vec![1_u8; 16]],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO extension_grant_host_permissions(install_id, pattern)
             VALUES (?1, 'https://example.com/*')",
            [vec![1_u8; 16]],
        )
        .unwrap();
        conn.execute("DELETE FROM extension_installs", []).unwrap();
        for table in [
            "extension_grants",
            "extension_grant_api_permissions",
            "extension_grant_host_permissions",
        ] {
            let count: i64 = conn
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(count, 0, "install deletion retained {table}");
        }
    }

    #[test]
    fn grant_migration_preserves_legacy_enabled_intent_without_inventing_authority() {
        let mut conn = Connection::open_in_memory().unwrap();
        apply(&mut conn, &PROFILE[..9]).unwrap();
        conn.execute(
            "INSERT INTO extension_installs(
                 id, revision, authority, package_key, package_revision,
                 archive_sha256, manifest_sha256, tree_sha256, desired_enabled
             ) VALUES (?1, 3, ?2, ?3, 4, ?4, ?5, ?6, 1)",
            rusqlite::params![
                vec![1_u8; 16],
                vec![2_u8; 32],
                vec![3_u8; 32],
                vec![4_u8; 32],
                vec![5_u8; 32],
                vec![6_u8; 32],
            ],
        )
        .unwrap();

        apply(&mut conn, &PROFILE[..10]).unwrap();
        let desired_enabled: i64 = conn
            .query_row(
                "SELECT desired_enabled FROM extension_installs",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let grants: i64 = conn
            .query_row("SELECT count(*) FROM extension_grants", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(desired_enabled, 1);
        assert_eq!(grants, 0, "migration must never backfill grant authority");
    }

    #[test]
    fn profile_v11_derives_exact_install_id_high_water_from_legacy_rows() {
        for ids in [Vec::new(), vec![5_u128, 10, 7]] {
            let mut conn = Connection::open_in_memory().unwrap();
            apply(&mut conn, &PROFILE[..10]).unwrap();
            for (index, id) in ids.iter().copied().enumerate() {
                conn.execute(
                    "INSERT INTO extension_installs(
                         id, revision, authority, package_key, package_revision,
                         archive_sha256, manifest_sha256, tree_sha256, desired_enabled
                     ) VALUES (?1, 1, ?2, ?3, 1, ?4, ?5, ?6, 0)",
                    rusqlite::params![
                        id.to_be_bytes().to_vec(),
                        vec![index as u8 + 1; 32],
                        vec![index as u8 + 11; 32],
                        vec![index as u8 + 21; 32],
                        vec![index as u8 + 31; 32],
                        vec![index as u8 + 41; 32],
                    ],
                )
                .unwrap();
            }

            apply(&mut conn, &PROFILE[..11]).unwrap();
            let high_water: Option<Vec<u8>> = conn
                .query_row(
                    "SELECT install_id_high_water
                     FROM extension_install_catalog WHERE id = 1",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(
                high_water,
                ids.iter().max().map(|id| id.to_be_bytes().to_vec())
            );
            let version: i64 = conn
                .query_row("PRAGMA user_version", [], |row| row.get(0))
                .unwrap();
            assert_eq!(version, 11);
        }
    }

    #[test]
    fn profile_v11_rejects_malformed_install_id_high_water() {
        let mut conn = Connection::open_in_memory().unwrap();
        apply(&mut conn, &PROFILE[..11]).unwrap();
        for value in [vec![1_u8; 15], vec![2_u8; 17]] {
            assert!(conn
                .execute(
                    "UPDATE extension_install_catalog
                     SET install_id_high_water = ?1 WHERE id = 1",
                    [value],
                )
                .is_err());
        }
        assert!(conn
            .execute(
                "UPDATE extension_install_catalog
                 SET install_id_high_water = 1 WHERE id = 1",
                [],
            )
            .is_err());
    }

    #[test]
    fn profile_v13_adds_bounded_default_allow_extension_policy() {
        let mut conn = Connection::open_in_memory().unwrap();
        apply(&mut conn, &PROFILE[..12]).unwrap();
        apply(&mut conn, &PROFILE[..13]).unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT revision, paused FROM extension_profile_policy WHERE id = 1",
                [],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
            )
            .unwrap(),
            (1, 0)
        );
        assert_eq!(
            conn.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            13
        );
        for index in 0..128 {
            conn.execute(
                "INSERT INTO extension_profile_site_denials(policy_id, pattern)
                 VALUES (1, ?1)",
                [format!("https://site-{index}.example/*")],
            )
            .unwrap();
        }
        assert!(conn
            .execute(
                "INSERT INTO extension_profile_site_denials(policy_id, pattern)
                 VALUES (1, 'https://overflow.example/*')",
                [],
            )
            .is_err());
        for statement in [
            "UPDATE extension_profile_policy SET revision = 0 WHERE id = 1",
            "UPDATE extension_profile_policy SET paused = 2 WHERE id = 1",
        ] {
            assert!(conn.execute(statement, []).is_err());
        }
    }

    #[test]
    fn profile_v10_rejects_malformed_grant_roots_children_and_orphans() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", true).unwrap();
        apply(&mut conn, &PROFILE[..10]).unwrap();
        let id = vec![1_u8; 16];
        conn.execute(
            "INSERT INTO extension_installs(
                 id, revision, authority, package_key, package_revision,
                 archive_sha256, manifest_sha256, tree_sha256, desired_enabled
             ) VALUES (?1, 1, ?2, ?3, 1, ?4, ?5, ?6, 0)",
            rusqlite::params![
                &id,
                vec![2_u8; 32],
                vec![3_u8; 32],
                vec![4_u8; 32],
                vec![5_u8; 32],
                vec![6_u8; 32],
            ],
        )
        .unwrap();
        assert!(conn
            .execute(
                "INSERT INTO extension_grants(
                     install_id, revision, authority, package_key, package_revision,
                     archive_sha256, manifest_sha256, tree_sha256, grant_sha256,
                     file_access, private_access
                 ) VALUES (?1, 1, ?2, ?3, 1, ?4, ?5, ?6, ?7, 0, 0)",
                rusqlite::params![
                    &id,
                    vec![2_u8; 32],
                    vec![3_u8; 32],
                    vec![4_u8; 32],
                    vec![5_u8; 32],
                    vec![6_u8; 32],
                    vec![7_u8; 31],
                ],
            )
            .is_err());
        conn.execute(
            "INSERT INTO extension_grants(
                 install_id, revision, authority, package_key, package_revision,
                 archive_sha256, manifest_sha256, tree_sha256, grant_sha256,
                 file_access, private_access
             ) VALUES (?1, 1, ?2, ?3, 1, ?4, ?5, ?6, ?7, 0, 0)",
            rusqlite::params![
                &id,
                vec![2_u8; 32],
                vec![3_u8; 32],
                vec![4_u8; 32],
                vec![5_u8; 32],
                vec![6_u8; 32],
                vec![7_u8; 32],
            ],
        )
        .unwrap();
        for invalid in [String::new(), "x".repeat(97), "bad\0name".into()] {
            assert!(conn
                .execute(
                    "INSERT INTO extension_grant_api_permissions(install_id, name)
                     VALUES (?1, ?2)",
                    rusqlite::params![&id, invalid],
                )
                .is_err());
        }
        for invalid in [String::new(), "x".repeat(2049), "bad\0pattern".into()] {
            assert!(conn
                .execute(
                    "INSERT INTO extension_grant_host_permissions(install_id, pattern)
                     VALUES (?1, ?2)",
                    rusqlite::params![&id, invalid],
                )
                .is_err());
        }
        assert!(conn
            .execute(
                "INSERT INTO extension_grant_api_permissions(install_id, name)
                 VALUES (?1, 'storage')",
                [vec![9_u8; 16]],
            )
            .is_err());
    }

    #[test]
    fn apply_rejects_schema_from_a_newer_binary() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "user_version", 10_000).unwrap();

        let error = apply(&mut conn, PROFILE).unwrap_err().to_string();
        assert!(error.contains("newer than supported"), "{error}");
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 10_000);
    }

    #[test]
    fn meta_v8_completion_boundary_migrates_without_losing_deletion_authorization() {
        let mut conn = Connection::open_in_memory().unwrap();
        apply(&mut conn, &META[..7]).unwrap();
        // Reproduce the exact schema already written by the version-8
        // development build. Do not build this fixture by invoking migration
        // 8: the test must detect any future edit to that shipped boundary.
        conn.execute_batch(
            "ALTER TABLE profile_deletion_journal
             ADD COLUMN local_unlink_completed INTEGER NOT NULL DEFAULT 0
             CHECK (local_unlink_completed IN (0, 1));
             PRAGMA user_version=8;",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO profile_deletion_journal(
                 profile_id,
                 authorized_at,
                 native_erasure_verified,
                 local_unlink_completed
             ) VALUES ('01J00000000000000000000000', 1, 1, 1)",
            [],
        )
        .unwrap();

        apply(&mut conn, META).unwrap();

        let process: Option<String> = conn
            .query_row(
                "SELECT local_unlink_process
                 FROM profile_deletion_journal
                 WHERE profile_id = '01J00000000000000000000000'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(process.as_deref(), Some("00000000000000000000000000"));
        let parsed = zephium_core::ids::ProfileId::parse(process.as_deref().unwrap()).unwrap();
        assert_eq!(parsed.to_string(), "00000000000000000000000000");
        let legacy_column: i64 = conn
            .query_row(
                "SELECT count(*) FROM pragma_table_info('profile_deletion_journal')
                 WHERE name = 'local_unlink_completed'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(legacy_column, 1);
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, META.last().unwrap().version);
    }

    #[test]
    fn meta_v9_rejects_impossible_legacy_completion_without_mutating_v8() {
        let mut conn = Connection::open_in_memory().unwrap();
        apply(&mut conn, &META[..8]).unwrap();
        conn.execute(
            "INSERT INTO profile_deletion_journal(
                 profile_id,
                 authorized_at,
                 native_erasure_verified,
                 local_unlink_completed
             ) VALUES ('01J00000000000000000000000', 1, 0, 1)",
            [],
        )
        .unwrap();

        let error = apply(&mut conn, META).unwrap_err().to_string();
        assert!(error.contains("without native proof"), "{error}");
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 8);
        let generation_column: i64 = conn
            .query_row(
                "SELECT count(*) FROM pragma_table_info('profile_deletion_journal')
                 WHERE name = 'local_unlink_process'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(generation_column, 0, "failed migration was not rolled back");
        let retained: (i64, i64) = conn
            .query_row(
                "SELECT native_erasure_verified, local_unlink_completed
                 FROM profile_deletion_journal",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(retained, (0, 1));
    }

    #[test]
    fn meta_v10_creates_an_exact_disabled_profile_blocker_cohort() {
        let mut conn = Connection::open_in_memory().unwrap();
        apply(&mut conn, &META[..9]).unwrap();
        for (position, profile) in [
            (0_i64, "01J00000000000000000000000"),
            (1_i64, "01J00000000000000000000001"),
        ] {
            conn.execute(
                "INSERT INTO profiles(id, name, kind, position)
                 VALUES (?1, 'Profile', 'default', ?2)",
                rusqlite::params![profile, position],
            )
            .unwrap();
        }

        apply(&mut conn, &META[..10]).unwrap();

        let rows: Vec<(String, i64, i64)> = {
            let mut statement = conn
                .prepare(
                    "SELECT profile_id, revision, enabled
                     FROM profile_blocker_settings ORDER BY profile_id",
                )
                .unwrap();
            statement
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap()
        };
        assert_eq!(
            rows,
            vec![
                ("01J00000000000000000000000".into(), 1, 0),
                ("01J00000000000000000000001".into(), 1, 0),
            ]
        );
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 10);
    }

    #[test]
    fn meta_v10_blocker_schema_rejects_invalid_durable_values() {
        let mut conn = Connection::open_in_memory().unwrap();
        apply(&mut conn, META).unwrap();

        for (profile, revision, enabled) in [
            ("short", 1_i64, 0_i64),
            ("01J00000000000000000000000", 0, 0),
            ("01J00000000000000000000001", 1, 2),
        ] {
            assert!(
                conn.execute(
                    "INSERT INTO profile_blocker_settings(
                         profile_id, revision, enabled
                     ) VALUES (?1, ?2, ?3)",
                    rusqlite::params![profile, revision, enabled],
                )
                .is_err(),
                "accepted invalid blocker setting ({profile}, {revision}, {enabled})"
            );
        }
        let count: i64 = conn
            .query_row("SELECT count(*) FROM profile_blocker_settings", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn meta_v11_adds_an_exact_empty_native_ownership_journal() {
        let mut conn = Connection::open_in_memory().unwrap();
        apply(&mut conn, &META[..10]).unwrap();

        apply(&mut conn, &META[..11]).unwrap();

        let state: (i64, i64, i64, i64) = conn
            .query_row(
                "SELECT id, revision, operation_high_water,
                        native_incarnation_high_water
                 FROM extension_native_ownership_journal_state",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(state, (1, 1, 0, 0));
        let entries: i64 = conn
            .query_row(
                "SELECT count(*) FROM extension_native_ownership_journal",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(entries, 0);
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 11);
    }

    #[test]
    fn meta_v12_adds_nullable_bounded_native_identity_without_inference() {
        let mut conn = Connection::open_in_memory().unwrap();
        apply(&mut conn, &META[..11]).unwrap();
        insert_native_ownership_test_row(&conn, 1, 2, 1, "acquire", "native_may_own").unwrap();
        conn.execute(
            "UPDATE extension_native_ownership_journal_state
             SET revision = 3,
                 operation_high_water = 1,
                 native_incarnation_high_water = 1",
            [],
        )
        .unwrap();

        apply(&mut conn, &META[..12]).unwrap();

        let identity: (Option<i64>, Option<Vec<u8>>) = conn
            .query_row(
                "SELECT native_identity_kind, native_identity
                 FROM extension_native_ownership_journal",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(identity, (None, None));
        assert_eq!(
            conn.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            12
        );
    }

    #[test]
    fn meta_v12_refuses_to_infer_legacy_native_owned_identity() {
        let mut conn = Connection::open_in_memory().unwrap();
        apply(&mut conn, &META[..11]).unwrap();
        insert_native_ownership_test_row(&conn, 1, 3, 1, "acquire", "native_owned").unwrap();
        conn.execute(
            "UPDATE extension_native_ownership_journal_state
             SET revision = 4,
                 operation_high_water = 1,
                 native_incarnation_high_water = 1",
            [],
        )
        .unwrap();

        assert!(apply(&mut conn, &META[..12]).is_err());
        assert_eq!(
            conn.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            11
        );
        let has_identity_column = conn
            .prepare("SELECT native_identity FROM extension_native_ownership_journal")
            .is_ok();
        assert!(!has_identity_column);
    }

    #[test]
    fn meta_v12_preserves_legacy_identityless_cleanup_frontiers() {
        for (entry_revision, phase) in [(4, "native_may_own"), (5, "native_absent_release_pending")]
        {
            let mut conn = Connection::open_in_memory().unwrap();
            apply(&mut conn, &META[..11]).unwrap();
            insert_native_ownership_test_row(&conn, 1, entry_revision, 1, "release", phase)
                .unwrap();
            conn.execute(
                "UPDATE extension_native_ownership_journal_state
                 SET revision = ?1,
                     operation_high_water = 1,
                     native_incarnation_high_water = 1",
                [1 + entry_revision],
            )
            .unwrap();

            apply(&mut conn, &META[..12]).unwrap();
            let row: (i64, Option<i64>, Option<Vec<u8>>) = conn
                .query_row(
                    "SELECT revision, native_identity_kind, native_identity
                     FROM extension_native_ownership_journal",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .unwrap();
            assert_eq!(row, (entry_revision, None, None));
        }
    }

    #[test]
    fn meta_v12_schema_mirrors_native_identity_shape_and_backend() {
        let mut conn = Connection::open_in_memory().unwrap();
        apply(&mut conn, &META[..12]).unwrap();
        insert_native_ownership_test_row(&conn, 1, 2, 1, "acquire", "native_may_own").unwrap();
        conn.execute(
            "UPDATE extension_native_ownership_journal
             SET revision = 3,
                 phase = 'native_owned',
                 native_identity_kind = 1,
                 native_identity = ?1",
            [vec![b'a'; 32]],
        )
        .unwrap();
        conn.execute(
            "UPDATE extension_native_ownership_journal_state
             SET revision = 4,
                 operation_high_water = 1,
                 native_incarnation_high_water = 1",
            [],
        )
        .unwrap();

        for update in [
            "native_identity_kind = NULL",
            "native_identity_kind = 2",
            "native_identity = zeroblob(31)",
            "native_identity = zeroblob(33)",
            "native_identity = zeroblob(32)",
            "native_identity = NULL",
        ] {
            assert!(
                conn.execute(
                    &format!("UPDATE extension_native_ownership_journal SET {update}"),
                    [],
                )
                .is_err(),
                "accepted invalid identity update: {update}"
            );
        }
        for (description, invalid_identity) in [
            ("embedded NUL", {
                let mut bytes = vec![b'a'; 32];
                bytes[16] = 0;
                bytes
            }),
            ("non-ASCII high byte", {
                let mut bytes = vec![b'a'; 32];
                bytes[16] = 0xff;
                bytes
            }),
        ] {
            assert!(
                conn.execute(
                    "UPDATE extension_native_ownership_journal
                     SET native_identity = ?1",
                    [invalid_identity],
                )
                .is_err(),
                "accepted {description} in native identity"
            );
        }
    }

    #[test]
    fn meta_v12_trigger_accepts_only_extended_reachable_history_ceiling() {
        let mut conn = Connection::open_in_memory().unwrap();
        apply(&mut conn, &META[..12]).unwrap();
        insert_native_ownership_test_row(&conn, 1, 1, 1, "acquire", "native_absent_preparing")
            .unwrap();
        conn.execute(
            "UPDATE extension_native_ownership_journal_state
             SET revision = 2,
                 operation_high_water = 1,
                 native_incarnation_high_water = 1",
            [],
        )
        .unwrap();
        conn.execute("DELETE FROM extension_native_ownership_journal", [])
            .unwrap();
        conn.execute(
            "UPDATE extension_native_ownership_journal_state SET revision = 8",
            [],
        )
        .unwrap();
        assert!(conn
            .execute(
                "UPDATE extension_native_ownership_journal_state SET revision = 9",
                [],
            )
            .is_err());
    }

    #[test]
    fn meta_v13_adds_expected_identity_without_inferring_it_from_observation() {
        let mut conn = Connection::open_in_memory().unwrap();
        apply(&mut conn, &META[..12]).unwrap();
        insert_native_ownership_test_row(&conn, 1, 2, 1, "acquire", "native_may_own").unwrap();
        let observed = vec![b'a'; 32];
        conn.execute(
            "UPDATE extension_native_ownership_journal
             SET revision = 3,
                 phase = 'native_owned',
                 native_identity_kind = 1,
                 native_identity = ?1",
            [&observed],
        )
        .unwrap();
        insert_native_ownership_test_row(&conn, 2, 4, 2, "release", "native_may_own").unwrap();
        conn.execute(
            "UPDATE extension_native_ownership_journal_state
             SET revision = 8,
                 operation_high_water = 2,
                 native_incarnation_high_water = 2",
            [],
        )
        .unwrap();

        apply(&mut conn, &META[..13]).unwrap();

        let mut statement = conn
            .prepare(
                "SELECT expected_native_identity_kind, expected_native_identity,
                        native_identity_kind, native_identity
                 FROM extension_native_ownership_journal
                 ORDER BY operation",
            )
            .unwrap();
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, Option<i64>>(0)?,
                    row.get::<_, Option<Vec<u8>>>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, Option<Vec<u8>>>(3)?,
                ))
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(
            rows,
            vec![
                (None, None, Some(1), Some(observed)),
                (None, None, None, None),
            ]
        );
        assert_eq!(
            conn.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            13
        );
    }

    #[test]
    fn meta_v13_enforces_expected_identity_shape_backend_and_owned_match() {
        let mut conn = Connection::open_in_memory().unwrap();
        apply(&mut conn, &META[..13]).unwrap();
        insert_native_ownership_test_row(&conn, 1, 2, 1, "acquire", "native_may_own").unwrap();
        let expected = vec![b'a'; 32];
        conn.execute(
            "UPDATE extension_native_ownership_journal
             SET expected_native_identity_kind = 1,
                 expected_native_identity = ?1",
            [&expected],
        )
        .unwrap();
        assert!(conn
            .execute(
                "UPDATE extension_native_ownership_journal
                 SET revision = 3, phase = 'native_owned'",
                [],
            )
            .is_err());

        for update in [
            "expected_native_identity_kind = NULL",
            "expected_native_identity_kind = 2",
            "expected_native_identity = zeroblob(31)",
            "expected_native_identity = zeroblob(33)",
            "expected_native_identity = zeroblob(32)",
            "expected_native_identity = NULL",
        ] {
            assert!(
                conn.execute(
                    &format!("UPDATE extension_native_ownership_journal SET {update}"),
                    [],
                )
                .is_err(),
                "accepted invalid expected identity update: {update}"
            );
        }

        let observed_mismatch = vec![b'b'; 32];
        conn.execute(
            "UPDATE extension_native_ownership_journal
             SET revision = 3,
                 native_identity_kind = 1,
                 native_identity = ?1",
            [&observed_mismatch],
        )
        .unwrap();
        assert!(conn
            .execute(
                "UPDATE extension_native_ownership_journal
                 SET revision = 4, phase = 'native_owned'",
                [],
            )
            .is_err());

        conn.execute(
            "UPDATE extension_native_ownership_journal
             SET native_identity = ?1",
            [&expected],
        )
        .unwrap();
        conn.execute(
            "UPDATE extension_native_ownership_journal
             SET revision = 4, phase = 'native_owned'",
            [],
        )
        .unwrap();
    }

    #[test]
    fn meta_v14_seeds_only_exact_regular_macos_possible_owner_evidence() {
        let mut conn = Connection::open_in_memory().unwrap();
        apply(&mut conn, &META[..13]).unwrap();
        let profiles: Vec<_> = (1_u128..=6)
            .map(|value| zephium_core::ids::ProfileId::from(value).to_string())
            .collect();
        for (position, profile) in profiles.iter().enumerate() {
            conn.execute(
                "INSERT INTO profiles(id, name, kind, position)
                 VALUES (?1, 'Fixture', 'named', ?2)",
                rusqlite::params![profile, position as i64],
            )
            .unwrap();
        }
        for (operation, revision, intent, phase) in [
            (1_i64, 2_i64, "acquire", "native_may_own"),
            (2, 2, "release", "native_absent_release_pending"),
            (3, 1, "acquire", "native_absent_preparing"),
            (4, 5, "release", "native_absent_release_pending"),
            (5, 2, "acquire", "native_may_own"),
            (6, 2, "acquire", "native_may_own"),
        ] {
            insert_native_ownership_test_row(&conn, operation, revision, operation, intent, phase)
                .unwrap();
            conn.execute(
                "UPDATE extension_native_ownership_journal
                 SET profile_id = ?2
                 WHERE operation = ?1",
                rusqlite::params![operation, &profiles[(operation - 1) as usize]],
            )
            .unwrap();
        }
        conn.execute(
            "UPDATE extension_native_ownership_journal
             SET runtime_backend = 'macos_compatibility'
             WHERE operation = 5",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE extension_native_ownership_journal
             SET browsing_context = 'private'
             WHERE operation = 6",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE extension_native_ownership_journal_state
             SET revision = 15,
                 operation_high_water = 6,
                 native_incarnation_high_water = 6",
            [],
        )
        .unwrap();

        apply(&mut conn, &META[..14]).unwrap();

        let seeded: Vec<String> = {
            let mut statement = conn
                .prepare(
                    "SELECT profile_id
                     FROM extension_native_namespace_obligations
                     ORDER BY profile_id",
                )
                .unwrap();
            statement
                .query_map([], |row| row.get(0))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap()
        };
        assert_eq!(seeded, vec![profiles[0].clone(), profiles[3].clone()]);
        assert_eq!(
            conn.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            14
        );
        assert_eq!(PROFILE.last().map(|migration| migration.version), Some(35));
    }

    #[test]
    fn meta_v15_separates_live_grant_rebinds_from_native_lifecycle_history() {
        let mut conn = Connection::open_in_memory().unwrap();
        apply(&mut conn, &META[..14]).unwrap();
        insert_native_ownership_test_row(&conn, 1, 2, 1, "acquire", "native_may_own").unwrap();
        conn.execute(
            "UPDATE extension_native_ownership_journal_state
             SET revision = 3,
                 operation_high_water = 1,
                 native_incarnation_high_water = 1",
            [],
        )
        .unwrap();

        apply(&mut conn, &META[..15]).unwrap();

        assert!(conn
            .execute(
                "UPDATE extension_native_ownership_journal_state
                 SET grant_rebind_count = 1",
                [],
            )
            .is_err());
        conn.execute(
            "UPDATE extension_native_ownership_journal
             SET store_grant_revision = 2,
                 grant_sha256 = ?1",
            [vec![8_u8; 32]],
        )
        .unwrap();
        conn.execute(
            "UPDATE extension_native_ownership_journal_state
             SET revision = 4,
                 grant_rebind_count = 1",
            [],
        )
        .unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT revision, grant_rebind_count
                 FROM extension_native_ownership_journal_state",
                [],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
            )
            .unwrap(),
            (4, 1)
        );
        assert!(conn
            .execute(
                "UPDATE extension_native_ownership_journal_state
                 SET revision = 5",
                [],
            )
            .is_err());
        assert_eq!(
            conn.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            15
        );
    }

    #[test]
    fn meta_v16_agent_audit_schema_is_bounded_immutable_and_counted() {
        let mut conn = Connection::open_in_memory().unwrap();
        apply(&mut conn, &META[..15]).unwrap();
        apply(&mut conn, &META[..16]).unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT delivery_count, event_count FROM agent_audit_state WHERE id = 1",
                [],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
            )
            .unwrap(),
            (0, 0)
        );

        let manifest = vec![1_u8; 16];
        let supervisor = 1_u64.to_be_bytes();
        let delivery = 1_u64.to_be_bytes();
        let event = 1_u64.to_be_bytes();
        let recorded_at = 100_u64.to_be_bytes();
        let mut record = vec![0_u8; zephium_agentic::AGENT_AUDIT_RECORD_V1_BYTES];
        record[0] = 1;
        conn.execute(
            "INSERT INTO agent_audit_deliveries(
                 manifest_id, supervisor_id, delivery_id,
                 first_event_id, last_event_id, event_count
             ) VALUES (?1, ?2, ?3, ?4, ?4, 1)",
            rusqlite::params![&manifest, &supervisor, &delivery, &event,],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO agent_audit_events(
                 manifest_id, supervisor_id, event_id, delivery_id,
                 batch_index, recorded_at, record_version, record
             ) VALUES (?1, ?2, ?3, ?4, 0, ?5, 1, ?6)",
            rusqlite::params![
                &manifest,
                &supervisor,
                &event,
                &delivery,
                &recorded_at,
                &record,
            ],
        )
        .unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT delivery_count, event_count FROM agent_audit_state WHERE id = 1",
                [],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
            )
            .unwrap(),
            (1, 1)
        );
        assert!(conn
            .execute(
                "UPDATE agent_audit_state SET event_count = event_count + 2",
                [],
            )
            .is_err());
        assert!(conn
            .execute(
                "UPDATE agent_audit_events SET recorded_at = ?1",
                [101_u64.to_be_bytes()],
            )
            .is_err());
        assert!(conn.execute("DELETE FROM agent_audit_events", []).is_err());
        assert!(conn
            .execute("DELETE FROM agent_audit_deliveries", [])
            .is_err());

        let tx = conn.transaction().unwrap();
        let second_delivery = 2_u64.to_be_bytes();
        let second_event = 2_u64.to_be_bytes();
        tx.execute(
            "INSERT INTO agent_audit_deliveries(
                 manifest_id, supervisor_id, delivery_id,
                 first_event_id, last_event_id, event_count
             ) VALUES (?1, ?2, ?3, ?4, ?4, 1)",
            rusqlite::params![&manifest, &supervisor, &second_delivery, &second_event,],
        )
        .unwrap();
        let mut invalid_record = record.clone();
        invalid_record[0] = 2;
        assert!(tx
            .execute(
                "INSERT INTO agent_audit_events(
                     manifest_id, supervisor_id, event_id, delivery_id,
                     batch_index, recorded_at, record_version, record
                 ) VALUES (?1, ?2, ?3, ?4, 0, ?5, 1, ?6)",
                rusqlite::params![
                    &manifest,
                    &supervisor,
                    &second_event,
                    &second_delivery,
                    &recorded_at,
                    &invalid_record,
                ],
            )
            .is_err());
        tx.rollback().unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT delivery_count, event_count FROM agent_audit_state WHERE id = 1",
                [],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
            )
            .unwrap(),
            (1, 1)
        );
        assert_eq!(
            conn.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            16
        );
    }

    #[test]
    fn meta_v14_preflights_seed_profile_identities_before_distinct_aggregation() {
        let canonical = zephium_core::ids::ProfileId::from(u128::MAX).to_string();
        let mut embedded_nul = canonical.clone();
        embedded_nul.replace_range(13..14, "\0");
        let invalid_profiles = [
            "short".to_owned(),
            canonical.to_ascii_lowercase(),
            format!("{}I", &canonical[..25]),
            format!("8{}", &canonical[1..]),
            embedded_nul,
            "A".repeat(1024 * 1024),
        ];

        for invalid_profile in invalid_profiles {
            let mut conn = Connection::open_in_memory().unwrap();
            apply(&mut conn, &META[..13]).unwrap();
            insert_native_ownership_test_row(&conn, 1, 2, 1, "acquire", "native_may_own").unwrap();
            conn.pragma_update(None, "ignore_check_constraints", true)
                .unwrap();
            conn.execute(
                "UPDATE extension_native_ownership_journal
                 SET profile_id = ?1 WHERE operation = 1",
                [&invalid_profile],
            )
            .unwrap();
            conn.pragma_update(None, "ignore_check_constraints", false)
                .unwrap();
            conn.execute(
                "UPDATE extension_native_ownership_journal_state
                 SET revision = 3,
                     operation_high_water = 1,
                     native_incarnation_high_water = 1",
                [],
            )
            .unwrap();

            let error = apply(&mut conn, META).unwrap_err();
            assert!(
                error.to_string().contains("invalid profile identity"),
                "unexpected migration error for corrupt profile identity: {error}"
            );
            assert_eq!(
                conn.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                    .unwrap(),
                13
            );
            assert!(conn
                .prepare("SELECT * FROM extension_native_namespace_obligations")
                .is_err());
        }
    }

    #[test]
    fn meta_v14_refuses_unanchored_contradictory_or_over_capacity_seeds_atomically() {
        let possible_owner = |conn: &Connection, profile: &str, operation: i64| {
            insert_native_ownership_test_row(
                conn,
                operation,
                2,
                operation,
                "acquire",
                "native_may_own",
            )
            .unwrap();
            conn.execute(
                "UPDATE extension_native_ownership_journal
                 SET profile_id = ?2 WHERE operation = ?1",
                rusqlite::params![operation, profile],
            )
            .unwrap();
        };

        let mut unanchored = Connection::open_in_memory().unwrap();
        apply(&mut unanchored, &META[..13]).unwrap();
        let profile = zephium_core::ids::ProfileId::from(1).to_string();
        possible_owner(&unanchored, &profile, 1);
        unanchored
            .execute(
                "UPDATE extension_native_ownership_journal_state
                 SET revision = 3, operation_high_water = 1,
                     native_incarnation_high_water = 1",
                [],
            )
            .unwrap();
        assert!(apply(&mut unanchored, META).is_err());
        assert_eq!(
            unanchored
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            13
        );
        assert!(unanchored
            .prepare("SELECT * FROM extension_native_namespace_obligations")
            .is_err());

        let mut contradicted = Connection::open_in_memory().unwrap();
        apply(&mut contradicted, &META[..13]).unwrap();
        possible_owner(&contradicted, &profile, 1);
        contradicted
            .execute(
                "UPDATE extension_native_ownership_journal_state
                 SET revision = 3, operation_high_water = 1,
                     native_incarnation_high_water = 1",
                [],
            )
            .unwrap();
        contradicted
            .execute(
                "INSERT INTO profile_deletion_journal(
                     profile_id, authorized_at, native_erasure_verified
                 ) VALUES (?1, 1, 1)",
                [&profile],
            )
            .unwrap();
        assert!(apply(&mut contradicted, META).is_err());
        assert_eq!(
            contradicted
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            13
        );

        let mut over_capacity = Connection::open_in_memory().unwrap();
        apply(&mut over_capacity, &META[..13]).unwrap();
        for operation in 1_i64..=129 {
            let profile = zephium_core::ids::ProfileId::from(operation as u128).to_string();
            over_capacity
                .execute(
                    "INSERT INTO profiles(id, name, kind, position)
                     VALUES (?1, 'Fixture', 'named', ?2)",
                    rusqlite::params![&profile, operation],
                )
                .unwrap();
            possible_owner(&over_capacity, &profile, operation);
        }
        over_capacity
            .execute(
                "UPDATE extension_native_ownership_journal_state
                 SET revision = 259, operation_high_water = 129,
                     native_incarnation_high_water = 129",
                [],
            )
            .unwrap();
        assert!(apply(&mut over_capacity, META).is_err());
        assert_eq!(
            over_capacity
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            13
        );
    }

    #[test]
    fn meta_v11_schema_rejects_invalid_native_ownership_state() {
        let mut conn = Connection::open_in_memory().unwrap();
        apply(&mut conn, META).unwrap();

        assert!(conn
            .execute(
                "UPDATE extension_native_ownership_journal_state SET revision = 0",
                [],
            )
            .is_err());
        for update in [
            "revision = 2",
            "revision = 2, operation_high_water = 1",
            "revision = 1, operation_high_water = 1, native_incarnation_high_water = 1",
            "revision = 2, operation_high_water = 2, native_incarnation_high_water = 2",
        ] {
            assert!(
                conn.execute(
                    &format!("UPDATE extension_native_ownership_journal_state SET {update}"),
                    [],
                )
                .is_err(),
                "accepted impossible state tuple: {update}"
            );
        }
        assert!(
            insert_native_ownership_test_row(&conn, 1, 1, 1, "acquire", "native_owned",).is_err()
        );
        assert!(insert_native_ownership_test_row(
            &conn,
            1,
            1,
            2,
            "acquire",
            "native_absent_preparing",
        )
        .is_err());
    }

    #[test]
    fn meta_v11_trigger_enforces_reachable_native_ownership_history_range() {
        let mut conn = Connection::open_in_memory().unwrap();
        apply(&mut conn, &META[..11]).unwrap();
        insert_native_ownership_test_row(&conn, 1, 1, 1, "acquire", "native_absent_preparing")
            .unwrap();

        assert!(conn
            .execute(
                "UPDATE extension_native_ownership_journal_state
                 SET revision = 3,
                     operation_high_water = 1,
                     native_incarnation_high_water = 1",
                [],
            )
            .is_err());
        conn.execute(
            "UPDATE extension_native_ownership_journal_state
             SET revision = 2,
                 operation_high_water = 1,
                 native_incarnation_high_water = 1",
            [],
        )
        .unwrap();
        conn.execute("DELETE FROM extension_native_ownership_journal", [])
            .unwrap();
        for impossible in [3_i64, 8] {
            assert!(
                conn.execute(
                    "UPDATE extension_native_ownership_journal_state SET revision = ?1",
                    [impossible],
                )
                .is_err(),
                "accepted unreachable cleared history revision {impossible}"
            );
        }
        conn.execute(
            "UPDATE extension_native_ownership_journal_state SET revision = 4",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE extension_native_ownership_journal_state SET revision = 7",
            [],
        )
        .unwrap();
    }

    #[test]
    fn profile_v7_adds_an_exact_empty_source_only_userscript_catalog() {
        let mut conn = Connection::open_in_memory().unwrap();
        apply(&mut conn, &PROFILE[..6]).unwrap();

        apply(&mut conn, &PROFILE[..7]).unwrap();

        let state: (i64, i64) = conn
            .query_row("SELECT id, revision FROM userscript_catalog", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .unwrap();
        assert_eq!(state, (1, 1));
        let scripts: i64 = conn
            .query_row("SELECT count(*) FROM userscripts", [], |row| row.get(0))
            .unwrap();
        assert_eq!(scripts, 0);
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 7);
    }

    #[test]
    fn profile_v7_schema_rejects_unbounded_or_malformed_userscript_rows() {
        let mut conn = Connection::open_in_memory().unwrap();
        apply(&mut conn, PROFILE).unwrap();
        let valid_source =
            "// ==UserScript==\n// @name A\n// @match https://example.com/*\n// ==/UserScript==\n";
        for (id, revision, enabled, format, source, digest) in [
            ("short", 1_i64, 0_i64, 1_i64, valid_source, vec![0_u8; 32]),
            (
                "01J00000000000000000000000",
                0,
                0,
                1,
                valid_source,
                vec![0_u8; 32],
            ),
            (
                "01J00000000000000000000001",
                1,
                2,
                1,
                valid_source,
                vec![0_u8; 32],
            ),
            (
                "01J00000000000000000000002",
                1,
                0,
                0,
                valid_source,
                vec![0_u8; 32],
            ),
            ("01J00000000000000000000003", 1, 0, 1, "", vec![0_u8; 32]),
            (
                "01J00000000000000000000004",
                1,
                0,
                1,
                valid_source,
                vec![0_u8; 31],
            ),
        ] {
            assert!(
                conn.execute(
                    "INSERT INTO userscripts(
                         id, revision, enabled, metadata_format, source, source_sha256_v1
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    rusqlite::params![id, revision, enabled, format, source, digest],
                )
                .is_err(),
                "accepted invalid userscript row {id}"
            );
        }

        assert!(conn
            .execute(
                "UPDATE userscript_catalog SET revision = 0 WHERE id = 1",
                []
            )
            .is_err());
        let oversized = "x".repeat(zephium_core::ports::engine::MAX_USER_SCRIPT_BYTES + 1);
        for source in [oversized.as_str(), "valid prefix\0invalid body"] {
            assert!(conn
                .execute(
                    "INSERT INTO userscripts(
                         id, revision, enabled, metadata_format, source, source_sha256_v1
                     ) VALUES (?1, 1, 0, 1, ?2, ?3)",
                    rusqlite::params!["01J00000000000000000000005", source, vec![0_u8; 32]],
                )
                .is_err());
        }
    }

    #[test]
    fn profile_v8_adds_an_exact_empty_page_permission_catalog() {
        let mut conn = Connection::open_in_memory().unwrap();
        apply(&mut conn, &PROFILE[..7]).unwrap();

        apply(&mut conn, &PROFILE[..8]).unwrap();

        let state: (i64, i64) = conn
            .query_row(
                "SELECT id, revision FROM page_permission_catalog",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(state, (1, 1));
        let grants: i64 = conn
            .query_row("SELECT count(*) FROM page_permission_grants", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(grants, 0);
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 8);
    }

    #[test]
    fn profile_v8_schema_rejects_unbounded_or_malformed_permission_rows() {
        let mut conn = Connection::open_in_memory().unwrap();
        apply(&mut conn, PROFILE).unwrap();
        for (id, revision, origin, kind, decision) in [
            ("short", 1_i64, "https://example.com", "camera", "allow"),
            (
                "01J00000000000000000000000",
                0,
                "https://example.com",
                "camera",
                "allow",
            ),
            ("01J00000000000000000000001", 1, "", "camera", "allow"),
            (
                "01J00000000000000000000002",
                1,
                "https://example.com",
                "unknown",
                "allow",
            ),
            (
                "01J00000000000000000000003",
                1,
                "https://example.com",
                "camera",
                "ask",
            ),
        ] {
            assert!(
                conn.execute(
                    "INSERT INTO page_permission_grants(
                         id, revision, origin, kind, decision
                     ) VALUES (?1, ?2, ?3, ?4, ?5)",
                    rusqlite::params![id, revision, origin, kind, decision],
                )
                .is_err(),
                "accepted invalid page-permission row {id}"
            );
        }
        for origin in ["x".repeat(513), "https://example.com\0suffix".into()] {
            assert!(conn
                .execute(
                    "INSERT INTO page_permission_grants(
                         id, revision, origin, kind, decision
                     ) VALUES ('01J00000000000000000000004', 1, ?1, 'camera', 'allow')",
                    [origin],
                )
                .is_err());
        }
        assert!(conn
            .execute(
                "UPDATE page_permission_catalog SET revision = 0 WHERE id = 1",
                [],
            )
            .is_err());
    }

    #[test]
    fn profile_v9_adds_an_exact_empty_extension_install_catalog() {
        let mut conn = Connection::open_in_memory().unwrap();
        apply(&mut conn, &PROFILE[..8]).unwrap();

        apply(&mut conn, &PROFILE[..9]).unwrap();

        let state: (i64, i64) = conn
            .query_row(
                "SELECT id, revision FROM extension_install_catalog",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(state, (1, 1));
        let installs: i64 = conn
            .query_row("SELECT count(*) FROM extension_installs", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(installs, 0);
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 9);
    }

    #[test]
    fn profile_v9_schema_rejects_malformed_structural_identity_rows() {
        let mut conn = Connection::open_in_memory().unwrap();
        apply(&mut conn, &PROFILE[..9]).unwrap();

        let cases = [
            (
                vec![1_u8; 15],
                1_i64,
                vec![2_u8; 32],
                vec![3_u8; 32],
                1_i64,
                vec![4_u8; 32],
                vec![5_u8; 32],
                vec![6_u8; 32],
                0_i64,
            ),
            (
                vec![1_u8; 16],
                0,
                vec![2_u8; 32],
                vec![3_u8; 32],
                1,
                vec![4_u8; 32],
                vec![5_u8; 32],
                vec![6_u8; 32],
                0,
            ),
            (
                vec![1_u8; 16],
                1,
                vec![2_u8; 31],
                vec![3_u8; 32],
                1,
                vec![4_u8; 32],
                vec![5_u8; 32],
                vec![6_u8; 32],
                0,
            ),
            (
                vec![1_u8; 16],
                1,
                vec![2_u8; 32],
                vec![3_u8; 33],
                1,
                vec![4_u8; 32],
                vec![5_u8; 32],
                vec![6_u8; 32],
                0,
            ),
            (
                vec![1_u8; 16],
                1,
                vec![2_u8; 32],
                vec![3_u8; 32],
                0,
                vec![4_u8; 32],
                vec![5_u8; 32],
                vec![6_u8; 32],
                0,
            ),
            (
                vec![1_u8; 16],
                1,
                vec![2_u8; 32],
                vec![3_u8; 32],
                1,
                vec![4_u8; 31],
                vec![5_u8; 32],
                vec![6_u8; 32],
                0,
            ),
            (
                vec![1_u8; 16],
                1,
                vec![2_u8; 32],
                vec![3_u8; 32],
                1,
                vec![4_u8; 32],
                vec![5_u8; 33],
                vec![6_u8; 32],
                0,
            ),
            (
                vec![1_u8; 16],
                1,
                vec![2_u8; 32],
                vec![3_u8; 32],
                1,
                vec![4_u8; 32],
                vec![5_u8; 32],
                vec![6_u8; 31],
                0,
            ),
            (
                vec![1_u8; 16],
                1,
                vec![2_u8; 32],
                vec![3_u8; 32],
                1,
                vec![4_u8; 32],
                vec![5_u8; 32],
                vec![6_u8; 32],
                2,
            ),
        ];
        for (
            id,
            revision,
            authority,
            key,
            package_revision,
            archive,
            manifest,
            tree,
            desired_enabled,
        ) in cases
        {
            assert!(
                conn.execute(
                    "INSERT INTO extension_installs(
                         id, revision, authority, package_key, package_revision,
                         archive_sha256, manifest_sha256, tree_sha256, desired_enabled
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    rusqlite::params![
                        id,
                        revision,
                        authority,
                        key,
                        package_revision,
                        archive,
                        manifest,
                        tree,
                        desired_enabled,
                    ],
                )
                .is_err(),
                "accepted malformed extension structural identity"
            );
        }
        assert!(conn
            .execute(
                "INSERT INTO extension_installs(
                     id, revision, authority, package_key, package_revision,
                     archive_sha256, manifest_sha256, tree_sha256, desired_enabled
                 ) VALUES ('not-a-blob-id', 1, ?1, ?2, 1, ?3, ?4, ?5, 0)",
                rusqlite::params![
                    vec![2_u8; 32],
                    vec![3_u8; 32],
                    vec![4_u8; 32],
                    vec![5_u8; 32],
                    vec![6_u8; 32]
                ],
            )
            .is_err());
        assert!(conn
            .execute(
                "UPDATE extension_install_catalog SET revision = 0 WHERE id = 1",
                [],
            )
            .is_err());
    }

    #[test]
    fn profile_v9_enforces_one_update_line_per_profile() {
        let mut conn = Connection::open_in_memory().unwrap();
        apply(&mut conn, &PROFILE[..9]).unwrap();
        for id in [vec![1_u8; 16], vec![2_u8; 16]] {
            let result = conn.execute(
                "INSERT INTO extension_installs(
                     id, revision, authority, package_key, package_revision,
                     archive_sha256, manifest_sha256, tree_sha256, desired_enabled
                 ) VALUES (?1, 1, ?2, ?3, 1, ?4, ?5, ?6, 0)",
                rusqlite::params![
                    id,
                    vec![7_u8; 32],
                    vec![8_u8; 32],
                    vec![9_u8; 32],
                    vec![10_u8; 32],
                    vec![11_u8; 32],
                ],
            );
            if id == vec![1_u8; 16] {
                assert!(result.is_ok());
            } else {
                assert!(result.is_err());
            }
        }
    }

    #[test]
    fn manifest_rejects_an_unexpected_trigger_before_migration_dml() {
        let mut conn = Connection::open_in_memory().unwrap();
        apply(&mut conn, &META[..3]).unwrap();
        conn.execute(
            "INSERT INTO settings(key, value) VALUES ('oversized', ?1)",
            ["x".repeat(65_537)],
        )
        .unwrap();
        conn.execute_batch(
            "CREATE TRIGGER hostile_setting_delete AFTER DELETE ON settings BEGIN
                 DELETE FROM session_snapshot;
             END;",
        )
        .unwrap();

        let error = apply(&mut conn, META).unwrap_err().to_string();
        assert!(error.contains("sqlite_schema"), "{error}");
        let retained: i64 = conn
            .query_row(
                "SELECT count(*) FROM settings WHERE key = 'oversized'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(retained, 1, "migration DML ran before schema validation");
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 3);
    }

    #[test]
    fn manifest_rejects_replaced_expected_trigger_sql() {
        let mut conn = Connection::open_in_memory().unwrap();
        apply(&mut conn, PROFILE).unwrap();
        conn.execute_batch(
            "DROP TRIGGER history_ai;
             CREATE TRIGGER history_ai AFTER INSERT ON history BEGIN
                 SELECT 1;
             END;",
        )
        .unwrap();

        let error = apply(&mut conn, PROFILE).unwrap_err().to_string();
        assert!(error.contains("sqlite_schema"), "{error}");
    }

    #[test]
    fn manifest_rejects_unexpected_views_and_indexes() {
        for ddl in [
            "CREATE VIEW hostile_view AS SELECT key FROM settings;",
            "CREATE INDEX hostile_index ON settings(value);",
        ] {
            let mut conn = Connection::open_in_memory().unwrap();
            apply(&mut conn, META).unwrap();
            conn.execute_batch(ddl).unwrap();
            assert!(apply(&mut conn, META).is_err(), "accepted {ddl}");
        }
    }

    #[test]
    fn manifest_rejects_unexpected_analyze_statistics() {
        let mut conn = Connection::open_in_memory().unwrap();
        apply(&mut conn, PROFILE).unwrap();
        conn.execute_batch("ANALYZE;").unwrap();

        let error = apply(&mut conn, PROFILE).unwrap_err().to_string();
        assert!(error.contains("sqlite_schema"), "{error}");
    }

    #[test]
    fn schema_normalization_preserves_token_literal_and_comment_boundaries() {
        assert_eq!(
            normalize_schema_sql("CREATE   TABLE x (a   INT)"),
            normalize_schema_sql("create table x (a int)")
        );
        assert_ne!(
            normalize_schema_sql("CREATE TABLE x(a IN T)"),
            normalize_schema_sql("CREATE TABLE x(a INT)")
        );
        assert_ne!(
            normalize_schema_sql("CREATE TABLE x(a CHECK(a = 'a b'))"),
            normalize_schema_sql("CREATE TABLE x(a CHECK(a = 'ab'))")
        );
        assert_ne!(
            normalize_schema_sql("CREATE TABLE x(a INT /* Keep Case */)"),
            normalize_schema_sql("CREATE TABLE x(a INT /* keep case */)")
        );
    }

    #[test]
    fn bundled_sqlite_has_fts5() {
        let mut conn = Connection::open_in_memory().unwrap();
        apply(&mut conn, PROFILE).unwrap();
        conn.execute(
            "INSERT INTO history(url, title, visited_at) VALUES('https://example.com/', 'Example Site', 1)",
            [],
        )
        .unwrap();
        let hits: i64 = conn
            .query_row(
                "SELECT count(*) FROM history_fts WHERE history_fts MATCH 'example'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(hits, 1);
    }

    #[test]
    fn profile_quota_migration_keeps_only_newest_rows() {
        let mut conn = Connection::open_in_memory().unwrap();
        apply(&mut conn, &PROFILE[..3]).unwrap();
        conn.execute_batch(
            "WITH RECURSIVE n(x) AS (
                 VALUES(1) UNION ALL SELECT x + 1 FROM n WHERE x < 50002
             )
             INSERT INTO history(url, title, visited_at)
             SELECT printf('https://example.com/%d', x), 'Title', x FROM n;
             WITH RECURSIVE n(x) AS (
                 VALUES(1) UNION ALL SELECT x + 1 FROM n WHERE x < 514
             )
             INSERT INTO favicons(origin, content_type, icon, fetched_at)
             SELECT printf('https://example%d.com', x), 'image/png', x'00', x FROM n;",
        )
        .unwrap();

        apply(&mut conn, PROFILE).unwrap();
        let (history, oldest): (i64, i64) = conn
            .query_row("SELECT count(*), min(visited_at) FROM history", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .unwrap();
        let (favicons, oldest_icon): (i64, i64) = conn
            .query_row(
                "SELECT count(*), min(fetched_at) FROM favicons",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        let fts: i64 = conn
            .query_row("SELECT count(*) FROM history_fts", [], |row| row.get(0))
            .unwrap();
        assert_eq!((history, oldest, fts), (50_000, 3, 50_000));
        assert_eq!((favicons, oldest_icon), (512, 3));
    }

    #[test]
    fn migrations_prune_oversized_settings_history_and_icons() {
        let mut meta = Connection::open_in_memory().unwrap();
        apply(&mut meta, &META[..3]).unwrap();
        let oversized_value = "v".repeat(65_537);
        meta.execute(
            "INSERT INTO settings(key, value) VALUES (?1, ?2)",
            rusqlite::params!["k", oversized_value],
        )
        .unwrap();
        apply(&mut meta, META).unwrap();
        let meta_settings: i64 = meta
            .query_row("SELECT count(*) FROM settings", [], |row| row.get(0))
            .unwrap();
        assert_eq!(meta_settings, 0);

        let mut profile = Connection::open_in_memory().unwrap();
        apply(&mut profile, &PROFILE[..4]).unwrap();
        let oversized_url = "x".repeat(8193);
        profile
            .execute(
                "INSERT INTO history(url, title, visited_at) VALUES (?1, 'T', 1)",
                [&oversized_url],
            )
            .unwrap();
        let oversized_icon = vec![0_u8; 262_145];
        profile
            .execute(
                "INSERT INTO favicons(origin, content_type, icon, fetched_at)
                 VALUES ('https://example.com', 'image/png', ?1, 1)",
                [oversized_icon],
            )
            .unwrap();
        apply(&mut profile, PROFILE).unwrap();
        let history: i64 = profile
            .query_row("SELECT count(*) FROM history", [], |row| row.get(0))
            .unwrap();
        let favicons: i64 = profile
            .query_row("SELECT count(*) FROM favicons", [], |row| row.get(0))
            .unwrap();
        assert_eq!((history, favicons), (0, 0));
    }
}
