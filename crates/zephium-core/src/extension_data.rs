//! Browser data extensions read and change through Chrome's APIs for it.
//! The shell answers from the profile's stores; a private profile keeps none
//! of it.

use crate::bookmarks::{BookmarkChange, BookmarkReply, BookmarkRequest};

/// Addresses one history search returns.
pub const MAX_HISTORY_RESULTS: u32 = 1_000;
/// Visits one address lists.
pub const MAX_VISITS: u32 = 1_000;
pub const MAX_TOP_SITES: u32 = 10;
pub const MAX_CLOSED_TABS: u32 = 25;
/// Addresses one removal names. Past it, listeners hear that all history
/// went, which tells them to drop whatever they cached.
pub const MAX_REMOVED_URLS: usize = 1_000;
pub const MAX_QUERY_BYTES: usize = 4 * 1024;

/// One address in history: Zephium keeps one row per visit, so an address
/// is named by its oldest visit that is still kept.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryEntry {
    pub id: i64,
    pub url: String,
    pub title: String,
    /// Unix seconds.
    pub last_visit: i64,
    pub visits: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VisitEntry {
    pub id: i64,
    /// The address's [`HistoryEntry::id`].
    pub entry: i64,
    /// Unix seconds.
    pub visited_at: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TopSite {
    pub url: String,
    pub title: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClosedTab {
    pub session: String,
    pub url: String,
    pub title: String,
    pub closed_at_ms: Option<u64>,
}

/// Times are unix seconds; ranges include `start` and exclude `end`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HistoryRequest {
    /// Addresses visited in the range whose address or title holds every
    /// word of `text` (all of them when empty), most recently visited first.
    Search {
        text: String,
        start: i64,
        end: i64,
        limit: u32,
    },
    Visits {
        url: String,
    },
    /// The one address, for the event about a visit to it.
    Address {
        url: String,
    },
    /// Web addresses visited most over recent weeks.
    MostVisited {
        limit: u32,
    },
    /// A visit now, as if the page had been opened; the shell records it.
    Add {
        url: String,
    },
    RemoveUrl {
        url: String,
    },
    RemoveRange {
        start: i64,
        end: i64,
    },
    RemoveAll,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HistoryReply {
    Entries(Vec<HistoryEntry>),
    Visits(Vec<VisitEntry>),
    Sites(Vec<TopSite>),
    /// `urls` lost their last visit; `all` when every visit went, or more
    /// addresses than [`MAX_REMOVED_URLS`] did.
    Removed {
        visits: u32,
        urls: Vec<String>,
        all: bool,
    },
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExtensionDataCall {
    History(HistoryRequest),
    Bookmarks(BookmarkRequest),
    /// The profile's closed tabs, newest first.
    RecentlyClosed {
        limit: u32,
    },
    /// The named closed tab, or the newest.
    Restore {
        session: Option<String>,
    },
}

impl ExtensionDataCall {
    /// What an extension may ask; imports, exports and the panel's own
    /// requests stay the browser's.
    pub fn validate(&self) -> bool {
        let address = |url: &str| url.len() <= crate::bookmarks::MAX_URL_BYTES;
        match self {
            Self::History(request) => match request {
                HistoryRequest::Search {
                    text, start, end, ..
                } => text.len() <= MAX_QUERY_BYTES && start <= end,
                HistoryRequest::Visits { url }
                | HistoryRequest::Address { url }
                | HistoryRequest::RemoveUrl { url } => address(url),
                HistoryRequest::Add { url } => crate::navigation::is_allowed_str(url),
                HistoryRequest::RemoveRange { start, end } => start <= end,
                HistoryRequest::MostVisited { .. } | HistoryRequest::RemoveAll => true,
            },
            Self::Bookmarks(request) => match request {
                BookmarkRequest::Entries { ids } => {
                    !ids.is_empty() && ids.len() <= crate::bookmarks::MAX_ENTRIES as usize
                }
                BookmarkRequest::Find { query, url, title } => {
                    query
                        .as_ref()
                        .is_none_or(|query| query.len() <= MAX_QUERY_BYTES)
                        && url.as_deref().is_none_or(address)
                        && title
                            .as_ref()
                            .is_none_or(|title| title.len() <= MAX_QUERY_BYTES)
                }
                BookmarkRequest::Create { url, title, .. } => {
                    url.as_deref().is_none_or(address) && title.len() <= MAX_QUERY_BYTES
                }
                BookmarkRequest::Update { url, title, .. } => {
                    url.as_deref().is_none_or(address)
                        && title
                            .as_ref()
                            .is_none_or(|title| title.len() <= MAX_QUERY_BYTES)
                }
                BookmarkRequest::ChildEntries { .. }
                | BookmarkRequest::Subtree { .. }
                | BookmarkRequest::Recent { .. }
                | BookmarkRequest::Relocate { .. }
                | BookmarkRequest::Delete { .. } => true,
                _ => false,
            },
            Self::RecentlyClosed { .. } => true,
            Self::Restore { session } => session.as_ref().is_none_or(|id| id.len() <= 64),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExtensionDataReply {
    History(HistoryReply),
    Bookmarks(BookmarkReply),
    Closed(Vec<ClosedTab>),
    Restored(ClosedTab),
    /// The profile keeps no such data: it is private, or its storage failed.
    Unavailable,
    /// Too many requests are already waiting.
    Busy,
}

/// What changed in a profile's data, for the extensions listening.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExtensionDataChange {
    Visited(HistoryEntry),
    VisitsRemoved {
        urls: Vec<String>,
        all: bool,
    },
    Bookmark(BookmarkChange),
    /// A tab was closed or reopened.
    Sessions,
}

/// Which changes some extension in a profile listens to; the shell does the
/// work of describing a change only when one does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ExtensionDataInterest {
    pub history: bool,
    pub bookmarks: bool,
    pub sessions: bool,
}

impl ExtensionDataInterest {
    pub fn any(self) -> bool {
        self.history || self.bookmarks || self.sessions
    }
}
