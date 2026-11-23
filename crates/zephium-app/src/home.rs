//! What the shell and the host of Home's maps say to each other. The shell
//! keeps the searches and the tabs opened from them; the host keeps each live
//! map and reads results pages only through the shell.

use std::fmt;
use std::sync::{Arc, Mutex};

use zephium_core::home::{MapId, Search, SearchId};
use zephium_core::ids::{ProfileId, SpaceId, WindowId};
use zephium_core::ports::engine::PageHtml;
use zephium_core::search::SearchEngine;

/// A search whose words read as a map at high confidence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MapKind {
    Place,
    Compare,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MapRefusal {
    /// The words are not a map, or not surely one: they search as before.
    NotAMap,
    /// The window holds as many live maps as it may.
    Busy,
    Unavailable,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MapStart {
    pub map: MapId,
    pub search: SearchId,
    pub window: WindowId,
    pub profile: ProfileId,
    pub private: bool,
    pub engine: SearchEngine,
    pub query: String,
}

/// Hosts the maps. Calls come from the shell's thread and never wait on it.
pub trait HomeMaps: Send + Sync {
    fn start(&self, start: MapStart) -> Result<MapKind, MapRefusal>;
    /// The host still holds the map's driver, running or finished.
    fn holds(&self, map: MapId) -> bool;
    fn forget(&self, map: MapId);
    /// Drops everything the host remembers for a profile.
    fn forget_profile(&self, profile: ProfileId);
}

#[derive(Clone)]
pub struct HomeMapsAttachment(pub Arc<dyn HomeMaps>);
impl fmt::Debug for HomeMapsAttachment {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("HomeMapsAttachment")
    }
}

/// The results page the shell read for a map.
#[derive(Clone, PartialEq, Eq)]
pub struct ResultsDocument {
    pub url: url::Url,
    pub html: PageHtml,
    pub truncated: bool,
}

impl fmt::Debug for ResultsDocument {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ResultsDocument")
            .field("html", &self.html)
            .field("truncated", &self.truncated)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResultsRefusal {
    /// No search on Home has this map, or the address is not its results.
    Unknown,
    /// No tab could load the page, or it closed or failed before it read.
    Unavailable,
    /// The page left the engine's results, for a consent or sign-in page.
    Redirected,
}

type Done<T> = Arc<Mutex<Option<Box<dyn FnOnce(T) + Send>>>>;

/// Answers once; dropping it unanswered answers nothing.
pub struct Reply<T>(Done<T>);
impl<T> Reply<T> {
    pub fn new(done: impl FnOnce(T) + Send + 'static) -> Self {
        Self(Arc::new(Mutex::new(Some(Box::new(done)))))
    }
    pub fn finish(self, value: T) {
        let done = self
            .0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        if let Some(done) = done {
            done(value);
        }
    }
}
impl<T> Clone for Reply<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl<T> fmt::Debug for Reply<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Reply")
    }
}

pub type ResultsReply = Reply<Result<ResultsDocument, ResultsRefusal>>;

/// The page open from a search, as Home lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HomePage {
    pub tab: Option<zephium_core::ids::ItemId>,
    pub row: Option<zephium_core::home::PageRow>,
    pub open: bool,
    pub title: String,
    pub host: String,
}

/// The searches on Home of the focused profile and space, newest first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HomeRecords {
    pub profile: ProfileId,
    pub space: SpaceId,
    pub searches: Vec<Search>,
    /// Pages of each search, with titles read from the tabs still open.
    pub pages: Vec<Vec<HomePage>>,
}

/// Its `Debug` names the variant only: no address or words reach a log.
#[derive(Clone)]
pub enum HomeCommand {
    Attach(HomeMapsAttachment),
    Records {
        done: Reply<Option<HomeRecords>>,
    },
    /// The map's driver asks for its results page.
    ReadResults {
        map: MapId,
        url: url::Url,
        done: ResultsReply,
    },
    /// The page read had no results drawn yet: read it again as it is now.
    ReadAgain {
        map: MapId,
        done: ResultsReply,
    },
    /// The driver stopped waiting for the read.
    ReadAbandoned {
        map: MapId,
    },
    /// A page the map offers, at the address the host resolved for it;
    /// none when the map offers no page there.
    OpenPage {
        search: SearchId,
        slot: u8,
        entry: String,
        url: Option<url::Url>,
    },
    OpenResults {
        search: SearchId,
    },
    FocusSearch {
        search: SearchId,
    },
    ForgetSearch {
        search: SearchId,
    },
}

impl fmt::Debug for HomeCommand {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Attach(_) => "Home::Attach",
            Self::Records { .. } => "Home::Records",
            Self::ReadResults { .. } => "Home::ReadResults",
            Self::ReadAgain { .. } => "Home::ReadAgain",
            Self::ReadAbandoned { .. } => "Home::ReadAbandoned",
            Self::OpenPage { .. } => "Home::OpenPage",
            Self::OpenResults { .. } => "Home::OpenResults",
            Self::FocusSearch { .. } => "Home::FocusSearch",
            Self::ForgetSearch { .. } => "Home::ForgetSearch",
        })
    }
}

impl HomeCommand {
    /// What a person asks for, admitted and settled as an operation.
    pub fn is_intent(&self) -> bool {
        matches!(
            self,
            Self::OpenPage { .. }
                | Self::OpenResults { .. }
                | Self::FocusSearch { .. }
                | Self::ForgetSearch { .. }
        )
    }
}
