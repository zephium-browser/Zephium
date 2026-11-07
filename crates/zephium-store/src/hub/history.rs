//! Bounded per-profile history writes, search, and byte-budget enforcement.

use super::*;

use zephium_core::extension_data::{
    HistoryEntry, HistoryReply, HistoryRequest, TopSite, VisitEntry,
    MAX_HISTORY_RESULTS as MAX_EXTENSION_RESULTS, MAX_REMOVED_URLS, MAX_TOP_SITES, MAX_VISITS,
};
use zephium_core::item::sanitize_page_title;
use zephium_core::ports::store::{
    HistoryHit, HistoryPageError, HistoryVisit, ImportedVisit, MAX_IMPORTED_VISITS,
};

pub(crate) const MAX_HISTORY_QUERY_BYTES: usize = 4 * 1024;
pub(crate) const MAX_HISTORY_RESULTS: u32 = 100;
const MAX_HISTORY_TOKEN_CHARS: usize = 256;
const MAX_VISIT_BATCH: usize = 2048;
pub(crate) const MAX_HISTORY_BYTES: i64 = 64 * 1024 * 1024;
const HISTORY_PRUNE_BATCH: i64 = 2048;
const MAX_HISTORY_PRUNE_PASSES: usize = 26;
/// Visits scanned per query before grouping. Bounds the work without
/// deciding which addresses are allowed to appear.
const MAX_HISTORY_SEARCH_ROWS: i64 = 4096;
/// Retained submitted searches per profile.
const MAX_SUBMITTED_SEARCHES: i64 = 1024;
/// Visits returned in one history page.
pub(crate) const MAX_HISTORY_PAGE: u32 = 200;
/// Addresses one forget request may name.
pub(crate) const MAX_HISTORY_FORGET_URLS: usize = 100;
/// The most visited sites are counted over this much recent history.
const MOST_VISITED_WINDOW_SECONDS: i64 = 90 * 24 * 60 * 60;
/// A title arriving later than this belongs to a different visit.
const TITLE_AMENDMENT_WINDOW_SECONDS: i64 = 60;

impl Hub {
    #[cfg(test)]
    pub(crate) fn record_visit(&mut self, profile: ProfileId, url: &str, title: &str) {
        let _ = self.record_visits([(profile, url.to_owned(), title.to_owned())]);
    }

    pub(crate) fn record_visits(
        &mut self,
        visits: impl IntoIterator<Item = (ProfileId, String, String)>,
    ) -> Result<(), Vec<(ProfileId, String, String)>> {
        let visits: Vec<_> = visits.into_iter().take(MAX_VISIT_BATCH).collect();
        if self.recovery_required.is_some() {
            return Err(visits);
        }
        let mut grouped: HashMap<ProfileId, Vec<(String, String)>> = HashMap::new();
        for (profile, url, title) in visits {
            if self.registry.contains(&profile)
                && !self.degraded_profiles.contains(&profile)
                && navigation::is_allowed_str(&url)
            {
                grouped
                    .entry(profile)
                    .or_default()
                    .push((url, sanitize_page_title(&title)));
            }
        }
        if grouped.is_empty() {
            return Ok(());
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let mut failed = Vec::new();
        for (profile, visits) in grouped {
            let since = self.visits_since_prune.entry(profile).or_insert(0);
            *since = since.saturating_add(u32::try_from(visits.len()).unwrap_or(u32::MAX));
            let prune_rows = *since >= ROW_CAP_PRUNE_EVERY;
            let result = self.profile_conn(profile).and_then(|conn| {
                let tx = conn.transaction()?;
                {
                    let mut insert = tx.prepare_cached(
                        "INSERT INTO history(url, title, visited_at) VALUES (?1, ?2, ?3)",
                    )?;
                    for (url, title) in &visits {
                        insert.execute(params![url, title, now])?;
                    }
                }
                // Bound page-controlled disk growth. The row cap walks up to
                // the cap in visit order, so it runs every few hundred visits
                // and history may briefly hold that many more rows; the byte
                // budget is cheap and holds on every batch.
                if prune_rows {
                    tx.execute(
                        "DELETE FROM history WHERE id IN (
                             SELECT id FROM history
                             ORDER BY visited_at DESC, id DESC
                             LIMIT -1 OFFSET 50000
                         )",
                        [],
                    )?;
                }
                enforce_history_budget(&tx)?;
                tx.commit()
            });
            if result.is_ok() && prune_rows {
                self.visits_since_prune.insert(profile, 0);
            }
            if let Err(e) = result {
                eprintln!("store: record_visits failed for profile {profile}: {e}");
                failed.extend(visits.into_iter().map(|(url, title)| (profile, url, title)));
            }
        }
        if failed.is_empty() {
            Ok(())
        } else {
            Err(failed)
        }
    }

    pub(crate) fn search_history(
        &mut self,
        profile: ProfileId,
        query: &str,
        limit: u32,
    ) -> Vec<HistoryHit> {
        if !self.registry.contains(&profile)
            || self.degraded_profiles.contains(&profile)
            || query.len() > MAX_HISTORY_QUERY_BYTES
            || limit == 0
        {
            return Vec::new();
        }
        let Some(fts) = fts_query(query) else {
            return Vec::new();
        };
        let limit = limit.min(MAX_HISTORY_RESULTS);
        if self.recovery_required.is_some() {
            return Vec::new();
        }
        let Ok(conn) = self.profile_conn(profile) else {
            return Vec::new();
        };
        // `history` holds one row per visit, so a candidate set capped before
        // grouping is spent by whichever address was visited most: a site
        // opened a few hundred times used to consume every slot and hide the
        // rest. Bound the scan by recency instead, then group, so the cap
        // limits work without deciding which addresses may be seen.
        //
        // Ranking is frecency, not recency alone. A daily destination should
        // outrank a page opened once an hour ago, which bare `last DESC` got
        // backwards.
        let Ok(mut stmt) = conn.prepare_cached(
            "WITH recent AS (
                 SELECT h.id, h.url, h.title, h.visited_at
                 FROM history_fts f JOIN history h ON h.id = f.rowid
                 WHERE history_fts MATCH ?1
                   AND length(CAST(h.url AS BLOB)) <= ?3
                   AND length(CAST(h.title AS BLOB)) <= ?4
                 ORDER BY h.id DESC
                 LIMIT ?5
             )
             SELECT url, title, MAX(visited_at) AS last, COUNT(*) AS visits
             FROM recent
             GROUP BY url
             ORDER BY visits * CASE
                     WHEN last >= ?6 - 86400 THEN 100
                     WHEN last >= ?6 - 604800 THEN 70
                     WHEN last >= ?6 - 2592000 THEN 50
                     WHEN last >= ?6 - 7776000 THEN 30
                     ELSE 10
                 END DESC,
                 last DESC,
                 url
             LIMIT ?2",
        ) else {
            return Vec::new();
        };
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |time| time.as_secs() as i64);
        stmt.query_map(
            params![
                fts,
                limit,
                MAX_URL_BYTES as i64,
                MAX_TITLE_BYTES as i64,
                MAX_HISTORY_SEARCH_ROWS,
                now
            ],
            |r| {
                Ok(HistoryHit {
                    url: r.get(0)?,
                    title: r.get(1)?,
                    last_visit: r.get(2)?,
                })
            },
        )
        .map(|rows| {
            rows.filter_map(Result::ok)
                .filter(|hit| navigation::is_allowed_str(&hit.url))
                .map(|mut hit| {
                    hit.title = sanitize_page_title(&hit.title);
                    hit
                })
                .collect()
        })
        .unwrap_or_default()
    }

    /// One page of visits, newest first. `history.id` rises with insertion, so
    /// paging on it is exact even when many visits share a second, and it is
    /// the order the primary key already provides.
    #[cfg(test)]
    pub(crate) fn history_page(
        &mut self,
        profile: ProfileId,
        query: &str,
        since: Option<i64>,
        before: Option<i64>,
        limit: u32,
    ) -> Vec<HistoryVisit> {
        self.history_page_checked(profile, query, since, before, limit)
            .unwrap_or_default()
    }

    pub(crate) fn history_page_checked(
        &mut self,
        profile: ProfileId,
        query: &str,
        since: Option<i64>,
        before: Option<i64>,
        limit: u32,
    ) -> Result<Vec<HistoryVisit>, HistoryPageError> {
        if !self.registry.contains(&profile)
            || self.degraded_profiles.contains(&profile)
            || self.recovery_required.is_some()
        {
            return Err(HistoryPageError::Unavailable);
        }
        if limit == 0 || query.len() > MAX_HISTORY_QUERY_BYTES {
            return Err(HistoryPageError::Invalid);
        }
        let trimmed = query.trim();
        let fts = if trimmed.is_empty() {
            None
        } else {
            match fts_query(trimmed) {
                Some(fts) => Some(fts),
                None => return Ok(Vec::new()),
            }
        };
        let limit = limit.min(MAX_HISTORY_PAGE);
        let mut before = before.unwrap_or(i64::MAX);
        let since = since.unwrap_or(i64::MIN);
        let conn = self
            .profile_conn(profile)
            .map_err(|_| HistoryPageError::Failed)?;
        let sql = if fts.is_some() {
            "SELECT h.id, h.url, h.title, h.visited_at
             FROM history_fts f JOIN history h ON h.id = f.rowid
             WHERE history_fts MATCH ?4
               AND h.id < ?1
               AND h.visited_at >= ?6
               AND length(CAST(h.url AS BLOB)) <= ?2
               AND length(CAST(h.title AS BLOB)) <= ?3
             ORDER BY h.id DESC
             LIMIT ?5"
        } else {
            "SELECT id, url, title, visited_at
             FROM history
             WHERE id < ?1
               AND visited_at >= ?6
               AND length(CAST(url AS BLOB)) <= ?2
               AND length(CAST(title AS BLOB)) <= ?3
             ORDER BY id DESC
             LIMIT ?5"
        };
        let mut stmt = conn
            .prepare_cached(sql)
            .map_err(|_| HistoryPageError::Failed)?;
        let mut page = Vec::with_capacity(limit as usize);
        loop {
            let visits = stmt
                .query_map(
                    params![
                        before,
                        MAX_URL_BYTES as i64,
                        MAX_TITLE_BYTES as i64,
                        fts.as_deref().unwrap_or_default(),
                        limit,
                        since
                    ],
                    |row| {
                        Ok(HistoryVisit {
                            id: row.get(0)?,
                            url: row.get(1)?,
                            title: row.get(2)?,
                            visited_at: row.get(3)?,
                        })
                    },
                )
                .and_then(|rows| rows.collect::<rusqlite::Result<Vec<_>>>())
                .map_err(|_| HistoryPageError::Failed)?;
            let exhausted = visits.len() < limit as usize;
            for mut visit in visits {
                // Advance over every raw row, including legacy or corrupt
                // addresses. A short allowed page means actual EOF to export.
                before = visit.id;
                if navigation::is_allowed_str(&visit.url) {
                    visit.title = sanitize_page_title(&visit.title);
                    page.push(visit);
                    if page.len() == limit as usize {
                        return Ok(page);
                    }
                }
            }
            if exhausted {
                return Ok(page);
            }
        }
    }

    /// Keeps visits from another browser with their own times. A visit to the
    /// same address at the same second is already here, so importing twice
    /// adds nothing. The usual count and byte budgets apply afterwards.
    pub(crate) fn import_history(
        &mut self,
        profile: ProfileId,
        visits: &[ImportedVisit],
    ) -> Option<u32> {
        if !self.registry.contains(&profile)
            || self.degraded_profiles.contains(&profile)
            || self.recovery_required.is_some()
        {
            return None;
        }
        let result = self.profile_conn(profile).and_then(|conn| {
            let tx = conn.transaction()?;
            let mut added = 0usize;
            {
                let mut insert = tx.prepare_cached(
                    "INSERT INTO history(url, title, visited_at)
                     SELECT ?1, ?2, ?3
                     WHERE NOT EXISTS (
                         SELECT 1 FROM history WHERE visited_at = ?3 AND url = ?1
                     )",
                )?;
                for visit in visits.iter().take(MAX_IMPORTED_VISITS) {
                    if visit.visited_at <= 0 || !navigation::is_allowed_str(&visit.url) {
                        continue;
                    }
                    added += insert.execute(params![
                        visit.url,
                        sanitize_page_title(&visit.title),
                        visit.visited_at
                    ])?;
                }
            }
            tx.execute(
                "DELETE FROM history WHERE id IN (
                     SELECT id FROM history
                     ORDER BY visited_at DESC, id DESC
                     LIMIT -1 OFFSET 50000
                 )",
                [],
            )?;
            enforce_history_budget(&tx)?;
            tx.commit()?;
            Ok(added)
        });
        match result {
            Ok(added) => Some(u32::try_from(added).unwrap_or(u32::MAX)),
            Err(e) => {
                eprintln!("store: import_history failed for profile {profile}: {e}");
                None
            }
        }
    }

    /// Removes every visit to each address. Forgetting one entry in a history
    /// list means forgetting the page, not one of the times it was opened.
    #[cfg(test)]
    pub(crate) fn forget_history_urls(&mut self, profile: ProfileId, urls: &[String]) -> u32 {
        self.forget_history_urls_checked(profile, urls)
            .unwrap_or_default()
    }

    pub(crate) fn forget_history_urls_checked(
        &mut self,
        profile: ProfileId,
        urls: &[String],
    ) -> Option<u32> {
        if !self.registry.contains(&profile)
            || self.degraded_profiles.contains(&profile)
            || self.recovery_required.is_some()
            || urls.is_empty()
            || urls.len() > MAX_HISTORY_FORGET_URLS
        {
            return None;
        }
        if let Err(error) = self.discard_session_backup(profile) {
            eprintln!(
                "store: forget_history backup disposal failed for profile {profile}: {error}"
            );
            return None;
        }
        let result = self.profile_conn(profile).and_then(|conn| {
            let tx = conn.transaction()?;
            let mut removed = 0usize;
            {
                let mut delete = tx.prepare_cached("DELETE FROM history WHERE url = ?1")?;
                let mut forget_icon =
                    tx.prepare_cached("DELETE FROM page_favicons WHERE page = ?1")?;
                for url in urls {
                    removed += delete.execute([url])?;
                    if let Some(page) = url::Url::parse(url)
                        .ok()
                        .and_then(|parsed| zephium_core::icon::page_key(&parsed))
                    {
                        forget_icon.execute([page])?;
                    }
                }
            }
            tx.commit()?;
            Ok(removed)
        });
        match result {
            Ok(removed) => Some(u32::try_from(removed).unwrap_or(u32::MAX)),
            Err(e) => {
                eprintln!("store: forget_history_urls failed for profile {profile}: {e}");
                None
            }
        }
    }

    /// Clears visits at or after `since`, or all of them when it is absent.
    #[cfg(test)]
    pub(crate) fn clear_history(&mut self, profile: ProfileId, since: Option<i64>) -> u32 {
        self.clear_history_checked(profile, since)
            .unwrap_or_default()
    }

    pub(crate) fn clear_history_checked(
        &mut self,
        profile: ProfileId,
        since: Option<i64>,
    ) -> Option<u32> {
        if !self.registry.contains(&profile)
            || self.degraded_profiles.contains(&profile)
            || self.recovery_required.is_some()
        {
            return None;
        }
        if let Err(error) = self.discard_session_backup(profile) {
            eprintln!("store: clear_history backup disposal failed for profile {profile}: {error}");
            return None;
        }
        let result = self.profile_conn(profile).and_then(|conn| {
            let tx = conn.transaction()?;
            let removed = match since {
                Some(since) => tx.execute("DELETE FROM history WHERE visited_at >= ?1", [since])?,
                None => tx.execute("DELETE FROM history", [])?,
            };
            match since {
                Some(since) => {
                    tx.execute("DELETE FROM search_queries WHERE last_used >= ?1", [since])?
                }
                None => tx.execute("DELETE FROM search_queries", [])?,
            };
            // A page's icon is kept under its address, which is history.
            match since {
                Some(since) => {
                    tx.execute("DELETE FROM page_favicons WHERE fetched_at >= ?1", [since])?
                }
                None => tx.execute("DELETE FROM page_favicons", [])?,
            };
            // A saved zoom level is a record of the sites visited.
            match since {
                Some(since) => tx.execute("DELETE FROM site_zoom WHERE set_at >= ?1", [since])?,
                None => tx.execute("DELETE FROM site_zoom", [])?,
            };
            tx.execute("DELETE FROM blocker_statistics", [])?;
            tx.commit()?;
            Ok(removed)
        });
        match result {
            Ok(removed) => Some(u32::try_from(removed).unwrap_or(u32::MAX)),
            Err(e) => {
                eprintln!("store: clear_history failed for profile {profile}: {e}");
                None
            }
        }
    }

    /// A visit is recorded when its URL commits, which is before the document
    /// publishes a title, so the row initially holds a URL-derived placeholder.
    /// The real title replaces it while that visit is still the newest one.
    pub(crate) fn amend_visit_title(&mut self, profile: ProfileId, url: &str, title: &str) {
        if !self.registry.contains(&profile)
            || self.degraded_profiles.contains(&profile)
            || self.recovery_required.is_some()
            || !navigation::is_allowed_str(url)
        {
            return;
        }
        let title = sanitize_page_title(title);
        if title.is_empty() {
            return;
        }
        let floor = now_secs().saturating_sub(TITLE_AMENDMENT_WINDOW_SECONDS);
        let result = self.profile_conn(profile).and_then(|conn| {
            conn.prepare_cached(
                "UPDATE history SET title = ?3
                 WHERE id = (
                     SELECT id FROM history
                     WHERE url = ?1 AND visited_at >= ?2
                     ORDER BY id DESC LIMIT 1
                 )",
            )?
            .execute(params![url, floor, title])
        });
        if let Err(e) = result {
            eprintln!("store: amend_visit_title failed for profile {profile}: {e}");
        }
    }

    #[cfg(test)]
    pub(crate) fn history_matches(&mut self, profile: ProfileId, query: &str) -> i64 {
        self.profile_conn(profile)
            .and_then(|conn| {
                conn.query_row(
                    "SELECT count(*) FROM history_fts WHERE history_fts MATCH ?1",
                    [query],
                    |r| r.get(0),
                )
            })
            .unwrap_or(-1)
    }

    #[cfg(test)]
    pub(crate) fn history_bytes(&mut self, profile: ProfileId) -> i64 {
        self.profile_conn(profile)
            .and_then(|conn| {
                conn.query_row("SELECT bytes FROM history_usage WHERE id = 1", [], |r| {
                    r.get(0)
                })
            })
            .unwrap_or(-1)
    }

    #[cfg(test)]
    pub(crate) fn backdate_history(&mut self, profile: ProfileId, seconds: i64) {
        let _ = self
            .profile_conn(profile)
            .map(|conn| conn.execute("UPDATE history SET visited_at = visited_at - ?1", [seconds]));
    }

    #[cfg(test)]
    pub(crate) fn history_count(&mut self, profile: ProfileId) -> i64 {
        self.profile_conn(profile)
            .and_then(|conn| conn.query_row("SELECT count(*) FROM history", [], |r| r.get(0)))
            .unwrap_or(-1)
    }

    #[cfg(test)]
    pub(crate) fn fail_history_writes(&mut self, profile: ProfileId) {
        self.profile_conn(profile)
            .unwrap()
            .execute_batch(
                "CREATE TRIGGER test_fail_history_write
                 BEFORE INSERT ON history BEGIN
                     SELECT RAISE(FAIL, 'injected history failure');
                 END;",
            )
            .unwrap();
    }
}

const ROW_CAP_PRUNE_EVERY: u32 = 256;

pub(super) fn enforce_history_budget(conn: &Connection) -> rusqlite::Result<()> {
    enforce_history_budget_to(conn, MAX_HISTORY_BYTES)
}

fn enforce_history_budget_to(conn: &Connection, maximum_bytes: i64) -> rusqlite::Result<()> {
    for _ in 0..MAX_HISTORY_PRUNE_PASSES {
        let bytes = conn.query_row("SELECT bytes FROM history_usage WHERE id = 1", [], |row| {
            row.get::<_, i64>(0)
        })?;
        if bytes <= maximum_bytes {
            return Ok(());
        }
        let deleted = conn.execute(
            "DELETE FROM history WHERE id IN (
                 SELECT id FROM history
                 ORDER BY visited_at, id
                 LIMIT ?1
             )",
            [HISTORY_PRUNE_BATCH],
        )?;
        if deleted == 0 {
            return Err(invalid_data("history byte accounting cannot be reconciled"));
        }
    }
    let bytes = conn.query_row("SELECT bytes FROM history_usage WHERE id = 1", [], |row| {
        row.get::<_, i64>(0)
    })?;
    if bytes > maximum_bytes {
        return Err(invalid_data("history exceeds bounded pruning work"));
    }
    Ok(())
}

/// Tokenized prefix query; every token is quoted so user input can never be
/// FTS5 syntax.
fn fts_query(query: &str) -> Option<String> {
    if query.len() > MAX_HISTORY_QUERY_BYTES {
        return None;
    }
    let tokens: Vec<String> = query
        .split_whitespace()
        .take(8)
        .map(|token| {
            token
                .chars()
                .filter(|c| !c.is_control() && *c != '"')
                .take(MAX_HISTORY_TOKEN_CHARS)
                .collect::<String>()
        })
        .map(|token| format!("\"{token}\"*"))
        .filter(|t| t.len() > 3)
        .collect();
    if tokens.is_empty() {
        None
    } else {
        Some(tokens.join(" "))
    }
}

impl Hub {
    pub(crate) fn record_search(&mut self, profile: ProfileId, query: &str, url: &str) {
        if self.recovery_required.is_some()
            || self.degraded_profiles.contains(&profile)
            || !self.knows(profile)
            || query.trim().is_empty()
            || query.len() > 512
            || !navigation::is_allowed_str(url)
        {
            return;
        }
        let Ok(conn) = self.profile_conn(profile) else {
            return;
        };
        let Ok(tx) = conn.transaction() else {
            return;
        };
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |time| time.as_secs() as i64);
        let recorded = tx.execute(
            "INSERT INTO search_queries(query_key, query, url, last_used, use_count)
             VALUES (?1, ?2, ?3, ?4, 1)
             ON CONFLICT(query_key) DO UPDATE SET
                 query = excluded.query,
                 url = excluded.url,
                 last_used = excluded.last_used,
                 use_count = min(use_count + 1, 1000000)",
            params![query.trim().to_lowercase(), query.trim(), url, now],
        );
        if recorded.is_err() {
            return;
        }
        let pruned = tx.execute(
            "DELETE FROM search_queries WHERE query_key IN (
                 SELECT query_key FROM search_queries
                 ORDER BY last_used DESC, query_key
                 LIMIT -1 OFFSET ?1
             )",
            params![MAX_SUBMITTED_SEARCHES],
        );
        if pruned.is_ok() {
            let _ = tx.commit();
        }
    }

    pub(crate) fn search_queries(&mut self, profile: ProfileId, query: &str) -> Vec<HistoryHit> {
        if !self.knows(profile) || query.is_empty() || query.len() > 512 {
            return Vec::new();
        }
        let Ok(conn) = self.profile_conn(profile) else {
            return Vec::new();
        };
        let Ok(mut statement) = conn.prepare_cached(
            "SELECT url, query, last_used FROM search_queries
             WHERE query_key >= ?1 AND query_key < ?2
             ORDER BY use_count DESC, last_used DESC
             LIMIT 3",
        ) else {
            return Vec::new();
        };
        let query = query.to_lowercase();
        statement
            .query_map(params![query, format!("{query}\u{10ffff}")], |row| {
                Ok(HistoryHit {
                    url: row.get(0)?,
                    title: row.get(1)?,
                    last_visit: row.get(2)?,
                })
            })
            .map(|rows| rows.filter_map(Result::ok).collect())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod connection_hardening_tests {
    use super::*;

    #[test]
    fn history_usage_triggers_and_byte_budget_prune_oldest_rows() {
        let mut connection = Connection::open_in_memory().unwrap();
        configure(&connection).unwrap();
        migrations::apply(&mut connection, migrations::PROFILE).unwrap();
        for visited_at in 0..3000 {
            connection
                .execute(
                    "INSERT INTO history(url, title, visited_at) VALUES (?1, ?2, ?3)",
                    params![
                        format!("https://example.com/{visited_at}"),
                        "x".repeat(100),
                        visited_at
                    ],
                )
                .unwrap();
        }
        let before: i64 = connection
            .query_row("SELECT bytes FROM history_usage WHERE id = 1", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert!(before > 150_000);

        enforce_history_budget_to(&connection, 150_000).unwrap();
        let after: i64 = connection
            .query_row("SELECT bytes FROM history_usage WHERE id = 1", [], |row| {
                row.get(0)
            })
            .unwrap();
        let oldest: i64 = connection
            .query_row("SELECT min(visited_at) FROM history", [], |row| row.get(0))
            .unwrap();
        assert!(after <= 150_000);
        assert!(oldest > 0, "oldest rows must be pruned first");
    }
}

/// One address of history: the oldest visit kept names it, the newest gives
/// its title.
const ENTRY_SELECT: &str = "SELECT g.first, g.url, t.title, g.latest, g.visits
     FROM (
         SELECT h.url AS url, MIN(h.id) AS first, MAX(h.id) AS newest,
                MAX(h.visited_at) AS latest, COUNT(*) AS visits
         FROM history h JOIN picked p ON p.url = h.url
         GROUP BY h.url
     ) g JOIN history t ON t.id = g.newest JOIN picked p ON p.url = g.url
     ORDER BY p.rank DESC, g.newest DESC";

fn history_entry(row: &rusqlite::Row<'_>) -> rusqlite::Result<HistoryEntry> {
    Ok(HistoryEntry {
        id: row.get(0)?,
        url: row.get(1)?,
        title: row.get(2)?,
        last_visit: row.get(3)?,
        visits: row.get(4)?,
    })
}

fn allowed_entries(entries: Vec<HistoryEntry>) -> Vec<HistoryEntry> {
    entries
        .into_iter()
        .filter(|entry| navigation::is_allowed_str(&entry.url))
        .map(|mut entry| {
            entry.title = sanitize_page_title(&entry.title);
            entry
        })
        .collect()
}

impl Hub {
    pub(crate) fn extension_history(
        &mut self,
        profile: ProfileId,
        request: HistoryRequest,
    ) -> HistoryReply {
        if !self.registry.contains(&profile)
            || self.degraded_profiles.contains(&profile)
            || self.recovery_required.is_some()
        {
            return HistoryReply::Failed;
        }
        let removes = matches!(
            request,
            HistoryRequest::RemoveUrl { .. }
                | HistoryRequest::RemoveRange { .. }
                | HistoryRequest::RemoveAll
        );
        if removes {
            if let Err(error) = self.discard_session_backup(profile) {
                eprintln!(
                    "store: history removal backup disposal failed for profile {profile}: {error}"
                );
                return HistoryReply::Failed;
            }
        }
        let result = self
            .profile_conn(profile)
            .and_then(|conn| extension_history(conn, request));
        result.unwrap_or_else(|error| {
            eprintln!("store: extension history failed for profile {profile}: {error}");
            HistoryReply::Failed
        })
    }
}

fn extension_history(
    conn: &mut Connection,
    request: HistoryRequest,
) -> rusqlite::Result<HistoryReply> {
    match request {
        HistoryRequest::Search {
            text,
            start,
            end,
            limit,
        } => {
            let fts = if text.trim().is_empty() {
                None
            } else {
                match fts_query(&text) {
                    Some(fts) => Some(fts),
                    None => return Ok(HistoryReply::Entries(Vec::new())),
                }
            };
            let matching = if fts.is_some() {
                "AND id IN (SELECT rowid FROM history_fts WHERE history_fts MATCH ?5)"
            } else {
                "AND ?5 = ''"
            };
            let entries = conn
                .prepare_cached(&format!(
                    "WITH picked AS (
                         SELECT url, MAX(visited_at) AS rank FROM history
                         WHERE visited_at >= ?1 AND visited_at < ?2
                           AND length(CAST(url AS BLOB)) <= ?4 {matching}
                         GROUP BY url ORDER BY rank DESC LIMIT ?3
                     ) {ENTRY_SELECT}"
                ))?
                .query_map(
                    params![
                        start,
                        end,
                        limit.min(MAX_EXTENSION_RESULTS),
                        MAX_URL_BYTES as i64,
                        fts.unwrap_or_default()
                    ],
                    history_entry,
                )?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(HistoryReply::Entries(allowed_entries(entries)))
        }
        HistoryRequest::Address { url } => {
            let entries = conn
                .prepare_cached(&format!(
                    "WITH picked AS (SELECT ?1 AS url, 0 AS rank) {ENTRY_SELECT}"
                ))?
                .query_map([url], history_entry)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(HistoryReply::Entries(allowed_entries(entries)))
        }
        HistoryRequest::Visits { url } => {
            let first: Option<i64> = conn.query_row(
                "SELECT MIN(id) FROM history WHERE url = ?1",
                [&url],
                |row| row.get(0),
            )?;
            let Some(first) = first else {
                return Ok(HistoryReply::Visits(Vec::new()));
            };
            let mut visits = conn
                .prepare_cached(
                    "SELECT id, visited_at FROM history WHERE url = ?1 ORDER BY id DESC LIMIT ?2",
                )?
                .query_map(params![url, MAX_VISITS], |row| {
                    Ok(VisitEntry {
                        id: row.get(0)?,
                        entry: first,
                        visited_at: row.get(1)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            visits.reverse();
            Ok(HistoryReply::Visits(visits))
        }
        HistoryRequest::MostVisited { limit } => {
            let limit = limit.min(MAX_TOP_SITES);
            let since = now_secs().saturating_sub(MOST_VISITED_WINDOW_SECONDS);
            let sites = conn
                .prepare_cached(
                    "SELECT g.url, t.title FROM (
                         SELECT url, COUNT(*) AS visits, MAX(id) AS newest FROM history
                         WHERE visited_at >= ?1 AND length(CAST(url AS BLOB)) <= ?3
                           AND (url LIKE 'https://%' OR url LIKE 'http://%')
                         GROUP BY url ORDER BY visits DESC, newest DESC LIMIT ?2
                     ) g JOIN history t ON t.id = g.newest
                     ORDER BY g.visits DESC, g.newest DESC",
                )?
                .query_map(params![since, limit * 2, MAX_URL_BYTES as i64], |row| {
                    Ok(TopSite {
                        url: row.get(0)?,
                        title: row.get(1)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?
                .into_iter()
                .filter(|site| navigation::is_allowed_str(&site.url))
                .map(|mut site| {
                    site.title = sanitize_page_title(&site.title);
                    site
                })
                .take(limit as usize)
                .collect();
            Ok(HistoryReply::Sites(sites))
        }
        HistoryRequest::RemoveUrl { url } => {
            let tx = conn.transaction()?;
            let visits = tx.execute("DELETE FROM history WHERE url = ?1", [&url])?;
            forget_page_icons(&tx, std::slice::from_ref(&url))?;
            tx.commit()?;
            Ok(HistoryReply::Removed {
                visits: u32::try_from(visits).unwrap_or(u32::MAX),
                urls: if visits > 0 { vec![url] } else { Vec::new() },
                all: false,
            })
        }
        HistoryRequest::RemoveRange { start, end } => {
            let tx = conn.transaction()?;
            let candidates: Vec<String> = tx
                .prepare_cached(
                    "SELECT DISTINCT url FROM history
                     WHERE visited_at >= ?1 AND visited_at < ?2 LIMIT ?3",
                )?
                .query_map(params![start, end, MAX_REMOVED_URLS as i64 + 1], |row| {
                    row.get(0)
                })?
                .collect::<rusqlite::Result<_>>()?;
            let visits = tx.execute(
                "DELETE FROM history WHERE visited_at >= ?1 AND visited_at < ?2",
                params![start, end],
            )?;
            tx.execute(
                "DELETE FROM search_queries WHERE last_used >= ?1 AND last_used < ?2",
                params![start, end],
            )?;
            let all = candidates.len() > MAX_REMOVED_URLS;
            let urls = if all {
                Vec::new()
            } else {
                let listed = serde_json::to_string(&candidates).unwrap_or_default();
                let kept: HashSet<String> = tx
                    .prepare_cached(
                        "SELECT DISTINCT url FROM history
                         WHERE url IN (SELECT value FROM json_each(?1))",
                    )?
                    .query_map([listed], |row| row.get(0))?
                    .collect::<rusqlite::Result<_>>()?;
                candidates
                    .into_iter()
                    .filter(|url| !kept.contains(url))
                    .collect::<Vec<_>>()
            };
            forget_page_icons(&tx, &urls)?;
            tx.commit()?;
            Ok(HistoryReply::Removed {
                visits: u32::try_from(visits).unwrap_or(u32::MAX),
                urls,
                all,
            })
        }
        // Visits are recorded through the visit mailbox, not here.
        HistoryRequest::Add { .. } => Ok(HistoryReply::Failed),
        HistoryRequest::RemoveAll => {
            let tx = conn.transaction()?;
            let visits = tx.execute("DELETE FROM history", [])?;
            tx.execute("DELETE FROM search_queries", [])?;
            tx.execute("DELETE FROM page_favicons", [])?;
            tx.commit()?;
            Ok(HistoryReply::Removed {
                visits: u32::try_from(visits).unwrap_or(u32::MAX),
                urls: Vec::new(),
                all: true,
            })
        }
    }
}

/// A page's own icon is kept under its address, which is history.
fn forget_page_icons(tx: &rusqlite::Transaction<'_>, urls: &[String]) -> rusqlite::Result<()> {
    let mut forget = tx.prepare_cached("DELETE FROM page_favicons WHERE page = ?1")?;
    for url in urls {
        if let Some(page) = url::Url::parse(url)
            .ok()
            .and_then(|parsed| zephium_core::icon::page_key(&parsed))
        {
            forget.execute([page])?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod extension_tests {
    use super::*;

    fn connection(visits: &[(&str, &str, i64)]) -> Connection {
        let mut connection = Connection::open_in_memory().unwrap();
        configure(&connection).unwrap();
        migrations::apply(&mut connection, migrations::PROFILE).unwrap();
        for (url, title, at) in visits {
            connection
                .execute(
                    "INSERT INTO history(url, title, visited_at) VALUES (?1, ?2, ?3)",
                    params![url, title, at],
                )
                .unwrap();
        }
        connection
    }

    fn entries(reply: HistoryReply) -> Vec<HistoryEntry> {
        match reply {
            HistoryReply::Entries(entries) => entries,
            other => panic!("expected entries, got {other:?}"),
        }
    }

    #[test]
    fn a_search_names_each_address_once_with_its_visits_and_newest_title() {
        let mut conn = connection(&[
            ("https://a.example/", "Old A", 100),
            ("https://b.example/", "B", 200),
            ("https://a.example/", "New A", 300),
            ("https://c.example/docs", "Docs", 50),
        ]);
        let found = entries(
            extension_history(
                &mut conn,
                HistoryRequest::Search {
                    text: String::new(),
                    start: 0,
                    end: i64::MAX,
                    limit: 10,
                },
            )
            .unwrap(),
        );
        assert_eq!(
            found,
            vec![
                HistoryEntry {
                    id: 1,
                    url: "https://a.example/".into(),
                    title: "New A".into(),
                    last_visit: 300,
                    visits: 2
                },
                HistoryEntry {
                    id: 2,
                    url: "https://b.example/".into(),
                    title: "B".into(),
                    last_visit: 200,
                    visits: 1
                },
                HistoryEntry {
                    id: 4,
                    url: "https://c.example/docs".into(),
                    title: "Docs".into(),
                    last_visit: 50,
                    visits: 1
                },
            ]
        );
        // The range picks addresses by their visits in it; what is reported
        // about them is every visit.
        let ranged = entries(
            extension_history(
                &mut conn,
                HistoryRequest::Search {
                    text: String::new(),
                    start: 90,
                    end: 150,
                    limit: 10,
                },
            )
            .unwrap(),
        );
        assert_eq!(
            ranged
                .iter()
                .map(|entry| (entry.id, entry.visits))
                .collect::<Vec<_>>(),
            vec![(1, 2)]
        );
        let worded = entries(
            extension_history(
                &mut conn,
                HistoryRequest::Search {
                    text: "docs".into(),
                    start: 0,
                    end: i64::MAX,
                    limit: 10,
                },
            )
            .unwrap(),
        );
        assert_eq!(worded.len(), 1);
        assert_eq!(
            entries(
                extension_history(
                    &mut conn,
                    HistoryRequest::Address {
                        url: "https://b.example/".into()
                    }
                )
                .unwrap()
            )
            .len(),
            1
        );
        assert_eq!(
            extension_history(
                &mut conn,
                HistoryRequest::Visits {
                    url: "https://a.example/".into()
                }
            )
            .unwrap(),
            HistoryReply::Visits(vec![
                VisitEntry {
                    id: 1,
                    entry: 1,
                    visited_at: 100
                },
                VisitEntry {
                    id: 3,
                    entry: 1,
                    visited_at: 300
                },
            ])
        );
    }

    #[test]
    fn removing_a_range_names_only_addresses_left_without_visits() {
        let now = now_secs();
        let mut conn = connection(&[
            ("https://a.example/", "A", now - 30),
            ("https://a.example/", "A", now - 10),
            ("https://b.example/", "B", now - 10),
            ("https://b.example/", "B", now - 5),
            ("https://b.example/", "B", now - 4),
        ]);
        assert_eq!(
            extension_history(&mut conn, HistoryRequest::MostVisited { limit: 5 }).unwrap(),
            HistoryReply::Sites(vec![
                TopSite {
                    url: "https://b.example/".into(),
                    title: "B".into()
                },
                TopSite {
                    url: "https://a.example/".into(),
                    title: "A".into()
                },
            ])
        );
        assert_eq!(
            extension_history(
                &mut conn,
                HistoryRequest::RemoveRange {
                    start: now - 11,
                    end: now - 4
                }
            )
            .unwrap(),
            HistoryReply::Removed {
                visits: 3,
                urls: vec![],
                all: false
            }
        );
        assert_eq!(
            extension_history(
                &mut conn,
                HistoryRequest::RemoveRange {
                    start: now - 100,
                    end: now - 20
                }
            )
            .unwrap(),
            HistoryReply::Removed {
                visits: 1,
                urls: vec!["https://a.example/".into()],
                all: false
            }
        );
        assert_eq!(
            extension_history(
                &mut conn,
                HistoryRequest::RemoveUrl {
                    url: "https://b.example/".into()
                }
            )
            .unwrap(),
            HistoryReply::Removed {
                visits: 1,
                urls: vec!["https://b.example/".into()],
                all: false
            }
        );
        assert_eq!(
            extension_history(
                &mut conn,
                HistoryRequest::RemoveUrl {
                    url: "https://b.example/".into()
                }
            )
            .unwrap(),
            HistoryReply::Removed {
                visits: 0,
                urls: vec![],
                all: false
            }
        );
        assert_eq!(
            extension_history(&mut conn, HistoryRequest::RemoveAll).unwrap(),
            HistoryReply::Removed {
                visits: 0,
                urls: vec![],
                all: true
            }
        );
    }
}
