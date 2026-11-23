use serde::{Deserialize, Serialize};
use tauri_specta::Event;
use zephium_core::search::SearchEngine;
use zephium_map_driver::wire::MapChange;

/// The searches on Home of one profile and space, newest first.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, specta::Type)]
pub(crate) struct HomeSnapshotV1 {
    pub(crate) profile: String,
    pub(crate) space: String,
    pub(crate) searches: Vec<HomeSearchV1>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub(crate) enum HomeSearchKindV1 {
    Place,
    Compare,
    Other,
    Address,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, specta::Type)]
pub(crate) struct HomeSearchV1 {
    pub(crate) id: String,
    pub(crate) text: String,
    #[specta(type = f64)]
    pub(crate) at_ms: u64,
    pub(crate) kind: HomeSearchKindV1,
    /// The engine the search was asked with, for its mark.
    pub(crate) engine: SearchEngine,
    /// A tab shows the search's results page.
    pub(crate) results_open: bool,
    pub(crate) map: Option<HomeMapV1>,
    pub(crate) pages: Vec<HomePageV1>,
}

/// A map as folded so far: replaying `changes` in order gives what the
/// stream showed; a streamed change at or below `seq` is already in it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, specta::Type)]
pub(crate) struct HomeMapV1 {
    pub(crate) id: String,
    pub(crate) seq: Option<u32>,
    /// The map can still open a page inside itself.
    pub(crate) live: bool,
    pub(crate) changes: Vec<MapChange>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub(crate) struct HomePageV1 {
    /// The tab while it is open.
    pub(crate) tab: Option<String>,
    pub(crate) slot: Option<u8>,
    pub(crate) entry: Option<String>,
    pub(crate) open: bool,
    pub(crate) title: String,
    pub(crate) host: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, specta::Type, Event)]
pub(crate) struct HomeMapChangedV1 {
    pub(crate) map: String,
    pub(crate) search: String,
    pub(crate) change: MapChange,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, specta::Type, Event)]
pub(crate) struct HomeChangedV1 {
    pub(crate) profile: String,
    pub(crate) space: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, specta::Type, Event)]
pub(crate) struct HomeShowV1 {
    pub(crate) search: String,
    /// The row the closed page was opened from, as the snapshot's pages
    /// name it (`entry` is the entry id, `site`, `social-<network>` or
    /// `also-<index>`); null for anything not opened from a row.
    pub(crate) slot: Option<u8>,
    pub(crate) entry: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub(crate) enum HomeInsideRefusalV1 {
    /// The search has no running map any more.
    MapGone,
    SearchUnknown,
    NoPlace,
    UnknownEntry,
    NotOpenable,
    TooManyWaiting,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub(crate) struct HomeInsideAdmissionV1 {
    pub(crate) accepted: bool,
    pub(crate) refusal: Option<HomeInsideRefusalV1>,
}

impl HomeInsideAdmissionV1 {
    pub(crate) fn refused(refusal: HomeInsideRefusalV1) -> Self {
        Self {
            accepted: false,
            refusal: Some(refusal),
        }
    }
}
