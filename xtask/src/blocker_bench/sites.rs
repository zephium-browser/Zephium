//! The sites the benchmark visits, how each one's consent wall is accepted,
//! and the scripts every driver runs in the page.

use serde::{Deserialize, Serialize};

const SITES: &str = include_str!("sites.json");
const PROBE: &str = include_str!("probe.js");
const YOUTUBE_WATCH: &str = include_str!("youtube-watch.js");
pub(super) const YOUTUBE_FEED: &str = include_str!("youtube-feed.js");
pub(super) const YOUTUBE_SHORTS: &str = include_str!("youtube-shorts.js");
const CONSENT: &str = include_str!("consent.js");

/// The least a visit waits for a consent wall to appear.
pub(super) const MIN_CONSENT_WAIT_MS: u64 = 2500;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Site {
    pub(super) name: String,
    pub(super) kind: String,
    pub(super) url: String,
    /// Selectors of the site's own ad slots, for the probe.
    pub(super) ads: Vec<String>,
    #[serde(default)]
    pub(super) consent: Option<Recipe>,
}

/// How to accept everything on a site's consent wall.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Recipe {
    /// Tried in order; the first visible match is clicked.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) click: Vec<String>,
    /// A shadow host the `click` selectors are looked up in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) shadow: Option<String>,
    /// Fall back to the CMP's script API where the wall has no stable button.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(super) tcf: bool,
    /// Why a wall cannot be cleared, for sites where none of this works.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) note: Option<String>,
}

pub(super) fn all() -> Result<Vec<Site>, String> {
    serde_json::from_str(SITES).map_err(|e| format!("sites.json: {e}"))
}

pub(super) fn probe_script(site: &Site) -> String {
    let selectors = serde_json::to_string(&site.ads).unwrap_or_else(|_| "[]".into());
    PROBE.replacen(
        "const siteSelectors = __SITE__;",
        &format!("const siteSelectors = {selectors};"),
        1,
    )
}

pub(super) fn watch_script(seconds: u64) -> String {
    YOUTUBE_WATCH.replacen(
        "const seconds = __SECONDS__;",
        &format!("const seconds = {seconds};"),
        1,
    )
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum ConsentMode {
    Act,
    Check,
}

pub(super) fn consent_script(recipe: Option<&Recipe>, mode: ConsentMode, wait_ms: u64) -> String {
    let recipe = recipe
        .and_then(|recipe| serde_json::to_string(recipe).ok())
        .unwrap_or_else(|| "null".into());
    let mode = match mode {
        ConsentMode::Act => "'act'",
        ConsentMode::Check => "'check'",
    };
    CONSENT
        .replacen("__RECIPE__", &recipe, 1)
        .replacen("__MODE__", mode, 1)
        .replacen("__WAIT__", &wait_ms.to_string(), 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_site_list_is_well_formed() {
        let sites = all().unwrap();
        assert!(sites.len() >= 20);
        let mut names = std::collections::HashSet::new();
        for site in &sites {
            assert!(names.insert(&site.name), "{} is listed twice", site.name);
            assert!(site.url.starts_with("https://"), "{}", site.name);
            if let Some(recipe) = &site.consent {
                assert!(
                    !recipe.click.is_empty() || recipe.tcf || recipe.note.is_some(),
                    "{} has an empty consent recipe",
                    site.name
                );
                assert!(recipe.shadow.is_none() || !recipe.click.is_empty());
            }
        }
    }

    #[test]
    fn scripts_get_their_parameters_and_keep_no_placeholder() {
        let site = Site {
            name: "x".into(),
            kind: "news".into(),
            url: "https://example.com/".into(),
            ads: vec![".ad-slot".into(), "[id^='sda-']".into()],
            consent: None,
        };
        let probe = probe_script(&site);
        assert!(probe.contains("const siteSelectors = [\".ad-slot\",\"[id^='sda-']\"];"));
        let watch = watch_script(30);
        assert!(watch.contains("const seconds = 30;"));
        let recipe = Recipe {
            click: vec!["#accept".into()],
            shadow: Some("#host".into()),
            ..Recipe::default()
        };
        let act = consent_script(Some(&recipe), ConsentMode::Act, 2500);
        assert!(act.contains("const recipe = {\"click\":[\"#accept\"],\"shadow\":\"#host\"};"));
        assert!(act.contains("const mode = 'act';"));
        assert!(act.contains("const waitMs = 2500;"));
        let check = consent_script(None, ConsentMode::Check, 0);
        assert!(check.contains("const recipe = null;"));
        assert!(!probe.contains("= __SITE__;"));
        assert!(!watch.contains("= __SECONDS__;"));
        for placeholder in ["__RECIPE__", "__MODE__", "__WAIT__"] {
            assert!(!act.contains(placeholder) && !check.contains(placeholder));
        }
    }
}
