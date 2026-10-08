//! One profile's notes: its folder, the index describing it, and every
//! operation the browser performs on them. Runs on the notes thread only.
use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use zephium_core::ids::ResourceId;
use zephium_core::notes::{
    NoteCall, NoteError, NoteRecord, NoteResponse, NoteTarget, MAX_NOTES, MAX_NOTE_BYTES,
};

use crate::folder::{Contents, Entry, FileMeta, Folder};
use crate::index::{Index, Row};
use crate::markdown::{link_key, outline};
use crate::names::{self, path_key, stem_for, stem_of, unique_path, TRASH};

/// Trashed notes are deleted for good after this long, as in Recently Deleted.
pub const TRASH_RETENTION_MS: i64 = 30 * 24 * 60 * 60 * 1000;

#[derive(Debug)]
pub enum Failure {
    Io(io::Error),
    Index(rusqlite::Error),
}

impl From<io::Error> for Failure {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<rusqlite::Error> for Failure {
    fn from(error: rusqlite::Error) -> Self {
        Self::Index(error)
    }
}

type Result<T> = std::result::Result<T, Failure>;

/// What a call or a scan changed, for the browser's change event.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Changes {
    pub ids: Vec<String>,
    /// Notes appeared or disappeared, so listings must reload.
    pub reset: bool,
    /// Link keys that may lead somewhere else now.
    pub links: Vec<String>,
}

impl Changes {
    fn note(&mut self, id: &str) {
        if !self.ids.iter().any(|known| known == id) {
            self.ids.push(id.to_string());
        }
    }

    /// A note stopped or started answering to `names`, as a title or a stem.
    fn relink(&mut self, names: &[&str]) {
        for name in names {
            let key = link_key(name);
            if !key.is_empty() && !self.links.contains(&key) {
                self.links.push(key);
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty() && !self.reset
    }
}

pub fn revision(bytes: &[u8]) -> String {
    Sha256::digest(bytes)[..16]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

const BOM: &[u8] = b"\xEF\xBB\xBF";

struct Decoded {
    markdown: String,
    crlf: bool,
    bom: bool,
    utf8: bool,
}

/// The editor sees `\n` line endings and no byte-order mark; a file written
/// with either gets them back on save.
fn decode(bytes: &[u8]) -> Decoded {
    let bom = bytes.starts_with(BOM);
    let body = if bom { &bytes[BOM.len()..] } else { bytes };
    let (text, utf8) = match std::str::from_utf8(body) {
        Ok(text) => (text.to_string(), true),
        Err(_) => (String::from_utf8_lossy(body).into_owned(), false),
    };
    let crlf = text.contains("\r\n");
    Decoded {
        markdown: if crlf {
            text.replace("\r\n", "\n")
        } else {
            text
        },
        crlf,
        bom,
        utf8,
    }
}

fn encode(markdown: &str, crlf: bool, bom: bool) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(markdown.len() + 3);
    if bom {
        bytes.extend_from_slice(BOM);
    }
    if crlf {
        bytes.extend_from_slice(
            markdown
                .replace("\r\n", "\n")
                .replace('\n', "\r\n")
                .as_bytes(),
        );
    } else {
        bytes.extend_from_slice(markdown.as_bytes());
    }
    bytes
}

/// Everything the index keeps about one file, apart from where it came from.
struct Description {
    row: Row,
    text: String,
    links: Vec<String>,
}

fn describe(id: &str, path: &str, contents: &Contents) -> Description {
    let complete = contents.complete();
    let decoded = decode(&contents.bytes);
    let outline = outline(&decoded.markdown);
    let headed = outline.heading.is_some();
    Description {
        row: Row {
            id: id.to_string(),
            path: path.to_string(),
            origin: None,
            trashed_at: None,
            pinned: false,
            title: outline.heading.unwrap_or_else(|| stem_of(path).to_string()),
            headed,
            preview: outline.preview,
            revision: revision(&contents.bytes),
            size: contents.meta.size as i64,
            modified: contents.meta.modified,
            created: contents.meta.created,
            identity: contents.meta.identity,
            editable: complete && decoded.utf8,
            crlf: decoded.crlf,
            bom: decoded.bom,
        },
        text: outline.text,
        links: outline.links,
    }
}

fn unchanged(row: &Row, meta: &FileMeta) -> bool {
    row.size == meta.size as i64
        && row.modified == meta.modified
        && (row.identity.is_none() || meta.identity.is_none() || row.identity == meta.identity)
}

/// Whether a file stem is the one `title` would produce, allowing for the
/// ` 2`, ` 3`… a collision added.
fn follows(stem: &str, title: &str) -> bool {
    let expected = stem_for(title);
    stem == expected
        || stem
            .strip_prefix(expected.as_str())
            .and_then(|rest| rest.strip_prefix(' '))
            .is_some_and(|n| n.parse::<u32>().is_ok_and(|n| n >= 2))
}

fn directory_of(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(directory, _)| directory)
}

fn error(error: NoteError) -> NoteResponse {
    NoteResponse::Error { error }
}

pub struct Library {
    folder: Folder,
    index: Index,
}

impl Library {
    /// `base` is the application data directory; the profile's notes live in
    /// `notes/<profile>/Notes` and its index beside them, outside the folder
    /// so that moving or syncing the notes never carries a database along.
    pub fn open(base: &Path, profile: &str) -> Result<Self> {
        let folder = Folder::open(base, &["notes", profile, "Notes"])?;
        let index_path = folder
            .root()
            .parent()
            .map(|parent| parent.join("index.sqlite"))
            .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?;
        let index = Index::open(&index_path)?;
        Ok(Self { folder, index })
    }

    #[cfg(test)]
    pub fn in_folder(folder: Folder) -> Self {
        Self {
            folder,
            index: Index::memory(),
        }
    }

    pub fn index(&self) -> &Index {
        &self.index
    }

    pub fn root(&self) -> &Path {
        self.folder.root()
    }

    pub fn absolute(&self, id: &str) -> Option<PathBuf> {
        let row = self.index.get(id).ok()??;
        self.folder.resolve(&row.path).ok()
    }

    /// Whether file-system events for `paths` describe anything the index
    /// does not already know. The browser's own saves come back as events;
    /// answering them from the index keeps a save from costing a full scan.
    pub fn stale(&self, paths: &[PathBuf]) -> bool {
        paths.iter().any(|absolute| {
            let Ok(relative) = absolute.strip_prefix(self.folder.root()) else {
                return true;
            };
            let parts: Option<Vec<&str>> = relative
                .components()
                .map(|c| c.as_os_str().to_str())
                .collect();
            let Some(parts) = parts else { return true };
            let path = parts.join("/");
            if path.is_empty() {
                return true;
            }
            let name = parts.last().copied().unwrap_or_default();
            // Save temporaries, and hidden files other than the trash, are never notes.
            if name.ends_with(".zephium-save") || (name.starts_with('.') && path != TRASH) {
                return false;
            }
            if !names::valid_path(&path) {
                // A directory event: something inside may have moved.
                return !path.ends_with(names::EXTENSION);
            }
            match (self.index.row_at(&path_key(&path)), self.folder.stat(&path)) {
                (Ok(Some(row)), Ok(meta)) => !unchanged(&row, &meta),
                (Ok(None), Err(_)) => false,
                _ => true,
            }
        })
    }

    /// Brings the index in line with the folder: new, edited, moved and
    /// removed files, including changes made while the browser was closed.
    pub fn reconcile(&mut self, now: i64) -> Result<Changes> {
        let (notes, trash) = self.folder.scan()?;
        let rows = self.index.rows()?;
        let mut changes = Changes::default();
        let mut by_key: HashMap<String, Row> = rows
            .into_iter()
            .map(|row| (path_key(&row.path), row))
            .collect();
        let mut fresh: Vec<(Entry, bool)> = Vec::new();
        let mut kept: HashSet<String> = HashSet::new();
        for (entry, trashed) in notes
            .into_iter()
            .map(|entry| (entry, false))
            .chain(trash.into_iter().map(|entry| (entry, true)))
        {
            match by_key.remove(&path_key(&entry.path)) {
                Some(row) => {
                    kept.insert(row.id.clone());
                    if unchanged(&row, &entry.meta) {
                        if row.path != entry.path {
                            self.index.relocate(&Row {
                                path: entry.path,
                                ..row.clone()
                            })?;
                            changes.note(&row.id);
                        }
                        continue;
                    }
                    if let Ok(contents) = self.folder.read(&entry.path) {
                        self.store(&row, &entry.path, &contents, trashed, now)?;
                        changes.note(&row.id);
                    }
                }
                None => fresh.push((entry, trashed)),
            }
        }
        let mut missing: Vec<Row> = by_key.into_values().collect();
        let mut count = kept.len();
        for (entry, trashed) in fresh {
            let Ok(contents) = self.folder.read(&entry.path) else {
                continue;
            };
            let digest = revision(&contents.bytes);
            // A file that moved keeps its identity, found by inode or content.
            let moved = missing
                .iter()
                .position(|row| row.identity.is_some() && row.identity == entry.meta.identity)
                .or_else(|| missing.iter().position(|row| row.revision == digest));
            let previous = match moved {
                Some(position) => missing.swap_remove(position),
                None => {
                    if count >= MAX_NOTES {
                        continue;
                    }
                    Row {
                        id: ResourceId::generate().to_string(),
                        ..describe("", &entry.path, &contents).row
                    }
                }
            };
            self.store(&previous, &entry.path, &contents, trashed, now)?;
            count += 1;
            changes.note(&previous.id);
            changes.reset = true;
        }
        for row in missing {
            self.index.remove(&row.id)?;
            changes.note(&row.id);
            changes.reset = true;
        }
        Ok(changes)
    }

    /// Indexes a file's contents under `previous`'s identity, pin and trash
    /// history, with `trashed` saying where the file now is.
    fn store(
        &mut self,
        previous: &Row,
        path: &str,
        contents: &Contents,
        trashed: bool,
        now: i64,
    ) -> Result<Row> {
        let mut description = describe(&previous.id, path, contents);
        let row = &mut description.row;
        row.pinned = previous.pinned;
        // A save replaces the file, which gives it a new birth time.
        row.created = previous.created;
        match (trashed, previous.trashed()) {
            (true, true) => {
                row.origin = previous.origin.clone();
                row.trashed_at = previous.trashed_at;
            }
            (true, false) => {
                // Put in the trash by something else; restore to where it was.
                let origin = if previous.path.is_empty() || previous.path.starts_with(TRASH) {
                    path.strip_prefix(".trash/").unwrap_or(path).to_string()
                } else {
                    previous.path.clone()
                };
                row.origin = Some(origin);
                row.trashed_at = Some(now);
            }
            (false, _) => {}
        }
        self.index.put(row, &description.text, &description.links)?;
        Ok(description.row)
    }

    /// Deletes trashed notes older than the retention period.
    pub fn expire_trash(&mut self, now: i64) -> Result<Changes> {
        let mut changes = Changes::default();
        for row in self.index.rows()? {
            if row
                .trashed_at
                .is_some_and(|at| now - at > TRASH_RETENTION_MS)
            {
                match self.folder.remove(&row.path) {
                    Ok(()) => {}
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error.into()),
                }
                self.index.remove(&row.id)?;
                changes.note(&row.id);
                changes.reset = true;
            }
        }
        Ok(changes)
    }

    fn taken(&self, path: &str, own: Option<&str>) -> bool {
        let key = path_key(path);
        if own.is_some_and(|own| path_key(own) == key) {
            return false;
        }
        self.index.taken(&key).unwrap_or(true) || self.folder.exists(path)
    }

    fn record(&mut self, row: &Row) -> Result<NoteResponse> {
        let contents = self.folder.read(&row.path)?;
        let row = if unchanged(row, &contents.meta) && revision(&contents.bytes) == row.revision {
            row.clone()
        } else {
            self.store(
                row,
                &row.path.clone(),
                &contents,
                row.trashed(),
                row.trashed_at.unwrap_or(0),
            )?
        };
        if !contents.complete() {
            return Ok(error(NoteError::TooLarge));
        }
        Ok(NoteResponse::Record {
            record: NoteRecord {
                summary: row.summary(),
                markdown: decode(&contents.bytes).markdown,
            },
        })
    }

    pub fn call(&mut self, call: NoteCall, now: i64) -> (NoteResponse, Changes) {
        let mut changes = Changes::default();
        let response = match self.dispatch(call, now, &mut changes) {
            Ok(response) => response,
            Err(Failure::Io(failure)) if failure.kind() == io::ErrorKind::NotFound => {
                // The file went away underneath the index; the scan that
                // follows the watcher's event will settle the listing.
                error(NoteError::NotFound)
            }
            Err(Failure::Io(failure)) => {
                tracing::warn!(%failure, "note file operation failed");
                error(NoteError::Unavailable)
            }
            Err(Failure::Index(failure)) => {
                tracing::warn!(%failure, "notes index failed");
                error(NoteError::Unavailable)
            }
        };
        (response, changes)
    }

    fn dispatch(
        &mut self,
        call: NoteCall,
        now: i64,
        changes: &mut Changes,
    ) -> Result<NoteResponse> {
        Ok(match call {
            NoteCall::List { query } => match self.index.list(
                &query.search,
                query.trashed,
                query.after.as_deref(),
                query.limit,
            )? {
                Some(page) => NoteResponse::Page {
                    items: page.rows.iter().map(Row::summary).collect(),
                    next: page.next,
                },
                None => error(NoteError::Invalid),
            },
            NoteCall::Get { id } => match self.index.get(&id)? {
                Some(row) => self.record(&row)?,
                None => error(NoteError::NotFound),
            },
            NoteCall::Create {
                request_id,
                markdown,
            } => self.create(&request_id, &markdown, now, changes)?,
            NoteCall::Write {
                request_id,
                id,
                base_revision,
                markdown,
                settle,
            } => self.write(&request_id, &id, &base_revision, &markdown, settle, changes)?,
            NoteCall::SetPinned { id, pinned } => match self.index.get(&id)? {
                Some(row) => {
                    let row = Row { pinned, ..row };
                    self.index.relocate(&row)?;
                    changes.note(&id);
                    changes.reset = true;
                    NoteResponse::Applied {
                        request_id: String::new(),
                        summary: row.summary(),
                    }
                }
                None => error(NoteError::NotFound),
            },
            NoteCall::Trash { id } => self.trash(&id, now, changes)?,
            NoteCall::Restore { id } => self.restore(&id, changes)?,
            NoteCall::Delete { id } => match self.index.get(&id)? {
                Some(row) if row.trashed() => {
                    match self.folder.remove(&row.path) {
                        Err(error) if error.kind() != io::ErrorKind::NotFound => {
                            return Err(error.into())
                        }
                        _ => {}
                    }
                    self.index.remove(&id)?;
                    changes.note(&id);
                    changes.reset = true;
                    NoteResponse::Done
                }
                Some(_) => error(NoteError::Invalid),
                None => error(NoteError::NotFound),
            },
            NoteCall::Resolve { targets } => {
                let mut items = Vec::with_capacity(targets.len());
                for target in targets {
                    let note = self
                        .index
                        .resolve(&link_key(&target))?
                        .map(|row| row.summary());
                    items.push(NoteTarget { target, note });
                }
                NoteResponse::Targets { items }
            }
            NoteCall::Backlinks { id } => NoteResponse::Page {
                items: self
                    .index
                    .backlinks(&id)?
                    .iter()
                    .map(Row::summary)
                    .collect(),
                next: None,
            },
            // Carried out by the service, which owns the platform hook.
            NoteCall::Reveal { .. } => error(NoteError::Invalid),
        })
    }

    fn create(
        &mut self,
        request_id: &str,
        markdown: &str,
        now: i64,
        changes: &mut Changes,
    ) -> Result<NoteResponse> {
        if let Some(id) = self.index.receipt(request_id)? {
            if let Some(row) = self.index.get(&id)? {
                return Ok(NoteResponse::Applied {
                    request_id: request_id.to_string(),
                    summary: row.summary(),
                });
            }
        }
        if self.index.count()? >= MAX_NOTES {
            return Ok(error(NoteError::Capacity));
        }
        let title = outline(markdown).heading;
        let stem = stem_for(title.as_deref().unwrap_or("Untitled"));
        let Some(path) = unique_path("", &stem, |candidate| self.taken(candidate, None)) else {
            return Ok(error(NoteError::Capacity));
        };
        let bytes = encode(markdown, false, false);
        let meta = self.folder.create(&path, &bytes, None)?;
        let contents = Contents { bytes, meta };
        let id = ResourceId::generate().to_string();
        let row = describe(&id, &path, &contents).row;
        let row = self.store(&row, &path, &contents, false, now)?;
        self.index.remember_receipt(request_id, &id)?;
        changes.note(&id);
        changes.reset = true;
        Ok(NoteResponse::Applied {
            request_id: request_id.to_string(),
            summary: row.summary(),
        })
    }

    fn write(
        &mut self,
        request_id: &str,
        id: &str,
        base: &str,
        markdown: &str,
        settle: bool,
        changes: &mut Changes,
    ) -> Result<NoteResponse> {
        let Some(row) = self.index.get(id)? else {
            return Ok(error(NoteError::NotFound));
        };
        if row.trashed() || !row.editable {
            return Ok(error(NoteError::ReadOnly));
        }
        let bytes = encode(markdown, row.crlf, row.bom);
        if bytes.len() > MAX_NOTE_BYTES {
            return Ok(error(NoteError::TooLarge));
        }
        let next = revision(&bytes);
        let meta = self.folder.stat(&row.path)?;
        let current = if unchanged(&row, &meta) {
            row.revision.clone()
        } else {
            revision(&self.folder.read(&row.path)?.bytes)
        };
        let applied = |summary| NoteResponse::Applied {
            request_id: request_id.to_string(),
            summary,
        };
        if current == next {
            // Already saved, by this request's first attempt or an identical edit.
            let contents = self.folder.read(&row.path)?;
            let stored = self.store(&row, &row.path.clone(), &contents, false, 0)?;
            let stored = if settle {
                self.follow_title(&row, stored, changes)?
            } else {
                stored
            };
            return Ok(applied(stored.summary()));
        }
        if current != base {
            let contents = self.folder.read(&row.path)?;
            let row = self.store(&row, &row.path.clone(), &contents, false, 0)?;
            changes.note(id);
            return Ok(NoteResponse::Conflict {
                current: NoteRecord {
                    summary: row.summary(),
                    markdown: decode(&contents.bytes).markdown,
                },
            });
        }
        let meta = self.folder.replace(&row.path, &bytes)?;
        let contents = Contents { bytes, meta };
        let stored = self.store(&row, &row.path.clone(), &contents, false, 0)?;
        changes.note(id);
        // Only a new title changes where links lead; an ordinary edit is
        // news for this note alone.
        if stored.title != row.title {
            changes.relink(&[&row.title, &stored.title]);
        }
        let stored = if settle {
            self.follow_title(&row, stored, changes)?
        } else {
            if stored.title != row.title
                && follows(stem_of(&row.path), &row.title)
                && self.index.named_after(id)?.is_none()
            {
                self.index.remember_named_after(id, &row.title)?;
            }
            stored
        };
        Ok(applied(stored.summary()))
    }

    /// Renames a file after its note's title, while the file still carries
    /// the title it was named after: the one before this write, or the one
    /// before a retitle that was not yet allowed to rename it. A name
    /// chosen elsewhere is left alone.
    fn follow_title(&mut self, before: &Row, stored: Row, changes: &mut Changes) -> Result<Row> {
        let named = self.index.named_after(&stored.id)?;
        if named.is_some() {
            self.index.forget_named_after(&stored.id)?;
        }
        let stem = stem_of(&stored.path);
        if !stored.headed
            || !follows(stem, named.as_deref().unwrap_or(&before.title))
            || follows(stem, &stored.title)
        {
            return Ok(stored);
        }
        let directory = directory_of(&stored.path).to_string();
        let Some(target) = unique_path(&directory, &stem_for(&stored.title), |candidate| {
            self.taken(candidate, Some(&stored.path))
        }) else {
            return Ok(stored);
        };
        if target == stored.path {
            return Ok(stored);
        }
        let Ok(meta) = self.folder.rename(&stored.path, &target) else {
            return Ok(stored);
        };
        let old = stem.to_string();
        let row = Row {
            path: target,
            size: meta.size as i64,
            modified: meta.modified,
            identity: meta.identity,
            ..stored
        };
        self.index.relocate(&row)?;
        changes.note(&row.id);
        changes.relink(&[&old, stem_of(&row.path)]);
        Ok(row)
    }

    fn trash(&mut self, id: &str, now: i64, changes: &mut Changes) -> Result<NoteResponse> {
        let Some(row) = self.index.get(id)? else {
            return Ok(error(NoteError::NotFound));
        };
        if row.trashed() {
            return Ok(NoteResponse::Applied {
                request_id: String::new(),
                summary: row.summary(),
            });
        }
        let Some(target) = unique_path(TRASH, stem_of(&row.path), |candidate| {
            self.taken(candidate, None)
        }) else {
            return Ok(error(NoteError::Capacity));
        };
        let meta = self.folder.rename(&row.path, &target)?;
        let row = Row {
            origin: Some(row.path.clone()),
            path: target,
            trashed_at: Some(now),
            size: meta.size as i64,
            modified: meta.modified,
            identity: meta.identity,
            ..row
        };
        self.index.relocate(&row)?;
        changes.note(id);
        changes.reset = true;
        Ok(NoteResponse::Applied {
            request_id: String::new(),
            summary: row.summary(),
        })
    }

    fn restore(&mut self, id: &str, changes: &mut Changes) -> Result<NoteResponse> {
        let Some(row) = self.index.get(id)? else {
            return Ok(error(NoteError::NotFound));
        };
        if !row.trashed() {
            return Ok(NoteResponse::Applied {
                request_id: String::new(),
                summary: row.summary(),
            });
        }
        let origin = row
            .origin
            .clone()
            .filter(|origin| names::valid_path(origin) && !origin.starts_with(TRASH))
            .unwrap_or_else(|| row.path.trim_start_matches(".trash/").to_string());
        let Some(target) = unique_path(directory_of(&origin), stem_of(&origin), |candidate| {
            self.taken(candidate, None)
        }) else {
            return Ok(error(NoteError::Capacity));
        };
        let meta = self.folder.rename(&row.path, &target)?;
        let row = Row {
            origin: None,
            path: target,
            trashed_at: None,
            size: meta.size as i64,
            modified: meta.modified,
            identity: meta.identity,
            ..row
        };
        self.index.relocate(&row)?;
        changes.note(id);
        changes.reset = true;
        Ok(NoteResponse::Applied {
            request_id: String::new(),
            summary: row.summary(),
        })
    }

    /// Writes an imported note under a known identity, keeping its dates.
    pub fn import(
        &mut self,
        id: &str,
        markdown: &str,
        pinned: bool,
        trashed: bool,
        modified: std::time::SystemTime,
        now: i64,
    ) -> Result<()> {
        if self.index.get(id)?.is_some() {
            return Ok(());
        }
        let stem = stem_for(outline(markdown).heading.as_deref().unwrap_or("Untitled"));
        let directory = if trashed { TRASH } else { "" };
        let Some(path) = unique_path(directory, &stem, |candidate| self.taken(candidate, None))
        else {
            return Err(io::Error::from(io::ErrorKind::AlreadyExists).into());
        };
        let bytes = encode(markdown, false, false);
        let meta = self.folder.create(&path, &bytes, Some(modified))?;
        let contents = Contents { bytes, meta };
        let previous = Row {
            pinned,
            origin: trashed.then(|| path.trim_start_matches(".trash/").to_string()),
            trashed_at: trashed.then_some(now),
            ..describe(id, &path, &contents).row
        };
        self.store(&previous, &path, &contents, trashed, now)?;
        Ok(())
    }
}

#[cfg(test)]
#[path = "library/tests.rs"]
mod tests;
