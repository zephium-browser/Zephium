//! A cache of what the folder holds: identities, titles, previews, pins,
//! links and a full-text index. Losing it loses pins and identities, never a
//! note; everything else is rebuilt from the files.
use std::path::Path;

use rusqlite::{params, Connection, OpenFlags, OptionalExtension, Row as SqlRow};
use zephium_core::notes::NoteSummary;

const SCHEMA: i64 = 1;
const MAX_RECEIPTS: i64 = 512;
const MAX_BACKLINKS: i64 = 100;
const NAMED_AFTER: &str = "named-after:";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub id: String,
    pub path: String,
    /// Where a trashed note lived, so restoring puts it back.
    pub origin: Option<String>,
    pub trashed_at: Option<i64>,
    pub pinned: bool,
    pub title: String,
    /// Whether `title` came from the note's leading heading rather than its
    /// file name. Only then does the file name follow the title.
    pub headed: bool,
    pub preview: String,
    pub revision: String,
    pub size: i64,
    pub modified: i64,
    pub created: i64,
    pub identity: Option<(u64, u64)>,
    pub editable: bool,
    pub crlf: bool,
    pub bom: bool,
}

impl Row {
    pub fn trashed(&self) -> bool {
        self.trashed_at.is_some()
    }

    pub fn summary(&self) -> NoteSummary {
        NoteSummary {
            id: self.id.clone(),
            revision: self.revision.clone(),
            title: self.title.clone(),
            preview: self.preview.clone(),
            pinned: self.pinned,
            trashed: self.trashed(),
            editable: self.editable,
            created_at: self.created.to_string(),
            modified_at: self.modified.to_string(),
            path: self.origin.clone().unwrap_or_else(|| self.path.clone()),
        }
    }
}

pub struct Index {
    conn: Connection,
}

const COLUMNS: &str = "id,path,origin,trashed_at,pinned,title,headed,preview,revision,size,modified,created,dev,ino,editable,crlf,bom";

fn row(sql: &SqlRow) -> rusqlite::Result<Row> {
    let dev: Option<i64> = sql.get(12)?;
    let ino: Option<i64> = sql.get(13)?;
    Ok(Row {
        id: sql.get(0)?,
        path: sql.get(1)?,
        origin: sql.get(2)?,
        trashed_at: sql.get(3)?,
        pinned: sql.get(4)?,
        title: sql.get(5)?,
        headed: sql.get(6)?,
        preview: sql.get(7)?,
        revision: sql.get(8)?,
        size: sql.get(9)?,
        modified: sql.get(10)?,
        created: sql.get(11)?,
        // Stored bit-for-bit; SQLite integers are signed.
        identity: dev.zip(ino).map(|(dev, ino)| (dev as u64, ino as u64)),
        editable: sql.get(14)?,
        crlf: sql.get(15)?,
        bom: sql.get(16)?,
    })
}

/// A full-text query matching every word as a prefix, or `None` when the
/// search has no words.
fn match_expression(search: &str) -> Option<String> {
    let terms: Vec<String> = search
        .split_whitespace()
        .take(8)
        .map(|word| format!("\"{}\"*", word.replace('"', "\"\"")))
        .collect();
    (!terms.is_empty()).then(|| terms.join(" "))
}

pub struct Page {
    pub rows: Vec<Row>,
    pub next: Option<String>,
}

impl Index {
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        if let Ok(meta) = std::fs::symlink_metadata(path) {
            if !meta.file_type().is_file() {
                return Err(rusqlite::Error::InvalidPath(path.to_path_buf()));
            }
        }
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX
                | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )?;
        Self::configure(conn)
    }

    #[cfg(test)]
    pub fn memory() -> Self {
        Self::configure(Connection::open_in_memory().unwrap()).unwrap()
    }

    fn configure(conn: Connection) -> rusqlite::Result<Self> {
        // The files are the durable record, so an index write lost to a power
        // cut is recovered by the next scan; WAL with NORMAL sync avoids an
        // fsync per keystroke-driven save. Every save also rewrites a note's
        // searchable text: FAST secure-delete still clears what is removed,
        // except where clearing it would cost a write of its own.
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             PRAGMA foreign_keys=ON;
             PRAGMA secure_delete=FAST;
             PRAGMA trusted_schema=OFF;
             PRAGMA cache_size=-512;",
        )?;
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        match version {
            0 => conn.execute_batch(
                "BEGIN;
                 CREATE TABLE notes(
                    id TEXT PRIMARY KEY NOT NULL CHECK(length(id)=26),
                    path TEXT NOT NULL,
                    path_key TEXT NOT NULL UNIQUE,
                    origin TEXT,
                    trashed_at INTEGER,
                    pinned INTEGER NOT NULL CHECK(pinned IN (0,1)),
                    title TEXT NOT NULL,
                    title_key TEXT NOT NULL,
                    stem_key TEXT NOT NULL,
                    headed INTEGER NOT NULL CHECK(headed IN (0,1)),
                    preview TEXT NOT NULL,
                    revision TEXT NOT NULL,
                    size INTEGER NOT NULL,
                    modified INTEGER NOT NULL,
                    created INTEGER NOT NULL,
                    dev INTEGER,
                    ino INTEGER,
                    editable INTEGER NOT NULL CHECK(editable IN (0,1)),
                    crlf INTEGER NOT NULL CHECK(crlf IN (0,1)),
                    bom INTEGER NOT NULL CHECK(bom IN (0,1))
                 ) STRICT;
                 CREATE INDEX notes_listing ON notes(trashed_at IS NOT NULL, pinned DESC, modified DESC, id DESC);
                 CREATE INDEX notes_stem ON notes(stem_key);
                 CREATE INDEX notes_title ON notes(title_key);
                 CREATE TABLE note_links(
                    source TEXT NOT NULL REFERENCES notes(id) ON DELETE CASCADE,
                    target TEXT NOT NULL,
                    PRIMARY KEY(source, target)
                 ) STRICT, WITHOUT ROWID;
                 CREATE INDEX note_links_target ON note_links(target);
                 CREATE VIRTUAL TABLE note_text USING fts5(
                    title, body, content='', contentless_delete=1,
                    tokenize='unicode61 remove_diacritics 2', prefix='2 3'
                 );
                 CREATE TABLE receipts(
                    request_id TEXT PRIMARY KEY NOT NULL,
                    id TEXT NOT NULL,
                    sequence INTEGER NOT NULL
                 ) STRICT;
                 CREATE TABLE settings(key TEXT PRIMARY KEY NOT NULL, value TEXT NOT NULL) STRICT;
                 PRAGMA user_version=1;
                 COMMIT;",
            )?,
            SCHEMA => {}
            _ => return Err(rusqlite::Error::InvalidQuery),
        }
        Ok(Self { conn })
    }

    pub fn setting(&self, key: &str) -> rusqlite::Result<Option<String>> {
        self.conn
            .query_row("SELECT value FROM settings WHERE key=?1", [key], |r| {
                r.get(0)
            })
            .optional()
    }

    pub fn set_setting(&self, key: &str, value: &str) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO settings(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            [key, value],
        )?;
        Ok(())
    }

    /// The title a note's file was named after, kept while a retitle has
    /// not yet been allowed to rename it.
    pub fn named_after(&self, id: &str) -> rusqlite::Result<Option<String>> {
        self.setting(&format!("{NAMED_AFTER}{id}"))
    }

    pub fn remember_named_after(&self, id: &str, title: &str) -> rusqlite::Result<()> {
        self.set_setting(&format!("{NAMED_AFTER}{id}"), title)
    }

    pub fn forget_named_after(&self, id: &str) -> rusqlite::Result<()> {
        self.conn.execute(
            "DELETE FROM settings WHERE key=?1",
            [format!("{NAMED_AFTER}{id}")],
        )?;
        Ok(())
    }

    pub fn rows(&self) -> rusqlite::Result<Vec<Row>> {
        let mut statement = self.conn.prepare(&format!("SELECT {COLUMNS} FROM notes"))?;
        let rows = statement.query_map([], row)?.collect();
        rows
    }

    pub fn get(&self, id: &str) -> rusqlite::Result<Option<Row>> {
        self.conn
            .prepare_cached(&format!("SELECT {COLUMNS} FROM notes WHERE id=?1"))?
            .query_row([id], row)
            .optional()
    }

    pub fn row_at(&self, path_key: &str) -> rusqlite::Result<Option<Row>> {
        self.conn
            .prepare_cached(&format!("SELECT {COLUMNS} FROM notes WHERE path_key=?1"))?
            .query_row([path_key], row)
            .optional()
    }

    pub fn count(&self) -> rusqlite::Result<usize> {
        self.conn
            .query_row("SELECT count(*) FROM notes", [], |r| r.get::<_, i64>(0))
            .map(|n| n as usize)
    }

    pub fn taken(&self, path_key: &str) -> rusqlite::Result<bool> {
        self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM notes WHERE path_key=?1)",
            [path_key],
            |r| r.get(0),
        )
    }

    /// Writes a note's row, its searchable text and its outgoing links as one
    /// change, so search and backlinks never disagree with the listing.
    pub fn put(&mut self, note: &Row, text: &str, links: &[String]) -> rusqlite::Result<()> {
        let tx = self.conn.transaction()?;
        let previous: Option<i64> = tx
            .query_row("SELECT rowid FROM notes WHERE id=?1", [&note.id], |r| {
                r.get(0)
            })
            .optional()?;
        if let Some(rowid) = previous {
            tx.execute("DELETE FROM note_text WHERE rowid=?1", [rowid])?;
        }
        let stem = crate::names::stem_of(note.origin.as_deref().unwrap_or(&note.path));
        tx.execute(
            "INSERT INTO notes(id,path,path_key,origin,trashed_at,pinned,title,title_key,stem_key,headed,preview,revision,size,modified,created,dev,ino,editable,crlf,bom)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20)
             ON CONFLICT(id) DO UPDATE SET path=excluded.path,path_key=excluded.path_key,origin=excluded.origin,
               trashed_at=excluded.trashed_at,pinned=excluded.pinned,title=excluded.title,title_key=excluded.title_key,
               stem_key=excluded.stem_key,headed=excluded.headed,preview=excluded.preview,revision=excluded.revision,
               size=excluded.size,modified=excluded.modified,created=excluded.created,dev=excluded.dev,ino=excluded.ino,
               editable=excluded.editable,crlf=excluded.crlf,bom=excluded.bom",
            params![
                note.id,
                note.path,
                crate::names::path_key(&note.path),
                note.origin,
                note.trashed_at,
                note.pinned,
                note.title,
                crate::markdown::link_key(&note.title),
                crate::markdown::link_key(stem),
                note.headed,
                note.preview,
                note.revision,
                note.size,
                note.modified,
                note.created,
                note.identity.map(|(dev, _)| dev as i64),
                note.identity.map(|(_, ino)| ino as i64),
                note.editable,
                note.crlf,
                note.bom,
            ],
        )?;
        let rowid: i64 = tx.query_row("SELECT rowid FROM notes WHERE id=?1", [&note.id], |r| {
            r.get(0)
        })?;
        tx.execute(
            "INSERT INTO note_text(rowid,title,body) VALUES(?1,?2,?3)",
            params![rowid, note.title, text],
        )?;
        tx.execute("DELETE FROM note_links WHERE source=?1", [&note.id])?;
        for link in links {
            tx.execute(
                "INSERT OR IGNORE INTO note_links(source,target) VALUES(?1,?2)",
                [&note.id, link],
            )?;
        }
        tx.commit()
    }

    /// Updates only where a note is and how it is kept, not what it says.
    pub fn relocate(&self, note: &Row) -> rusqlite::Result<()> {
        let stem = crate::names::stem_of(note.origin.as_deref().unwrap_or(&note.path));
        self.conn.execute(
            "UPDATE notes SET path=?2,path_key=?3,origin=?4,trashed_at=?5,pinned=?6,stem_key=?7,size=?8,modified=?9,dev=?10,ino=?11 WHERE id=?1",
            params![
                note.id,
                note.path,
                crate::names::path_key(&note.path),
                note.origin,
                note.trashed_at,
                note.pinned,
                crate::markdown::link_key(stem),
                note.size,
                note.modified,
                note.identity.map(|(dev, _)| dev as i64),
                note.identity.map(|(_, ino)| ino as i64),
            ],
        )?;
        Ok(())
    }

    pub fn remove(&mut self, id: &str) -> rusqlite::Result<()> {
        let tx = self.conn.transaction()?;
        if let Some(rowid) = tx
            .query_row("SELECT rowid FROM notes WHERE id=?1", [id], |r| {
                r.get::<_, i64>(0)
            })
            .optional()?
        {
            tx.execute("DELETE FROM note_text WHERE rowid=?1", [rowid])?;
        }
        tx.execute("DELETE FROM notes WHERE id=?1", [id])?;
        tx.execute("DELETE FROM receipts WHERE id=?1", [id])?;
        tx.execute(
            "DELETE FROM settings WHERE key=?1",
            [format!("{NAMED_AFTER}{id}")],
        )?;
        tx.commit()
    }

    pub fn list(
        &self,
        search: &str,
        trashed: bool,
        after: Option<&str>,
        limit: u16,
    ) -> rusqlite::Result<Option<Page>> {
        let limit = i64::from(limit);
        if let Some(expression) = match_expression(search) {
            // Ranked results page by position; there is no stable key to resume from.
            let offset = match after {
                None => 0,
                Some(cursor) => match cursor
                    .strip_prefix("s:")
                    .and_then(|n| n.parse::<i64>().ok())
                {
                    Some(offset) if (0..=100_000).contains(&offset) => offset,
                    _ => return Ok(None),
                },
            };
            let mut statement = self.conn.prepare_cached(&format!(
                "SELECT {} FROM note_text JOIN notes n ON n.rowid=note_text.rowid
                 WHERE note_text MATCH ?1 AND (n.trashed_at IS NOT NULL)=?2
                 ORDER BY bm25(note_text, 8.0, 1.0), n.modified DESC LIMIT ?3 OFFSET ?4",
                COLUMNS
                    .split(',')
                    .map(|c| format!("n.{c}"))
                    .collect::<Vec<_>>()
                    .join(",")
            ))?;
            let mut rows: Vec<Row> = statement
                .query_map(params![expression, trashed, limit + 1, offset], row)?
                .collect::<rusqlite::Result<_>>()?;
            let more = rows.len() as i64 > limit;
            rows.truncate(limit as usize);
            return Ok(Some(Page {
                next: more.then(|| format!("s:{}", offset + limit)),
                rows,
            }));
        }
        let (pinned, modified, id) = match after {
            None => (2, i64::MAX, String::new()),
            Some(cursor) => {
                let mut parts = cursor.splitn(3, ':');
                match (
                    parts.next().and_then(|p| p.parse::<i64>().ok()),
                    parts.next().and_then(|m| m.parse::<i64>().ok()),
                    parts.next(),
                ) {
                    (Some(pinned @ 0..=1), Some(modified), Some(id))
                        if zephium_core::resources::valid_id(id) =>
                    {
                        (pinned, modified, id.to_string())
                    }
                    _ => return Ok(None),
                }
            }
        };
        let mut statement = self.conn.prepare_cached(&format!(
            "SELECT {COLUMNS} FROM notes WHERE (trashed_at IS NOT NULL)=?1
               AND (pinned<?2 OR (pinned=?2 AND (modified<?3 OR (modified=?3 AND id<?4))))
             ORDER BY trashed_at IS NOT NULL, pinned DESC, modified DESC, id DESC LIMIT ?5"
        ))?;
        let mut rows: Vec<Row> = statement
            .query_map(params![trashed, pinned, modified, id, limit + 1], row)?
            .collect::<rusqlite::Result<_>>()?;
        let more = rows.len() as i64 > limit;
        rows.truncate(limit as usize);
        let next = if more {
            rows.last()
                .map(|last| format!("{}:{}:{}", u8::from(last.pinned), last.modified, last.id))
        } else {
            None
        };
        Ok(Some(Page { rows, next }))
    }

    /// The live note a `[[target]]` names: by file name first, as other
    /// Markdown tools resolve it, then by title.
    pub fn resolve(&self, key: &str) -> rusqlite::Result<Option<Row>> {
        self.conn
            .prepare_cached(&format!(
                "SELECT {COLUMNS} FROM notes WHERE trashed_at IS NULL AND (stem_key=?1 OR title_key=?1)
                 ORDER BY stem_key=?1 DESC, modified DESC LIMIT 1"
            ))?
            .query_row([key], row)
            .optional()
    }

    pub fn backlinks(&self, id: &str) -> rusqlite::Result<Vec<Row>> {
        let mut statement = self.conn.prepare_cached(&format!(
            "SELECT {} FROM notes n WHERE n.trashed_at IS NULL AND n.id<>?1 AND EXISTS(
               SELECT 1 FROM note_links l, notes me WHERE me.id=?1 AND l.source=n.id
                 AND (l.target=me.stem_key OR l.target=me.title_key))
             ORDER BY n.modified DESC LIMIT ?2",
            COLUMNS
                .split(',')
                .map(|c| format!("n.{c}"))
                .collect::<Vec<_>>()
                .join(",")
        ))?;
        let rows = statement
            .query_map(params![id, MAX_BACKLINKS], row)?
            .collect();
        rows
    }

    pub fn receipt(&self, request_id: &str) -> rusqlite::Result<Option<String>> {
        self.conn
            .query_row(
                "SELECT id FROM receipts WHERE request_id=?1",
                [request_id],
                |r| r.get(0),
            )
            .optional()
    }

    pub fn remember_receipt(&self, request_id: &str, id: &str) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO receipts(request_id,id,sequence)
             VALUES(?1,?2,coalesce((SELECT max(sequence) FROM receipts),0)+1)",
            [request_id, id],
        )?;
        self.conn.execute(
            "DELETE FROM receipts WHERE sequence <= (SELECT max(sequence) FROM receipts)-?1",
            [MAX_RECEIPTS],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn note(id: &str, path: &str, title: &str, modified: i64) -> Row {
        Row {
            id: id.into(),
            path: path.into(),
            origin: None,
            trashed_at: None,
            pinned: false,
            title: title.into(),
            headed: true,
            preview: String::new(),
            revision: "0".repeat(32),
            size: 1,
            modified,
            created: modified,
            identity: Some((1, modified as u64)),
            editable: true,
            crlf: false,
            bom: false,
        }
    }

    const A: &str = "01J9ZQ3V6Q4M8Y2K7T5R1N0B3A";
    const B: &str = "01J9ZQ3V6Q4M8Y2K7T5R1N0B3B";
    const C: &str = "01J9ZQ3V6Q4M8Y2K7T5R1N0B3C";

    #[test]
    fn lists_pinned_then_recent_across_pages() {
        let mut index = Index::memory();
        index.put(&note(A, "A.md", "A", 1), "", &[]).unwrap();
        index.put(&note(B, "B.md", "B", 3), "", &[]).unwrap();
        let mut pinned = note(C, "C.md", "C", 2);
        pinned.pinned = true;
        index.put(&pinned, "", &[]).unwrap();
        let first = index.list("", false, None, 2).unwrap().unwrap();
        assert_eq!(
            first.rows.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            [C, B]
        );
        let second = index
            .list("", false, first.next.as_deref(), 2)
            .unwrap()
            .unwrap();
        assert_eq!(
            second
                .rows
                .iter()
                .map(|r| r.id.as_str())
                .collect::<Vec<_>>(),
            [A]
        );
        assert!(second.next.is_none());
        assert!(index.list("", false, Some("9:0:x"), 2).unwrap().is_none());
    }

    #[test]
    fn searches_bodies_by_word_prefix_and_forgets_removed_text() {
        let mut index = Index::memory();
        index
            .put(
                &note(A, "Trip.md", "Trip", 1),
                "Trip Book the train to Lisbon",
                &[],
            )
            .unwrap();
        index
            .put(&note(B, "Food.md", "Food", 2), "Food Lisbon pastries", &[])
            .unwrap();
        let hits = |index: &Index, q: &str| {
            index
                .list(q, false, None, 10)
                .unwrap()
                .unwrap()
                .rows
                .into_iter()
                .map(|r| r.id)
                .collect::<Vec<_>>()
        };
        assert_eq!(hits(&index, "lisb"), [B, A]);
        assert_eq!(hits(&index, "trai lis"), [A]);
        assert_eq!(hits(&index, "\"quoted"), Vec::<String>::new());
        index
            .put(&note(A, "Trip.md", "Trip", 3), "Trip by plane", &[])
            .unwrap();
        assert_eq!(hits(&index, "train"), Vec::<String>::new());
        index.remove(B).unwrap();
        assert_eq!(hits(&index, "pastries"), Vec::<String>::new());
    }

    #[test]
    fn links_resolve_by_file_name_then_title_and_report_backlinks() {
        let mut index = Index::memory();
        index
            .put(&note(A, "Plans.md", "Summer plans", 1), "", &[])
            .unwrap();
        index
            .put(
                &note(B, "Ideas.md", "Ideas", 2),
                "",
                &["plans".into(), "missing".into()],
            )
            .unwrap();
        index
            .put(
                &note(C, "Other.md", "Other", 3),
                "",
                &["summer plans".into()],
            )
            .unwrap();
        assert_eq!(index.resolve("plans").unwrap().unwrap().id, A);
        assert_eq!(index.resolve("summer plans").unwrap().unwrap().id, A);
        assert!(index.resolve("missing").unwrap().is_none());
        let backlinks: Vec<String> = index
            .backlinks(A)
            .unwrap()
            .into_iter()
            .map(|r| r.id)
            .collect();
        assert_eq!(backlinks, [C, B]);
    }

    #[test]
    fn receipts_are_bounded() {
        let index = Index::memory();
        for n in 0..(MAX_RECEIPTS + 5) {
            index
                .remember_receipt(&format!("request-{n:012}"), A)
                .unwrap();
        }
        assert!(index.receipt("request-000000000000").unwrap().is_none());
        assert_eq!(
            index
                .receipt(&format!("request-{:012}", MAX_RECEIPTS + 4))
                .unwrap()
                .as_deref(),
            Some(A)
        );
    }
}
