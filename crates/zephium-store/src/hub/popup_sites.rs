//! Sites a person let open new tabs without a click, one canonical origin per
//! row in the profile database. Private profiles never reach the store.

use super::*;

use zephium_core::permissions::PageOrigin;
use zephium_core::ports::store::MAX_POPUP_SITES;

impl Hub {
    fn popup_sites_available(&self, profile: ProfileId) -> bool {
        self.registry.contains(&profile)
            && !self.degraded_profiles.contains(&profile)
            && self.recovery_required.is_none()
    }

    pub(crate) fn popup_sites(&mut self, profile: ProfileId) -> Option<Vec<PageOrigin>> {
        if !self.popup_sites_available(profile) {
            return None;
        }
        let rows: rusqlite::Result<Vec<String>> = self.profile_conn(profile).and_then(|conn| {
            conn.prepare_cached("SELECT origin FROM popup_sites ORDER BY origin LIMIT ?1")?
                .query_map([MAX_POPUP_SITES as i64], |row| row.get(0))?
                .collect()
        });
        match rows {
            // A row that is not a canonical origin is never trusted as one.
            Ok(rows) => Some(
                rows.iter()
                    .filter_map(|origin| PageOrigin::parse_exact(origin).ok())
                    .collect(),
            ),
            Err(e) => {
                eprintln!("store: popup_sites failed for profile {profile}: {e}");
                None
            }
        }
    }

    /// Adds or removes one origin. Adding past `MAX_POPUP_SITES` is refused.
    pub(crate) fn set_popup_site(
        &mut self,
        profile: ProfileId,
        origin: &PageOrigin,
        allowed: bool,
    ) -> bool {
        if !self.popup_sites_available(profile) {
            return false;
        }
        let result = self.profile_conn(profile).and_then(|conn| {
            if !allowed {
                conn.prepare_cached("DELETE FROM popup_sites WHERE origin = ?1")?
                    .execute([origin.as_str()])?;
                return Ok(true);
            }
            let tx = conn.transaction()?;
            let (count, present) = tx
                .prepare_cached(
                    "SELECT count(*), EXISTS(SELECT 1 FROM popup_sites WHERE origin = ?1)
                     FROM popup_sites",
                )?
                .query_row([origin.as_str()], |row| {
                    Ok((row.get::<_, i64>(0)?, row.get::<_, bool>(1)?))
                })?;
            if !present && count >= MAX_POPUP_SITES as i64 {
                return Ok(false);
            }
            tx.prepare_cached("INSERT OR IGNORE INTO popup_sites(origin) VALUES (?1)")?
                .execute([origin.as_str()])?;
            tx.commit()?;
            Ok(true)
        });
        result.unwrap_or_else(|e| {
            eprintln!("store: set_popup_site failed for profile {profile}: {e}");
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
    fn allowed_sites_read_back_in_order_and_removal_forgets_them() {
        let (mut hub, profile) = registered_hub();
        assert_eq!(hub.popup_sites(profile), Some(Vec::new()));
        assert!(hub.set_popup_site(profile, &origin("https://mail.example"), true));
        assert!(hub.set_popup_site(profile, &origin("https://bank.example"), true));
        assert!(hub.set_popup_site(profile, &origin("https://bank.example"), true));
        assert_eq!(
            hub.popup_sites(profile),
            Some(vec![
                origin("https://bank.example"),
                origin("https://mail.example")
            ])
        );
        assert!(hub.set_popup_site(profile, &origin("https://bank.example"), false));
        assert_eq!(
            hub.popup_sites(profile),
            Some(vec![origin("https://mail.example")])
        );
    }

    #[test]
    fn the_list_is_bounded_and_an_unknown_profile_has_none() {
        let (mut hub, profile) = registered_hub();
        for index in 0..MAX_POPUP_SITES {
            assert!(hub.set_popup_site(
                profile,
                &origin(&format!("https://site{index}.example")),
                true
            ));
        }
        assert!(!hub.set_popup_site(profile, &origin("https://one-more.example"), true));
        assert!(hub.set_popup_site(profile, &origin("https://site0.example"), true));
        assert_eq!(hub.popup_sites(profile).unwrap().len(), MAX_POPUP_SITES);

        let stranger = ProfileId::from(2);
        assert_eq!(hub.popup_sites(stranger), None);
        assert!(!hub.set_popup_site(stranger, &origin("https://mail.example"), true));
    }

    #[test]
    fn the_schema_refuses_a_value_that_is_not_an_origin() {
        let (mut hub, profile) = registered_hub();
        let conn = hub.profile_conn(profile).unwrap();
        assert!(conn
            .execute("INSERT INTO popup_sites(origin) VALUES ('')", [])
            .is_err());
        assert!(conn
            .execute(
                "INSERT INTO popup_sites(origin) VALUES (?1)",
                ["x".repeat(513)]
            )
            .is_err());
    }
}
