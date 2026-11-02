//! Per-profile bookmarks: one ordered tree of folders and links, bounded in
//! size and depth. Every write runs in one transaction and re-checks the tree
//! it changes, so a stale view in chrome can never corrupt it.

use super::*;

use zephium_core::bookmarks::{
    clean_title, BookmarkChange, BookmarkEntry, BookmarkFailure, BookmarkNode, BookmarkReply,
    BookmarkRequest, ImportNode, MAX_BOOKMARKS, MAX_DEPTH, MAX_ENTRIES, MAX_LISTING,
    MAX_QUERY_BYTES, MAX_SEARCH_RESULTS, MAX_SUBTREE,
};

/// Words of a search that count; the rest are ignored.
const MAX_QUERY_WORDS: usize = 8;

const NODE_COLUMNS: &str = "b.id, b.parent_id, b.url, b.title,
     (SELECT count(*) FROM bookmarks c WHERE c.parent_id = b.id)";

/// An entry's place among its siblings, counted rather than stored: positions
/// keep gaps where bookmarks were removed.
const ENTRY_COLUMNS: &str = "b.id, b.parent_id,
     (SELECT count(*) FROM bookmarks s WHERE s.parent_id IS b.parent_id
        AND (s.position < b.position OR (s.position = b.position AND s.id < b.id))),
     b.url, b.title, b.added_at";

fn entry_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<BookmarkEntry> {
    Ok(BookmarkEntry {
        id: row.get(0)?,
        parent: row.get(1)?,
        index: row.get(2)?,
        url: row.get(3)?,
        title: row.get(4)?,
        added_at: row.get(5)?,
    })
}

fn entry(conn: &Connection, id: i64) -> rusqlite::Result<Option<BookmarkEntry>> {
    conn.prepare_cached(&format!(
        "SELECT {ENTRY_COLUMNS} FROM bookmarks b WHERE b.id = ?1"
    ))?
    .query_row([id], entry_row)
    .optional()
}

fn node(row: &rusqlite::Row<'_>) -> rusqlite::Result<BookmarkNode> {
    Ok(BookmarkNode {
        id: row.get(0)?,
        parent: row.get(1)?,
        url: row.get(2)?,
        title: row.get(3)?,
        children: row.get(4)?,
    })
}

/// A failure the request caused, as opposed to one SQLite reported.
struct Refused(BookmarkFailure);

type Outcome<T> = Result<Result<T, Refused>, rusqlite::Error>;

impl Hub {
    pub(crate) fn bookmarks(
        &mut self,
        profile: ProfileId,
        request: BookmarkRequest,
    ) -> BookmarkReply {
        if !self.registry.contains(&profile)
            || self.degraded_profiles.contains(&profile)
            || self.recovery_required.is_some()
        {
            return BookmarkReply::Failed(BookmarkFailure::Unavailable);
        }
        let result = self
            .profile_conn(profile)
            .and_then(|conn| apply(conn, request));
        match result {
            Ok(Ok(reply)) => reply,
            Ok(Err(Refused(failure))) => BookmarkReply::Failed(failure),
            Err(e) => {
                eprintln!("store: bookmarks failed for profile {profile}: {e}");
                BookmarkReply::Failed(BookmarkFailure::Unavailable)
            }
        }
    }

    /// [`Self::bookmarks`], with what a write changed as Chrome's events
    /// describe it. The actor runs one request at a time, so the reads around
    /// a write see exactly its effect.
    pub(crate) fn bookmarks_observed(
        &mut self,
        profile: ProfileId,
        request: BookmarkRequest,
    ) -> (BookmarkReply, Option<BookmarkChange>) {
        let target = match &request {
            BookmarkRequest::Rename { id, .. }
            | BookmarkRequest::Move { id, .. }
            | BookmarkRequest::Relocate { id, .. }
            | BookmarkRequest::Update { id, .. }
            | BookmarkRequest::Remove { id }
            | BookmarkRequest::Delete { id, .. } => Some(*id),
            _ => None,
        };
        let existing_link = match &request {
            BookmarkRequest::AddLink {
                url,
                if_absent: true,
                ..
            } => Some(url.clone()),
            _ => None,
        };
        let read = |hub: &mut Self, f: &dyn Fn(&Connection) -> rusqlite::Result<Option<i64>>| {
            hub.profile_conn(profile)
                .ok()
                .and_then(|conn| f(conn).ok().flatten())
        };
        let before = target.and_then(|id| {
            self.profile_conn(profile)
                .ok()
                .and_then(|conn| entry(conn, id).ok().flatten())
        });
        let existed = existing_link.and_then(|url| {
            read(self, &|conn| {
                conn.query_row(
                    "SELECT id FROM bookmarks WHERE url = ?1 ORDER BY id LIMIT 1",
                    [&url],
                    |row| row.get(0),
                )
                .optional()
            })
        });
        let imported = matches!(request, BookmarkRequest::Import { .. });
        let creates = matches!(request, BookmarkRequest::Create { .. });
        let reply = self.bookmarks(profile, request);
        let after = |hub: &mut Self, id: i64| {
            hub.profile_conn(profile)
                .ok()
                .and_then(|conn| entry(conn, id).ok().flatten())
        };
        let change = match (&reply, before) {
            (BookmarkReply::Added(id), _) if existed != Some(*id) => {
                after(self, *id).map(BookmarkChange::Created)
            }
            (BookmarkReply::Entries(entries), None) if creates => {
                entries.first().cloned().map(BookmarkChange::Created)
            }
            (BookmarkReply::Imported(added), _) if imported && *added > 0 => {
                Some(BookmarkChange::Imported)
            }
            (BookmarkReply::Done | BookmarkReply::Entries(_), Some(before)) => {
                match after(self, before.id) {
                    None => Some(BookmarkChange::Removed(before)),
                    Some(now) if now.parent != before.parent || now.index != before.index => {
                        Some(BookmarkChange::Moved {
                            id: now.id,
                            parent: now.parent,
                            index: now.index,
                            old_parent: before.parent,
                            old_index: before.index,
                        })
                    }
                    Some(now) if now.title != before.title || now.url != before.url => {
                        Some(BookmarkChange::Changed {
                            id: now.id,
                            title: now.title,
                            url: now.url,
                        })
                    }
                    Some(_) => None,
                }
            }
            _ => None,
        };
        (reply, change)
    }
}

fn apply(conn: &mut Connection, request: BookmarkRequest) -> Outcome<BookmarkReply> {
    match request {
        BookmarkRequest::Children { parent } => {
            if let Some(parent) = parent {
                match folder(conn, parent)? {
                    Ok(()) => {}
                    Err(refused) => return Ok(Err(refused)),
                }
            }
            let mut rows = conn.prepare_cached(&format!(
                "SELECT {NODE_COLUMNS} FROM bookmarks b WHERE b.parent_id IS ?1
                 ORDER BY b.position, b.id LIMIT ?2"
            ))?;
            let nodes = rows
                .query_map(params![parent, MAX_LISTING], node)?
                .collect::<rusqlite::Result<_>>()?;
            Ok(Ok(BookmarkReply::Nodes(nodes)))
        }
        BookmarkRequest::Path { id } => {
            let mut rows = conn.prepare_cached(&format!(
                "WITH RECURSIVE up(id, depth) AS (
                     SELECT ?1, 0
                     UNION ALL
                     SELECT b.parent_id, up.depth + 1 FROM bookmarks b JOIN up ON b.id = up.id
                     WHERE b.parent_id IS NOT NULL AND up.depth < ?2
                 )
                 SELECT {NODE_COLUMNS} FROM bookmarks b JOIN up ON b.id = up.id
                 ORDER BY up.depth DESC"
            ))?;
            let path: Vec<BookmarkNode> = rows
                .query_map(params![id, MAX_DEPTH as i64], node)?
                .collect::<rusqlite::Result<_>>()?;
            if path.last().is_none_or(|last| last.id != id) {
                return Ok(Err(Refused(BookmarkFailure::Missing)));
            }
            Ok(Ok(BookmarkReply::Nodes(path)))
        }
        BookmarkRequest::Search { query } => {
            let query = query.trim();
            if query.is_empty() || query.len() > MAX_QUERY_BYTES {
                return Ok(Err(Refused(BookmarkFailure::Invalid)));
            }
            // Every word must appear in the title or the address, in any
            // order, as the launcher scores a match.
            let patterns: Vec<String> = query
                .split_whitespace()
                .take(MAX_QUERY_WORDS)
                .map(|word| {
                    format!(
                        "%{}%",
                        word.replace('\\', "\\\\")
                            .replace('%', "\\%")
                            .replace('_', "\\_")
                    )
                })
                .collect();
            let matches = (1..=patterns.len())
                .map(|at| {
                    format!(" AND (b.title LIKE ?{at} ESCAPE '\\' OR b.url LIKE ?{at} ESCAPE '\\')")
                })
                .collect::<String>();
            let limit = MAX_SEARCH_RESULTS;
            let mut rows = conn.prepare_cached(&format!(
                "SELECT {NODE_COLUMNS} FROM bookmarks b
                 WHERE b.url IS NOT NULL{matches}
                 ORDER BY b.added_at DESC, b.id DESC LIMIT {limit}"
            ))?;
            let nodes = rows
                .query_map(rusqlite::params_from_iter(&patterns), node)?
                .collect::<rusqlite::Result<_>>()?;
            Ok(Ok(BookmarkReply::Nodes(nodes)))
        }
        BookmarkRequest::All => {
            let mut rows = conn.prepare_cached(
                "SELECT b.id, b.parent_id, b.url, b.title, 0 FROM bookmarks b
                 ORDER BY b.parent_id, b.position, b.id LIMIT ?1",
            )?;
            let nodes = rows
                .query_map(params![MAX_BOOKMARKS], node)?
                .collect::<rusqlite::Result<_>>()?;
            Ok(Ok(BookmarkReply::Nodes(nodes)))
        }
        BookmarkRequest::AddLink {
            parent,
            title,
            url,
            if_absent,
        } => {
            if !zephium_core::navigation::is_allowed_str(&url) {
                return Ok(Err(Refused(BookmarkFailure::Invalid)));
            }
            let title = match clean_title(&title) {
                empty if empty.is_empty() => clean_title(&url),
                title => title,
            };
            let tx = conn.transaction()?;
            if if_absent {
                let existing = tx
                    .query_row(
                        "SELECT id FROM bookmarks WHERE url = ?1 ORDER BY id LIMIT 1",
                        [&url],
                        |row| row.get::<_, i64>(0),
                    )
                    .optional()?;
                if let Some(existing) = existing {
                    return Ok(Ok(BookmarkReply::Added(existing)));
                }
            }
            let added = insert(&tx, parent, &title, Some(&url))?;
            if added.is_ok() {
                tx.commit()?;
            }
            Ok(added.map(BookmarkReply::Added))
        }
        BookmarkRequest::AddFolder { parent, title } => {
            let title = clean_title(&title);
            if title.is_empty() {
                return Ok(Err(Refused(BookmarkFailure::Invalid)));
            }
            let tx = conn.transaction()?;
            let added = insert(&tx, parent, &title, None)?;
            if added.is_ok() {
                tx.commit()?;
            }
            Ok(added.map(BookmarkReply::Added))
        }
        BookmarkRequest::Rename { id, title } => {
            let title = clean_title(&title);
            if title.is_empty() {
                return Ok(Err(Refused(BookmarkFailure::Invalid)));
            }
            let changed = conn.execute(
                "UPDATE bookmarks SET title = ?2 WHERE id = ?1",
                params![id, title],
            )?;
            Ok(if changed == 0 {
                Err(Refused(BookmarkFailure::Missing))
            } else {
                Ok(BookmarkReply::Done)
            })
        }
        BookmarkRequest::Move { id, parent, index } => {
            let tx = conn.transaction()?;
            let moved = relocate(&tx, id, parent, index)?;
            if moved.is_ok() {
                tx.commit()?;
            }
            Ok(moved.map(|()| BookmarkReply::Done))
        }
        BookmarkRequest::Import { folder, nodes } => {
            let title = clean_title(&folder);
            if title.is_empty() {
                return Ok(Err(Refused(BookmarkFailure::Invalid)));
            }
            let tx = conn.transaction()?;
            let count: i64 =
                tx.query_row("SELECT count(*) FROM bookmarks", [], |row| row.get(0))?;
            let mut import = Import {
                tx: &tx,
                budget: u32::try_from(i64::from(MAX_BOOKMARKS) - count).unwrap_or(0),
                added: 0,
                now: SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_or(0, |elapsed| elapsed.as_secs() as i64),
            };
            let Some(root) = import.folder(None, &title)? else {
                return Ok(Err(Refused(BookmarkFailure::Full)));
            };
            import.merge(root, 1, &nodes)?;
            let added = import.added;
            if added == 0 {
                // Nothing came over, so nothing is kept: not the import's
                // folder, not an empty one inside it. Dropping the
                // transaction undoes them.
                return Ok(Ok(BookmarkReply::Imported(0)));
            }
            tx.commit()?;
            Ok(Ok(BookmarkReply::Imported(added)))
        }
        BookmarkRequest::Remove { id } => {
            // Folder contents go with it through ON DELETE CASCADE.
            let removed = conn.execute("DELETE FROM bookmarks WHERE id = ?1", [id])?;
            Ok(if removed == 0 {
                Err(Refused(BookmarkFailure::Missing))
            } else {
                Ok(BookmarkReply::Done)
            })
        }
        BookmarkRequest::Entries { ids } => {
            let mut found = Vec::with_capacity(ids.len());
            for id in ids.into_iter().take(MAX_ENTRIES as usize) {
                match entry(conn, id)? {
                    Some(entry) => found.push(entry),
                    None => return Ok(Err(Refused(BookmarkFailure::Missing))),
                }
            }
            Ok(Ok(BookmarkReply::Entries(found)))
        }
        BookmarkRequest::ChildEntries { parent } => {
            if let Some(parent) = parent {
                if let Err(refused) = folder(conn, parent)? {
                    return Ok(Err(refused));
                }
            }
            let entries = conn
                .prepare_cached(
                    "SELECT b.id, b.parent_id, 0, b.url, b.title, b.added_at FROM bookmarks b
                     WHERE b.parent_id IS ?1 ORDER BY b.position, b.id LIMIT ?2",
                )?
                .query_map(params![parent, MAX_LISTING], entry_row)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(Ok(BookmarkReply::Entries(numbered(entries))))
        }
        BookmarkRequest::Subtree { root } => {
            let mut entries = match root {
                None => conn
                    .prepare_cached(
                        "SELECT b.id, b.parent_id, 0, b.url, b.title, b.added_at FROM bookmarks b
                         ORDER BY b.parent_id, b.position, b.id LIMIT ?1",
                    )?
                    .query_map([MAX_SUBTREE + 1], entry_row)?
                    .collect::<rusqlite::Result<Vec<_>>>()?,
                Some(root) => {
                    let Some(top) = entry(conn, root)? else {
                        return Ok(Err(Refused(BookmarkFailure::Missing)));
                    };
                    let mut below = conn
                        .prepare_cached(
                            "WITH RECURSIVE down(id) AS (
                                 SELECT id FROM bookmarks WHERE parent_id = ?1
                                 UNION ALL
                                 SELECT b.id FROM bookmarks b JOIN down ON b.parent_id = down.id
                             )
                             SELECT b.id, b.parent_id, 0, b.url, b.title, b.added_at
                             FROM bookmarks b JOIN down ON b.id = down.id
                             ORDER BY b.parent_id, b.position, b.id LIMIT ?2",
                        )?
                        .query_map(params![root, MAX_SUBTREE], entry_row)?
                        .collect::<rusqlite::Result<Vec<_>>>()?;
                    below.insert(0, top);
                    below
                }
            };
            if entries.len() > MAX_SUBTREE as usize {
                return Ok(Err(Refused(BookmarkFailure::Full)));
            }
            let top = root.map(|_| entries.remove(0));
            let mut entries = numbered(entries);
            if let Some(top) = top {
                entries.insert(0, top);
            }
            Ok(Ok(BookmarkReply::Entries(entries)))
        }
        BookmarkRequest::Recent { limit } => {
            let entries = conn
                .prepare_cached(&format!(
                    "SELECT {ENTRY_COLUMNS} FROM bookmarks b WHERE b.url IS NOT NULL
                     ORDER BY b.added_at DESC, b.id DESC LIMIT ?1"
                ))?
                .query_map([limit.min(MAX_ENTRIES)], entry_row)?
                .collect::<rusqlite::Result<_>>()?;
            Ok(Ok(BookmarkReply::Entries(entries)))
        }
        BookmarkRequest::Find { query, url, title } => {
            let words: Vec<String> = query
                .as_deref()
                .unwrap_or_default()
                .split_whitespace()
                .take(MAX_QUERY_WORDS)
                .map(|word| {
                    format!(
                        "%{}%",
                        word.replace('\\', "\\\\")
                            .replace('%', "\\%")
                            .replace('_', "\\_")
                    )
                })
                .collect();
            if words.is_empty() && url.is_none() && title.is_none() {
                return Ok(Ok(BookmarkReply::Entries(Vec::new())));
            }
            // Words match links, as a search does; an exact title can name a
            // folder too.
            let mut clauses = String::new();
            if !words.is_empty() || url.is_some() {
                clauses.push_str(" AND b.url IS NOT NULL");
            }
            let mut values: Vec<String> = Vec::new();
            for word in words {
                values.push(word);
                let at = values.len();
                clauses.push_str(&format!(
                    " AND (b.title LIKE ?{at} ESCAPE '\\' OR b.url LIKE ?{at} ESCAPE '\\')"
                ));
            }
            if let Some(url) = url {
                values.push(url);
                clauses.push_str(&format!(" AND b.url = ?{}", values.len()));
            }
            if let Some(title) = title {
                values.push(title);
                clauses.push_str(&format!(" AND b.title = ?{}", values.len()));
            }
            let entries = conn
                .prepare(&format!(
                    "SELECT {ENTRY_COLUMNS} FROM bookmarks b WHERE 1{clauses}
                     ORDER BY b.added_at DESC, b.id DESC LIMIT {MAX_ENTRIES}"
                ))?
                .query_map(rusqlite::params_from_iter(&values), entry_row)?
                .collect::<rusqlite::Result<_>>()?;
            Ok(Ok(BookmarkReply::Entries(entries)))
        }
        BookmarkRequest::Create {
            parent,
            index,
            title,
            url,
        } => {
            if url
                .as_deref()
                .is_some_and(|url| !zephium_core::navigation::is_allowed_str(url))
            {
                return Ok(Err(Refused(BookmarkFailure::Invalid)));
            }
            let title = match (clean_title(&title), &url) {
                (empty, Some(url)) if empty.is_empty() => clean_title(url),
                (empty, None) if empty.is_empty() => {
                    return Ok(Err(Refused(BookmarkFailure::Invalid)))
                }
                (title, _) => title,
            };
            let tx = conn.transaction()?;
            let id = match insert(&tx, parent, &title, url.as_deref())? {
                Ok(id) => id,
                Err(refused) => return Ok(Err(refused)),
            };
            if let Some(index) = index {
                if let Err(refused) = relocate(&tx, id, parent, index)? {
                    return Ok(Err(refused));
                }
            }
            let created = entry(&tx, id)?;
            tx.commit()?;
            Ok(Ok(BookmarkReply::Entries(created.into_iter().collect())))
        }
        BookmarkRequest::Update { id, title, url } => {
            let Some(current) = entry(conn, id)? else {
                return Ok(Err(Refused(BookmarkFailure::Missing)));
            };
            if url.is_some()
                && (current.url.is_none()
                    || !url
                        .as_deref()
                        .is_some_and(zephium_core::navigation::is_allowed_str))
            {
                return Ok(Err(Refused(BookmarkFailure::Invalid)));
            }
            let title = match title.map(|title| clean_title(&title)) {
                Some(empty) if empty.is_empty() && current.url.is_none() => {
                    return Ok(Err(Refused(BookmarkFailure::Invalid)))
                }
                Some(title) => title,
                None => current.title,
            };
            conn.execute(
                "UPDATE bookmarks SET title = ?2, url = ?3 WHERE id = ?1",
                params![id, title, url.or(current.url)],
            )?;
            Ok(Ok(BookmarkReply::Entries(
                entry(conn, id)?.into_iter().collect(),
            )))
        }
        BookmarkRequest::Relocate { id, parent, index } => {
            let Some(current) = entry(conn, id)? else {
                return Ok(Err(Refused(BookmarkFailure::Missing)));
            };
            let parent = parent.unwrap_or(current.parent);
            let index = match index {
                Some(index) if parent == current.parent && index > current.index => index - 1,
                Some(index) => index,
                None => u32::MAX,
            };
            let tx = conn.transaction()?;
            if let Err(refused) = relocate(&tx, id, parent, index)? {
                return Ok(Err(refused));
            }
            let moved = entry(&tx, id)?;
            tx.commit()?;
            Ok(Ok(BookmarkReply::Entries(moved.into_iter().collect())))
        }
        BookmarkRequest::Delete { id, recursive } => {
            let holds: i64 = conn.query_row(
                "SELECT count(*) FROM bookmarks WHERE parent_id = ?1",
                [id],
                |row| row.get(0),
            )?;
            if holds > 0 && !recursive {
                return Ok(Err(Refused(BookmarkFailure::Invalid)));
            }
            let removed = conn.execute("DELETE FROM bookmarks WHERE id = ?1", [id])?;
            Ok(if removed == 0 {
                Err(Refused(BookmarkFailure::Missing))
            } else {
                Ok(BookmarkReply::Done)
            })
        }
    }
}

/// Numbers entries listed in sibling order by their place among siblings.
fn numbered(mut entries: Vec<BookmarkEntry>) -> Vec<BookmarkEntry> {
    let mut next: HashMap<Option<i64>, u32> = HashMap::new();
    for entry in &mut entries {
        let index = next.entry(entry.parent).or_insert(0);
        entry.index = *index;
        *index += 1;
    }
    entries
}

/// Merges an imported tree in one transaction, counting against what the
/// profile may still hold. Depth is tracked as it descends, so each row costs
/// one insert rather than a walk of its ancestors.
struct Import<'a> {
    tx: &'a rusqlite::Transaction<'a>,
    budget: u32,
    added: u32,
    now: i64,
}

impl Import<'_> {
    /// The folder titled `title` directly under `parent`, created when absent.
    /// None once the profile is full.
    fn folder(&mut self, parent: Option<i64>, title: &str) -> rusqlite::Result<Option<i64>> {
        let existing = self
            .tx
            .prepare_cached(
                "SELECT id FROM bookmarks WHERE parent_id IS ?1 AND url IS NULL AND title = ?2
                 ORDER BY position, id LIMIT 1",
            )?
            .query_row(params![parent, title], |row| row.get::<_, i64>(0))
            .optional()?;
        if existing.is_some() {
            return Ok(existing);
        }
        self.insert(parent, title, None)
    }

    fn insert(
        &mut self,
        parent: Option<i64>,
        title: &str,
        url: Option<&str>,
    ) -> rusqlite::Result<Option<i64>> {
        if self.budget == 0 {
            return Ok(None);
        }
        self.tx
            .prepare_cached(
                "INSERT INTO bookmarks(parent_id, position, title, url, added_at)
                 VALUES (?1,
                         (SELECT coalesce(max(position) + 1, 0) FROM bookmarks WHERE parent_id IS ?1),
                         ?2, ?3, ?4)",
            )?
            .execute(params![parent, title, url, self.now])?;
        self.budget -= 1;
        Ok(Some(self.tx.last_insert_rowid()))
    }

    /// `depth` is the level of `parent`, the top level being one. Folders that
    /// would nest past the limit are opened into the deepest allowed one.
    fn merge(&mut self, parent: i64, depth: usize, nodes: &[ImportNode]) -> rusqlite::Result<()> {
        let mut kept: HashSet<String> = self
            .tx
            .prepare_cached("SELECT url FROM bookmarks WHERE parent_id = ?1 AND url IS NOT NULL")?
            .query_map([parent], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<_>>()?;
        for node in nodes {
            if self.budget == 0 {
                break;
            }
            match node {
                ImportNode::Link { title, url } => {
                    if !navigation::is_allowed_str(url) || kept.contains(url) {
                        continue;
                    }
                    let title = match clean_title(title) {
                        empty if empty.is_empty() => clean_title(url),
                        title => title,
                    };
                    if self.insert(Some(parent), &title, Some(url))?.is_some() {
                        self.added += 1;
                        kept.insert(url.clone());
                    }
                }
                ImportNode::Folder { title, children } => {
                    // The folder sits a level down and its contents one more.
                    if depth + 2 > MAX_DEPTH {
                        self.merge(parent, depth, children)?;
                        continue;
                    }
                    let title = match clean_title(title) {
                        empty if empty.is_empty() => "Folder".to_owned(),
                        title => title,
                    };
                    if let Some(folder) = self.folder(Some(parent), &title)? {
                        self.merge(folder, depth + 1, children)?;
                    }
                }
            }
        }
        Ok(())
    }
}

/// Refuses unless `id` is an existing folder.
fn folder(conn: &Connection, id: i64) -> Outcome<()> {
    let url = conn
        .query_row("SELECT url FROM bookmarks WHERE id = ?1", [id], |row| {
            row.get::<_, Option<String>>(0)
        })
        .optional()?;
    Ok(match url {
        None => Err(Refused(BookmarkFailure::Missing)),
        Some(Some(_)) => Err(Refused(BookmarkFailure::Invalid)),
        Some(None) => Ok(()),
    })
}

/// Levels from the top down to `id` inclusive; the top level is one.
fn depth(conn: &Connection, id: i64) -> rusqlite::Result<usize> {
    conn.query_row(
        "WITH RECURSIVE up(id, depth) AS (
             SELECT ?1, 1
             UNION ALL
             SELECT b.parent_id, up.depth + 1 FROM bookmarks b JOIN up ON b.id = up.id
             WHERE b.parent_id IS NOT NULL AND up.depth <= ?2
         )
         SELECT max(depth) FROM up",
        params![id, MAX_DEPTH as i64],
        |row| row.get::<_, i64>(0),
    )
    .map(|depth| usize::try_from(depth).unwrap_or(usize::MAX))
}

/// Levels below and including `id`: one for a link or an empty folder.
fn height(conn: &Connection, id: i64) -> rusqlite::Result<usize> {
    conn.query_row(
        "WITH RECURSIVE down(id, height) AS (
             SELECT ?1, 1
             UNION ALL
             SELECT b.id, down.height + 1 FROM bookmarks b JOIN down ON b.parent_id = down.id
             WHERE down.height <= ?2
         )
         SELECT max(height) FROM down",
        params![id, MAX_DEPTH as i64],
        |row| row.get::<_, i64>(0),
    )
    .map(|height| usize::try_from(height).unwrap_or(usize::MAX))
}

fn insert(
    tx: &rusqlite::Transaction<'_>,
    parent: Option<i64>,
    title: &str,
    url: Option<&str>,
) -> Outcome<i64> {
    if let Some(parent) = parent {
        if let Err(refused) = folder(tx, parent)? {
            return Ok(Err(refused));
        }
        if depth(tx, parent)? + 1 > MAX_DEPTH {
            return Ok(Err(Refused(BookmarkFailure::Full)));
        }
    }
    let count: i64 = tx.query_row("SELECT count(*) FROM bookmarks", [], |row| row.get(0))?;
    if count >= i64::from(MAX_BOOKMARKS) {
        return Ok(Err(Refused(BookmarkFailure::Full)));
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs() as i64);
    tx.execute(
        "INSERT INTO bookmarks(parent_id, position, title, url, added_at)
         VALUES (?1,
                 (SELECT coalesce(max(position) + 1, 0) FROM bookmarks WHERE parent_id IS ?1),
                 ?2, ?3, ?4)",
        params![parent, title, url, now],
    )?;
    Ok(Ok(tx.last_insert_rowid()))
}

fn relocate(
    tx: &rusqlite::Transaction<'_>,
    id: i64,
    parent: Option<i64>,
    index: u32,
) -> Outcome<()> {
    let exists = tx
        .query_row("SELECT 1 FROM bookmarks WHERE id = ?1", [id], |_| Ok(()))
        .optional()?;
    if exists.is_none() {
        return Ok(Err(Refused(BookmarkFailure::Missing)));
    }
    if let Some(parent) = parent {
        if let Err(refused) = folder(tx, parent)? {
            return Ok(Err(refused));
        }
        let inside_itself = tx
            .query_row(
                "WITH RECURSIVE up(id) AS (
                     SELECT ?1
                     UNION
                     SELECT b.parent_id FROM bookmarks b JOIN up ON b.id = up.id
                     WHERE b.parent_id IS NOT NULL
                 )
                 SELECT 1 FROM up WHERE id = ?2",
                params![parent, id],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if inside_itself {
            return Ok(Err(Refused(BookmarkFailure::Cycle)));
        }
        if depth(tx, parent)? + height(tx, id)? > MAX_DEPTH {
            return Ok(Err(Refused(BookmarkFailure::Full)));
        }
    }
    let mut siblings: Vec<i64> = tx
        .prepare_cached(
            "SELECT id FROM bookmarks WHERE parent_id IS ?1 AND id != ?2 ORDER BY position, id",
        )?
        .query_map(params![parent, id], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    let at = usize::try_from(index).map_or(siblings.len(), |index| index.min(siblings.len()));
    siblings.insert(at, id);
    tx.execute(
        "UPDATE bookmarks SET parent_id = ?2 WHERE id = ?1",
        params![id, parent],
    )?;
    let mut place = tx.prepare_cached("UPDATE bookmarks SET position = ?2 WHERE id = ?1")?;
    for (position, sibling) in siblings.iter().enumerate() {
        place.execute(params![sibling, position as i64])?;
    }
    Ok(Ok(()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hub() -> (Hub, ProfileId) {
        let mut hub = Hub::in_memory().unwrap();
        let profile = ProfileId::from(1);
        let space = SpaceId::from(2);
        hub.save(&SessionState {
            profiles: vec![PersistedProfile {
                id: profile,
                name: "Personal".into(),
                kind: ProfileKind::Default,
            }],
            spaces: vec![PersistedSpace {
                id: space,
                profile,
                name: "Space".into(),
                look: Default::default(),
            }],
            items: Vec::new(),
            active_space: Some(space),
            active_item: None,
            splits: None,
            recently_closed: Vec::new(),
        })
        .unwrap();
        (hub, profile)
    }

    fn added(reply: BookmarkReply) -> i64 {
        match reply {
            BookmarkReply::Added(id) => id,
            other => panic!("expected an addition, got {other:?}"),
        }
    }

    fn titles(reply: BookmarkReply) -> Vec<String> {
        match reply {
            BookmarkReply::Nodes(nodes) => nodes.into_iter().map(|node| node.title).collect(),
            other => panic!("expected nodes, got {other:?}"),
        }
    }

    fn link(parent: Option<i64>, title: &str, url: &str) -> BookmarkRequest {
        BookmarkRequest::AddLink {
            parent,
            title: title.into(),
            url: url.into(),
            if_absent: false,
        }
    }

    fn entries(reply: BookmarkReply) -> Vec<(i64, Option<i64>, u32, String)> {
        match reply {
            BookmarkReply::Entries(entries) => entries
                .into_iter()
                .map(|entry| (entry.id, entry.parent, entry.index, entry.title))
                .collect(),
            other => panic!("expected entries, got {other:?}"),
        }
    }

    fn made(reply: BookmarkReply) -> i64 {
        match reply {
            BookmarkReply::Entries(entries) if entries.len() == 1 => entries[0].id,
            other => panic!("expected the entry made, got {other:?}"),
        }
    }

    fn create(
        parent: Option<i64>,
        index: Option<u32>,
        title: &str,
        url: Option<&str>,
    ) -> BookmarkRequest {
        BookmarkRequest::Create {
            parent,
            index,
            title: title.into(),
            url: url.map(str::to_owned),
        }
    }

    #[test]
    fn extension_entries_count_places_among_siblings_despite_gaps() {
        let (mut hub, profile) = hub();
        let folder = made(hub.bookmarks(profile, create(None, None, "Folder", None)));
        let a = made(hub.bookmarks(
            profile,
            create(Some(folder), None, "A", Some("https://a.example/")),
        ));
        let b = made(hub.bookmarks(
            profile,
            create(Some(folder), None, "B", Some("https://b.example/")),
        ));
        let c = made(hub.bookmarks(
            profile,
            create(Some(folder), Some(0), "C", Some("https://c.example/")),
        ));
        assert_eq!(
            hub.bookmarks(profile, BookmarkRequest::Remove { id: a }),
            BookmarkReply::Done
        );
        assert_eq!(
            entries(hub.bookmarks(
                profile,
                BookmarkRequest::ChildEntries {
                    parent: Some(folder)
                }
            )),
            vec![
                (c, Some(folder), 0, "C".into()),
                (b, Some(folder), 1, "B".into())
            ]
        );
        assert_eq!(
            entries(hub.bookmarks(profile, BookmarkRequest::Entries { ids: vec![b] })),
            vec![(b, Some(folder), 1, "B".into())]
        );
        assert_eq!(
            hub.bookmarks(profile, BookmarkRequest::Entries { ids: vec![b, a] }),
            BookmarkReply::Failed(BookmarkFailure::Missing)
        );
        assert_eq!(
            entries(hub.bookmarks(profile, BookmarkRequest::Subtree { root: None })),
            vec![
                (folder, None, 0, "Folder".into()),
                (c, Some(folder), 0, "C".into()),
                (b, Some(folder), 1, "B".into()),
            ]
        );
        assert_eq!(
            entries(hub.bookmarks(profile, BookmarkRequest::Subtree { root: Some(folder) })).len(),
            3
        );
        assert_eq!(
            entries(hub.bookmarks(profile, BookmarkRequest::Recent { limit: 1 })),
            vec![(c, Some(folder), 0, "C".into())]
        );
        assert_eq!(
            entries(hub.bookmarks(
                profile,
                BookmarkRequest::Find {
                    query: Some("b.example".into()),
                    url: None,
                    title: None
                }
            )),
            vec![(b, Some(folder), 1, "B".into())]
        );
        assert_eq!(
            entries(hub.bookmarks(
                profile,
                BookmarkRequest::Find {
                    query: None,
                    url: None,
                    title: Some("Folder".into())
                }
            )),
            vec![(folder, None, 0, "Folder".into())]
        );
    }

    #[test]
    fn extension_writes_follow_chrome_rules_and_report_what_changed() {
        let (mut hub, profile) = hub();
        let (reply, change) = hub.bookmarks_observed(profile, create(None, None, "Folder", None));
        let folder = made(reply);
        assert!(
            matches!(change, Some(BookmarkChange::Created(entry)) if entry.id == folder && entry.index == 0)
        );
        let a = made(hub.bookmarks(
            profile,
            create(Some(folder), None, "A", Some("https://a.example/")),
        ));
        let b = made(hub.bookmarks(
            profile,
            create(Some(folder), None, "B", Some("https://b.example/")),
        ));
        let c = made(hub.bookmarks(
            profile,
            create(Some(folder), None, "C", Some("https://c.example/")),
        ));

        // Chrome counts the target place before the move: index 2 puts A
        // after B, not after C.
        let (reply, change) = hub.bookmarks_observed(
            profile,
            BookmarkRequest::Relocate {
                id: a,
                parent: None,
                index: Some(2),
            },
        );
        assert_eq!(made(reply), a);
        assert_eq!(
            change,
            Some(BookmarkChange::Moved {
                id: a,
                parent: Some(folder),
                index: 1,
                old_parent: Some(folder),
                old_index: 0
            })
        );
        assert_eq!(
            entries(hub.bookmarks(
                profile,
                BookmarkRequest::ChildEntries {
                    parent: Some(folder)
                }
            ))
            .into_iter()
            .map(|entry| entry.0)
            .collect::<Vec<_>>(),
            vec![b, a, c]
        );

        let (reply, change) = hub.bookmarks_observed(
            profile,
            BookmarkRequest::Update {
                id: b,
                title: Some("Bee".into()),
                url: Some("https://bee.example/".into()),
            },
        );
        assert_eq!(made(reply), b);
        assert_eq!(
            change,
            Some(BookmarkChange::Changed {
                id: b,
                title: "Bee".into(),
                url: Some("https://bee.example/".into())
            })
        );
        assert_eq!(
            hub.bookmarks(
                profile,
                BookmarkRequest::Update {
                    id: folder,
                    title: None,
                    url: Some("https://x.example/".into())
                }
            ),
            BookmarkReply::Failed(BookmarkFailure::Invalid)
        );
        assert_eq!(
            hub.bookmarks(
                profile,
                create(None, None, "Script", Some("javascript:alert(1)"))
            ),
            BookmarkReply::Failed(BookmarkFailure::Invalid)
        );

        assert_eq!(
            hub.bookmarks(
                profile,
                BookmarkRequest::Delete {
                    id: folder,
                    recursive: false
                }
            ),
            BookmarkReply::Failed(BookmarkFailure::Invalid)
        );
        let (reply, change) = hub.bookmarks_observed(
            profile,
            BookmarkRequest::Delete {
                id: folder,
                recursive: true,
            },
        );
        assert_eq!(reply, BookmarkReply::Done);
        assert!(
            matches!(change, Some(BookmarkChange::Removed(entry)) if entry.id == folder && entry.parent.is_none())
        );
        assert_eq!(
            hub.bookmarks(profile, BookmarkRequest::Entries { ids: vec![c] }),
            BookmarkReply::Failed(BookmarkFailure::Missing)
        );

        // An address already kept is found, not created again.
        let kept = made(hub.bookmarks(
            profile,
            create(None, None, "Kept", Some("https://kept.example/")),
        ));
        let (reply, change) = hub.bookmarks_observed(
            profile,
            BookmarkRequest::AddLink {
                parent: None,
                title: "Again".into(),
                url: "https://kept.example/".into(),
                if_absent: true,
            },
        );
        assert_eq!((reply, change), (BookmarkReply::Added(kept), None));
    }

    #[test]
    fn folders_keep_their_links_in_order_and_paths_lead_back_to_the_top() {
        let (mut hub, profile) = hub();
        let reading = added(hub.bookmarks(
            profile,
            BookmarkRequest::AddFolder {
                parent: None,
                title: "Reading".into(),
            },
        ));
        let nested = added(hub.bookmarks(
            profile,
            BookmarkRequest::AddFolder {
                parent: Some(reading),
                title: "Later".into(),
            },
        ));
        added(hub.bookmarks(profile, link(Some(reading), "One", "https://one.example/")));
        added(hub.bookmarks(profile, link(Some(reading), "", "https://two.example/")));
        assert_eq!(
            titles(hub.bookmarks(
                profile,
                BookmarkRequest::Children {
                    parent: Some(reading)
                }
            )),
            vec!["Later", "One", "https://two.example/"]
        );
        assert_eq!(
            titles(hub.bookmarks(profile, BookmarkRequest::Path { id: nested })),
            vec!["Reading", "Later"]
        );
        let BookmarkReply::Nodes(top) =
            hub.bookmarks(profile, BookmarkRequest::Children { parent: None })
        else {
            panic!("top level");
        };
        assert_eq!(top.len(), 1);
        assert_eq!(top[0].children, 3);
        assert!(top[0].is_folder());
    }

    #[test]
    fn adding_a_known_address_can_return_the_bookmark_already_kept() {
        let (mut hub, profile) = hub();
        let first = added(hub.bookmarks(profile, link(None, "Docs", "https://docs.example/")));
        let again = added(hub.bookmarks(
            profile,
            BookmarkRequest::AddLink {
                parent: None,
                title: "Docs again".into(),
                url: "https://docs.example/".into(),
                if_absent: true,
            },
        ));
        assert_eq!(first, again);
        assert_eq!(
            titles(hub.bookmarks(profile, BookmarkRequest::Children { parent: None })),
            vec!["Docs"]
        );
    }

    #[test]
    fn writes_refuse_what_would_break_the_tree() {
        let (mut hub, profile) = hub();
        let outer = added(hub.bookmarks(
            profile,
            BookmarkRequest::AddFolder {
                parent: None,
                title: "Outer".into(),
            },
        ));
        let inner = added(hub.bookmarks(
            profile,
            BookmarkRequest::AddFolder {
                parent: Some(outer),
                title: "Inner".into(),
            },
        ));
        let page = added(hub.bookmarks(profile, link(None, "Page", "https://page.example/")));
        let refused = |reply| match reply {
            BookmarkReply::Failed(failure) => failure,
            other => panic!("expected a refusal, got {other:?}"),
        };
        assert_eq!(
            refused(hub.bookmarks(
                profile,
                BookmarkRequest::Move {
                    id: outer,
                    parent: Some(inner),
                    index: 0
                }
            )),
            BookmarkFailure::Cycle
        );
        assert_eq!(
            refused(hub.bookmarks(profile, link(Some(page), "Child", "https://x.example/"))),
            BookmarkFailure::Invalid
        );
        assert_eq!(
            refused(hub.bookmarks(profile, link(None, "Script", "javascript:alert(1)"))),
            BookmarkFailure::Invalid
        );
        assert_eq!(
            refused(hub.bookmarks(
                profile,
                BookmarkRequest::Rename {
                    id: page,
                    title: " \n ".into()
                }
            )),
            BookmarkFailure::Invalid
        );
        assert_eq!(
            refused(hub.bookmarks(profile, BookmarkRequest::Remove { id: 9_999 })),
            BookmarkFailure::Missing
        );
        assert_eq!(
            refused(hub.bookmarks(profile, BookmarkRequest::Children { parent: Some(page) })),
            BookmarkFailure::Invalid
        );
    }

    #[test]
    fn folders_nest_only_to_the_depth_limit() {
        let (mut hub, profile) = hub();
        let mut parent = None;
        for level in 0..MAX_DEPTH {
            parent = Some(added(hub.bookmarks(
                profile,
                BookmarkRequest::AddFolder {
                    parent,
                    title: format!("Level {level}"),
                },
            )));
        }
        assert_eq!(
            hub.bookmarks(profile, link(parent, "Too deep", "https://deep.example/")),
            BookmarkReply::Failed(BookmarkFailure::Full)
        );
        // A subtree may not move under a folder where it would end too deep.
        let branch = added(hub.bookmarks(
            profile,
            BookmarkRequest::AddFolder {
                parent: None,
                title: "Branch".into(),
            },
        ));
        added(hub.bookmarks(profile, link(Some(branch), "Leaf", "https://leaf.example/")));
        let BookmarkReply::Nodes(path) = hub.bookmarks(
            profile,
            BookmarkRequest::Path {
                id: parent.unwrap(),
            },
        ) else {
            panic!("path");
        };
        let shallow_enough = path[MAX_DEPTH - 3].id;
        let too_deep = path[MAX_DEPTH - 2].id;
        assert_eq!(
            hub.bookmarks(
                profile,
                BookmarkRequest::Move {
                    id: branch,
                    parent: Some(too_deep),
                    index: 0
                }
            ),
            BookmarkReply::Failed(BookmarkFailure::Full)
        );
        assert_eq!(
            hub.bookmarks(
                profile,
                BookmarkRequest::Move {
                    id: branch,
                    parent: Some(shallow_enough),
                    index: 0
                }
            ),
            BookmarkReply::Done
        );
    }

    #[test]
    fn importing_again_adds_only_what_is_new_and_keeps_the_tree() {
        let (mut hub, profile) = hub();
        let tree = |extra: bool| {
            let mut docs = vec![ImportNode::Link {
                title: "Std".into(),
                url: "https://doc.rust-lang.org/std/".into(),
            }];
            if extra {
                docs.push(ImportNode::Link {
                    title: "Book".into(),
                    url: "https://doc.rust-lang.org/book/".into(),
                });
            }
            vec![
                ImportNode::Link {
                    title: "".into(),
                    url: "https://news.example/".into(),
                },
                ImportNode::Link {
                    title: "Bookmarklet".into(),
                    url: "javascript:void(0)".into(),
                },
                ImportNode::Folder {
                    title: "Docs".into(),
                    children: docs,
                },
            ]
        };
        let import = |hub: &mut Hub, extra| {
            hub.bookmarks(
                profile,
                BookmarkRequest::Import {
                    folder: "From Chrome".into(),
                    nodes: tree(extra),
                },
            )
        };
        assert_eq!(import(&mut hub, false), BookmarkReply::Imported(2));
        assert_eq!(import(&mut hub, true), BookmarkReply::Imported(1));
        let BookmarkReply::Nodes(top) =
            hub.bookmarks(profile, BookmarkRequest::Children { parent: None })
        else {
            panic!("top level");
        };
        assert_eq!(top.len(), 1);
        assert_eq!(top[0].title, "From Chrome");
        assert_eq!(
            titles(hub.bookmarks(
                profile,
                BookmarkRequest::Children {
                    parent: Some(top[0].id)
                }
            )),
            vec!["https://news.example/", "Docs"]
        );
        assert_eq!(
            titles(hub.bookmarks(
                profile,
                BookmarkRequest::Search {
                    query: "rust-lang".into()
                }
            )),
            vec!["Book", "Std"]
        );
    }

    #[test]
    fn an_import_that_adds_nothing_leaves_no_folder_behind() {
        let (mut hub, profile) = hub();
        let top = |hub: &mut Hub| {
            let BookmarkReply::Nodes(top) =
                hub.bookmarks(profile, BookmarkRequest::Children { parent: None })
            else {
                panic!("top level");
            };
            top
        };
        let import = |hub: &mut Hub, nodes| {
            hub.bookmarks(
                profile,
                BookmarkRequest::Import {
                    folder: "Imported from Chrome".into(),
                    nodes,
                },
            )
        };
        assert_eq!(import(&mut hub, vec![]), BookmarkReply::Imported(0));
        // Links the profile cannot hold leave no folder either, nor an empty
        // one inside it.
        assert_eq!(
            import(
                &mut hub,
                vec![
                    ImportNode::Link {
                        title: "Settings".into(),
                        url: "chrome://settings/".into(),
                    },
                    ImportNode::Folder {
                        title: "Internal".into(),
                        children: vec![ImportNode::Link {
                            title: "Flags".into(),
                            url: "chrome://flags/".into(),
                        }],
                    },
                ]
            ),
            BookmarkReply::Imported(0)
        );
        assert!(top(&mut hub).is_empty());

        // An import that did bring something is kept, and a later one with
        // nothing new keeps it as it was.
        let kept = vec![ImportNode::Link {
            title: "Яндекс".into(),
            url: "https://ya.ru/".into(),
        }];
        assert_eq!(import(&mut hub, kept.clone()), BookmarkReply::Imported(1));
        assert_eq!(import(&mut hub, kept), BookmarkReply::Imported(0));
        assert_eq!(import(&mut hub, vec![]), BookmarkReply::Imported(0));
        let folders = top(&mut hub);
        assert_eq!(folders.len(), 1);
        assert_eq!(folders[0].children, 1);
    }

    #[test]
    fn an_import_nested_past_the_limit_opens_into_the_deepest_folder() {
        let (mut hub, profile) = hub();
        let mut node = ImportNode::Link {
            title: "Deep".into(),
            url: "https://deep.example/".into(),
        };
        for level in 0..(MAX_DEPTH + 4) {
            node = ImportNode::Folder {
                title: format!("Level {level}"),
                children: vec![node],
            };
        }
        assert_eq!(
            hub.bookmarks(
                profile,
                BookmarkRequest::Import {
                    folder: "Imported".into(),
                    nodes: vec![node],
                },
            ),
            BookmarkReply::Imported(1)
        );
        let BookmarkReply::Nodes(found) = hub.bookmarks(
            profile,
            BookmarkRequest::Search {
                query: "deep.example".into(),
            },
        ) else {
            panic!("search");
        };
        let BookmarkReply::Nodes(path) =
            hub.bookmarks(profile, BookmarkRequest::Path { id: found[0].id })
        else {
            panic!("path");
        };
        assert_eq!(path.len(), MAX_DEPTH);
    }

    #[test]
    fn moving_reorders_siblings_and_removing_a_folder_removes_its_contents() {
        let (mut hub, profile) = hub();
        let a = added(hub.bookmarks(profile, link(None, "A", "https://a.example/")));
        added(hub.bookmarks(profile, link(None, "B", "https://b.example/")));
        let folder = added(hub.bookmarks(
            profile,
            BookmarkRequest::AddFolder {
                parent: None,
                title: "Folder".into(),
            },
        ));
        assert_eq!(
            hub.bookmarks(
                profile,
                BookmarkRequest::Move {
                    id: a,
                    parent: None,
                    index: 99
                }
            ),
            BookmarkReply::Done
        );
        assert_eq!(
            titles(hub.bookmarks(profile, BookmarkRequest::Children { parent: None })),
            vec!["B", "Folder", "A"]
        );
        hub.bookmarks(
            profile,
            BookmarkRequest::Move {
                id: a,
                parent: Some(folder),
                index: 0,
            },
        );
        assert_eq!(
            titles(hub.bookmarks(
                profile,
                BookmarkRequest::Search {
                    query: "a.exam".into()
                }
            )),
            vec!["A"]
        );
        assert_eq!(
            hub.bookmarks(profile, BookmarkRequest::Remove { id: folder }),
            BookmarkReply::Done
        );
        assert_eq!(
            titles(hub.bookmarks(
                profile,
                BookmarkRequest::Search {
                    query: "a.example".into()
                }
            )),
            Vec::<String>::new()
        );
        // LIKE wildcards in a query are literal text.
        assert_eq!(
            titles(hub.bookmarks(profile, BookmarkRequest::Search { query: "%".into() })),
            Vec::<String>::new()
        );
    }

    #[test]
    fn a_search_matches_each_word_in_the_title_or_the_address() {
        let (mut hub, profile) = hub();
        added(hub.bookmarks(profile, link(None, "Guide to Rust", "https://doc.example/")));
        added(hub.bookmarks(profile, link(None, "Rust", "https://example.com/guide")));
        added(hub.bookmarks(profile, link(None, "Rust", "https://example.com/other")));
        let found = |hub: &mut Hub, query: &str| {
            let mut found = titles(hub.bookmarks(
                profile,
                BookmarkRequest::Search {
                    query: query.into(),
                },
            ));
            found.sort();
            found
        };
        assert_eq!(found(&mut hub, "rust guide"), vec!["Guide to Rust", "Rust"]);
        assert_eq!(
            found(&mut hub, "  GUIDE   rust "),
            vec!["Guide to Rust", "Rust"]
        );
        assert!(found(&mut hub, "rust missing").is_empty());
        assert!(found(&mut hub, "rust %").is_empty());
    }

    #[test]
    fn all_lists_every_node_with_siblings_in_their_order() {
        let (mut hub, profile) = hub();
        let folder = added(hub.bookmarks(
            profile,
            BookmarkRequest::AddFolder {
                parent: None,
                title: "Folder".into(),
            },
        ));
        added(hub.bookmarks(profile, link(Some(folder), "Inside", "https://in.example/")));
        added(hub.bookmarks(profile, link(None, "Top", "https://top.example/")));
        added(hub.bookmarks(
            profile,
            link(Some(folder), "Second", "https://two.example/"),
        ));
        let BookmarkReply::Nodes(all) = hub.bookmarks(profile, BookmarkRequest::All) else {
            panic!("all");
        };
        assert_eq!(
            all.iter()
                .map(|node| (node.parent, node.title.as_str()))
                .collect::<Vec<_>>(),
            vec![
                (None, "Folder"),
                (None, "Top"),
                (Some(folder), "Inside"),
                (Some(folder), "Second"),
            ]
        );
    }
}
