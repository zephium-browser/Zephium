//! Page zoom a person set per site, held in the profile database and capped.

use super::*;

use zephium_core::time::MAX_SITE_BYTES;
use zephium_core::zoom::{valid_percent, SavedLevel, MAX_SITE_LEVELS};

pub(crate) fn valid_zoom_site(site: &str) -> bool {
    !site.is_empty() && site.len() <= MAX_SITE_BYTES
}

impl Hub {
    fn zoom_available(&self, profile: ProfileId) -> bool {
        self.registry.contains(&profile)
            && !self.degraded_profiles.contains(&profile)
            && self.recovery_required.is_none()
    }

    pub(crate) fn zoom_levels(&mut self, profile: ProfileId) -> Vec<SavedLevel> {
        if !self.zoom_available(profile) {
            return Vec::new();
        }
        self.profile_conn(profile)
            .and_then(|conn| {
                conn.prepare_cached(
                    "SELECT site, percent, set_at FROM site_zoom
                     ORDER BY set_at DESC, rowid DESC LIMIT ?1",
                )?
                .query_map([MAX_SITE_LEVELS as i64], |row| {
                    Ok(SavedLevel {
                        site: row.get(0)?,
                        percent: row.get(1)?,
                        set_at: row.get(2)?,
                    })
                })?
                .collect()
            })
            .unwrap_or_default()
    }

    /// Saves a site's level, dropping the oldest past `MAX_SITE_LEVELS` in the
    /// same transaction, or forgets the site when `percent` is none.
    pub(crate) fn save_zoom_level(
        &mut self,
        profile: ProfileId,
        site: &str,
        percent: Option<u16>,
        set_at: i64,
    ) {
        if !valid_zoom_site(site)
            || percent.is_some_and(|percent| !valid_percent(percent))
            || !self.zoom_available(profile)
        {
            return;
        }
        let result = self.profile_conn(profile).and_then(|conn| {
            let tx = conn.transaction()?;
            match percent {
                Some(percent) => {
                    tx.prepare_cached(
                        "INSERT INTO site_zoom(site, percent, set_at) VALUES (?1, ?2, ?3)
                         ON CONFLICT(site) DO UPDATE SET percent = ?2, set_at = ?3",
                    )?
                    .execute(params![site, percent, set_at])?;
                    tx.prepare_cached(
                        "DELETE FROM site_zoom WHERE site IN (
                             SELECT site FROM site_zoom
                             ORDER BY set_at DESC, rowid DESC
                             LIMIT -1 OFFSET ?1
                         )",
                    )?
                    .execute([MAX_SITE_LEVELS as i64])?;
                }
                None => {
                    tx.prepare_cached("DELETE FROM site_zoom WHERE site = ?1")?
                        .execute([site])?;
                }
            }
            tx.commit()
        });
        if let Err(e) = result {
            eprintln!("store: save_zoom_level failed for profile {profile}: {e}");
        }
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

    fn sites(hub: &mut Hub, profile: ProfileId) -> Vec<(String, u16)> {
        hub.zoom_levels(profile)
            .into_iter()
            .map(|level| (level.site, level.percent))
            .collect()
    }

    #[test]
    fn a_site_keeps_its_latest_level_and_forgetting_removes_it() {
        let (mut hub, profile) = registered_hub();
        hub.save_zoom_level(profile, "github.com", Some(80), 10);
        hub.save_zoom_level(profile, "example.org", Some(150), 20);
        hub.save_zoom_level(profile, "github.com", Some(90), 30);
        assert_eq!(
            sites(&mut hub, profile),
            [("github.com".into(), 90), ("example.org".into(), 150)]
        );
        hub.save_zoom_level(profile, "github.com", None, 40);
        assert_eq!(sites(&mut hub, profile), [("example.org".into(), 150)]);
        hub.save_zoom_level(profile, "github.com", None, 50);
        assert_eq!(sites(&mut hub, profile).len(), 1);
    }

    #[test]
    fn the_table_is_capped_and_the_oldest_site_goes() {
        let (mut hub, profile) = registered_hub();
        for n in 0..=MAX_SITE_LEVELS {
            hub.save_zoom_level(profile, &format!("site{n}.example"), Some(110), n as i64);
        }
        let kept = sites(&mut hub, profile);
        assert_eq!(kept.len(), MAX_SITE_LEVELS);
        assert!(!kept.iter().any(|(site, _)| site == "site0.example"));
        assert!(kept.iter().any(|(site, _)| site == "site1.example"));
        // Updating a kept site grows nothing and drops nothing.
        hub.save_zoom_level(profile, "site1.example", Some(125), 9_999);
        assert_eq!(sites(&mut hub, profile).len(), MAX_SITE_LEVELS);
        assert_eq!(sites(&mut hub, profile)[0], ("site1.example".into(), 125));
    }

    #[test]
    fn out_of_range_levels_and_unusable_sites_are_refused() {
        let (mut hub, profile) = registered_hub();
        hub.save_zoom_level(profile, "example.com", Some(29), 1);
        hub.save_zoom_level(profile, "example.com", Some(301), 1);
        hub.save_zoom_level(profile, "", Some(110), 1);
        hub.save_zoom_level(profile, &"a".repeat(254), Some(110), 1);
        assert!(sites(&mut hub, profile).is_empty());
    }

    #[test]
    fn levels_belong_to_their_profile_and_only_registered_ones_store() {
        let (mut hub, profile) = registered_hub();
        let other = ProfileId::from(2);
        hub.registry.insert(other);
        hub.save_zoom_level(profile, "example.com", Some(125), 1);
        hub.save_zoom_level(ProfileId::from(3), "example.com", Some(125), 1);
        assert_eq!(sites(&mut hub, profile).len(), 1);
        assert!(sites(&mut hub, other).is_empty());
        assert!(sites(&mut hub, ProfileId::from(3)).is_empty());
    }

    #[test]
    fn a_degraded_profile_stores_and_reads_nothing() {
        let (mut hub, profile) = registered_hub();
        hub.save_zoom_level(profile, "example.com", Some(125), 1);
        hub.degraded_profiles.insert(profile);
        hub.save_zoom_level(profile, "other.com", Some(125), 2);
        assert!(sites(&mut hub, profile).is_empty());
    }

    #[test]
    fn clearing_history_drops_levels_set_since_or_all_of_them() {
        let (mut hub, profile) = registered_hub();
        hub.save_zoom_level(profile, "old.example", Some(80), 100);
        hub.save_zoom_level(profile, "edge.example", Some(90), 200);
        hub.save_zoom_level(profile, "new.example", Some(110), 300);
        hub.clear_history(profile, Some(200));
        assert_eq!(sites(&mut hub, profile), [("old.example".into(), 80)]);
        hub.clear_history(profile, None);
        assert!(sites(&mut hub, profile).is_empty());
    }
}
