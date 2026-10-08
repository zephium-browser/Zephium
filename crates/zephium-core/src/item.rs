//! A sidebar item: folder or tab. A pinned tab is a bookmark that can be
//! alive; there is no separate bookmarks model.

use url::Url;

use crate::ids::{ItemId, ProfileId, SpaceId};

pub const MAX_PAGE_TITLE_CHARS: usize = 512;

fn page_title_char_is_allowed(c: char) -> bool {
    !c.is_control()
        && !matches!(
            c,
            '\u{061c}'
                | '\u{200e}'
                | '\u{200f}'
                | '\u{202a}'..='\u{202e}'
                | '\u{2066}'..='\u{2069}'
        )
}

/// Reports whether a title is already in the canonical representation used
/// by browser chrome. This is the allocation-free validation counterpart to
/// [`sanitize_page_title`] for trusted-state projection boundaries.
pub fn page_title_is_sanitized(title: &str) -> bool {
    if title.is_empty() {
        return false;
    }
    let mut characters = 0_usize;
    for c in title.chars() {
        if !page_title_char_is_allowed(c) {
            return false;
        }
        characters += 1;
        if characters > MAX_PAGE_TITLE_CHARS {
            return false;
        }
    }
    true
}

/// Page titles and legacy/session titles are equally untrusted. Keep one
/// canonical sanitizer so restored data cannot bypass the renderer boundary.
pub fn sanitize_page_title(title: &str) -> String {
    let title: String = title
        .chars()
        .filter(|c| page_title_char_is_allowed(*c))
        .take(MAX_PAGE_TITLE_CHARS)
        .collect();
    if title.is_empty() {
        "Untitled".into()
    } else {
        title
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lifecycle {
    Active,
    Inactive,
    Hibernated,
}

/// A browser-owned page has no URL or native content WebView. The closed
/// discriminator can later be extended for independently owned documents.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserOwnedTab {
    Settings,
    Extensions,
}

impl BrowserOwnedTab {
    pub const fn title(self) -> &'static str {
        match self {
            Self::Settings => "Settings",
            Self::Extensions => "Extensions",
        }
    }
}

/// Content ownership is separate from a committed network URL.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabContent {
    Web,
    BrowserOwned(BrowserOwnedTab),
    /// Native guest ownership is bound separately by the extension broker.
    /// This marker carries no URL, install ID, or execution authority.
    ExtensionOwned,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum SpaceSection {
    Pinned,
    Today,
}

/// Where an item lives: the profile-wide favorites grid, or a section of a
/// space. Children of a folder share the folder's placement.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Placement {
    Favorites {
        profile: ProfileId,
    },
    Space {
        space: SpaceId,
        section: SpaceSection,
    },
}

#[derive(Clone, Debug)]
pub struct TabState {
    pub content: TabContent,
    pub title: String,
    pub url: Option<Url>,
    pub loading: bool,
    /// Something the page asked for that waits on the person. Runtime only,
    /// and cleared when the tab commits another document.
    pub page_request: Option<Box<PageRequest>>,
    pub can_go_back: bool,
    pub can_go_forward: bool,
    pub zoom: f64,
    pub lifecycle: Lifecycle,
    /// The last navigation the person asked for that did not load. Runtime
    /// only: never persisted, and cleared by the next attempt or commit.
    pub failure: Option<Box<NavigationFailure>>,
    /// Process-local native state, never restored or persisted.
    pub capture: Option<(
        crate::ports::engine::NavigationPresentationId,
        crate::ports::engine::MediaCaptureState,
    )>,
    // Distinct from `url`: a restored/hibernated tab has a url but no live view.
    pub(crate) view: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PageRequest {
    /// A link for an application on this computer, and that application's
    /// name when the system knows one.
    ExternalApp { url: Url, app: Option<String> },
    /// A new tab the page tried to open that was not let through, with its
    /// address when it was an ordinary web page.
    Popup { url: Option<Url> },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NavigationFailure {
    pub url: Url,
    pub reason: crate::ports::engine::NavigationFailureReason,
}

impl TabState {
    pub(crate) fn new() -> Self {
        Self {
            content: TabContent::Web,
            title: "New Tab".into(),
            url: None,
            loading: false,
            page_request: None,
            can_go_back: false,
            can_go_forward: false,
            zoom: 1.0,
            lifecycle: Lifecycle::Inactive,
            failure: None,
            capture: None,
            view: false,
        }
    }

    pub(crate) fn browser_owned(page: BrowserOwnedTab) -> Self {
        Self {
            content: TabContent::BrowserOwned(page),
            title: page.title().into(),
            ..Self::new()
        }
    }

    pub(crate) fn extension_owned() -> Self {
        Self {
            content: TabContent::ExtensionOwned,
            title: "Extension".into(),
            ..Self::new()
        }
    }

    pub fn has_view(&self) -> bool {
        self.view
    }
}

#[derive(Clone, Debug)]
pub enum ItemKind {
    Folder { name: String },
    Tab(TabState),
}

#[derive(Clone, Debug)]
pub struct Item {
    pub id: ItemId,
    pub parent: Option<ItemId>,
    pub placement: Placement,
    pub kind: ItemKind,
}

impl Item {
    pub fn tab(&self) -> Option<&TabState> {
        match &self.kind {
            ItemKind::Tab(t) => Some(t),
            ItemKind::Folder { .. } => None,
        }
    }

    pub(crate) fn tab_mut(&mut self) -> Option<&mut TabState> {
        match &mut self.kind {
            ItemKind::Tab(t) => Some(t),
            ItemKind::Folder { .. } => None,
        }
    }
}
