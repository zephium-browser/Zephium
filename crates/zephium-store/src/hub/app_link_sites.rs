//! Kinds of application link each site may open without asking, one
//! (origin, scheme) per row in the profile database. Private profiles never
//! reach the store.

use super::*;

use zephium_core::navigation::{app_link_scheme, MAX_APP_LINK_SITES};
use zephium_core::permissions::PageOrigin;

impl Hub {
    fn app_link_sites_available(&self, profile: ProfileId) -> bool {
        self.registry.contains(&profile)
            && !self.degraded_profiles.contains(&profile)
            && self.recovery_required.is_none()
    }

    pub(crate) fn app_link_sites(
        &mut self,
        profile: ProfileId,
    ) -> Option<Vec<(PageOrigin, String)>> {
        if !self.app_link_sites_available(profile) {
            return None;
        }
        let rows: rusqlite::Result<Vec<(String, String)>> =
            self.profile_conn(profile).and_then(|conn| {
                conn.prepare_cached(
                    "SELECT origin, scheme FROM app_link_sites ORDER BY origin, scheme LIMIT ?1",
                )?
                .query_map([MAX_APP_LINK_SITES as i64], |row| {
                    Ok((row.get(0)?, row.get(1)?))
                })?
                .collect()
            });
        match rows {
            // A row that is not a canonical origin or a scheme an application
            // link may use is never trusted as one.
            Ok(rows) => Some(
                rows.into_iter()
                    .filter(|(_, scheme)| app_link_scheme(scheme))
                    .filter_map(|(origin, scheme)| {
                        PageOrigin::parse_exact(&origin)
                            .ok()
                            .map(|origin| (origin, scheme))
                    })
                    .collect(),
            ),
            Err(e) => {
                eprintln!("store: app_link_sites failed for profile {profile}: {e}");
                None
            }
        }
    }

    /// Keeps or forgets one (site, scheme). A new one past
    /// `MAX_APP_LINK_SITES`, or a scheme no application link uses, is refused.
    pub(crate) fn set_app_link_site(
        &mut self,
        profile: ProfileId,
        origin: &PageOrigin,
        scheme: &str,
        allowed: bool,
    ) -> bool {
        if !self.app_link_sites_available(profile) || !app_link_scheme(scheme) {
            return false;
        }
        let result = self.profile_conn(profile).and_then(|conn| {
            if !allowed {
                conn.prepare_cached(
                    "DELETE FROM app_link_sites WHERE origin = ?1 AND scheme = ?2",
                )?
                .execute([origin.as_str(), scheme])?;
                return Ok(true);
            }
            let tx = conn.transaction()?;
            let (count, present) = tx
                .prepare_cached(
                    "SELECT count(*),
                            EXISTS(SELECT 1 FROM app_link_sites WHERE origin = ?1 AND scheme = ?2)
                     FROM app_link_sites",
                )?
                .query_row([origin.as_str(), scheme], |row| {
                    Ok((row.get::<_, i64>(0)?, row.get::<_, bool>(1)?))
                })?;
            if !present && count >= MAX_APP_LINK_SITES as i64 {
                return Ok(false);
            }
            tx.prepare_cached(
                "INSERT OR IGNORE INTO app_link_sites(origin, scheme) VALUES (?1, ?2)",
            )?
            .execute([origin.as_str(), scheme])?;
            tx.commit()?;
            Ok(true)
        });
        result.unwrap_or_else(|e| {
            eprintln!("store: set_app_link_site failed for profile {profile}: {e}");
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
    fn grants_read_back_in_order_and_removal_forgets_one_scheme() {
        let (mut hub, profile) = registered_hub();
        assert_eq!(hub.app_link_sites(profile), Some(Vec::new()));
        assert!(hub.set_app_link_site(profile, &origin("https://zoom.us"), "zoommtg", true));
        assert!(hub.set_app_link_site(profile, &origin("https://zoom.us"), "zoomus", true));
        assert!(hub.set_app_link_site(profile, &origin("https://app.slack.com"), "slack", true));
        assert!(hub.set_app_link_site(profile, &origin("https://zoom.us"), "zoommtg", true));
        assert!(hub.set_app_link_site(profile, &origin("https://zoom.us"), "zoomus", false));
        assert_eq!(
            hub.app_link_sites(profile),
            Some(vec![
                (origin("https://app.slack.com"), "slack".to_owned()),
                (origin("https://zoom.us"), "zoommtg".to_owned()),
            ])
        );
    }

    #[test]
    fn the_list_is_bounded_refuses_unusable_schemes_and_an_unknown_profile() {
        let (mut hub, profile) = registered_hub();
        for scheme in ["https", "file", "javascript", "zephium", "", "Zoom", "a b"] {
            assert!(
                !hub.set_app_link_site(profile, &origin("https://a.example"), scheme, true),
                "{scheme}"
            );
        }
        for index in 0..MAX_APP_LINK_SITES {
            assert!(hub.set_app_link_site(
                profile,
                &origin(&format!("https://site{index}.example")),
                "zoommtg",
                true
            ));
        }
        assert!(!hub.set_app_link_site(
            profile,
            &origin("https://one-more.example"),
            "zoommtg",
            true
        ));
        assert_eq!(
            hub.app_link_sites(profile).unwrap().len(),
            MAX_APP_LINK_SITES
        );
        assert_eq!(hub.app_link_sites(ProfileId::from(2)), None);
    }

    #[test]
    fn the_schema_refuses_a_value_that_is_not_an_origin_or_a_scheme() {
        let (mut hub, profile) = registered_hub();
        let conn = hub.profile_conn(profile).unwrap();
        assert!(conn
            .execute(
                "INSERT INTO app_link_sites(origin, scheme) VALUES ('', 'zoommtg')",
                []
            )
            .is_err());
        assert!(conn
            .execute(
                "INSERT INTO app_link_sites(origin, scheme) VALUES ('https://a.example', ?1)",
                ["x".repeat(65)]
            )
            .is_err());
    }
}
