use std::collections::BTreeSet;
use zephium_map::{site::MAX_PRIMARY_LABEL_CHARS, Place, SectionRole, Source, Tier};

pub(in crate::map_bench) const CLASSES: [&str; 10] = [
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
];

pub(super) fn measure(place: &Place) -> [u64; CLASSES.len()] {
    let mut counts = [0; CLASSES.len()];
    let mut labels = BTreeSet::new();
    let mut destinations = BTreeSet::new();
    let mut lower_priority = false;
    for entry in place.sections.iter().filter(|e| e.tier == Tier::Primary) {
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
        let home = entry.key.page_match() == place.entry.page_match()
            && entry.locator == Default::default();
        let nameless = matches!(
            label.as_str(),
            "" | "more" | "other" | "optional" | "menu" | "learn more"
        );
        let fresh_label = labels.insert(label.clone());
        let fresh_destination = destinations.insert((entry.card_page(), entry.locator.fragment()));
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
        let direct = entry.sources.contains(&Source::PrimaryNav)
            || entry.sources.contains(&Source::MenuGroup);
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
