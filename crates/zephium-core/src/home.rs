//! Home: the searches asked from the untouched tab, what was opened from
//! them, and where each of those pages came from. Kept in memory for the
//! session; the record is plain data so a durable log can replace `Searches`.

use std::collections::VecDeque;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::ids::{ItemId, ProfileId, SpaceId};
use crate::search::SearchEngine;

/// What a profile gets until it chooses.
pub const DEFAULT_HOME_MODE: HomeMode = HomeMode::Home;
pub const MAX_SEARCHES_PER_PROFILE: usize = 32;
pub const MAX_PAGES_PER_SEARCH: usize = 24;
pub const MAX_SEARCH_TEXT_BYTES: usize = 2048;
/// Kept from a page as it was; cleaned and cut shorter where it is shown.
pub const MAX_PAGE_TITLE_BYTES: usize = 1024;
pub const MAX_PAGE_HOST_BYTES: usize = 253;
pub const ID_HEX_BYTES: usize = 16;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HomeMode {
    #[default]
    Home,
    Classic,
}

impl HomeMode {
    pub fn from_setting(value: &str) -> Option<Self> {
        match value {
            "home" => Some(Self::Home),
            "classic" => Some(Self::Classic),
            _ => None,
        }
    }

    pub fn as_setting(self) -> &'static str {
        match self {
            Self::Home => "home",
            Self::Classic => "classic",
        }
    }
}

fn parse_hex(value: &str) -> Option<u64> {
    (value.len() == ID_HEX_BYTES
        && value
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')))
    .then(|| u64::from_str_radix(value, 16).ok())
    .flatten()
    .filter(|id| *id != 0)
}

macro_rules! hex_id {
    ($name:ident) => {
        #[derive(
            Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
        )]
        pub struct $name(pub u64);

        impl $name {
            /// Exactly sixteen lowercase hexadecimal digits, never zero.
            pub fn parse(value: &str) -> Option<Self> {
                parse_hex(value).map(Self)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{:016x}", self.0)
            }
        }
    };
}

hex_id!(SearchId);
hex_id!(MapId);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchKind {
    Place,
    Compare,
    Other,
    /// An address or app opened from Home's field: one line, no results.
    Address,
}

/// The row of a map a page was opened from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageRow {
    pub slot: u8,
    pub entry: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Page {
    pub tab: ItemId,
    pub row: Option<PageRow>,
    pub open: bool,
    pub title: String,
    pub host: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Search {
    pub id: SearchId,
    pub text: String,
    pub at_ms: u64,
    pub kind: SearchKind,
    pub engine: SearchEngine,
    pub profile: ProfileId,
    pub space: SpaceId,
    pub map: Option<MapId>,
    /// The tab the results page was opened in, while it is open.
    pub results: Option<ItemId>,
    /// Where the results page is, to open it again. Never shown.
    pub results_url: Option<String>,
    /// The results tab has been the tab in front.
    #[serde(default)]
    pub results_seen: bool,
    pub pages: Vec<Page>,
}

impl Search {
    pub fn page(&self, tab: ItemId) -> Option<&Page> {
        self.pages.iter().find(|page| page.tab == tab)
    }

    pub fn page_mut(&mut self, tab: ItemId) -> Option<&mut Page> {
        self.pages.iter_mut().find(|page| page.tab == tab)
    }

    /// Adds or refreshes the page for `tab`; past the bound the oldest
    /// closed page goes first, then the oldest.
    pub fn keep_page(&mut self, page: Page) {
        if let Some(held) = self.page_mut(page.tab) {
            *held = page;
            return;
        }
        if self.pages.len() >= MAX_PAGES_PER_SEARCH {
            let at = self.pages.iter().position(|page| !page.open).unwrap_or(0);
            self.pages.remove(at);
        }
        self.pages.push(page);
    }
}

/// Where a tab opened from Home came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Origin {
    pub search: SearchId,
    pub opener: Option<ItemId>,
}

/// How a search's text is compared with another asked in the same space.
pub fn normalized(text: &str) -> String {
    text.split_whitespace()
        .map(str::to_lowercase)
        .collect::<Vec<_>>()
        .join(" ")
}

/// The session's searches, oldest first, at most `MAX_SEARCHES_PER_PROFILE`
/// for each profile.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Searches {
    searches: VecDeque<Search>,
}

impl Searches {
    /// Records `search` as the newest and returns those it pushed out.
    pub fn record(&mut self, search: Search) -> Vec<Search> {
        let profile = search.profile;
        self.searches.push_back(search);
        let mut evicted = Vec::new();
        while self.count(profile) > MAX_SEARCHES_PER_PROFILE {
            let Some(at) = self.searches.iter().position(|s| s.profile == profile) else {
                break;
            };
            if let Some(old) = self.searches.remove(at) {
                evicted.push(old);
            }
        }
        evicted
    }

    fn count(&self, profile: ProfileId) -> usize {
        self.searches
            .iter()
            .filter(|s| s.profile == profile)
            .count()
    }

    /// The search with the same normalized text asked in the same space.
    pub fn same(&self, profile: ProfileId, space: SpaceId, text: &str) -> Option<SearchId> {
        let wanted = normalized(text);
        self.searches
            .iter()
            .rev()
            .find(|s| s.profile == profile && s.space == space && normalized(&s.text) == wanted)
            .map(|s| s.id)
    }

    /// Moves `id` to the newest place, asked again at `at_ms`.
    pub fn touch(&mut self, id: SearchId, at_ms: u64) -> bool {
        let Some(at) = self.searches.iter().position(|s| s.id == id) else {
            return false;
        };
        let Some(mut search) = self.searches.remove(at) else {
            return false;
        };
        search.at_ms = at_ms;
        self.searches.push_back(search);
        true
    }

    pub fn get(&self, id: SearchId) -> Option<&Search> {
        self.searches.iter().find(|s| s.id == id)
    }

    pub fn get_mut(&mut self, id: SearchId) -> Option<&mut Search> {
        self.searches.iter_mut().find(|s| s.id == id)
    }

    pub fn by_map(&self, map: MapId) -> Option<&Search> {
        self.searches.iter().find(|s| s.map == Some(map))
    }

    pub fn forget(&mut self, id: SearchId) -> Option<Search> {
        let at = self.searches.iter().position(|s| s.id == id)?;
        self.searches.remove(at)
    }

    /// Forgets every search `drop` accepts, returning them.
    pub fn forget_where(&mut self, mut drop: impl FnMut(&Search) -> bool) -> Vec<Search> {
        let mut gone = Vec::new();
        let mut kept = VecDeque::with_capacity(self.searches.len());
        for search in self.searches.drain(..) {
            if drop(&search) {
                gone.push(search);
            } else {
                kept.push_back(search);
            }
        }
        self.searches = kept;
        gone
    }

    /// The searches of one profile and space, oldest first.
    pub fn in_scope(
        &self,
        profile: ProfileId,
        space: SpaceId,
    ) -> impl DoubleEndedIterator<Item = &Search> {
        self.searches
            .iter()
            .filter(move |s| s.profile == profile && s.space == space)
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut Search> {
        self.searches.iter_mut()
    }

    pub fn len(&self) -> usize {
        self.searches.len()
    }

    pub fn is_empty(&self) -> bool {
        self.searches.is_empty()
    }
}

/// Cuts `text` to at most `max` bytes at a character boundary.
pub fn clipped(text: &str, max: usize) -> String {
    let mut at = text.len().min(max);
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    text[..at].to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn search(id: u64, profile: ProfileId, space: SpaceId, text: &str) -> Search {
        Search {
            id: SearchId(id),
            text: text.into(),
            at_ms: id,
            kind: SearchKind::Other,
            engine: SearchEngine::DuckDuckGo,
            profile,
            space,
            map: None,
            results: None,
            results_url: None,
            results_seen: false,
            pages: Vec::new(),
        }
    }

    fn page(tab: ItemId, open: bool) -> Page {
        Page {
            tab,
            row: None,
            open,
            title: String::new(),
            host: String::new(),
        }
    }

    #[test]
    fn ids_are_exactly_sixteen_lowercase_hex_digits_and_never_zero() {
        assert_eq!(SearchId(0x2a).to_string(), "000000000000002a");
        assert_eq!(SearchId::parse("000000000000002a"), Some(SearchId(0x2a)));
        assert_eq!(MapId::parse("ffffffffffffffff"), Some(MapId(u64::MAX)));
        for refused in [
            "",
            "2a",
            "000000000000000",
            "00000000000000002a",
            "000000000000002A",
            "000000000000002g",
            "0000000000000000",
            "00000000000000\u{e9}",
            "+00000000000002a",
        ] {
            assert_eq!(SearchId::parse(refused), None, "{refused}");
        }
    }

    #[test]
    fn modes_read_only_their_two_values_and_start_as_home() {
        assert_eq!(HomeMode::from_setting("home"), Some(HomeMode::Home));
        assert_eq!(HomeMode::from_setting("classic"), Some(HomeMode::Classic));
        assert_eq!(HomeMode::from_setting("Home"), None);
        assert_eq!(HomeMode::from_setting(""), None);
        for mode in [HomeMode::Home, HomeMode::Classic] {
            assert_eq!(HomeMode::from_setting(mode.as_setting()), Some(mode));
        }
        assert_eq!(HomeMode::default(), DEFAULT_HOME_MODE);
    }

    #[test]
    fn each_profile_keeps_its_newest_searches_only() {
        let (a, b) = (ProfileId::generate(), ProfileId::generate());
        let space = SpaceId::generate();
        let mut searches = Searches::default();
        searches.record(search(1_000, b, space, "other"));
        for n in 1..=MAX_SEARCHES_PER_PROFILE as u64 {
            assert!(searches.record(search(n, a, space, "q")).is_empty());
        }
        let evicted = searches.record(search(99, a, space, "q"));
        assert_eq!(evicted.len(), 1);
        assert_eq!(evicted[0].id, SearchId(1));
        assert!(searches.get(SearchId(1_000)).is_some());
        assert_eq!(
            searches.in_scope(a, space).count(),
            MAX_SEARCHES_PER_PROFILE
        );
        assert_eq!(
            searches.in_scope(a, space).last().map(|s| s.id),
            Some(SearchId(99))
        );
    }

    #[test]
    fn the_same_text_in_the_same_space_is_found_and_moves_to_the_front() {
        let profile = ProfileId::generate();
        let (space, other) = (SpaceId::generate(), SpaceId::generate());
        let mut searches = Searches::default();
        searches.record(search(1, profile, space, "Stripe  Pricing"));
        searches.record(search(2, profile, space, "figma"));
        searches.record(search(3, profile, other, "stripe pricing"));
        assert_eq!(
            searches.same(profile, space, " stripe pricing "),
            Some(SearchId(1))
        );
        assert_eq!(
            searches.same(profile, other, "STRIPE pricing"),
            Some(SearchId(3))
        );
        assert_eq!(searches.same(ProfileId::generate(), space, "figma"), None);
        assert!(searches.touch(SearchId(1), 50));
        let order: Vec<_> = searches.in_scope(profile, space).map(|s| s.id).collect();
        assert_eq!(order, [SearchId(2), SearchId(1)]);
        assert_eq!(searches.get(SearchId(1)).map(|s| s.at_ms), Some(50));
        assert!(!searches.touch(SearchId(9), 1));
    }

    #[test]
    fn pages_are_bounded_and_closed_ones_go_first() {
        let mut held = search(1, ProfileId::generate(), SpaceId::generate(), "q");
        let tabs: Vec<_> = (0..=MAX_PAGES_PER_SEARCH)
            .map(|_| ItemId::generate())
            .collect();
        for (n, tab) in tabs.iter().take(MAX_PAGES_PER_SEARCH).enumerate() {
            held.keep_page(page(*tab, n != 3));
        }
        held.keep_page(page(tabs[MAX_PAGES_PER_SEARCH], true));
        assert_eq!(held.pages.len(), MAX_PAGES_PER_SEARCH);
        assert!(held.page(tabs[3]).is_none());
        assert!(held.page(tabs[0]).is_some());
        held.keep_page(page(ItemId::generate(), true));
        assert!(held.page(tabs[0]).is_none());
        let mut refreshed = page(tabs[1], false);
        refreshed.title = "t".into();
        held.keep_page(refreshed);
        assert_eq!(held.page(tabs[1]).map(|p| p.title.as_str()), Some("t"));
        assert_eq!(held.pages.len(), MAX_PAGES_PER_SEARCH);
    }

    #[test]
    fn clipping_keeps_a_character_boundary() {
        assert_eq!(clipped("héllo", 2), "h");
        assert_eq!(clipped("héllo", 3), "hé");
        assert_eq!(clipped("abc", 10), "abc");
    }
}
