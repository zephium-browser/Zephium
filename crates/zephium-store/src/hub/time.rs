//! Time per site in local hours, and focus sessions.

use super::*;
use sha2::{Digest, Sha256};

use zephium_core::time::{
    BucketTime, FocusDay, FocusRecord, HourTally, Place, SiteTime, TimeQuery, TimeReport,
    HIGHLIGHTED_SITES, MAX_PENDING_TALLIES, MAX_REPORT_BUCKETS, MAX_REPORT_SITES, MAX_SITE_BYTES,
};

/// Focus sessions kept, about four years of a few a day.
const MAX_FOCUS_SESSIONS: i64 = 4096;
pub(crate) const MAX_TIME_BATCH_RECEIPTS: usize = 128;

/// An internal identity minted once at admission and retained with the batch
/// across every retry. It has no IPC or native authority representation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TimeBatchId(ulid::Ulid);

impl TimeBatchId {
    pub(crate) fn generate() -> Self {
        Self(ulid::Ulid::new())
    }

    fn bytes(self) -> [u8; 16] {
        self.0 .0.to_be_bytes()
    }
}

fn time_batch_digest(
    profile: ProfileId,
    tallies: &[HourTally],
    keep_from_hour: i64,
) -> rusqlite::Result<[u8; 32]> {
    if tallies.len() > MAX_PENDING_TALLIES {
        return Err(invalid_data("time batch exceeds tally capacity"));
    }
    let mut hash = Sha256::new();
    hash.update(b"zephium:time-batch:v1\0");
    hash.update(profile.bytes());
    hash.update(keep_from_hour.to_be_bytes());
    hash.update((tallies.len() as u64).to_be_bytes());
    for tally in tallies {
        let place = place_key(&tally.place);
        if place.len() > MAX_SITE_BYTES {
            return Err(invalid_data("time batch site exceeds capacity"));
        }
        hash.update(tally.hour.to_be_bytes());
        hash.update((place.len() as u32).to_be_bytes());
        hash.update(place.as_bytes());
        hash.update(tally.tally.spent_ms.to_be_bytes());
        hash.update(tally.tally.opens.to_be_bytes());
    }
    Ok(hash.finalize().into())
}

fn place_key(place: &Place) -> &str {
    match place {
        Place::Site(site) => site,
        Place::Work => "",
    }
}

impl Hub {
    fn time_profile(&mut self, profile: ProfileId) -> rusqlite::Result<&mut Connection> {
        if !self.registry.contains(&profile)
            || self.degraded_profiles.contains(&profile)
            || self.recovery_required.is_some()
        {
            return Err(invalid_data("time profile unavailable"));
        }
        self.profile_conn(profile)
    }

    #[cfg(test)]
    pub(crate) fn record_time(
        &mut self,
        profile: ProfileId,
        tallies: &[HourTally],
        keep_from_hour: i64,
    ) -> rusqlite::Result<()> {
        self.record_time_batch(profile, TimeBatchId::generate(), tallies, keep_from_hour)
    }

    pub(crate) fn record_time_batch(
        &mut self,
        profile: ProfileId,
        batch: TimeBatchId,
        tallies: &[HourTally],
        keep_from_hour: i64,
    ) -> rusqlite::Result<()> {
        let digest = time_batch_digest(profile, tallies, keep_from_hour)?;
        let tx = self.time_profile(profile)?.transaction()?;
        let retained: Option<Option<Vec<u8>>> = tx
            .query_row(
                "SELECT CASE WHEN typeof(digest) = 'blob' AND length(digest) = 32 THEN digest END
             FROM time_batch_receipts WHERE batch_id = ?1",
                [batch.bytes().as_slice()],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(retained) = retained {
            return if retained.as_deref() == Some(digest.as_slice()) {
                Ok(())
            } else {
                Err(invalid_data(
                    "time batch identity conflicts with retained receipt",
                ))
            };
        }
        {
            let mut add = tx.prepare_cached(
                "INSERT INTO time_spent(hour, place, spent_ms, opens) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(hour, place) DO UPDATE SET
                     spent_ms = min(spent_ms + excluded.spent_ms, 3600000),
                     opens = opens + excluded.opens",
            )?;
            for tally in tallies {
                let place = place_key(&tally.place);
                if place.len() > MAX_SITE_BYTES || tally.hour < keep_from_hour {
                    continue;
                }
                add.execute(params![
                    tally.hour,
                    place,
                    tally.tally.spent_ms.clamp(0, 3_600_000),
                    tally.tally.opens
                ])?;
            }
        }
        tx.execute("DELETE FROM time_spent WHERE hour < ?1", [keep_from_hour])?;
        // The receipt and additive updates share the exact commit boundary.
        // Its insertion trigger retains the newest bounded receipt cohort.
        // The actor's per-profile FIFO cannot commit a later activity batch past
        // an ambiguous head; its newest receipt therefore cannot be pruned
        // while its original admitted ID is still awaiting retry.
        tx.execute(
            "INSERT INTO time_batch_receipts(batch_id, digest) VALUES(?1, ?2)",
            params![batch.bytes().as_slice(), digest.as_slice()],
        )?;
        tx.commit()?;
        #[cfg(test)]
        if std::mem::take(&mut self.ambiguous_time_commit_once) {
            return Err(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_IOERR),
                Some("fixture time batch committed before acknowledgement failed".into()),
            ));
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn fail_next_time_commit_as_ambiguous(&mut self) {
        self.ambiguous_time_commit_once = true;
    }

    #[cfg(test)]
    pub(crate) fn immediate_time_lock_failures_for_test(&mut self, profile: ProfileId) {
        self.time_profile(profile)
            .unwrap()
            .busy_timeout(std::time::Duration::ZERO)
            .unwrap();
    }

    pub(crate) fn time_report(
        &mut self,
        profile: ProfileId,
        query: &TimeQuery,
    ) -> rusqlite::Result<TimeReport> {
        if !query.valid() {
            return Err(invalid_data("invalid time query"));
        }
        let conn = self.time_profile(profile)?;
        let from = query.from_hour;
        let to = from + query.span_hours();
        let width = i64::from(query.bucket_hours);
        let count = query.buckets.min(MAX_REPORT_BUCKETS) as usize;
        let site = query.site.as_deref();
        let mut report = TimeReport {
            buckets: vec![BucketTime::default(); count],
            ..TimeReport::default()
        };

        // With a site, every total is that site's own; Work never matches it.
        let mut totals = conn.prepare_cached(
            "SELECT (hour - ?1) / ?3, place = '', sum(spent_ms) FROM time_spent
             WHERE hour >= ?1 AND hour < ?2 AND (?4 IS NULL OR place = ?4)
             GROUP BY 1, 2",
        )?;
        let rows = totals.query_map(params![from, to, width, site], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, bool>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?;
        for row in rows {
            let (index, work, spent) = row?;
            if let Some(bucket) = usize::try_from(index)
                .ok()
                .and_then(|index| report.buckets.get_mut(index))
            {
                if work {
                    bucket.work_ms += spent;
                } else {
                    bucket.browse_ms += spent;
                }
            }
        }

        let mut previous = conn.prepare_cached(
            "SELECT place = '', sum(spent_ms) FROM time_spent
             WHERE hour >= ?1 AND hour < ?2 AND (?3 IS NULL OR place = ?3)
             GROUP BY 1",
        )?;
        let rows = previous.query_map(params![from - query.span_hours(), from, site], |row| {
            Ok((row.get::<_, bool>(0)?, row.get::<_, i64>(1)?))
        })?;
        for row in rows {
            match row? {
                (true, spent) => report.previous.work_ms += spent,
                (false, spent) => report.previous.browse_ms += spent,
            }
        }

        let mut ranked = conn.prepare_cached(
            "SELECT place, sum(spent_ms) AS spent, sum(opens) FROM time_spent
             WHERE hour >= ?1 AND hour < ?2 AND place <> '' AND (?3 IS NULL OR place = ?3)
             GROUP BY place HAVING spent > 0 OR sum(opens) > 0
             ORDER BY spent DESC, place LIMIT ?4",
        )?;
        let rows = ranked.query_map(params![from, to, site, MAX_REPORT_SITES as i64], |row| {
            Ok(SiteTime {
                site: row.get(0)?,
                spent_ms: row.get(1)?,
                opens: u32::try_from(row.get::<_, i64>(2)?).unwrap_or(u32::MAX),
                series: Vec::new(),
            })
        })?;
        for row in rows {
            report.sites.push(row?);
        }

        let mut series = conn.prepare_cached(
            "SELECT (hour - ?1) / ?3, spent_ms FROM time_spent
             WHERE hour >= ?1 AND hour < ?2 AND place = ?4",
        )?;
        for entry in report.sites.iter_mut().take(HIGHLIGHTED_SITES) {
            entry.series = vec![0; count];
            let rows = series.query_map(params![from, to, width, entry.site], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
            })?;
            for row in rows {
                let (index, spent) = row?;
                if let Some(slot) = usize::try_from(index)
                    .ok()
                    .and_then(|index| entry.series.get_mut(index))
                {
                    *slot += spent;
                }
            }
        }
        Ok(report)
    }

    pub(crate) fn clear_time(
        &mut self,
        profile: ProfileId,
        since_hour: Option<i64>,
    ) -> rusqlite::Result<()> {
        let conn = self.time_profile(profile)?;
        match since_hour {
            Some(since) => conn.execute("DELETE FROM time_spent WHERE hour >= ?1", [since])?,
            None => conn.execute("DELETE FROM time_spent", [])?,
        };
        Ok(())
    }

    pub(crate) fn record_focus(&mut self, record: &FocusRecord, day: i64) -> rusqlite::Result<()> {
        if self.recovery_required.is_some() {
            return Err(invalid_data("focus storage unavailable"));
        }
        let tx = self.meta.transaction()?;
        tx.execute(
            "INSERT INTO focus_sessions(started_ms, ended_ms, day, focused_ms, rounds, completed)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(started_ms) DO UPDATE SET
                 ended_ms = excluded.ended_ms, focused_ms = excluded.focused_ms,
                 rounds = excluded.rounds, completed = excluded.completed",
            params![
                record.started_ms,
                record.ended_ms.max(record.started_ms),
                day,
                record.focused_ms.max(0),
                record.rounds,
                record.completed
            ],
        )?;
        tx.execute(
            "DELETE FROM focus_sessions WHERE started_ms IN (
                 SELECT started_ms FROM focus_sessions
                 ORDER BY started_ms DESC LIMIT -1 OFFSET ?1
             )",
            [MAX_FOCUS_SESSIONS],
        )?;
        tx.commit()
    }

    pub(crate) fn focus_days(
        &mut self,
        from_day: i64,
        days: u32,
    ) -> rusqlite::Result<Vec<FocusDay>> {
        if self.recovery_required.is_some() || days == 0 || days > MAX_REPORT_BUCKETS {
            return Err(invalid_data("invalid focus query"));
        }
        let mut statement = self.meta.prepare_cached(
            "SELECT day, sum(focused_ms), count(*), sum(completed) FROM focus_sessions
             WHERE day >= ?1 AND day < ?2 GROUP BY day ORDER BY day",
        )?;
        let rows = statement.query_map(params![from_day, from_day + i64::from(days)], |row| {
            Ok(FocusDay {
                day: row.get(0)?,
                focused_ms: row.get(1)?,
                sessions: u32::try_from(row.get::<_, i64>(2)?).unwrap_or(u32::MAX),
                completed: u32::try_from(row.get::<_, i64>(3)?).unwrap_or(u32::MAX),
            })
        })?;
        rows.collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zephium_core::time::Tally;

    fn tally(hour: i64, place: Place, spent_ms: i64, opens: u32) -> HourTally {
        HourTally {
            hour,
            place,
            tally: Tally { spent_ms, opens },
        }
    }

    fn site(name: &str) -> Place {
        Place::Site(name.into())
    }

    fn hub() -> (Hub, ProfileId) {
        let mut hub = Hub::in_memory().unwrap();
        let profile = ProfileId::from(1);
        hub.registry.insert(profile);
        (hub, profile)
    }

    #[test]
    fn time_batch_replay_is_idempotent_and_distinct_batches_remain_additive() {
        let (mut hub, profile) = hub();
        let first = TimeBatchId::generate();
        let values = [tally(100, site("example.test"), 1_000, 1)];
        hub.record_time_batch(profile, first, &values, 0).unwrap();
        hub.record_time_batch(profile, first, &values, 0).unwrap();
        hub.record_time_batch(profile, TimeBatchId::generate(), &values, 0)
            .unwrap();
        let conn = hub.time_profile(profile).unwrap();
        let result: (i64, i64, i64) = conn.query_row(
            "SELECT spent_ms, opens, (SELECT count(*) FROM time_batch_receipts) FROM time_spent",
            [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        ).unwrap();
        assert_eq!(result, (2_000, 2, 2));
        assert!(hub
            .record_time_batch(
                profile,
                first,
                &[tally(100, site("example.test"), 2_000, 1)],
                0
            )
            .is_err());
        assert!(hub.record_time_batch(profile, first, &values, 1).is_err());
    }

    #[test]
    fn time_batch_receipt_and_tallies_commit_atomically() {
        let (mut hub, profile) = hub();
        let first = TimeBatchId::generate();
        let values = [tally(100, site("example.test"), 1_000, 1)];
        let conn = hub.time_profile(profile).unwrap();
        conn.execute_batch(
            "CREATE TRIGGER fail_time_receipt BEFORE INSERT ON time_batch_receipts
             BEGIN SELECT RAISE(ABORT, 'fixture receipt failure'); END;",
        )
        .unwrap();
        assert!(hub.record_time_batch(profile, first, &values, 0).is_err());
        let conn = hub.time_profile(profile).unwrap();
        let rows: (i64, i64) = conn.query_row(
            "SELECT (SELECT count(*) FROM time_spent), (SELECT count(*) FROM time_batch_receipts)",
            [], |row| Ok((row.get(0)?, row.get(1)?)),
        ).unwrap();
        assert_eq!(rows, (0, 0));
        conn.execute_batch("DROP TRIGGER fail_time_receipt")
            .unwrap();
        hub.record_time_batch(profile, first, &values, 0).unwrap();
        hub.record_time_batch(profile, first, &values, 0).unwrap();
        assert_eq!(
            hub.time_profile(profile)
                .unwrap()
                .query_row("SELECT spent_ms FROM time_spent", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            1_000
        );
    }

    #[test]
    fn time_batch_receipts_are_bounded_and_replay_cannot_undo_a_clear() {
        let (mut hub, profile) = hub();
        let values = [tally(100, site("example.test"), 1, 1)];
        for _ in 0..MAX_TIME_BATCH_RECEIPTS + 8 {
            hub.record_time_batch(profile, TimeBatchId::generate(), &values, 0)
                .unwrap();
        }
        let newest = TimeBatchId::generate();
        hub.record_time_batch(profile, newest, &values, 0).unwrap();
        let conn = hub.time_profile(profile).unwrap();
        assert_eq!(
            conn.query_row("SELECT count(*) FROM time_batch_receipts", [], |row| row
                .get::<_, usize>(
                0
            ))
            .unwrap(),
            MAX_TIME_BATCH_RECEIPTS
        );
        assert_eq!(
            conn.query_row(
                "SELECT count(*) FROM time_batch_receipts WHERE batch_id=?1",
                [newest.bytes().as_slice()],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            1
        );
        hub.clear_time(profile, None).unwrap();
        hub.record_time_batch(profile, newest, &values, 0).unwrap();
        assert_eq!(
            hub.time_profile(profile)
                .unwrap()
                .query_row("SELECT count(*) FROM time_spent", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }

    #[test]
    fn time_batch_replay_remains_exact_after_reopening() {
        let dir = tempfile::tempdir().unwrap();
        let profile = ProfileId::from(1);
        let batch = TimeBatchId::generate();
        let values = [tally(100, site("example.test"), 1_000, 1)];
        let mut hub = Hub::open(dir.path().to_path_buf()).unwrap();
        hub.save(&SessionState {
            profiles: vec![PersistedProfile {
                id: profile,
                name: "Fixture".into(),
                kind: ProfileKind::Default,
            }],
            spaces: vec![PersistedSpace {
                id: SpaceId::from(2),
                profile,
                name: "Fixture".into(),
            }],
            active_space: Some(SpaceId::from(2)),
            ..SessionState::default()
        })
        .unwrap();
        hub.record_time_batch(profile, batch, &values, 0).unwrap();
        drop(hub);
        let mut reopened = Hub::open(dir.path().to_path_buf()).unwrap();
        reopened
            .record_time_batch(profile, batch, &values, 0)
            .unwrap();
        assert_eq!(
            reopened
                .time_profile(profile)
                .unwrap()
                .query_row("SELECT spent_ms FROM time_spent", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            1_000
        );
    }

    #[test]
    fn tallies_add_up_into_buckets_sites_and_the_span_before() {
        let (mut hub, profile) = hub();
        let day = 20_000 * 24;
        hub.record_time(
            profile,
            &[
                tally(day - 3, site("x.com"), 120_000, 1),
                tally(day + 9, site("github.com"), 600_000, 2),
                tally(day + 9, Place::Work, 300_000, 0),
                tally(day + 10, site("github.com"), 60_000, 0),
                tally(day + 10, site("x.com"), 30_000, 1),
            ],
            0,
        )
        .unwrap();
        hub.record_time(profile, &[tally(day + 9, site("github.com"), 1_000, 1)], 0)
            .unwrap();
        let report = hub
            .time_report(
                profile,
                &TimeQuery {
                    from_hour: day,
                    bucket_hours: 1,
                    buckets: 24,
                    site: None,
                },
            )
            .unwrap();
        assert_eq!(report.buckets[9].browse_ms, 601_000);
        assert_eq!(report.buckets[9].work_ms, 300_000);
        assert_eq!(report.buckets[10].browse_ms, 90_000);
        assert_eq!(report.previous.browse_ms, 120_000);
        let ranked: Vec<_> = report
            .sites
            .iter()
            .map(|entry| (entry.site.as_str(), entry.spent_ms, entry.opens))
            .collect();
        assert_eq!(
            ranked,
            vec![("github.com", 661_000, 3), ("x.com", 30_000, 1)]
        );
        assert_eq!(report.sites[0].series[9], 601_000);
        assert_eq!(report.sites[0].series[10], 60_000);

        let one = hub
            .time_report(
                profile,
                &TimeQuery {
                    from_hour: day,
                    bucket_hours: 24,
                    buckets: 1,
                    site: Some("x.com".into()),
                },
            )
            .unwrap();
        assert_eq!(
            one.buckets,
            vec![BucketTime {
                browse_ms: 30_000,
                work_ms: 0
            }]
        );
        assert_eq!(one.previous.browse_ms, 120_000);
        assert_eq!(one.sites.len(), 1);
    }

    #[test]
    fn retention_and_clearing_remove_old_and_recent_hours() {
        let (mut hub, profile) = hub();
        hub.record_time(
            profile,
            &[
                tally(10, site("a.com"), 1_000, 0),
                tally(20, site("a.com"), 1_000, 0),
                tally(30, site("a.com"), 1_000, 0),
            ],
            15,
        )
        .unwrap();
        hub.clear_time(profile, Some(25)).unwrap();
        let report = hub
            .time_report(
                profile,
                &TimeQuery {
                    from_hour: 0,
                    bucket_hours: 24,
                    buckets: 2,
                    site: None,
                },
            )
            .unwrap();
        assert_eq!(report.buckets[0].browse_ms, 1_000);
        assert_eq!(report.buckets[1].browse_ms, 0);
    }

    #[test]
    fn focus_sessions_group_by_their_day() {
        let (mut hub, _profile) = hub();
        let record = |started_ms, focused_ms, completed| FocusRecord {
            started_ms,
            ended_ms: started_ms + focused_ms,
            focused_ms,
            rounds: 1,
            completed,
        };
        hub.record_focus(&record(1_000, 1_500_000, true), 7)
            .unwrap();
        hub.record_focus(&record(9_000_000, 600_000, false), 7)
            .unwrap();
        hub.record_focus(&record(99_000_000, 1_500_000, true), 8)
            .unwrap();
        // A rewrite of the same session replaces it.
        hub.record_focus(&record(1_000, 1_400_000, true), 7)
            .unwrap();
        let days = hub.focus_days(7, 2).unwrap();
        assert_eq!(
            days,
            vec![
                FocusDay {
                    day: 7,
                    focused_ms: 2_000_000,
                    sessions: 2,
                    completed: 1
                },
                FocusDay {
                    day: 8,
                    focused_ms: 1_500_000,
                    sessions: 1,
                    completed: 1
                },
            ]
        );
    }
}
