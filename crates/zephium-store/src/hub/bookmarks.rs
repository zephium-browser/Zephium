use super::*;

use zephium_core::bookmarks::{
    clean_title, BookmarkChange, BookmarkEntry, BookmarkFailure, BookmarkNode, BookmarkReply,
    BookmarkRequest, ImportNode, MAX_BOOKMARKS, MAX_DEPTH, MAX_ENTRIES, MAX_LISTING,
    MAX_QUERY_BYTES, MAX_SEARCH_RESULTS, MAX_SUBTREE,
};

/// Words of a search that count; the rest are ignored.
const MAX_QUERY_WORDS: usize = 8;

#[derive(Clone)]
struct LegacyBookmark {
    id: i64,
    parent: Option<i64>,
    position: i64,
    title: String,
    url: Option<String>,
    added_at: i64,
}

fn read_legacy_bookmarks(conn: &Connection) -> rusqlite::Result<Vec<LegacyBookmark>> {
    let mut statement = conn.prepare(
        "SELECT id, parent_id, position, title, url, added_at FROM bookmarks ORDER BY id",
    )?;
    let rows = statement
        .query_map([], |row| {
            Ok(LegacyBookmark {
                id: row.get(0)?,
                parent: row.get(1)?,
                position: row.get(2)?,
                title: row.get(3)?,
                url: row.get(4)?,
                added_at: row.get(5)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

fn insert_legacy_bookmarks(
    tx: &rusqlite::Transaction<'_>,
    rows: &[LegacyBookmark],
    root: i64,
) -> rusqlite::Result<()> {
    if rows.is_empty() {
        return Ok(());
    }
    let mut children: HashMap<Option<i64>, Vec<&LegacyBookmark>> = HashMap::new();
    for row in rows {
        children.entry(row.parent).or_default().push(row);
    }
    for siblings in children.values_mut() {
        siblings.sort_by_key(|row| (row.position, row.id));
    }
    let existing_urls: HashSet<String> = tx
        .prepare(
            "WITH RECURSIVE subtree(id, url) AS (
                 SELECT id, url FROM bookmarks WHERE id=?1
                 UNION ALL
                 SELECT b.id, b.url FROM bookmarks b JOIN subtree s ON b.parent_id=s.id
             )
             SELECT url FROM subtree WHERE url IS NOT NULL",
        )?
        .query_map([root], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    let mut visited = HashSet::with_capacity(rows.len());
    insert_legacy_children(
        tx,
        &children,
        None,
        Some(root),
        1,
        &mut visited,
        &existing_urls,
    )?;
    if visited.len() != rows.len() {
        return Err(rusqlite::Error::InvalidQuery);
    }
    Ok(())
}

fn insert_legacy_children(
    tx: &rusqlite::Transaction<'_>,
    children: &HashMap<Option<i64>, Vec<&LegacyBookmark>>,
    legacy_parent: Option<i64>,
    shared_parent: Option<i64>,
    depth: usize,
    visited: &mut HashSet<i64>,
    existing_urls: &HashSet<String>,
) -> rusqlite::Result<()> {
    let Some(siblings) = children.get(&legacy_parent) else {
        return Ok(());
    };
    if depth > MAX_DEPTH as usize {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let mut position: i64 = tx.query_row(
        "SELECT coalesce(max(position) + 1, 0) FROM bookmarks WHERE parent_id IS ?1",
        [shared_parent],
        |row| row.get(0),
    )?;
    for row in siblings {
        if !visited.insert(row.id) {
            return Err(rusqlite::Error::InvalidQuery);
        }
        if row
            .url
            .as_ref()
            .is_some_and(|url| existing_urls.contains(url))
        {
            continue;
        }
        tx.execute(
            "INSERT INTO bookmarks(parent_id, position, title, url, added_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![shared_parent, position, row.title, row.url, row.added_at],
        )?;
        position += 1;
        let shared_id = tx.last_insert_rowid();
        insert_legacy_children(
            tx,
            children,
            Some(row.id),
            Some(shared_id),
            depth + 1,
            visited,
            existing_urls,
        )?;
    }
    Ok(())
}

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

fn visible_entry(conn: &Connection, root: Option<i64>, id: i64) -> Option<BookmarkEntry> {
    entry(conn, id).ok().flatten().map(|mut entry| {
        if entry.parent == root {
            entry.parent = None;
        }
        entry
    })
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
    pub(crate) fn bookmark_folders(
        &mut self,
        profile: ProfileId,
    ) -> Vec<zephium_core::bookmarks::BookmarkFolder> {
        if !self.registry.contains(&profile)
            || self.degraded_profiles.contains(&profile)
            || self.recovery_required.is_some()
            || self.prepare_shared_bookmarks().is_err()
        {
            return Vec::new();
        }
        self.meta
            .prepare_cached(
                "SELECT DISTINCT b.id, b.title FROM bookmarks b
                 JOIN profile_bookmark_folders pbf ON pbf.folder_id=b.id
                 JOIN profiles p ON p.id=pbf.profile_id
                 WHERE b.parent_id IS NULL AND b.url IS NULL
                 ORDER BY b.title COLLATE NOCASE, b.id LIMIT 64",
            )
            .and_then(|mut statement| {
                statement
                    .query_map([], |row| {
                        Ok(zephium_core::bookmarks::BookmarkFolder {
                            id: row.get(0)?,
                            title: row.get(1)?,
                        })
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()
            })
            .unwrap_or_default()
    }

    pub(super) fn prepare_shared_bookmarks(&mut self) -> rusqlite::Result<()> {
        let profiles: Vec<_> = {
            let mut statement = self
                .meta
                .prepare("SELECT id, name FROM profiles ORDER BY position, id")?;
            let rows = statement.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            rows.map(|row| {
                let (raw, name) = row?;
                zephium_core::ids::ProfileId::parse(&raw)
                    .filter(|profile| profile.to_string() == raw)
                    .map(|profile| (profile, name))
                    .ok_or_else(|| invalid_data("profile registry contains an invalid id"))
            })
            .collect::<rusqlite::Result<Vec<_>>>()?
        };
        let any_mapping: bool = self.meta.query_row(
            "SELECT EXISTS(SELECT 1 FROM profile_bookmark_folders)",
            [],
            |row| row.get(0),
        )?;
        let has_import_marker =
            profiles
                .iter()
                .try_fold(false, |found, (profile, _)| -> rusqlite::Result<bool> {
                    if found {
                        return Ok(true);
                    }
                    self.meta.query_row(
                "SELECT EXISTS(SELECT 1 FROM bookmark_profile_migrations WHERE profile_id=?1)",
                [profile.to_string()],
                |row| row.get::<_, bool>(0),
            )
                })?;
        let has_unmapped_top_level_links: bool = self.meta.query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM bookmarks b
                 WHERE b.parent_id IS NULL AND b.url IS NOT NULL
                   AND NOT EXISTS(
                       SELECT 1 FROM profile_bookmark_folders pbf WHERE pbf.folder_id=b.id
                   )
             )",
            [],
            |row| row.get(0),
        )?;
        if has_import_marker && (!any_mapping || has_unmapped_top_level_links) {
            let mut unimported = Vec::new();
            for (profile, _) in &profiles {
                let imported: bool = self.meta.query_row(
                    "SELECT EXISTS(SELECT 1 FROM bookmark_profile_migrations WHERE profile_id=?1)",
                    [profile.to_string()],
                    |row| row.get(0),
                )?;
                if imported || self.degraded_profiles.contains(profile) {
                    continue;
                }
                let legacy = self
                    .profile_conn(*profile)
                    .and_then(|conn| read_legacy_bookmarks(&conn));
                match legacy {
                    Ok(rows) => unimported.push((*profile, rows)),
                    Err(_) => {
                        self.degraded_profiles.insert(*profile);
                    }
                }
            }
            let root_name = profiles
                .first()
                .map_or("Bookmarks", |(_, name)| name.as_str());
            let tx = self
                .meta
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let existing_roots: Vec<i64> = {
                let mut statement = tx.prepare(
                    "SELECT DISTINCT folder_id FROM profile_bookmark_folders ORDER BY folder_id",
                )?;
                let roots = statement
                    .query_map([], |row| row.get(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                roots
            };
            let root: i64 = match existing_roots.first().copied() {
                Some(root) => root,
                None => tx.query_row(
                    "INSERT INTO bookmarks(parent_id, position, title, url, added_at)
                     VALUES (NULL, coalesce((SELECT max(position)+1 FROM bookmarks WHERE parent_id IS NULL),0), ?1, NULL, ?2)
                     RETURNING id",
                    params![root_name, now_secs()],
                    |row| row.get(0),
                )?,
            };
            let top_level: Vec<i64> = {
                let mut statement = tx.prepare(
                    "SELECT id FROM bookmarks WHERE parent_id IS NULL ORDER BY position, id",
                )?;
                let roots = statement
                    .query_map([], |row| row.get(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                roots
            };
            let mut position: i64 = tx.query_row(
                "SELECT coalesce(max(position) + 1, 0) FROM bookmarks WHERE parent_id=?1",
                [root],
                |row| row.get(0),
            )?;
            for id in top_level {
                if id != root && !existing_roots.contains(&id) {
                    tx.execute(
                        "UPDATE bookmarks SET parent_id=?1, position=?2 WHERE id=?3",
                        params![root, position, id],
                    )?;
                    position += 1;
                }
            }
            for old_root in existing_roots.iter().copied().filter(|old| *old != root) {
                let children: Vec<i64> = {
                    let mut statement = tx.prepare(
                        "SELECT id FROM bookmarks WHERE parent_id=?1 ORDER BY position, id",
                    )?;
                    let children = statement
                        .query_map([old_root], |row| row.get(0))?
                        .collect::<rusqlite::Result<_>>()?;
                    children
                };
                let mut position: i64 = tx.query_row(
                    "SELECT coalesce(max(position) + 1, 0) FROM bookmarks WHERE parent_id=?1",
                    [root],
                    |row| row.get(0),
                )?;
                for child in children {
                    tx.execute(
                        "UPDATE bookmarks SET parent_id=?1, position=?2 WHERE id=?3",
                        params![root, position, child],
                    )?;
                    position += 1;
                }
                tx.execute("DELETE FROM bookmarks WHERE id=?1", [old_root])?;
            }
            for (profile, legacy) in unimported {
                tx.execute_batch("SAVEPOINT import_profile")?;
                match insert_legacy_bookmarks(&tx, &legacy, root) {
                    Ok(()) => {
                        tx.execute(
                            "INSERT INTO bookmark_profile_migrations(profile_id) VALUES (?1)",
                            [profile.to_string()],
                        )?;
                        tx.execute_batch("RELEASE import_profile")?;
                    }
                    Err(_) => {
                        tx.execute_batch("ROLLBACK TO import_profile; RELEASE import_profile")?;
                        self.degraded_profiles.insert(profile);
                    }
                }
            }
            for (profile, _) in &profiles {
                tx.execute(
                    "INSERT INTO profile_bookmark_folders(profile_id, folder_id) VALUES (?1, ?2)
                     ON CONFLICT(profile_id) DO UPDATE SET folder_id=excluded.folder_id",
                    params![profile.to_string(), root],
                )?;
                if !self.degraded_profiles.contains(profile) {
                    tx.execute(
                        "INSERT INTO bookmark_profile_migrations(profile_id) VALUES (?1) ON CONFLICT(profile_id) DO NOTHING",
                        [profile.to_string()],
                    )?;
                }
            }
            tx.commit()?;
            return Ok(());
        }
        for (profile, name) in profiles {
            if self.degraded_profiles.contains(&profile) {
                continue;
            }
            let mapped: bool = self.meta.query_row(
                "SELECT EXISTS(SELECT 1 FROM profile_bookmark_folders WHERE profile_id=?1)",
                [profile.to_string()],
                |row| row.get(0),
            )?;
            let imported: bool = self.meta.query_row(
                "SELECT EXISTS(SELECT 1 FROM bookmark_profile_migrations WHERE profile_id=?1)",
                [profile.to_string()],
                |row| row.get(0),
            )?;
            if mapped && imported {
                continue;
            }

            let legacy = match self
                .profile_conn(profile)
                .and_then(|conn| read_legacy_bookmarks(&conn))
            {
                Ok(legacy) => legacy,
                Err(_) => {
                    self.degraded_profiles.insert(profile);
                    continue;
                }
            };

            let tx = self
                .meta
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let already_imported: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM bookmark_profile_migrations WHERE profile_id=?1)",
                [profile.to_string()],
                |row| row.get(0),
            )?;
            let root = tx
                .query_row(
                    "SELECT folder_id FROM profile_bookmark_folders WHERE profile_id=?1",
                    [profile.to_string()],
                    |row| row.get::<_, i64>(0),
                )
                .optional()?
                .map(Ok)
                .unwrap_or_else(|| {
                    tx.query_row(
                        "INSERT INTO bookmarks(parent_id, position, title, url, added_at)
                         VALUES (NULL, coalesce((SELECT max(position) + 1 FROM bookmarks WHERE parent_id IS NULL), 0), ?1, NULL, ?2)
                         RETURNING id",
                        params![name, now_secs()],
                        |row| row.get::<_, i64>(0),
                    )
                })?;
            if already_imported {
                let has_mapping: bool = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM profile_bookmark_folders)",
                    [],
                    |row| row.get(0),
                )?;
                if !has_mapping && !mapped {
                    tx.execute(
                        "UPDATE bookmarks SET parent_id=?1 WHERE parent_id IS NULL AND id != ?1",
                        [root],
                    )?;
                }
            } else {
                tx.execute_batch("SAVEPOINT import_profile")?;
                if insert_legacy_bookmarks(&tx, &legacy, root).is_err() {
                    tx.execute_batch("ROLLBACK TO import_profile; RELEASE import_profile")?;
                    self.degraded_profiles.insert(profile);
                    tx.commit()?;
                    continue;
                }
                tx.execute(
                    "INSERT INTO bookmark_profile_migrations(profile_id) VALUES (?1)",
                    [profile.to_string()],
                )?;
                tx.execute_batch("RELEASE import_profile")?;
            }
            if !mapped {
                tx.execute(
                    "INSERT INTO profile_bookmark_folders(profile_id, folder_id) VALUES (?1, ?2)",
                    params![profile.to_string(), root],
                )?;
            }
            tx.commit()?;
        }
        Ok(())
    }

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
        let result = self.prepare_shared_bookmarks().and_then(|()| {
            let root = self.meta.query_row(
                "SELECT folder_id FROM profile_bookmark_folders WHERE profile_id=?1",
                [profile.to_string()],
                |row| row.get::<_, i64>(0),
            )?;
            apply_scoped(&mut self.meta, root, request)
        });
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
        if !self.registry.contains(&profile)
            || self.degraded_profiles.contains(&profile)
            || self.recovery_required.is_some()
            || self.prepare_shared_bookmarks().is_err()
        {
            return (BookmarkReply::Failed(BookmarkFailure::Unavailable), None);
        }
        let target = match &request {
            BookmarkRequest::Rename { id, .. }
            | BookmarkRequest::Move { id, .. }
            | BookmarkRequest::Relocate { id, .. }
            | BookmarkRequest::Update { id, .. }
            | BookmarkRequest::Remove { id }
            | BookmarkRequest::Delete { id, .. } => Some(*id),
            _ => None,
        };
        let root = self
            .meta
            .query_row(
                "SELECT folder_id FROM profile_bookmark_folders WHERE profile_id=?1",
                [profile.to_string()],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .ok()
            .flatten();
        let existing_link = match (&request, root) {
            (
                BookmarkRequest::AddLink {
                    url,
                    if_absent: true,
                    ..
                },
                Some(root),
            ) => self
                .meta
                .query_row(
                    "WITH RECURSIVE tree(id) AS (
                         SELECT id FROM bookmarks WHERE id=?1
                         UNION ALL SELECT b.id FROM bookmarks b JOIN tree ON b.parent_id=tree.id
                     ) SELECT id FROM bookmarks WHERE url=?2 AND id IN (SELECT id FROM tree)
                     ORDER BY id LIMIT 1",
                    params![root, url],
                    |row| row.get::<_, i64>(0),
                )
                .optional()
                .ok()
                .flatten()
                .map(|_| url.clone()),
            _ => None,
        };
        let before = target.and_then(|id| visible_entry(&self.meta, root, id));
        let existed = existing_link.and_then(|url| {
            self.meta
                .query_row(
                    "WITH RECURSIVE tree(id) AS (
                         SELECT id FROM bookmarks WHERE id=?1
                         UNION ALL SELECT b.id FROM bookmarks b JOIN tree ON b.parent_id=tree.id
                     ) SELECT id FROM bookmarks WHERE url=?2 AND id IN (SELECT id FROM tree)
                     ORDER BY id LIMIT 1",
                    params![root?, url],
                    |row| row.get(0),
                )
                .optional()
                .ok()
                .flatten()
        });
        let imported = matches!(request, BookmarkRequest::Import { .. });
        let creates = matches!(request, BookmarkRequest::Create { .. });
        let reply = self.bookmarks(profile, request);
        let after = |hub: &mut Self, id: i64| visible_entry(&hub.meta, root, id);
        let change = match (&reply, before) {
            (BookmarkReply::Added(id), _) if existed != Some(*id) => {
                after(self, *id).map(BookmarkChange::Created)
            }
            (BookmarkReply::Entries(entries), None) if creates => {
                entries.first().cloned().map(BookmarkChange::Created)
            }
            (BookmarkReply::Imported { added, .. }, _) if imported && *added > 0 => {
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

fn in_folder(conn: &Connection, root: i64, id: i64) -> rusqlite::Result<bool> {
    conn.query_row(
        "WITH RECURSIVE up(id) AS (
             SELECT ?1
             UNION ALL
             SELECT b.parent_id FROM bookmarks b JOIN up ON b.id=up.id WHERE b.parent_id IS NOT NULL
         ) SELECT EXISTS(SELECT 1 FROM up WHERE id=?2)",
        params![id, root],
        |row| row.get(0),
    )
}

fn apply_scoped(
    conn: &mut Connection,
    root: i64,
    request: BookmarkRequest,
) -> Outcome<BookmarkReply> {
    use BookmarkRequest as R;
    let visible_id =
        |id: i64| -> rusqlite::Result<bool> { Ok(id != root && in_folder(conn, root, id)?) };
    macro_rules! scoped_parent {
        ($parent:expr) => {{
            match $parent {
                None => Some(root),
                Some(id) if id == root || in_folder(conn, root, id)? => Some(id),
                Some(_) => return Ok(Err(Refused(BookmarkFailure::Missing))),
            }
        }};
    }
    let scoped = match request {
        R::Children { parent: at } => R::Children {
            parent: scoped_parent!(at),
        },
        R::Path { id } => {
            if !visible_id(id)? {
                return Ok(Err(Refused(BookmarkFailure::Missing)));
            }
            R::Path { id }
        }
        R::Search { query } => R::Search { query },
        R::All => R::All,
        R::AddLink {
            parent: at,
            title,
            url,
            if_absent,
        } => R::AddLink {
            parent: scoped_parent!(at),
            title,
            url,
            if_absent,
        },
        R::AddFolder { parent: at, title } => R::AddFolder {
            parent: scoped_parent!(at),
            title,
        },
        R::Rename { id, title } => {
            if !visible_id(id)? {
                return Ok(Err(Refused(BookmarkFailure::Missing)));
            }
            R::Rename { id, title }
        }
        R::Move {
            id,
            parent: at,
            index,
        } => {
            if !visible_id(id)? {
                return Ok(Err(Refused(BookmarkFailure::Missing)));
            }
            R::Move {
                id,
                parent: scoped_parent!(at),
                index,
            }
        }
        R::Import { folder, nodes } => R::Import { folder, nodes },
        R::Remove { id } => {
            if !visible_id(id)? {
                return Ok(Err(Refused(BookmarkFailure::Missing)));
            }
            R::Remove { id }
        }
        R::Entries { ids } => {
            for id in &ids {
                if !visible_id(*id)? {
                    return Ok(Err(Refused(BookmarkFailure::Missing)));
                }
            }
            R::Entries { ids }
        }
        R::ChildEntries { parent: at } => R::ChildEntries {
            parent: scoped_parent!(at),
        },
        R::Subtree { root: at } => {
            let target = match at {
                None => root,
                Some(id) if id == root || in_folder(conn, root, id)? => id,
                Some(_) => return Ok(Err(Refused(BookmarkFailure::Missing))),
            };
            R::Subtree { root: Some(target) }
        }
        R::Recent { limit } => R::Recent { limit },
        R::Find { query, url, title } => R::Find { query, url, title },
        R::Create {
            parent: at,
            index,
            title,
            url,
        } => R::Create {
            parent: scoped_parent!(at),
            index,
            title,
            url,
        },
        R::Update { id, title, url } => {
            if !visible_id(id)? {
                return Ok(Err(Refused(BookmarkFailure::Missing)));
            }
            R::Update { id, title, url }
        }
        R::Relocate {
            id,
            parent: at,
            index,
        } => {
            if !visible_id(id)? {
                return Ok(Err(Refused(BookmarkFailure::Missing)));
            }
            let parent = match at {
                None => None,
                Some(None) => Some(Some(root)),
                Some(Some(id)) if id == root || in_folder(conn, root, id)? => Some(Some(id)),
                Some(Some(_)) => return Ok(Err(Refused(BookmarkFailure::Missing))),
            };
            R::Relocate { id, parent, index }
        }
        R::Delete { id, recursive } => {
            if !visible_id(id)? {
                return Ok(Err(Refused(BookmarkFailure::Missing)));
            }
            R::Delete { id, recursive }
        }
    };
    let mut reply = apply_at(conn, scoped, Some(root))?;
    let Ok(value) = &mut reply else {
        return Ok(reply);
    };
    match value {
        BookmarkReply::Nodes(nodes) => {
            nodes.retain(|node| node.id != root && in_folder(conn, root, node.id).unwrap_or(false));
            for node in nodes {
                if node.parent == Some(root) {
                    node.parent = None;
                }
            }
        }
        BookmarkReply::Entries(entries) => {
            entries.retain(|entry| {
                entry.id != root && in_folder(conn, root, entry.id).unwrap_or(false)
            });
            for entry in entries {
                if entry.parent == Some(root) {
                    entry.parent = None;
                }
            }
        }
        _ => {}
    }
    Ok(reply)
}

fn apply_at(
    conn: &mut Connection,
    request: BookmarkRequest,
    scope_root: Option<i64>,
) -> Outcome<BookmarkReply> {
    let scoped = scope_root.map_or_else(String::new, |root| {
        format!(
            " AND b.id IN (WITH RECURSIVE tree(id) AS (
                 SELECT id FROM bookmarks WHERE id={root}
                 UNION ALL SELECT child.id FROM bookmarks child JOIN tree ON child.parent_id=tree.id
             ) SELECT id FROM tree WHERE id!={root})"
        )
    });
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
                 ORDER BY b.url IS NOT NULL, b.position, b.id
                 LIMIT max(?2, (SELECT count(*) FROM bookmarks
                                WHERE parent_id IS ?1 AND url IS NULL))"
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
                 WHERE b.url IS NOT NULL{matches}{scoped}
                 ORDER BY b.added_at DESC, b.id DESC LIMIT {limit}"
            ))?;
            let nodes = rows
                .query_map(rusqlite::params_from_iter(&patterns), node)?
                .collect::<rusqlite::Result<_>>()?;
            Ok(Ok(BookmarkReply::Nodes(nodes)))
        }
        BookmarkRequest::All => {
            let mut rows = conn.prepare_cached(&format!(
                "SELECT b.id, b.parent_id, b.url, b.title, 0 FROM bookmarks b
                 WHERE 1=1{scoped} ORDER BY b.parent_id, b.position, b.id LIMIT ?1"
            ))?;
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
                        "WITH RECURSIVE tree(id) AS (
                             SELECT id FROM bookmarks WHERE id=?1
                             UNION ALL SELECT b.id FROM bookmarks b JOIN tree ON b.parent_id=tree.id
                         ) SELECT id FROM bookmarks WHERE url=?2 AND id IN (SELECT id FROM tree)
                         ORDER BY id LIMIT 1",
                        params![scope_root, url],
                        |row| row.get::<_, i64>(0),
                    )
                    .optional()?;
                if let Some(existing) = existing {
                    return Ok(Ok(BookmarkReply::Added(existing)));
                }
            }
            let added = insert(&tx, parent, &title, Some(&url), scope_root)?;
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
            let added = insert(&tx, parent, &title, None, scope_root)?;
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
            let moved = relocate(&tx, id, parent, index, true)?;
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
            let count: i64 = tx.query_row(
                "WITH RECURSIVE tree(id) AS (
                     SELECT id FROM bookmarks WHERE parent_id=?1
                     UNION ALL SELECT b.id FROM bookmarks b JOIN tree ON b.parent_id=tree.id
                 ) SELECT count(*) FROM tree",
                [scope_root],
                |row| row.get(0),
            )?;
            let mut import = Import {
                tx: &tx,
                budget: u32::try_from(i64::from(MAX_BOOKMARKS) - count).unwrap_or(0),
                added: 0,
                partial: false,
                now: SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_or(0, |elapsed| elapsed.as_secs() as i64),
            };
            let Some(root) = import.folder(scope_root, &title)? else {
                let mut remaining: Vec<&ImportNode> = nodes.iter().collect();
                let mut partial = false;
                while let Some(node) = remaining.pop() {
                    match node {
                        ImportNode::Link { url, .. } => {
                            if navigation::is_allowed_str(url) {
                                partial = true;
                                break;
                            }
                        }
                        ImportNode::Folder { children, .. } => remaining.extend(children),
                    }
                }
                return Ok(Ok(BookmarkReply::Imported { added: 0, partial }));
            };
            import.merge(root, 1, &nodes)?;
            let added = import.added;
            let partial = import.partial;
            if added == 0 {
                // Nothing came over, so nothing is kept: not the import's
                // folder, not an empty one inside it. Dropping the
                // transaction undoes them.
                return Ok(Ok(BookmarkReply::Imported { added: 0, partial }));
            }
            tx.commit()?;
            Ok(Ok(BookmarkReply::Imported { added, partial }))
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
            let hidden_root = root.is_some_and(|id| Some(id) == scope_root);
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
                        .query_map(
                            params![root, i64::from(MAX_SUBTREE) + i64::from(!hidden_root)],
                            entry_row,
                        )?
                        .collect::<rusqlite::Result<Vec<_>>>()?;
                    if !hidden_root {
                        below.insert(0, top);
                    }
                    below
                }
            };
            if entries.len() > MAX_SUBTREE as usize {
                return Ok(Err(Refused(BookmarkFailure::Full)));
            }
            let top = root.filter(|_| !hidden_root).map(|_| entries.remove(0));
            let mut entries = numbered(entries);
            if let Some(top) = top {
                entries.insert(0, top);
            }
            Ok(Ok(BookmarkReply::Entries(entries)))
        }
        BookmarkRequest::Recent { limit } => {
            let entries = conn
                .prepare_cached(&format!(
                    "SELECT {ENTRY_COLUMNS} FROM bookmarks b WHERE b.url IS NOT NULL{scoped}
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
                    "SELECT {ENTRY_COLUMNS} FROM bookmarks b WHERE 1{clauses}{scoped}
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
            let id = match insert(&tx, parent, &title, url.as_deref(), scope_root)? {
                Ok(id) => id,
                Err(refused) => return Ok(Err(refused)),
            };
            if let Some(index) = index {
                if let Err(refused) = relocate(&tx, id, parent, index, false)? {
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
            if let Err(refused) = relocate(&tx, id, parent, index, false)? {
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
    partial: bool,
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
            self.partial = true;
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
                self.partial = true;
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
    let levels = conn.query_row(
        "WITH RECURSIVE up(id, depth) AS (
             SELECT ?1, 1
             UNION ALL
             SELECT b.parent_id, up.depth + 1 FROM bookmarks b JOIN up ON b.id = up.id
             WHERE b.parent_id IS NOT NULL AND up.depth <= ?2
         )
         SELECT max(depth) FROM up",
        params![id, MAX_DEPTH as i64],
        |row| row.get::<_, i64>(0),
    )?;
    let root_depth: i64 = conn.query_row(
        "WITH RECURSIVE up(id) AS (
             SELECT ?1 UNION ALL SELECT b.parent_id FROM bookmarks b JOIN up ON b.id=up.id WHERE b.parent_id IS NOT NULL
         ) SELECT EXISTS(SELECT 1 FROM up JOIN profile_bookmark_folders pbf ON pbf.folder_id=up.id)",
        [id],
        |row| row.get(0),
    )?;
    Ok(usize::try_from(levels.saturating_sub(root_depth)).unwrap_or(usize::MAX))
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
    scope_root: Option<i64>,
) -> Outcome<i64> {
    if let Some(parent) = parent {
        if let Err(refused) = folder(tx, parent)? {
            return Ok(Err(refused));
        }
        if depth(tx, parent)? + 1 > MAX_DEPTH {
            return Ok(Err(Refused(BookmarkFailure::Full)));
        }
    }
    let count: i64 = tx.query_row(
        "WITH RECURSIVE tree(id) AS (
             SELECT id FROM bookmarks WHERE parent_id=?1
             UNION ALL SELECT b.id FROM bookmarks b JOIN tree ON b.parent_id=tree.id
         ) SELECT count(*) FROM tree",
        [scope_root],
        |row| row.get(0),
    )?;
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
    folders_first: bool,
) -> Outcome<()> {
    let is_folder = tx
        .query_row(
            "SELECT url IS NULL FROM bookmarks WHERE id = ?1",
            [id],
            |row| row.get::<_, bool>(0),
        )
        .optional()?;
    let Some(is_folder) = is_folder else {
        return Ok(Err(Refused(BookmarkFailure::Missing)));
    };
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
    let siblings: Vec<(i64, bool)> = tx
        .prepare_cached(
            "SELECT id, url IS NULL FROM bookmarks WHERE parent_id IS ?1 AND id != ?2
             ORDER BY position, id",
        )?
        .query_map(params![parent, id], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let at = if folders_first {
        let folders = siblings.iter().filter(|(_, folder)| *folder).count();
        let index = if is_folder {
            index
        } else {
            index.saturating_sub(folders as u32)
        };
        let peers: Vec<usize> = siblings
            .iter()
            .enumerate()
            .filter_map(|(at, (_, folder))| (*folder == is_folder).then_some(at))
            .collect();
        peers
            .get(index as usize)
            .copied()
            .or_else(|| peers.last().map(|at| at + 1))
            .unwrap_or(siblings.len())
    } else {
        usize::try_from(index).map_or(siblings.len(), |index| index.min(siblings.len()))
    };
    let mut siblings: Vec<i64> = siblings.into_iter().map(|(id, _)| id).collect();
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
            shared_bookmark_folders: Vec::new(),
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

    fn bookmark_root(hub: &Hub, profile: ProfileId) -> i64 {
        hub.meta
            .query_row(
                "SELECT folder_id FROM profile_bookmark_folders WHERE profile_id=?1",
                [profile.to_string()],
                |row| row.get(0),
            )
            .unwrap()
    }

    #[test]
    fn empty_profile_bookmark_folders_remain_separate_after_preparation() {
        let (mut hub, first) = hub();
        let second = ProfileId::from(3);
        hub.meta
            .execute(
                "INSERT INTO profiles(id, name, kind, position) VALUES (?1, 'Work', 'named', 1)",
                [second.to_string()],
            )
            .unwrap();
        hub.registry.insert(second);
        let first_root = bookmark_root(&hub, first);
        let second_root = {
            hub.meta
                .query_row(
                    "INSERT INTO bookmarks(parent_id, position, title, url, added_at)
                     VALUES (NULL, 1, 'Work', NULL, 1) RETURNING id",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap()
        };
        hub.meta
            .execute(
                "INSERT INTO profile_bookmark_folders(profile_id, folder_id) VALUES (?1, ?2)",
                params![second.to_string(), second_root],
            )
            .unwrap();
        for profile in [first, second] {
            hub.meta
                .execute(
                    "INSERT INTO bookmark_profile_migrations(profile_id) VALUES (?1)
                     ON CONFLICT(profile_id) DO NOTHING",
                    [profile.to_string()],
                )
                .unwrap();
        }

        hub.prepare_shared_bookmarks().unwrap();
        hub.prepare_shared_bookmarks().unwrap();

        assert_eq!(bookmark_root(&hub, first), first_root);
        assert_eq!(bookmark_root(&hub, second), second_root);
    }

    #[test]
    fn shared_folder_import_skips_overlapping_urls_but_keeps_source_duplicates() {
        let (mut hub, first) = hub();
        let root = bookmark_root(&hub, first);
        assert_eq!(
            hub.bookmarks(first, link(None, "Shared", "https://shared.example/")),
            BookmarkReply::Added(2)
        );
        let second = ProfileId::from(3);
        hub.meta
            .execute(
                "INSERT INTO profiles(id, name, kind, position) VALUES (?1, 'Work', 'named', 1)",
                [second.to_string()],
            )
            .unwrap();
        hub.registry.insert(second);
        hub.meta
            .execute(
                "INSERT INTO profile_bookmark_folders(profile_id, folder_id) VALUES (?1, ?2)",
                params![second.to_string(), root],
            )
            .unwrap();
        let legacy = hub.profile_conn(second).unwrap();
        for (position, title, url) in [
            (0, "Same URL", "https://shared.example/"),
            (1, "Duplicate one", "https://other.example/"),
            (2, "Duplicate two", "https://other.example/"),
        ] {
            legacy
                .execute(
                    "INSERT INTO bookmarks(parent_id, position, title, url, added_at)
                     VALUES (NULL, ?1, ?2, ?3, 1)",
                    params![position, title, url],
                )
                .unwrap();
        }

        hub.prepare_shared_bookmarks().unwrap();

        assert_eq!(
            titles(hub.bookmarks(first, BookmarkRequest::Children { parent: None })),
            vec!["Shared", "Duplicate one", "Duplicate two"]
        );
        assert_eq!(bookmark_root(&hub, second), root);
    }

    fn link(parent: Option<i64>, title: &str, url: &str) -> BookmarkRequest {
        BookmarkRequest::AddLink {
            parent,
            title: title.into(),
            url: url.into(),
            if_absent: false,
        }
    }

    #[test]
    fn legacy_bookmarks_migrate_into_separate_folders_and_shared_choices_share_only_bookmarks() {
        let (mut hub, first) = hub();
        let second = ProfileId::from(3);
        hub.meta
            .execute(
                "INSERT INTO profiles(id, name, kind, position) VALUES (?1, 'Work', 'named', 1)",
                [second.to_string()],
            )
            .unwrap();
        hub.registry.insert(second);

        for (profile, title, url) in [
            (first, "Personal", "https://personal.example/"),
            (second, "Work", "https://work.example/"),
        ] {
            let conn = hub.profile_conn(profile).unwrap();
            conn.execute(
                "INSERT INTO bookmarks(parent_id, position, title, url, added_at)
                 VALUES (NULL, 0, ?1, ?2, 1)",
                params![title, url],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO history(url, title, visited_at) VALUES (?1, ?2, 1)",
                params![url, title],
            )
            .unwrap();
        }

        let BookmarkReply::Nodes(first_nodes) =
            hub.bookmarks(first, BookmarkRequest::Children { parent: None })
        else {
            panic!("expected the migrated bookmarks");
        };
        let personal = first_nodes
            .iter()
            .find(|node| node.title == "Personal")
            .unwrap()
            .id;
        assert_eq!(
            hub.bookmark_folders(first)
                .into_iter()
                .map(|folder| folder.title)
                .collect::<Vec<_>>(),
            vec!["Personal", "Work"]
        );
        assert_eq!(
            titles(hub.bookmarks(second, BookmarkRequest::Children { parent: None })),
            vec!["Work"]
        );
        assert_eq!(
            hub.bookmarks(second, BookmarkRequest::Path { id: personal }),
            BookmarkReply::Failed(BookmarkFailure::Missing)
        );
        assert_eq!(
            hub.bookmarks(
                second,
                BookmarkRequest::Entries {
                    ids: vec![personal]
                }
            ),
            BookmarkReply::Failed(BookmarkFailure::Missing)
        );
        assert_eq!(
            titles(hub.bookmarks(
                second,
                BookmarkRequest::Search {
                    query: "Personal".into()
                },
            )),
            Vec::<String>::new()
        );
        assert_eq!(
            titles(hub.bookmarks(second, BookmarkRequest::All)),
            vec!["Work"]
        );
        assert_eq!(
            hub.bookmarks(
                second,
                BookmarkRequest::Subtree {
                    root: Some(personal)
                }
            ),
            BookmarkReply::Failed(BookmarkFailure::Missing)
        );
        assert!(matches!(
            hub.bookmarks(second, BookmarkRequest::Recent { limit: 10 }),
            BookmarkReply::Entries(entries) if entries.iter().map(|entry| entry.title.as_str()).collect::<Vec<_>>() == ["Work"]
        ));
        assert!(matches!(
            hub.bookmarks(
                second,
                BookmarkRequest::Find {
                    query: None,
                    url: Some("https://personal.example/".into()),
                    title: None,
                },
            ),
            BookmarkReply::Entries(entries) if entries.is_empty()
        ));
        assert_eq!(
            hub.bookmarks(
                second,
                BookmarkRequest::Rename {
                    id: personal,
                    title: "Renamed in Work".into(),
                },
            ),
            BookmarkReply::Failed(BookmarkFailure::Missing)
        );
        let (reply, change) = hub.bookmarks_observed(
            second,
            BookmarkRequest::Rename {
                id: personal,
                title: "Not visible in Work".into(),
            },
        );
        assert_eq!(reply, BookmarkReply::Failed(BookmarkFailure::Missing));
        assert_eq!(change, None);
        assert_eq!(
            hub.bookmarks(
                first,
                BookmarkRequest::Rename {
                    id: personal,
                    title: "Renamed in Personal".into(),
                },
            ),
            BookmarkReply::Done
        );
        assert_eq!(
            titles(hub.bookmarks(first, BookmarkRequest::Children { parent: None })),
            vec!["Renamed in Personal"]
        );
        assert_eq!(
            titles(hub.bookmarks(second, BookmarkRequest::Children { parent: None })),
            vec!["Work"]
        );

        let personal_folder: i64 = hub
            .meta
            .query_row(
                "SELECT folder_id FROM profile_bookmark_folders WHERE profile_id=?1",
                [first.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        hub.meta
            .execute(
                "UPDATE profile_bookmark_folders SET folder_id=?2 WHERE profile_id=?1",
                params![second.to_string(), personal_folder],
            )
            .unwrap();
        assert_eq!(
            titles(hub.bookmarks(second, BookmarkRequest::Children { parent: None })),
            vec!["Renamed in Personal"]
        );

        let first_history: i64 = hub
            .profile_conn(first)
            .unwrap()
            .query_row("SELECT count(*) FROM history", [], |row| row.get(0))
            .unwrap();
        let second_history: i64 = hub
            .profile_conn(second)
            .unwrap()
            .query_row("SELECT count(*) FROM history", [], |row| row.get(0))
            .unwrap();
        assert_eq!((first_history, second_history), (1, 1));
        assert_eq!(
            hub.profile_conn(first)
                .unwrap()
                .query_row("SELECT url FROM history", [], |row| row.get::<_, String>(0))
                .unwrap(),
            "https://personal.example/"
        );
        assert_eq!(
            hub.profile_conn(second)
                .unwrap()
                .query_row("SELECT url FROM history", [], |row| row.get::<_, String>(0))
                .unwrap(),
            "https://work.example/"
        );

        let imported_profiles: i64 = hub
            .meta
            .query_row(
                "SELECT count(*) FROM bookmark_profile_migrations",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(imported_profiles, 2);
    }

    #[test]
    fn an_unimportable_legacy_tree_does_not_block_other_profiles() {
        let (mut hub, profile) = hub();
        let other = ProfileId::from(3);
        let space = SpaceId::from(4);
        hub.save(&SessionState {
            profiles: vec![
                PersistedProfile {
                    id: profile,
                    name: "Personal".into(),
                    kind: ProfileKind::Default,
                },
                PersistedProfile {
                    id: other,
                    name: "Broken".into(),
                    kind: ProfileKind::Named,
                },
            ],
            shared_bookmark_folders: Vec::new(),
            spaces: vec![
                PersistedSpace {
                    id: SpaceId::from(2),
                    profile,
                    name: "Personal".into(),
                    look: Default::default(),
                },
                PersistedSpace {
                    id: space,
                    profile: other,
                    name: "Broken".into(),
                    look: Default::default(),
                },
            ],
            items: Vec::new(),
            active_space: Some(SpaceId::from(2)),
            active_item: None,
            splits: None,
            recently_closed: Vec::new(),
        })
        .unwrap();
        let legacy = hub.profile_conn(other).unwrap();
        legacy
            .execute(
                "INSERT INTO bookmarks(parent_id,position,title,url,added_at) VALUES (NULL,0,'Loop',NULL,1)",
                [],
            )
            .unwrap();
        let id = legacy.last_insert_rowid();
        legacy
            .execute("UPDATE bookmarks SET parent_id=?1 WHERE id=?1", [id])
            .unwrap();

        assert!(matches!(
            hub.bookmarks(profile, BookmarkRequest::Children { parent: None }),
            BookmarkReply::Nodes(_)
        ));
        assert!(hub.degraded_profiles.contains(&other));
        let surviving = SessionState {
            profiles: vec![PersistedProfile {
                id: other,
                name: "Broken".into(),
                kind: ProfileKind::Named,
            }],
            shared_bookmark_folders: Vec::new(),
            spaces: vec![PersistedSpace {
                id: space,
                profile: other,
                name: "Broken".into(),
                look: Default::default(),
            }],
            items: Vec::new(),
            active_space: Some(space),
            active_item: None,
            splits: None,
            recently_closed: Vec::new(),
        };
        assert_eq!(
            hub.authorize_profile_deletion(profile, &surviving).unwrap(),
            zephium_core::ports::store::ProfileDeletionAuthorizeOutcome::Authorized
        );
    }

    #[test]
    fn a_v30_shared_bookmark_tree_stays_shared_during_folder_migration() {
        let (mut hub, first) = hub();
        let first_root = bookmark_root(&hub, first);
        hub.meta
            .execute(
                "INSERT INTO bookmarks(parent_id,position,title,url,added_at) VALUES (?1,0,'Personal saved','https://personal.example/',1)",
                [first_root],
            )
            .unwrap();
        let second = ProfileId::from(3);
        hub.meta
            .execute(
                "INSERT INTO profiles(id,name,kind,position) VALUES (?1,'Work','named',1)",
                [second.to_string()],
            )
            .unwrap();
        hub.registry.insert(second);
        hub.meta
            .execute(
                "INSERT INTO bookmarks(parent_id,position,title,url,added_at) VALUES(NULL,1,'Work',NULL,1)",
                [],
            )
            .unwrap();
        let second_root = hub.meta.last_insert_rowid();
        hub.meta
            .execute(
                "INSERT INTO profile_bookmark_folders(profile_id,folder_id) VALUES (?1,?2)",
                params![second.to_string(), second_root],
            )
            .unwrap();
        hub.meta
            .execute(
                "INSERT INTO bookmarks(parent_id,position,title,url,added_at) VALUES (?1,0,'Work saved','https://work.example/',1)",
                [second_root],
            )
            .unwrap();
        hub.meta
            .execute(
                "INSERT INTO bookmarks(parent_id,position,title,url,added_at) VALUES(NULL,0,'Shared link','https://shared.example/',1)",
                [],
            )
            .unwrap();
        for profile in [first, second] {
            hub.meta
                .execute(
                    "INSERT INTO bookmark_profile_migrations(profile_id) VALUES (?1)",
                    [profile.to_string()],
                )
                .unwrap();
        }

        let folders = hub.bookmark_folders(first);
        assert_eq!(folders.len(), 1);
        let first_folder: i64 = hub
            .meta
            .query_row(
                "SELECT folder_id FROM profile_bookmark_folders WHERE profile_id=?1",
                [first.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        let second_folder: i64 = hub
            .meta
            .query_row(
                "SELECT folder_id FROM profile_bookmark_folders WHERE profile_id=?1",
                [second.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(first_folder, second_folder);
        assert_eq!(first_folder, first_root);
        let shared_urls: Vec<String> = {
            let mut statement = hub
                .meta
                .prepare("SELECT url FROM bookmarks WHERE parent_id=?1 ORDER BY position, id")
                .unwrap();
            statement
                .query_map([first_folder], |row| row.get(0))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap()
        };
        assert_eq!(
            shared_urls,
            vec![
                "https://personal.example/".to_owned(),
                "https://shared.example/".to_owned(),
                "https://work.example/".to_owned()
            ]
        );
        assert_eq!(
            titles(hub.bookmarks(second, BookmarkRequest::Children { parent: None })),
            vec!["Personal saved", "Shared link", "Work saved"]
        );
    }

    #[test]
    fn partial_v30_imports_with_empty_mappings_keep_the_shared_tree_visible() {
        let (mut hub, first) = hub();
        let second = ProfileId::from(3);
        hub.meta
            .execute(
                "INSERT INTO profiles(id,name,kind,position) VALUES (?1,'Work','named',1)",
                [second.to_string()],
            )
            .unwrap();
        hub.registry.insert(second);
        let second_root: i64 = hub
            .meta
            .query_row(
                "INSERT INTO bookmarks(parent_id,position,title,url,added_at) VALUES(NULL,1,'Work',NULL,1) RETURNING id",
                [],
                |row| row.get(0),
            )
            .unwrap();
        hub.meta
            .execute(
                "INSERT INTO profile_bookmark_folders(profile_id,folder_id) VALUES (?1,?2)",
                params![second.to_string(), second_root],
            )
            .unwrap();
        hub.meta
            .execute(
                "INSERT INTO bookmark_profile_migrations(profile_id) VALUES (?1)",
                [first.to_string()],
            )
            .unwrap();
        hub.meta
            .execute(
                "INSERT INTO bookmarks(parent_id,position,title,url,added_at) VALUES(NULL,0,'Shared link','https://shared.example/',1)",
                [],
            )
            .unwrap();
        let legacy = hub.profile_conn(second).unwrap();
        legacy
            .execute(
                "INSERT INTO bookmarks(parent_id,position,title,url,added_at) VALUES(NULL,0,'Work link','https://work.example/',1)",
                [],
            )
            .unwrap();

        assert_eq!(
            titles(hub.bookmarks(first, BookmarkRequest::Children { parent: None })),
            vec!["Shared link", "Work link"]
        );
        let first_root = bookmark_root(&hub, first);
        assert_eq!(bookmark_root(&hub, second), first_root);
        assert_eq!(
            titles(hub.bookmarks(second, BookmarkRequest::Children { parent: None })),
            vec!["Shared link", "Work link"]
        );
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
    fn folder_listings_keep_all_subfolders_past_the_link_and_folder_caps() {
        for nested in [false, true] {
            let (mut hub, profile) = hub();
            let root = bookmark_root(&hub, profile);
            let parent = nested
                .then(|| {
                    added(hub.bookmarks(
                        profile,
                        BookmarkRequest::AddFolder {
                            parent: Some(root),
                            title: "Parent".into(),
                        },
                    ))
                })
                .unwrap_or(root);
            let conn = &hub.meta;
            conn.execute(
                "WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x < ?2)
                 INSERT INTO bookmarks(parent_id, position, title, url, added_at)
                 SELECT ?1, x, 'Link ' || x, 'https://example.com/' || x, 0 FROM n",
                params![parent, MAX_LISTING + 1],
            )
            .unwrap();
            let folder = added(hub.bookmarks(
                profile,
                BookmarkRequest::AddFolder {
                    parent: Some(parent),
                    title: "Last folder".into(),
                },
            ));
            let BookmarkReply::Nodes(nodes) = hub.bookmarks(
                profile,
                BookmarkRequest::Children {
                    parent: Some(parent),
                },
            ) else {
                panic!("expected a listing");
            };
            assert_eq!(nodes.len(), MAX_LISTING as usize);
            assert!(nodes.iter().any(|node| node.id == folder));
            assert!(nodes[0].is_folder());
            assert!(nodes.iter().any(|node| !node.is_folder()));

            let conn = &hub.meta;
            conn.execute(
                "WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x < ?2)
                 INSERT INTO bookmarks(parent_id, position, title, url, added_at)
                 SELECT ?1, ?2 + x, 'Folder ' || x, NULL, 0 FROM n",
                params![parent, MAX_LISTING],
            )
            .unwrap();
            let BookmarkReply::Nodes(nodes) = hub.bookmarks(
                profile,
                BookmarkRequest::Children {
                    parent: Some(parent),
                },
            ) else {
                panic!("expected a listing");
            };
            assert!(nodes.len() > MAX_LISTING as usize);
            assert!(nodes.iter().all(BookmarkNode::is_folder));
            assert_eq!(nodes.last().unwrap().title, format!("Folder {MAX_LISTING}"));
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

        let nested = added(hub.bookmarks(
            profile,
            BookmarkRequest::AddFolder {
                parent: None,
                title: "Nested".into(),
            },
        ));
        let nested_link = made(hub.bookmarks(
            profile,
            create(
                Some(nested),
                None,
                "Nested link",
                Some("https://nested.example/"),
            ),
        ));
        assert_eq!(
            hub.bookmarks(
                profile,
                BookmarkRequest::AddLink {
                    parent: None,
                    title: "Duplicate".into(),
                    url: "https://nested.example/".into(),
                    if_absent: true,
                },
            ),
            BookmarkReply::Added(nested_link)
        );
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
        assert_eq!(
            import(&mut hub, false),
            BookmarkReply::Imported {
                added: 2,
                partial: false
            }
        );
        assert_eq!(
            import(&mut hub, true),
            BookmarkReply::Imported {
                added: 1,
                partial: false
            }
        );
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
            vec!["Docs", "https://news.example/"]
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
        assert_eq!(
            import(&mut hub, vec![]),
            BookmarkReply::Imported {
                added: 0,
                partial: false
            }
        );
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
            BookmarkReply::Imported {
                added: 0,
                partial: false
            }
        );
        assert!(top(&mut hub).is_empty());

        // An import that did bring something is kept, and a later one with
        // nothing new keeps it as it was.
        let kept = vec![ImportNode::Link {
            title: "Яндекс".into(),
            url: "https://ya.ru/".into(),
        }];
        assert_eq!(
            import(&mut hub, kept.clone()),
            BookmarkReply::Imported {
                added: 1,
                partial: false
            }
        );
        assert_eq!(
            import(&mut hub, kept),
            BookmarkReply::Imported {
                added: 0,
                partial: false
            }
        );
        assert_eq!(
            import(&mut hub, vec![]),
            BookmarkReply::Imported {
                added: 0,
                partial: false
            }
        );
        let folders = top(&mut hub);
        assert_eq!(folders.len(), 1);
        assert_eq!(folders[0].children, 1);
    }

    #[test]
    fn capacity_limited_import_reports_partial_including_zero_added() {
        let (mut hub, profile) = hub();
        let root = bookmark_root(&hub, profile);
        hub.meta
            .execute(
                "WITH RECURSIVE rows(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM rows WHERE n < ?1)
             INSERT INTO bookmarks(parent_id, position, title, url, added_at)
             SELECT ?2, n, 'Kept', 'https://kept.example/' || n, 0 FROM rows",
                params![MAX_BOOKMARKS - 2, root],
            )
            .unwrap();
        let request = || BookmarkRequest::Import {
            folder: "Imported".into(),
            nodes: vec![
                ImportNode::Link {
                    title: "One".into(),
                    url: "https://one.example/".into(),
                },
                ImportNode::Link {
                    title: "Two".into(),
                    url: "https://two.example/".into(),
                },
            ],
        };
        assert_eq!(
            hub.bookmarks(profile, request()),
            BookmarkReply::Imported {
                added: 1,
                partial: true
            }
        );
        assert_eq!(
            hub.bookmarks(profile, request()),
            BookmarkReply::Imported {
                added: 0,
                partial: true
            }
        );
    }

    #[test]
    fn independent_bookmark_roots_have_independent_quotas() {
        let (mut hub, first) = hub();
        let second = ProfileId::from(3);
        hub.meta
            .execute(
                "INSERT INTO profiles(id,name,kind,position) VALUES (?1,'Work','named',1)",
                [second.to_string()],
            )
            .unwrap();
        hub.registry.insert(second);
        hub.prepare_shared_bookmarks().unwrap();
        let first_root = bookmark_root(&hub, first);
        let second_root = bookmark_root(&hub, second);
        assert_ne!(first_root, second_root);
        hub.meta
            .execute(
                "WITH RECURSIVE rows(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM rows WHERE n < ?1)
                 INSERT INTO bookmarks(parent_id, position, title, url, added_at)
                 SELECT ?2, n, 'Kept', 'https://kept.example/' || n, 0 FROM rows",
                params![MAX_BOOKMARKS, first_root],
            )
            .unwrap();
        assert_eq!(
            hub.bookmarks(
                first,
                BookmarkRequest::AddLink {
                    parent: None,
                    title: "Full".into(),
                    url: "https://full.example/".into(),
                    if_absent: false,
                }
            ),
            BookmarkReply::Failed(BookmarkFailure::Full)
        );
        assert!(matches!(
            hub.bookmarks(
                second,
                BookmarkRequest::AddLink {
                    parent: None,
                    title: "Available".into(),
                    url: "https://available.example/".into(),
                    if_absent: false,
                }
            ),
            BookmarkReply::Added(_)
        ));
    }

    #[test]
    fn a_full_profile_does_not_report_skipped_links_as_a_partial_import() {
        let (mut hub, profile) = hub();
        hub.profile_conn(profile)
            .unwrap()
            .execute(
                "WITH RECURSIVE rows(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM rows WHERE n < ?1)
             INSERT INTO bookmarks(parent_id, position, title, url, added_at)
             SELECT NULL, n, 'Kept', 'https://kept.example/' || n, 0 FROM rows",
                [MAX_BOOKMARKS],
            )
            .unwrap();
        for url in ["javascript:alert(1)", "https://valid.example/"] {
            assert_eq!(
                hub.bookmarks(
                    profile,
                    BookmarkRequest::Import {
                        folder: "Absent".into(),
                        nodes: vec![ImportNode::Folder {
                            title: "Nested".into(),
                            children: vec![ImportNode::Link {
                                title: "Link".into(),
                                url: url.into()
                            }]
                        }]
                    }
                ),
                BookmarkReply::Imported {
                    added: 0,
                    partial: url.starts_with("https:")
                }
            );
        }
    }

    #[test]
    fn panel_moves_follow_folder_first_order_without_changing_extension_indexes() {
        for panel in [true, false] {
            let (mut hub, profile) = hub();
            added(hub.bookmarks(profile, link(None, "Link", "https://link.example/")));
            let a = added(hub.bookmarks(
                profile,
                BookmarkRequest::AddFolder {
                    parent: None,
                    title: "A".into(),
                },
            ));
            added(hub.bookmarks(
                profile,
                BookmarkRequest::AddFolder {
                    parent: None,
                    title: "B".into(),
                },
            ));
            let request = if panel {
                BookmarkRequest::Move {
                    id: a,
                    parent: None,
                    index: 1,
                }
            } else {
                BookmarkRequest::Relocate {
                    id: a,
                    parent: None,
                    index: Some(3),
                }
            };
            hub.bookmarks(profile, request);
            assert_eq!(
                titles(hub.bookmarks(profile, BookmarkRequest::Children { parent: None })),
                ["B", "A", "Link"],
                "panel={panel}"
            );
            let BookmarkReply::Entries(entries) =
                hub.bookmarks(profile, BookmarkRequest::ChildEntries { parent: None })
            else {
                panic!("entries")
            };
            assert_eq!(
                entries
                    .iter()
                    .map(|entry| entry.title.as_str())
                    .collect::<Vec<_>>(),
                vec!["Link", "B", "A"]
            );
        }
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
            BookmarkReply::Imported {
                added: 1,
                partial: false
            }
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
            vec!["Folder", "B", "A"]
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
