//! Whether each site may save files, one canonical origin per row in the
//! profile database. A site without a row is asked. Private profiles never
//! reach the store.

use super::*;

use zephium_core::permissions::{PageOrigin, MAX_DOWNLOAD_SITES};

impl Hub {
    fn download_sites_available(&self, profile: ProfileId) -> bool {
        self.registry.contains(&profile)
            && !self.degraded_profiles.contains(&profile)
            && self.recovery_required.is_none()
    }

    pub(crate) fn download_sites(&mut self, profile: ProfileId) -> Option<Vec<(PageOrigin, bool)>> {
        if !self.download_sites_available(profile) {
            return None;
        }
        let rows: rusqlite::Result<Vec<(String, bool)>> =
            self.profile_conn(profile).and_then(|conn| {
                conn.prepare_cached(
                    "SELECT origin, allowed FROM download_sites ORDER BY origin LIMIT ?1",
                )?
                .query_map([MAX_DOWNLOAD_SITES as i64], |row| {
                    Ok((row.get(0)?, row.get(1)?))
                })?
                .collect()
            });
        match rows {
            // A row that is not a canonical origin is never trusted as one.
            Ok(rows) => Some(
                rows.into_iter()
                    .filter_map(|(origin, allowed)| {
                        PageOrigin::parse_exact(&origin)
                            .ok()
                            .map(|origin| (origin, allowed))
                    })
                    .collect(),
            ),
            Err(e) => {
                eprintln!("store: download_sites failed for profile {profile}: {e}");
                None
            }
        }
    }

    /// Keeps or forgets one site's decision. A new site past
    /// `MAX_DOWNLOAD_SITES` is refused.
    pub(crate) fn set_download_site(
        &mut self,
        profile: ProfileId,
        origin: &PageOrigin,
        allowed: Option<bool>,
    ) -> bool {
        if !self.download_sites_available(profile) {
            return false;
        }
        let result = self.profile_conn(profile).and_then(|conn| {
            let Some(allowed) = allowed else {
                conn.prepare_cached("DELETE FROM download_sites WHERE origin = ?1")?
                    .execute([origin.as_str()])?;
                return Ok(true);
            };
            let tx = conn.transaction()?;
            let (count, present) = tx
                .prepare_cached(
                    "SELECT count(*), EXISTS(SELECT 1 FROM download_sites WHERE origin = ?1)
                     FROM download_sites",
                )?
                .query_row([origin.as_str()], |row| {
                    Ok((row.get::<_, i64>(0)?, row.get::<_, bool>(1)?))
                })?;
            if !present && count >= MAX_DOWNLOAD_SITES as i64 {
                return Ok(false);
            }
            tx.prepare_cached(
                "INSERT INTO download_sites(origin, allowed) VALUES (?1, ?2)
                 ON CONFLICT(origin) DO UPDATE SET allowed = excluded.allowed",
            )?
            .execute(rusqlite::params![origin.as_str(), allowed])?;
            tx.commit()?;
            Ok(true)
        });
        result.unwrap_or_else(|e| {
            eprintln!("store: set_download_site failed for profile {profile}: {e}");
            false
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registered_hub() -> (Hub, ProfileId) {
        let mut hub = Hub::in_memory().unwrap();
        let profile = ProfileId::from(1);
        hub.registry.insert(profile);
        (hub, profile)
    }

    fn origin(value: &str) -> PageOrigin {
        PageOrigin::parse_exact(value).unwrap()
    }

    #[test]
    fn decisions_read_back_in_order_change_in_place_and_can_be_forgotten() {
        let (mut hub, profile) = registered_hub();
        assert_eq!(hub.download_sites(profile), Some(Vec::new()));
        assert!(hub.set_download_site(profile, &origin("https://mail.example"), Some(true)));
        assert!(hub.set_download_site(profile, &origin("https://bank.example"), Some(false)));
        assert_eq!(
            hub.download_sites(profile),
            Some(vec![
                (origin("https://bank.example"), false),
                (origin("https://mail.example"), true)
            ])
        );
        assert!(hub.set_download_site(profile, &origin("https://bank.example"), Some(true)));
        assert!(hub.set_download_site(profile, &origin("https://mail.example"), None));
        assert!(hub.set_download_site(profile, &origin("https://never.example"), None));
        assert_eq!(
            hub.download_sites(profile),
            Some(vec![(origin("https://bank.example"), true)])
        );
    }

    #[test]
    fn the_list_is_bounded_and_an_unknown_profile_has_none() {
        let (mut hub, profile) = registered_hub();
        for index in 0..MAX_DOWNLOAD_SITES {
            assert!(hub.set_download_site(
                profile,
                &origin(&format!("https://site{index}.example")),
                Some(true)
            ));
        }
        assert!(!hub.set_download_site(profile, &origin("https://one-more.example"), Some(true)));
        assert!(hub.set_download_site(profile, &origin("https://site0.example"), Some(false)));
        assert_eq!(
            hub.download_sites(profile).unwrap().len(),
            MAX_DOWNLOAD_SITES
        );

        let stranger = ProfileId::from(2);
        assert_eq!(hub.download_sites(stranger), None);
        assert!(!hub.set_download_site(stranger, &origin("https://mail.example"), Some(true)));
    }

    #[test]
    fn the_schema_refuses_a_value_that_is_not_an_origin_or_a_decision() {
        let (mut hub, profile) = registered_hub();
        let conn = hub.profile_conn(profile).unwrap();
        assert!(conn
            .execute(
                "INSERT INTO download_sites(origin, allowed) VALUES ('', 1)",
                []
            )
            .is_err());
        assert!(conn
            .execute(
                "INSERT INTO download_sites(origin, allowed) VALUES (?1, 1)",
                ["x".repeat(513)]
            )
            .is_err());
        assert!(conn
            .execute(
                "INSERT INTO download_sites(origin, allowed) VALUES ('https://a.example', 2)",
                []
            )
            .is_err());
    }
}
