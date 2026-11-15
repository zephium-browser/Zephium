//! The host's own frame table for one attachment of one view, rebuilt from
//! the module's frame records. Every record is a renderer claim; the table
//! only gives later claims something to be consistent with.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use url::Url;
use zephium_webext_runtime::context::{DocumentGeneration, FrameId};
use zephium_webext_runtime::pattern::DocumentOrigin;
use zephium_webkit_web_process_protocol::protocol::{document_origin, FrameEvent, FrameInfo};

/// Frames one view may have, the module's own per-page limit.
pub const MAX_FRAMES_PER_VIEW: usize = 128;
const MAX_RETIRED_FRAME_IDS: usize = 128;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Document {
    pub seq: u64,
    pub external: BTreeSet<String>,
    pub generation: DocumentGeneration,
    pub url: Url,
    /// The exact origin string grants are matched against, if any.
    pub origin: Option<String>,
    pub facts: FrameInfo,
    pub security_origin: DocumentOrigin,
    pub creator: Option<Url>,
    commits: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Frame {
    pub chrome: FrameId,
    pub main: bool,
    pub document: Document,
}

/// What a frame record did to the table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum FrameChange {
    Created {
        module_frame: u64,
        frame: FrameId,
        main: bool,
        parent: Option<FrameId>,
        opaque: bool,
        creator: Option<Url>,
        document: DocumentGeneration,
        url: Url,
    },
    Loaded {
        frame: FrameId,
        document: DocumentGeneration,
    },
    Removed {
        module_frame: u64,
        frame: FrameId,
        document: DocumentGeneration,
    },
    /// The current document reached `DOMContentLoaded`, or `load` when `completed`.
    Progress {
        frame: FrameId,
        document: DocumentGeneration,
        completed: bool,
    },
}

/// A record the table refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FrameRefusal {
    InvalidUrl,
    InconsistentFacts,
    StaleDocument,
    Capacity,
    Exhausted,
}

#[derive(Debug, Default)]
pub(crate) struct FrameTable {
    frames: BTreeMap<u64, Frame>,
    retired: VecDeque<(u64, FrameId)>,
    next_chrome: u64,
}

impl FrameTable {
    pub fn get(&self, module_frame: u64) -> Option<&Frame> {
        self.frames.get(&module_frame)
    }

    pub fn record_external(&mut self, module_frame: u64, install: String) {
        if let Some(frame) = self.frames.get_mut(&module_frame) {
            frame.document.external.insert(install);
        }
    }

    pub fn by_chrome(&self, chrome: FrameId) -> Option<(u64, &Frame)> {
        self.frames
            .iter()
            .find(|(_, frame)| frame.chrome == chrome)
            .map(|(id, frame)| (*id, frame))
    }

    pub fn iter(&self) -> impl Iterator<Item = (u64, &Frame)> {
        self.frames.iter().map(|(id, frame)| (*id, frame))
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.frames.len()
    }

    /// Applies one record. `mint` yields host document generations.
    pub fn apply(
        &mut self,
        event: &FrameEvent,
        mint: &mut dyn FnMut() -> Option<DocumentGeneration>,
        changes: &mut Vec<FrameChange>,
    ) -> Result<(), FrameRefusal> {
        match event {
            FrameEvent::Created(info) => self.create(info, mint, changes),
            FrameEvent::Committed(info) => match self.frames.get_mut(&info.frame_id) {
                Some(frame) if frame.document.seq == info.document_seq => {
                    if frame.document.facts != *info {
                        return Err(FrameRefusal::InconsistentFacts);
                    }
                    frame.document.commits = frame.document.commits.saturating_add(1);
                    if frame.document.commits == 2 {
                        changes.push(FrameChange::Loaded {
                            frame: frame.chrome,
                            document: frame.document.generation,
                        });
                    }
                    Ok(())
                }
                _ => self.create(info, mint, changes),
            },
            FrameEvent::Destroyed {
                frame_id,
                document_seq,
            } => {
                if self
                    .frames
                    .get(frame_id)
                    .is_some_and(|frame| frame.document.seq == *document_seq)
                {
                    self.remove(*frame_id, changes);
                }
                Ok(())
            }
            FrameEvent::ContentLoaded {
                frame_id,
                document_seq,
            }
            | FrameEvent::Completed {
                frame_id,
                document_seq,
            } => {
                let frame = self
                    .frames
                    .get(frame_id)
                    .filter(|frame| frame.document.seq == *document_seq)
                    .ok_or(FrameRefusal::StaleDocument)?;
                changes.push(FrameChange::Progress {
                    frame: frame.chrome,
                    document: frame.document.generation,
                    completed: matches!(event, FrameEvent::Completed { .. }),
                });
                Ok(())
            }
            FrameEvent::Evaluated(_) | FrameEvent::ExternalPage { .. } => Ok(()),
            FrameEvent::Reset => {
                self.clear(changes);
                Ok(())
            }
        }
    }

    /// Removes every frame, as for a reset, detach or lost process.
    pub fn clear(&mut self, changes: &mut Vec<FrameChange>) {
        let ids: Vec<u64> = self.frames.keys().copied().collect();
        for id in ids {
            self.remove(id, changes);
        }
    }

    fn create(
        &mut self,
        info: &FrameInfo,
        mint: &mut dyn FnMut() -> Option<DocumentGeneration>,
        changes: &mut Vec<FrameChange>,
    ) -> Result<(), FrameRefusal> {
        let url = Url::parse(&info.uri).map_err(|_| FrameRefusal::InvalidUrl)?;
        let parent = match info.parent_frame_id {
            Some(id) if !info.is_main && id != info.frame_id => Some(
                self.frames
                    .get(&id)
                    .ok_or(FrameRefusal::InconsistentFacts)?,
            ),
            Some(_) => return Err(FrameRefusal::InconsistentFacts),
            None => None,
        };
        let mut ancestor = info.parent_frame_id;
        let mut remaining = MAX_FRAMES_PER_VIEW;
        while let Some(id) = ancestor {
            if id == info.frame_id || remaining == 0 {
                return Err(FrameRefusal::InconsistentFacts);
            }
            remaining -= 1;
            ancestor = self
                .frames
                .get(&id)
                .and_then(|frame| frame.document.facts.parent_frame_id);
        }
        let creator = match &info.creator_uri {
            Some(uri) => {
                let creator = Url::parse(uri).map_err(|_| FrameRefusal::InvalidUrl)?;
                let known = parent.and_then(|parent| {
                    if parent.document.url.scheme() == "about" {
                        parent.document.creator.as_ref()
                    } else {
                        Some(&parent.document.url)
                    }
                });
                if known != Some(&creator)
                    || !parent
                        .is_some_and(|parent| parent.document.security_origin.is_tuple_of(&creator))
                {
                    return Err(FrameRefusal::InconsistentFacts);
                }
                Some(creator)
            }
            None => None,
        };
        if url.scheme() == "data" && !info.opaque {
            return Err(FrameRefusal::InconsistentFacts);
        }
        let security_origin = if info.opaque {
            DocumentOrigin::opaque(creator.as_ref())
        } else if url.scheme() == "about" {
            creator
                .as_ref()
                .map_or_else(|| DocumentOrigin::of_url(&url), DocumentOrigin::of_url)
        } else {
            DocumentOrigin::of_url(&url)
        };
        let parent = parent.map(|parent| parent.chrome);
        if let Some(existing) = self.frames.get(&info.frame_id) {
            if info.document_seq <= existing.document.seq || existing.main != info.is_main {
                return Err(FrameRefusal::StaleDocument);
            }
        }
        if info.is_main {
            self.clear(changes);
        } else if !self.frames.contains_key(&info.frame_id)
            && self.frames.len() >= MAX_FRAMES_PER_VIEW
        {
            return Err(FrameRefusal::Capacity);
        }
        let generation = mint().ok_or(FrameRefusal::Exhausted)?;
        let chrome = if info.is_main {
            FrameId::MAIN
        } else if let Some(frame) = self.frames.get(&info.frame_id) {
            frame.chrome
        } else if let Some(at) = self.retired.iter().position(|(id, _)| *id == info.frame_id) {
            self.retired
                .remove(at)
                .map_or(FrameId::MAIN, |(_, chrome)| chrome)
        } else {
            self.next_chrome = self
                .next_chrome
                .checked_add(1)
                .ok_or(FrameRefusal::Exhausted)?;
            FrameId::new(self.next_chrome)
        };
        if let Some(old) = self.frames.remove(&info.frame_id) {
            changes.push(FrameChange::Removed {
                module_frame: info.frame_id,
                frame: old.chrome,
                document: old.document.generation,
            });
        }
        self.frames.insert(
            info.frame_id,
            Frame {
                chrome,
                main: info.is_main,
                document: Document {
                    seq: info.document_seq,
                    external: BTreeSet::new(),
                    generation,
                    origin: document_origin(url.as_str()),
                    url: url.clone(),
                    facts: info.clone(),
                    security_origin,
                    creator: creator.clone(),
                    commits: 1,
                },
            },
        );
        changes.push(FrameChange::Created {
            module_frame: info.frame_id,
            frame: chrome,
            main: info.is_main,
            parent,
            opaque: info.opaque,
            creator: creator.clone(),
            document: generation,
            url,
        });
        Ok(())
    }

    fn remove(&mut self, module_frame: u64, changes: &mut Vec<FrameChange>) {
        let Some(frame) = self.frames.remove(&module_frame) else {
            return;
        };
        if !frame.main {
            if self.retired.len() >= MAX_RETIRED_FRAME_IDS {
                self.retired.pop_front();
            }
            self.retired.push_back((module_frame, frame.chrome));
        }
        changes.push(FrameChange::Removed {
            module_frame,
            frame: frame.chrome,
            document: frame.document.generation,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(frame_id: u64, document_seq: u64, uri: &str, is_main: bool) -> FrameInfo {
        FrameInfo {
            frame_id,
            document_seq,
            uri: uri.into(),
            is_main,
            ..FrameInfo::default()
        }
    }

    struct Minter(u64);

    impl Minter {
        fn mint(&mut self) -> Option<DocumentGeneration> {
            self.0 += 1;
            DocumentGeneration::new(self.0)
        }
    }

    fn apply(
        table: &mut FrameTable,
        minter: &mut Minter,
        event: FrameEvent,
    ) -> (Result<(), FrameRefusal>, Vec<FrameChange>) {
        let mut changes = Vec::new();
        let result = table.apply(&event, &mut || minter.mint(), &mut changes);
        (result, changes)
    }

    #[test]
    fn main_frames_are_chrome_zero_and_subframes_keep_their_ids_across_documents() {
        let mut table = FrameTable::default();
        let mut minter = Minter(0);
        assert_eq!(
            apply(
                &mut table,
                &mut minter,
                FrameEvent::Created(info(7, 1, "https://a.test/", true)),
            )
            .0,
            Ok(())
        );
        assert_eq!(
            apply(
                &mut table,
                &mut minter,
                FrameEvent::Created(info(9, 2, "https://a.test/frame", false)),
            )
            .0,
            Ok(())
        );
        let main = table.get(7).expect("main");
        assert_eq!(main.chrome, FrameId::MAIN);
        let sub = table.get(9).expect("sub").chrome;
        assert_ne!(sub, FrameId::MAIN);
        assert_eq!(
            table.get(9).expect("sub").document.origin.as_deref(),
            Some("https://a.test")
        );

        let (result, changes) = apply(
            &mut table,
            &mut minter,
            FrameEvent::Destroyed {
                frame_id: 9,
                document_seq: 2,
            },
        );
        assert_eq!(result, Ok(()));
        assert!(matches!(changes[..], [FrameChange::Removed { frame, .. }] if frame == sub));
        let (_, changes) = apply(
            &mut table,
            &mut minter,
            FrameEvent::Created(info(9, 3, "https://b.test/", false)),
        );
        assert!(
            matches!(&changes[..], [FrameChange::Created { frame, .. }] if *frame == sub),
            "a navigated subframe keeps its frameId: {changes:?}"
        );
    }

    #[test]
    fn a_new_main_document_removes_every_frame_and_stale_records_are_refused() {
        let mut table = FrameTable::default();
        let mut minter = Minter(0);
        for (frame, seq, main) in [(1, 1, true), (2, 2, false), (3, 3, false)] {
            assert_eq!(
                apply(
                    &mut table,
                    &mut minter,
                    FrameEvent::Created(info(frame, seq, "https://a.test/", main)),
                )
                .0,
                Ok(())
            );
        }
        let (_, changes) = apply(
            &mut table,
            &mut minter,
            FrameEvent::Created(info(1, 4, "https://b.test/", true)),
        );
        let removed = changes
            .iter()
            .filter(|change| matches!(change, FrameChange::Removed { .. }))
            .count();
        assert_eq!(removed, 3);
        assert_eq!(table.len(), 1);
        let (result, changes) = apply(
            &mut table,
            &mut minter,
            FrameEvent::Created(info(1, 4, "https://c.test/", true)),
        );
        assert_eq!(result, Err(FrameRefusal::StaleDocument));
        assert!(changes.is_empty());
        let (result, _) = apply(
            &mut table,
            &mut minter,
            FrameEvent::Created(info(1, 5, "https://c.test/", false)),
        );
        assert_eq!(
            result,
            Err(FrameRefusal::StaleDocument),
            "main frame cannot become a subframe"
        );
        let (_, changes) = apply(
            &mut table,
            &mut minter,
            FrameEvent::Destroyed {
                frame_id: 1,
                document_seq: 3,
            },
        );
        assert!(
            changes.is_empty(),
            "destroying an older document does nothing"
        );
        let (result, _) = apply(
            &mut table,
            &mut minter,
            FrameEvent::Created(info(5, 9, "not a url", false)),
        );
        assert_eq!(result, Err(FrameRefusal::InvalidUrl));
    }

    #[test]
    fn committed_marks_load_once_and_creates_unknown_documents() {
        let mut table = FrameTable::default();
        let mut minter = Minter(0);
        let (_, changes) = apply(
            &mut table,
            &mut minter,
            FrameEvent::Committed(info(1, 1, "https://a.test/", true)),
        );
        assert!(matches!(changes[..], [FrameChange::Created { .. }]));
        let (_, changes) = apply(
            &mut table,
            &mut minter,
            FrameEvent::Committed(info(1, 1, "https://a.test/", true)),
        );
        assert!(matches!(changes[..], [FrameChange::Loaded { .. }]));
        let (_, changes) = apply(
            &mut table,
            &mut minter,
            FrameEvent::Committed(info(1, 1, "https://a.test/", true)),
        );
        assert!(changes.is_empty());
    }

    #[test]
    fn load_phases_belong_to_the_current_document_only() {
        let (mut table, mut minter) = (FrameTable::default(), Minter(0));
        for created in [
            info(1, 1, "https://a.test/", true),
            info(2, 2, "https://a.test/frame", false),
        ] {
            let (result, _) = apply(&mut table, &mut minter, FrameEvent::Created(created));
            assert_eq!(result, Ok(()));
        }
        let (result, changes) = apply(
            &mut table,
            &mut minter,
            FrameEvent::ContentLoaded {
                frame_id: 2,
                document_seq: 2,
            },
        );
        assert_eq!(result, Ok(()));
        assert!(matches!(
            changes[..],
            [FrameChange::Progress { frame, completed: false, .. }] if frame != FrameId::MAIN
        ));
        let (_, changes) = apply(
            &mut table,
            &mut minter,
            FrameEvent::Completed {
                frame_id: 1,
                document_seq: 1,
            },
        );
        assert!(matches!(
            changes[..],
            [FrameChange::Progress {
                frame: FrameId::MAIN,
                completed: true,
                ..
            }]
        ));
        for stale in [
            FrameEvent::Completed {
                frame_id: 2,
                document_seq: 1,
            },
            FrameEvent::ContentLoaded {
                frame_id: 9,
                document_seq: 2,
            },
        ] {
            let (result, changes) = apply(&mut table, &mut minter, stale);
            assert_eq!(result, Err(FrameRefusal::StaleDocument));
            assert!(changes.is_empty());
        }
    }

    #[test]
    fn reset_clears_and_the_table_is_bounded() {
        let mut table = FrameTable::default();
        let mut minter = Minter(0);
        assert_eq!(
            apply(
                &mut table,
                &mut minter,
                FrameEvent::Created(info(0, 1, "https://a.test/", true)),
            )
            .0,
            Ok(())
        );
        for frame in 1..MAX_FRAMES_PER_VIEW as u64 {
            let (result, _) = apply(
                &mut table,
                &mut minter,
                FrameEvent::Created(info(frame, frame + 1, "https://a.test/", false)),
            );
            assert_eq!(result, Ok(()));
        }
        let (result, _) = apply(
            &mut table,
            &mut minter,
            FrameEvent::Created(info(10_000, 10_000, "https://a.test/", false)),
        );
        assert_eq!(result, Err(FrameRefusal::Capacity));
        let (_, changes) = apply(&mut table, &mut minter, FrameEvent::Reset);
        assert_eq!(changes.len(), MAX_FRAMES_PER_VIEW);
        assert_eq!(table.len(), 0);
        let mut exhausted = || None;
        let mut changes = Vec::new();
        assert_eq!(
            table.apply(
                &FrameEvent::Created(info(1, 1, "https://a.test/", true)),
                &mut exhausted,
                &mut changes
            ),
            Err(FrameRefusal::Exhausted)
        );
    }
    #[test]
    fn parent_creator_and_duplicate_commit_metadata_are_consistency_claims() {
        let mut table = FrameTable::default();
        let mut minter = Minter(0);
        apply(
            &mut table,
            &mut minter,
            FrameEvent::Created(info(1, 1, "https://a.test/page", true)),
        )
        .0
        .expect("parent");
        let mut child = info(2, 2, "about:blank", false);
        child.parent_frame_id = Some(1);
        child.creator_uri = Some("https://a.test/page".into());
        assert_eq!(
            apply(&mut table, &mut minter, FrameEvent::Created(child.clone())).0,
            Ok(())
        );
        let mut forged = child.clone();
        forged.creator_uri = Some("https://victim.test/".into());
        assert_eq!(
            apply(&mut table, &mut minter, FrameEvent::Committed(forged)).0,
            Err(FrameRefusal::InconsistentFacts)
        );
        let mut forged = child.clone();
        forged.opaque = true;
        assert_eq!(
            apply(&mut table, &mut minter, FrameEvent::Committed(forged)).0,
            Err(FrameRefusal::InconsistentFacts)
        );
        let mut forged = child.clone();
        forged.document_seq = 3;
        forged.parent_frame_id = Some(999);
        assert_eq!(
            apply(&mut table, &mut minter, FrameEvent::Created(forged)).0,
            Err(FrameRefusal::InconsistentFacts)
        );
        let mut forged = info(3, 3, "data:text/html,hello", false);
        forged.parent_frame_id = Some(1);
        assert_eq!(
            apply(&mut table, &mut minter, FrameEvent::Created(forged)).0,
            Err(FrameRefusal::InconsistentFacts)
        );
        assert_eq!(
            apply(&mut table, &mut minter, FrameEvent::Committed(child)).0,
            Ok(())
        );
    }
    #[test]
    fn an_opaque_parent_cannot_authenticate_a_tuple_creator_for_its_child() {
        let mut table = FrameTable::default();
        let mut minter = Minter(0);
        let mut parent = info(1, 1, "https://a.test/page", true);
        parent.opaque = true;
        assert_eq!(
            apply(&mut table, &mut minter, FrameEvent::Created(parent)).0,
            Ok(())
        );
        let mut child = info(2, 2, "about:blank", false);
        child.parent_frame_id = Some(1);
        child.creator_uri = Some("https://a.test/page".into());
        for opaque in [false, true] {
            child.opaque = opaque;
            assert_eq!(
                apply(&mut table, &mut minter, FrameEvent::Created(child.clone())).0,
                Err(FrameRefusal::InconsistentFacts)
            );
        }
    }
}
