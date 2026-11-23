use super::*;

#[derive(Default)]
pub(super) struct Editions {
    pub(super) targets: BTreeSet<crate::PageMatch>,
    languages: BTreeSet<String>,
    matched: bool,
    unrelated: bool,
}

pub(super) fn edition_path(url: &Url) -> (&str, &str) {
    let host = url.domain().unwrap_or_default().trim_start_matches("www.");
    let host = host
        .split_once('.')
        .filter(|(first, rest)| card::locale(first) && psl::domain_str(rest).is_some())
        .map_or(host, |(_, rest)| rest);
    let path = url.path().trim_matches('/');
    let (first, rest) = path.split_once('/').unwrap_or((path, ""));
    (host, if card::locale(first) { rest } else { path })
}

impl Editions {
    pub(super) fn observe(
        &mut self,
        url: &Url,
        language: Option<&str>,
        current: &Url,
        alternates: &BTreeSet<crate::PageMatch>,
    ) {
        let Ok(key) = PageKey::from_url(url) else {
            return;
        };
        let target = key.page_match();
        let edition = url.port_or_known_default() == current.port_or_known_default()
            && url.query() == current.query()
            && url.fragment().is_none()
            && edition_path(url) == edition_path(current);
        let alternate = alternates.contains(&target);
        self.unrelated |= !edition && !alternate && language.is_none();
        self.matched |= (edition && !crate::key::same_page(url, current)) || alternate;
        if let Some(lang) = language {
            self.languages.insert(lang.to_owned());
        }
        self.targets.insert(target);
    }

    pub(super) fn merge(&mut self, other: Self) {
        self.targets.extend(other.targets);
        self.languages.extend(other.languages);
        self.matched |= other.matched;
        self.unrelated |= other.unrelated;
    }

    pub(super) fn is_picker(&self) -> bool {
        !self.unrelated
            && self.targets.len() >= MIN_MENU_ITEMS
            && (self.matched || self.languages.len() >= MIN_MENU_ITEMS)
    }
}
