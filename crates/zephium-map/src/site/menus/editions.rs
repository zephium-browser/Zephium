use super::*;

#[derive(Default)]
pub(super) struct Editions {
    pub(super) targets: BTreeSet<crate::PageMatch>,
    matched: bool,
    unrelated: bool,
}

fn edition_path(url: &Url, localized: bool) -> (&str, &str) {
    let host = url.domain().unwrap_or_default().trim_start_matches("www.");
    let host = host
        .split_once('.')
        .filter(|(first, rest)| card::locale(first) && psl::domain_str(rest).is_some())
        .map_or(host, |(_, rest)| rest);
    let path = url.path().trim_matches('/');
    let (first, rest) = path.split_once('/').unwrap_or((path, ""));
    (
        host,
        if localized && card::locale(first) {
            rest
        } else {
            path
        },
    )
}

impl Editions {
    pub(super) fn observe(
        &mut self,
        url: &Url,
        localized: &BTreeSet<crate::PageMatch>,
        current: &Url,
        alternates: &BTreeSet<(crate::PageMatch, Option<String>)>,
    ) {
        let Ok(key) = PageKey::from_url(url) else {
            return;
        };
        let target = key.page_match();
        let edition = url.port_or_known_default() == current.port_or_known_default()
            && url.query() == current.query()
            && url.fragment().is_none()
            && edition_path(url, localized.contains(&target)) == edition_path(current, true);
        let alternate = alternates.contains(&(target.clone(), url.fragment().map(str::to_owned)));
        self.unrelated |= !edition && !alternate;
        self.matched |= (edition && !crate::key::same_page(url, current)) || alternate;
        self.targets.insert(target);
    }

    pub(super) fn merge(&mut self, other: Self) {
        self.targets.extend(other.targets);
        self.matched |= other.matched;
        self.unrelated |= other.unrelated;
    }

    pub(super) fn is_picker(&self) -> bool {
        !self.unrelated && self.targets.len() >= MIN_MENU_ITEMS && self.matched
    }
}
