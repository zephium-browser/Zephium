use std::collections::BTreeSet;
use zephium_map::{site::MAX_PRIMARY_LABEL_CHARS, Place, SectionRole, Source, Tier};

const SHORT_TITLE_WORDS: usize = 3;
const CARD_MEASURES: usize = 18;
pub(in crate::map_bench) const CLASSES: [&str; 21] = [
    "action",
    "sentence",
    "self",
    "nameless",
    "duplicate",
    "off_host_without_display",
    "order",
    "utility",
    "detail",
    "off_host_root_children",
    "top_level_menu_items",
    "supplemental_only",
    "same_role_pairs",
    "path_names",
    "thin_cards",
    "duplicate_children",
    "skip_links",
    "long_compound_titles",
    "invalid_group_pages",
    "missing_header_links",
    "language_groups",
];

pub(super) fn measure(place: &Place) -> [u64; CARD_MEASURES] {
    let mut counts = [0; CARD_MEASURES];
    counts[17] = u64::from(
        zephium_map::site::title_segments(&place.title).count() > 1
            && zephium_map::site::title_segments(&place.title)
                .flat_map(str::split_whitespace)
                .count()
                > SHORT_TITLE_WORDS,
    );
    for row in &place.sections {
        let mut children = BTreeSet::new();
        for child in &row.children {
            counts[15] +=
                u64::from(!children.insert((child.card_page(), child.locator.fragment())));
        }
        for e in std::iter::once(row).chain(&row.children) {
            counts[16] += u64::from(
                e.locator.element.is_some()
                    && e.key.page_match() == place.entry.page_match()
                    && zephium_map::site::skip_label(&e.label),
            );
        }
    }
    let mut labels = BTreeSet::new();
    let mut destinations = BTreeSet::new();
    let mut roles = std::collections::BTreeMap::new();
    counts[14] = u64::from(
        place
            .sections
            .iter()
            .filter(|e| e.tier == Tier::Primary)
            .count()
            < zephium_map::site::MIN_NAV_ROWS,
    );
    let mut lower_priority = false;
    for entry in place.sections.iter().filter(|e| e.tier == Tier::Primary) {
        counts[10] += u64::from(entry.menu_item);
        counts[11] += u64::from(
            entry
                .sources
                .iter()
                .all(|s| matches!(s, Source::Llms | Source::Sitemap)),
        );
        if let Some(role) = entry.role {
            let previous = roles.entry(role).or_insert(0);
            counts[12] += *previous;
            *previous += 1;
        }
        counts[13] += u64::from(entry.path_name);
        let url = entry.key.as_url();
        let label = entry.label.to_lowercase();
        let path = url.path().trim_matches('/');
        let leaf = path.rsplit('/').next().unwrap_or_default();
        let action = matches!(
            leaf,
            "signup" | "sign-up" | "login" | "register" | "signin" | "contact-sales"
        ) || path.ends_with("contact/sales")
            || matches!(
                label.as_str(),
                "sign in"
                    | "sign up"
                    | "log in"
                    | "request a demo"
                    | "contact sales"
                    | "start free trial"
            );
        let sentence = entry.label.chars().count() > MAX_PRIMARY_LABEL_CHARS
            || label.contains(". ")
            || label.split_whitespace().count() >= 7;
        let home = !entry.menu_only
            && entry.key.page_match() == place.entry.page_match()
            && entry.locator == Default::default();
        let nameless = matches!(
            label.as_str(),
            "" | "more" | "other" | "optional" | "menu" | "learn more"
        );
        let fresh_label = labels.insert(label.clone());
        let fresh_destination = destinations.insert((
            entry.card_page(),
            entry.locator.fragment(),
            entry.menu_only.then_some(&entry.id),
        ));
        let utility = entry.role == Some(SectionRole::Legal)
            || matches!(
                label.as_str(),
                "settings" | "rss" | "account" | "dashboard" | "translations" | "search"
            )
            || leaf.ends_with(".xml")
            || leaf.starts_with("llms") && leaf.ends_with(".txt");
        let detail = entry.locator.element.is_some()
            && url.host_str() != place.entry.as_url().host_str()
            && path.contains('/');
        let minor = action || home || nameless || utility || detail;
        let direct = entry.navigation_row();
        let order = !minor && direct && lower_priority;
        lower_priority |= minor || !direct;
        for (n, hit) in counts.iter_mut().zip([
            action,
            sentence,
            home,
            nameless,
            !fresh_label || !fresh_destination,
            false,
            order,
            utility,
            detail,
            false,
        ]) {
            *n += u64::from(hit);
        }
    }
    for child in place.sections.iter().flat_map(|e| &e.children) {
        let url = child.key.as_url();
        if url.path() == "/" && url.host_str() != place.entry.as_url().host_str() {
            counts[9] += 1;
            counts[5] += u64::from(zephium_map::site::display_hostname(url).is_none());
        }
    }
    counts
}

#[cfg(test)]
mod tests {
    use super::*;
    use url::Url;
    use zephium_map::{site::read_site, Doc, DocLimits, PageKey};

    #[test]
    fn counts_rows_for_independent_defects_and_checks_the_root_child_host() {
        let url = Url::parse("https://acme.example/").unwrap();
        let doc = Doc::from_html(
            &url,
            b"<nav><a href='/a'>First</a><a href='/b'>Second</a><a href='/c'>Third</a></nav>",
            None,
            DocLimits::SITE_PAGE,
        )
        .unwrap();
        let mut p = read_site(&doc, &Default::default()).unwrap();
        p.sections[0].label = "More".into();
        p.sections[0].key = p.entry.clone();
        p.sections[1].key = PageKey::from_url(&url.join("/login").unwrap()).unwrap();
        p.sections[2].label = "More".into();
        let mut child = p.sections[1].clone();
        child.key = PageKey::from_url(&Url::parse("https://docs.acme.example/").unwrap()).unwrap();
        p.sections[2].children.push(child);
        let scores: std::collections::BTreeMap<_, _> =
            CLASSES.into_iter().zip(measure(&p)).collect();
        assert_eq!(scores["nameless"], 2);
        for class in ["action", "self", "duplicate", "off_host_root_children"] {
            assert_eq!(scores[class], 1);
        }
        assert_eq!(scores["off_host_without_display"], 0);
    }
}

#[cfg(test)]
mod extended_tests {
    use super::*;
    use zephium_map::{site::read_site, Doc, DocLimits};
    #[test]
    fn counts_locale_duplicates_skip_links_and_compound_titles_across_tiers() {
        let url = url::Url::parse("https://workshop.example/").unwrap();
        let doc = Doc::from_html(
            &url,
            b"<nav><a href='/a'>One</a><a href='/b'>Two</a></nav>",
            None,
            DocLimits::SITE_PAGE,
        )
        .unwrap();
        let mut place = read_site(&doc, &Default::default()).unwrap();
        let first = place.sections.pop().unwrap();
        let mut alias = first.clone();
        alias.key = zephium_map::PageKey::from_url(&url.join("/de/b").unwrap()).unwrap();
        let row = &mut place.sections[0];
        row.tier = Tier::Secondary;
        row.key = place.entry.clone();
        row.locator.element = Some(zephium_map::key::LocatorText::new("main").unwrap());
        row.label = "Skip to content".into();
        row.children = vec![first, alias];
        place.title = "Site | Three Words".into();
        assert_eq!(measure(&place)[15..], [1, 1, 0]);
        place.title.push_str(" Four");
        assert_eq!(measure(&place)[15..], [1, 1, 1]);
    }
    #[test]
    fn provenance_and_role_pairs_are_counted_per_primary_row() {
        let url = url::Url::parse("https://workshop.example/").unwrap();
        let doc = Doc::from_html(
            &url,
            b"<nav><a href='/a'>A</a><a href='/b'>B</a><a href='/c'>C</a></nav>",
            None,
            DocLimits::SITE_PAGE,
        )
        .unwrap();
        let mut place = read_site(&doc, &Default::default()).unwrap();
        for e in &mut place.sections {
            e.role = Some(SectionRole::Documentation);
        }
        place.sections[0].menu_item = true;
        place.sections[0].path_name = true;
        place.sections[0].sources = vec![Source::Sitemap, Source::Llms];
        place.sections[1].sources = vec![Source::Llms, Source::PrimaryNav];
        assert_eq!(measure(&place)[10..15], [1, 1, 3, 1, 0]);
        place.sections[0].tier = Tier::Secondary;
        assert_eq!(measure(&place)[10..15], [0, 0, 1, 0, 1]);
    }
}
