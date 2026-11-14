//! Safe attachment authority, document lifecycle and bridge admission.

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use crate::protocol::{
    document_origin, Attach, Deliver, Detach, FrameEvent, FrameInfo, PageRole, MAX_ATTACH_BYTES,
    MAX_BATCH_EVENTS, MAX_BOOTSTRAP_BYTES, MAX_ORIGINS, MAX_TOKEN_BYTES, MAX_URI_BYTES, MAX_WORLDS,
    MAX_WORLD_BYTES, PROTOCOL_VERSION,
};

pub const MAX_PAGES: usize = 32;
pub const MAX_TRACKED_PAGES: usize = 256;
pub const MAX_FRAMES_PER_PAGE: usize = 128;
pub const MAX_PROCESS_FRAMES: usize = 1024;
pub const MAX_PENDING_FRAME_EVENTS: usize = 512;
pub const MAX_INFLIGHT_POSTS: usize = 4096;
pub const MAX_PROCESS_INFLIGHT_BYTES: usize = 128 * 1024 * 1024;
pub const MAX_FRAME_EVALUATIONS: usize = 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StateError {
    Inert,
    InvalidAttach,
    Stale,
    UnknownFrame,
    WrongWorld,
    Capacity,
    Oversized,
    RateLimited,
    InflightLimited,
    Exhausted,
}

impl std::fmt::Display for StateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "web-process admission refused: {self:?}")
    }
}

impl std::error::Error for StateError {}

#[derive(Clone)]
struct Bucket {
    milli_tokens: u64,
    last_ms: u64,
    started: bool,
}

impl Bucket {
    fn new(burst: u16) -> Self {
        Self {
            milli_tokens: u64::from(burst) * 1000,
            last_ms: 0,
            started: false,
        }
    }

    fn refill(&mut self, now_ms: u64, per_second: u32, burst: u16) {
        if !self.started {
            self.last_ms = now_ms;
            self.started = true;
        }
        let elapsed = now_ms.saturating_sub(self.last_ms);
        self.milli_tokens = self
            .milli_tokens
            .saturating_add(elapsed.saturating_mul(u64::from(per_second)))
            .min(u64::from(burst) * 1000);
        self.last_ms = self.last_ms.max(now_ms);
    }
}

struct Frame {
    info: FrameInfo,
    bucket: Bucket,
    inflight_bytes: usize,
    outstanding: usize,
    injected_bytes: usize,
    evaluations: usize,
    script_budget_exhausted: bool,
}

struct Page {
    epoch: u64,
    attach: Attach,
    frames: BTreeMap<u64, Frame>,
    events: VecDeque<FrameEvent>,
    bucket: Bucket,
    inflight_bytes: usize,
    outstanding: usize,
}

impl Page {
    fn queue(&mut self, event: FrameEvent) {
        if self.events.len() >= MAX_PENDING_FRAME_EVENTS {
            self.events.clear();
            self.events.push_back(FrameEvent::Reset);
            self.events.extend(
                self.frames
                    .values()
                    .map(|frame| FrameEvent::Created(frame.info.clone())),
            );
        } else {
            self.events.push_back(event);
        }
    }
}

struct Reservation {
    page_id: u64,
    generation: u64,
    epoch: u64,
    frame_id: u64,
    document_seq: u64,
    bytes: usize,
}

pub struct State {
    token: Option<String>,
    pages: BTreeMap<u64, Page>,
    page_generations: BTreeMap<u64, u64>,
    reservations: BTreeMap<u64, Reservation>,
    next_document: u64,
    next_post: u64,
    next_attachment: u64,
    inflight_bytes: usize,
}

impl State {
    pub fn new(version: u32, token: String) -> Self {
        Self {
            token: (version == PROTOCOL_VERSION && valid_token(&token)).then_some(token),
            pages: BTreeMap::new(),
            page_generations: BTreeMap::new(),
            reservations: BTreeMap::new(),
            next_document: 0,
            next_post: 0,
            next_attachment: 0,
            inflight_bytes: 0,
        }
    }

    pub fn is_initialized(&self) -> bool {
        self.token.is_some()
    }

    pub fn attach(&mut self, page_id: u64, attach: Attach) -> Result<(), StateError> {
        let previous = self.page_generations.get(&page_id).copied();
        let widened = self.pages.get(&page_id).is_some_and(|page| {
            attach.generation == page.attach.generation
                && attachment_widens(&page.attach, &attach)
                && attach != page.attach
        });
        let valid = self.token.as_deref() == Some(attach.token.as_str())
            && self.token.is_some()
            && validate_attach(&attach)
            && (widened || previous.is_none_or(|generation| attach.generation > generation));
        if valid && widened {
            self.page_generations.insert(page_id, attach.generation);
            let page = self.pages.get_mut(&page_id).ok_or(StateError::Inert)?;
            page.attach = attach;
            return Ok(());
        }
        self.remove_page(page_id);
        if !valid {
            return Err(StateError::InvalidAttach);
        }
        if self.pages.len() >= MAX_PAGES
            || (previous.is_none() && self.page_generations.len() >= MAX_TRACKED_PAGES)
        {
            return Err(StateError::Capacity);
        }
        self.next_attachment = self
            .next_attachment
            .checked_add(1)
            .ok_or(StateError::Exhausted)?;
        self.page_generations.insert(page_id, attach.generation);
        self.pages.insert(
            page_id,
            Page {
                epoch: self.next_attachment,
                bucket: Bucket::new(attach.limits.page_burst),
                attach,
                frames: BTreeMap::new(),
                events: VecDeque::new(),
                inflight_bytes: 0,
                outstanding: 0,
            },
        );
        Ok(())
    }

    pub fn attachment(&self, page_id: u64) -> Option<&Attach> {
        self.pages.get(&page_id).map(|page| &page.attach)
    }

    pub fn validate_message(&self, page_id: u64, token: &str, generation: u64) -> bool {
        self.pages
            .get(&page_id)
            .is_some_and(|page| page.attach.token == token && page.attach.generation == generation)
    }

    pub fn detach(&mut self, page_id: u64, detach: &Detach) -> bool {
        if !self.validate_message(page_id, &detach.token, detach.generation) {
            return false;
        }
        self.remove_page(page_id);
        true
    }

    pub fn remove_page(&mut self, page_id: u64) {
        self.pages.remove(&page_id);
    }

    /// Release identity only when the native WebPage object is destroyed.
    pub fn forget_page(&mut self, page_id: u64) {
        self.remove_page(page_id);
        self.page_generations.remove(&page_id);
    }

    pub fn frame(&self, page_id: u64, frame_id: u64) -> Option<&FrameInfo> {
        self.pages
            .get(&page_id)?
            .frames
            .get(&frame_id)
            .map(|frame| &frame.info)
    }

    pub fn clear_document(
        &mut self,
        page_id: u64,
        frame_id: u64,
        uri: &str,
        is_main: bool,
    ) -> Result<u64, StateError> {
        if uri.len() > MAX_URI_BYTES || uri.contains('\0') {
            return Err(StateError::Oversized);
        }
        let page = self.pages.get(&page_id).ok_or(StateError::Inert)?;
        if !page.frames.contains_key(&frame_id)
            && (page.frames.len() >= MAX_FRAMES_PER_PAGE
                || self
                    .pages
                    .values()
                    .map(|page| page.frames.len())
                    .sum::<usize>()
                    >= MAX_PROCESS_FRAMES)
        {
            return Err(StateError::Capacity);
        }
        let sequence = self
            .next_document
            .checked_add(1)
            .ok_or(StateError::Exhausted)?;
        self.next_document = sequence;
        if is_main {
            let old_ids: Vec<u64> = self
                .pages
                .get(&page_id)
                .ok_or(StateError::Inert)?
                .frames
                .keys()
                .copied()
                .collect();
            for id in old_ids {
                self.remove_frame(page_id, id);
            }
        } else {
            self.remove_frame(page_id, frame_id);
        }
        let page = self.pages.get_mut(&page_id).ok_or(StateError::Inert)?;
        let info = FrameInfo {
            frame_id,
            document_seq: sequence,
            uri: uri.to_owned(),
            is_main,
            parent_frame_id: None,
            creator_uri: None,
            opaque: uri.starts_with("data:") || uri.starts_with("blob:"),
            table_generation: 0,
        };
        page.frames.insert(
            frame_id,
            Frame {
                info: info.clone(),
                bucket: Bucket::new(page.attach.limits.frame_burst),
                inflight_bytes: 0,
                outstanding: 0,
                injected_bytes: 0,
                evaluations: 0,
                script_budget_exhausted: false,
            },
        );
        page.queue(FrameEvent::Created(info));
        Ok(sequence)
    }

    pub fn commit_frame(&mut self, page_id: u64, frame_id: u64) -> bool {
        let Some(page) = self.pages.get_mut(&page_id) else {
            return false;
        };
        let Some(frame) = page.frames.get(&frame_id) else {
            return false;
        };
        let info = frame.info.clone();
        page.queue(FrameEvent::Committed(info));
        true
    }

    pub fn set_frame_facts(
        &mut self,
        page_id: u64,
        frame_id: u64,
        parent_frame_id: Option<u64>,
        creator_uri: Option<String>,
        opaque: bool,
    ) -> bool {
        let Some(page) = self.pages.get_mut(&page_id) else {
            return false;
        };
        let Some(frame) = page.frames.get_mut(&frame_id) else {
            return false;
        };
        frame.info.parent_frame_id = parent_frame_id;
        frame.info.creator_uri = creator_uri;
        frame.info.opaque = opaque;
        let info = frame.info.clone();
        // Facts are filled before the newly created document is exposed.
        for event in page.events.iter_mut().rev() {
            if let FrameEvent::Created(created) = event {
                if created.frame_id == frame_id && created.document_seq == info.document_seq {
                    *created = info;
                    return true;
                }
            }
        }
        false
    }

    pub fn set_frame_script_table(&mut self, page_id: u64, frame_id: u64, generation: u64) {
        if let Some(page) = self.pages.get_mut(&page_id) {
            if let Some(frame) = page.frames.get_mut(&frame_id) {
                frame.info.table_generation = generation;
                for event in page.events.iter_mut().rev() {
                    if let FrameEvent::Created(info) = event {
                        if info.frame_id == frame_id && info.document_seq == frame.info.document_seq
                        {
                            info.table_generation = generation;
                            break;
                        }
                    }
                }
            }
        }
    }

    pub fn charge_script_bytes(
        &mut self,
        page_id: u64,
        frame_id: u64,
        document_seq: u64,
        bytes: usize,
    ) -> Result<(), StateError> {
        let page = self.pages.get_mut(&page_id).ok_or(StateError::Inert)?;
        let frame = page
            .frames
            .get_mut(&frame_id)
            .ok_or(StateError::UnknownFrame)?;
        if frame.info.document_seq != document_seq {
            return Err(StateError::Stale);
        }
        let total = frame.injected_bytes.checked_add(bytes);
        if frame.script_budget_exhausted
            || frame.evaluations >= MAX_FRAME_EVALUATIONS
            || total.is_none_or(|total| total > crate::protocol::MAX_FRAME_SCRIPT_BYTES)
        {
            frame.script_budget_exhausted = true;
            return Err(StateError::Oversized);
        }
        frame.injected_bytes = total.ok_or(StateError::Oversized)?;
        frame.evaluations += 1;
        Ok(())
    }

    pub fn script_budget_exhausted(&self, page_id: u64, frame_id: u64, sequence: u64) -> bool {
        self.pages
            .get(&page_id)
            .and_then(|page| page.frames.get(&frame_id))
            .is_none_or(|frame| {
                frame.info.document_seq != sequence || frame.script_budget_exhausted
            })
    }

    pub fn queue_frame_event(&mut self, page_id: u64, event: FrameEvent) {
        if let Some(page) = self.pages.get_mut(&page_id) {
            page.queue(event);
        }
    }

    pub fn remove_frame(&mut self, page_id: u64, frame_id: u64) -> bool {
        let Some(page) = self.pages.get_mut(&page_id) else {
            return false;
        };
        let Some(frame) = page.frames.remove(&frame_id) else {
            return false;
        };
        page.queue(FrameEvent::Destroyed {
            frame_id,
            document_seq: frame.info.document_seq,
        });
        true
    }

    pub fn worlds_for_frame(&self, page_id: u64, frame_id: u64) -> Vec<String> {
        let Some(page) = self.pages.get(&page_id) else {
            return Vec::new();
        };
        let Some(frame) = page.frames.get(&frame_id) else {
            return Vec::new();
        };
        let mut worlds: Vec<String> = page
            .attach
            .worlds
            .iter()
            .filter(|world| world.matches.iter().any(|rule| rule.matches(&frame.info)))
            .map(|world| world.name.clone())
            .collect();
        if !frame.info.opaque
            && document_origin(&frame.info.uri)
                .is_some_and(|origin| page.attach.extension_origins.contains(&origin))
        {
            worlds.push(String::new());
        }
        if !frame.info.opaque
            && crate::protocol::external_matches(&page.attach.external, &frame.info)
            && !worlds.iter().any(String::is_empty)
        {
            worlds.push(String::new());
        }
        worlds
    }

    pub fn valid_target(&self, page_id: u64, deliver: &Deliver) -> bool {
        self.validate_message(page_id, &deliver.token, deliver.generation)
            && deliver.external_install.as_ref().is_none_or(|install| {
                self.pages.get(&page_id).is_some_and(|page| {
                    page.frames.get(&deliver.frame_id).is_some_and(|frame| {
                        deliver.world.is_empty()
                            && page
                                .attach
                                .external
                                .iter()
                                .any(|rule| rule.install_id == *install && rule.admits(&frame.info))
                    })
                })
            })
            && self
                .frame(page_id, deliver.frame_id)
                .is_some_and(|frame| frame.document_seq == deliver.document_seq)
            && self
                .message_cap(page_id, deliver.frame_id, &deliver.world)
                .is_some_and(|cap| deliver.bytes.len() <= cap)
    }

    pub fn message_cap(&self, page_id: u64, frame_id: u64, world: &str) -> Option<usize> {
        let page = self.pages.get(&page_id)?;
        let frame = page.frames.get(&frame_id)?;
        if !self
            .worlds_for_frame(page_id, frame_id)
            .iter()
            .any(|name| name == world)
        {
            return None;
        }
        let context_cap = if page.attach.role == PageRole::ExtensionOwned
            && frame.info.is_main
            && world.is_empty()
        {
            32 * 1024 * 1024
        } else {
            8 * 1024 * 1024
        };
        Some((page.attach.limits.max_message_bytes as usize).min(context_cap))
    }

    pub fn reserve_delivery(
        &mut self,
        page_id: u64,
        deliver: &Deliver,
        now_ms: u64,
    ) -> Result<u64, StateError> {
        if !self.validate_message(page_id, &deliver.token, deliver.generation)
            || self
                .frame(page_id, deliver.frame_id)
                .is_none_or(|frame| frame.document_seq != deliver.document_seq)
        {
            return Err(StateError::Stale);
        }
        if !self.valid_target(page_id, deliver) {
            return Err(StateError::WrongWorld);
        }
        self.reserve_post(
            page_id,
            deliver.frame_id,
            deliver.document_seq,
            &deliver.world,
            deliver.bytes.len(),
            now_ms,
        )
    }

    pub fn reserve_post(
        &mut self,
        page_id: u64,
        frame_id: u64,
        document_seq: u64,
        world: &str,
        bytes: usize,
        now_ms: u64,
    ) -> Result<u64, StateError> {
        let message_cap = self
            .message_cap(page_id, frame_id, world)
            .ok_or(StateError::WrongWorld)?;
        if self.reservations.len() >= MAX_INFLIGHT_POSTS
            || self.inflight_bytes.saturating_add(bytes) > MAX_PROCESS_INFLIGHT_BYTES
        {
            return Err(StateError::InflightLimited);
        }
        let page = self.pages.get_mut(&page_id).ok_or(StateError::Inert)?;
        let frame = page
            .frames
            .get_mut(&frame_id)
            .ok_or(StateError::UnknownFrame)?;
        if frame.info.document_seq != document_seq {
            return Err(StateError::Stale);
        }
        let limits = &page.attach.limits;
        if bytes > message_cap {
            return Err(StateError::Oversized);
        }
        if page.outstanding >= usize::from(limits.page_outstanding)
            || frame.outstanding >= usize::from(limits.frame_outstanding)
            || page.inflight_bytes.saturating_add(bytes) > limits.page_inflight_bytes as usize
            || frame.inflight_bytes.saturating_add(bytes) > limits.frame_inflight_bytes as usize
        {
            return Err(StateError::InflightLimited);
        }
        frame
            .bucket
            .refill(now_ms, limits.frame_messages_per_second, limits.frame_burst);
        page.bucket
            .refill(now_ms, limits.page_messages_per_second, limits.page_burst);
        if frame.bucket.milli_tokens < 1000 || page.bucket.milli_tokens < 1000 {
            return Err(StateError::RateLimited);
        }
        let post_id = self.next_post.checked_add(1).ok_or(StateError::Exhausted)?;
        self.next_post = post_id;
        frame.bucket.milli_tokens -= 1000;
        page.bucket.milli_tokens -= 1000;
        frame.inflight_bytes += bytes;
        page.inflight_bytes += bytes;
        frame.outstanding += 1;
        page.outstanding += 1;
        self.inflight_bytes += bytes;
        self.reservations.insert(
            post_id,
            Reservation {
                page_id,
                generation: page.attach.generation,
                epoch: page.epoch,
                frame_id,
                document_seq,
                bytes,
            },
        );
        Ok(post_id)
    }

    pub fn settle_post(&mut self, post_id: u64) {
        let Some(reservation) = self.reservations.remove(&post_id) else {
            return;
        };
        self.inflight_bytes = self.inflight_bytes.saturating_sub(reservation.bytes);
        let Some(page) = self.pages.get_mut(&reservation.page_id) else {
            return;
        };
        if page.attach.generation != reservation.generation || page.epoch != reservation.epoch {
            return;
        }
        page.inflight_bytes = page.inflight_bytes.saturating_sub(reservation.bytes);
        page.outstanding = page.outstanding.saturating_sub(1);
        if let Some(frame) = page.frames.get_mut(&reservation.frame_id) {
            if frame.info.document_seq == reservation.document_seq {
                frame.inflight_bytes = frame.inflight_bytes.saturating_sub(reservation.bytes);
                frame.outstanding = frame.outstanding.saturating_sub(1);
            }
        }
    }

    pub fn drain_frame_events(&mut self, page_id: u64) -> Vec<FrameEvent> {
        let Some(page) = self.pages.get_mut(&page_id) else {
            return Vec::new();
        };
        let count = page.events.len().min(MAX_BATCH_EVENTS);
        page.events.drain(..count).collect()
    }
}

fn valid_token(token: &str) -> bool {
    !token.is_empty()
        && token.len() <= MAX_TOKEN_BYTES
        && token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn valid_origin(origin: &str) -> bool {
    document_origin(origin).is_some_and(|canonical| canonical == origin)
}

pub fn attachment_widens(old: &Attach, new: &Attach) -> bool {
    old.token == new.token
        && old.protocol_version == new.protocol_version
        && old.role == new.role
        && old.limits == new.limits
        && new.generation >= old.generation
        && old.extension_bootstrap == new.extension_bootstrap
        && (!old.observe_forms || new.observe_forms)
        && (!old.observe_requests || new.observe_requests)
        && old
            .extension_origins
            .iter()
            .all(|origin| new.extension_origins.contains(origin))
        && old.external.iter().all(|rule| new.external.contains(rule))
        && old.worlds.iter().all(|world| {
            new.worlds.iter().any(|next| {
                world.name == next.name
                    && world.install_id == next.install_id
                    && world.bootstrap == next.bootstrap
                    && world.matches.iter().all(|rule| next.matches.contains(rule))
            })
        })
}

fn validate_attach(attach: &Attach) -> bool {
    if attach.protocol_version != PROTOCOL_VERSION
        || attach.generation == 0
        || !valid_token(&attach.token)
        || !attach.limits.validate(attach.role)
        || attach.worlds.len() > MAX_WORLDS
        || attach.extension_origins.len() > MAX_ORIGINS
        || attach.extension_bootstrap.len() > MAX_BOOTSTRAP_BYTES
        || attach.extension_bootstrap.contains('\0')
    {
        return false;
    }
    let mut names = BTreeSet::new();
    for world in &attach.worlds {
        if !crate::protocol::valid_install_id(&world.install_id)
            || world.name.is_empty()
            || world.name.len() > MAX_WORLD_BYTES
            || !(world.name.starts_with("zephium-principal-extension-")
                || world.name.starts_with("zephium-webext-"))
            || !world
                .name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
            || !names.insert(world.name.as_str())
            || world.matches.is_empty()
            || world.bootstrap.len() > MAX_BOOTSTRAP_BYTES
            || world.bootstrap.contains('\0')
        {
            return false;
        }
    }
    let mut external = BTreeSet::new();
    if attach.external.iter().any(|rule| {
        !crate::protocol::valid_install_id(&rule.install_id)
            || !external.insert(&rule.install_id)
            || rule.matches.is_empty()
            || rule.bootstrap.is_empty()
            || rule.bootstrap.contains('\0')
    }) {
        return false;
    }
    attach
        .extension_origins
        .iter()
        .all(|origin| origin.starts_with("chrome-extension://") && valid_origin(origin))
        && crate::protocol::encode_ui(&crate::protocol::UiMessage::Attach(attach.clone()))
            .is_ok_and(|bytes| bytes.len() <= MAX_ATTACH_BYTES)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{Limits, WorldRule};

    fn attachment() -> Attach {
        Attach {
            token: "process-token".into(),
            generation: 1,
            worlds: vec![WorldRule {
                name: "zephium-webext-isolated".into(),
                install_id: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
                matches: vec![crate::protocol::MatchRule {
                    matcher: crate::protocol::ContentScriptMatcher::new(
                        &["https://example.com:443/*"],
                        &[],
                        &[],
                        &[],
                        crate::protocol::MatchOptions::default(),
                    )
                    .expect("matcher"),
                    all_frames: true,
                    file_access: false,
                }],
                bootstrap: String::new(),
            }],
            ..Attach::default()
        }
    }

    fn attached() -> (State, u64) {
        let mut state = State::new(PROTOCOL_VERSION, "process-token".into());
        state.attach(7, attachment()).expect("attach");
        let seq = state
            .clear_document(7, 42, "https://example.com/path", true)
            .expect("document");
        (state, seq)
    }

    #[test]
    fn script_byte_budget_is_per_document_and_rejects_stale_targets() {
        let (mut state, sequence) = attached();
        assert_eq!(
            state.charge_script_bytes(7, 42, sequence + 1, 1),
            Err(StateError::Stale)
        );
        assert!(state
            .charge_script_bytes(7, 42, sequence, crate::protocol::MAX_FRAME_SCRIPT_BYTES)
            .is_ok());
        assert_eq!(
            state.charge_script_bytes(7, 42, sequence, 1),
            Err(StateError::Oversized)
        );
        assert_eq!(
            state.charge_script_bytes(7, 42, sequence, usize::MAX),
            Err(StateError::Oversized)
        );
        let child = state
            .clear_document(7, 43, "https://example.com", false)
            .expect("child");
        assert!(state.charge_script_bytes(7, 43, child, 1).is_ok());
        let replacement = state
            .clear_document(7, 42, "https://example.com/new", true)
            .expect("replacement");
        assert!(state.charge_script_bytes(7, 42, replacement, 1).is_ok());
    }

    #[test]
    fn empty_scripts_cannot_bypass_the_evaluation_budget() {
        let (mut state, sequence) = attached();
        for _ in 0..MAX_FRAME_EVALUATIONS {
            assert!(state.charge_script_bytes(7, 42, sequence, 0).is_ok());
        }
        assert_eq!(
            state.charge_script_bytes(7, 42, sequence, 0),
            Err(StateError::Oversized)
        );
        assert!(state.script_budget_exhausted(7, 42, sequence));
    }

    #[test]
    fn widening_preserves_documents_reservations_and_does_not_publish_duplicate_facts() {
        let (mut state, sequence) = attached();
        state.drain_frame_events(7);
        let pending = state
            .reserve_post(7, 42, sequence, "zephium-webext-isolated", 1, 0)
            .expect("pending");
        let mut wider = attachment();
        wider.worlds[0]
            .matches
            .push(crate::protocol::MatchRule::for_origin("https://another.test").expect("matcher"));
        state.attach(7, wider.clone()).expect("widen");
        assert_eq!(
            state.frame(7, 42).expect("preserved").document_seq,
            sequence
        );
        assert!(state.drain_frame_events(7).is_empty());
        assert_eq!(state.pages.get(&7).expect("page").outstanding, 1);
        state.settle_post(pending);
        assert_eq!(state.pages.get(&7).expect("page").outstanding, 0);
        let mut narrower = attachment();
        narrower.generation = 2;
        state.attach(7, narrower).expect("narrowed generation");
        assert!(state.frame(7, 42).is_none());
        assert_eq!(state.attach(7, wider), Err(StateError::InvalidAttach));
        assert!(state.attachment(7).is_none());
    }

    #[test]
    fn frame_facts_refuse_sandboxed_worlds_and_admit_only_explicit_blank_inheritance() {
        let mut attach = attachment();
        attach.worlds[0].matches[0].matcher = crate::protocol::ContentScriptMatcher::new(
            &["https://example.com:443/*"],
            &[],
            &[],
            &[],
            crate::protocol::MatchOptions {
                match_about_blank: true,
                match_origin_as_fallback: false,
            },
        )
        .expect("matcher");
        let mut state = State::new(PROTOCOL_VERSION, "process-token".into());
        state.attach(7, attach).expect("attach");
        state
            .clear_document(7, 1, "https://example.com", true)
            .expect("top");
        state.set_frame_facts(7, 1, None, None, true);
        assert!(state.worlds_for_frame(7, 1).is_empty());
        state
            .clear_document(7, 2, "about:blank", false)
            .expect("blank");
        assert!(state.worlds_for_frame(7, 2).is_empty());
        state.set_frame_facts(
            7,
            2,
            Some(1),
            Some("https://example.com/path".into()),
            false,
        );
        assert_eq!(
            state.worlds_for_frame(7, 2),
            vec!["zephium-webext-isolated"]
        );
        state.set_frame_facts(7, 2, Some(1), Some("https://example.com/path".into()), true);
        assert!(state.worlds_for_frame(7, 2).is_empty());
        let created = state.drain_frame_events(7);
        assert!(created.iter().any(|event| matches!(event, FrameEvent::Created(info) if info.frame_id == 2 && info.parent_frame_id == Some(1) && info.opaque)));
    }

    #[test]
    fn inert_until_valid_attach_and_bad_attach_retires_authority() {
        let mut state = State::new(PROTOCOL_VERSION, "process-token".into());
        assert_eq!(
            state.clear_document(7, 42, "https://example.com", true),
            Err(StateError::Inert)
        );
        state.attach(7, attachment()).expect("attach");
        let mut invalid = attachment();
        invalid.token = "wrong".into();
        invalid.generation = 2;
        assert_eq!(state.attach(7, invalid), Err(StateError::InvalidAttach));
        assert!(state.attachment(7).is_none());
        let mut wrong_version = State::new(PROTOCOL_VERSION + 1, "process-token".into());
        assert!(!wrong_version.is_initialized());
        assert!(wrong_version.attach(7, attachment()).is_err());
    }

    #[test]
    fn detached_and_malformed_attachments_cannot_replay_accepted_generations() {
        let (mut state, _) = attached();
        assert!(state.detach(
            7,
            &Detach {
                token: "process-token".into(),
                generation: 1,
            }
        ));
        assert_eq!(
            state.attach(7, attachment()),
            Err(StateError::InvalidAttach)
        );
        let mut attach = attachment();
        attach.generation = 2;
        state.attach(7, attach).expect("next attachment");
        let mut malformed = attachment();
        malformed.generation = 3;
        malformed.protocol_version = PROTOCOL_VERSION + 1;
        assert_eq!(state.attach(7, malformed), Err(StateError::InvalidAttach));
        assert!(state.attachment(7).is_none());
        for generation in [1, 2] {
            let mut stale = attachment();
            stale.generation = generation;
            assert_eq!(state.attach(7, stale), Err(StateError::InvalidAttach));
        }
        let mut attach = attachment();
        attach.generation = 3;
        state
            .attach(7, attach.clone())
            .expect("new valid generation");
        state.remove_page(7);
        assert_eq!(state.attach(7, attach), Err(StateError::InvalidAttach));
    }

    #[test]
    fn generation_highwater_is_bounded_and_released_only_for_destroyed_pages() {
        let mut state = State::new(PROTOCOL_VERSION, "process-token".into());
        for page_id in 0..MAX_TRACKED_PAGES as u64 {
            state.attach(page_id, attachment()).expect("new page");
            state.remove_page(page_id);
        }
        assert_eq!(state.page_generations.len(), MAX_TRACKED_PAGES);
        assert_eq!(
            state.attach(MAX_TRACKED_PAGES as u64, attachment()),
            Err(StateError::Capacity)
        );
        let mut attach = attachment();
        attach.generation = 2;
        state
            .attach(0, attach)
            .expect("known page remains admissible");
        assert_eq!(state.page_generations.len(), MAX_TRACKED_PAGES);
        state.forget_page(0);
        assert!(state.attachment(0).is_none());
        state
            .attach(MAX_TRACKED_PAGES as u64, attachment())
            .expect("destroyed page released tracking capacity");
        assert_eq!(state.page_generations.len(), MAX_TRACKED_PAGES);
    }

    #[test]
    fn destroyed_page_identity_can_restart_without_releasing_pending_bytes_early() {
        let (mut state, seq) = attached();
        let retired = state
            .reserve_post(7, 42, seq, "zephium-webext-isolated", 1, 0)
            .expect("retired post");
        state.forget_page(7);
        state
            .attach(7, attachment())
            .expect("new native page identity");
        let seq = state
            .clear_document(7, 42, "https://example.com", true)
            .expect("new document");
        let current = state
            .reserve_post(7, 42, seq, "zephium-webext-isolated", 1, 0)
            .expect("current post");
        assert_eq!(state.inflight_bytes, 2);
        state.settle_post(retired);
        assert_eq!(state.inflight_bytes, 1);
        assert_eq!(state.pages.get(&7).expect("page").outstanding, 1);
        state.settle_post(current);
        assert_eq!(state.inflight_bytes, 0);
    }

    #[test]
    fn exact_origins_and_worlds_do_not_inherit_authority() {
        let (mut state, seq) = attached();
        assert!(state
            .reserve_post(7, 42, seq, "zephium-webext-isolated", 1, 0)
            .is_ok());
        assert_eq!(
            state.reserve_post(7, 42, seq, "isolate", 1, 0),
            Err(StateError::WrongWorld)
        );
        for uri in [
            "https://example.com.evil/path",
            "http://example.com/path",
            "https://example.com:444/",
            "about:blank",
            "about:srcdoc",
            "data:text/html,hello",
        ] {
            let seq = state.clear_document(7, 43, uri, false).expect("document");
            assert!(state.worlds_for_frame(7, 43).is_empty());
            assert_eq!(
                state.reserve_post(7, 43, seq, "zephium-webext-isolated", 1, 0),
                Err(StateError::WrongWorld)
            );
        }
        assert_eq!(
            document_origin("HTTPS://EXAMPLE.COM:443/path"),
            Some("https://example.com".into())
        );
    }

    #[test]
    fn default_world_requires_exact_chrome_extension_origin() {
        let mut attach = attachment();
        attach.role = PageRole::ExtensionOwned;
        attach
            .extension_origins
            .push("chrome-extension://aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into());
        let mut state = State::new(PROTOCOL_VERSION, "process-token".into());
        state.attach(1, attach).expect("attach");
        state
            .clear_document(
                1,
                1,
                "chrome-extension://aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/page.html",
                true,
            )
            .expect("doc");
        assert_eq!(state.worlds_for_frame(1, 1), vec![String::new()]);
        state
            .clear_document(
                1,
                1,
                "chrome-extension://bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb/page.html",
                true,
            )
            .expect("doc");
        assert!(state.worlds_for_frame(1, 1).is_empty());
    }

    #[test]
    fn navigation_and_removal_retire_targets_and_pending_posts() {
        let (mut state, old_seq) = attached();
        let id = state
            .reserve_post(7, 42, old_seq, "zephium-webext-isolated", 3, 0)
            .expect("post");
        let seq = state
            .clear_document(7, 42, "https://example.com/new", true)
            .expect("doc");
        assert_ne!(old_seq, seq);
        state.settle_post(id);
        assert_eq!(
            state.reserve_post(7, 42, old_seq, "zephium-webext-isolated", 1, 0),
            Err(StateError::Stale)
        );
        assert!(state.remove_frame(7, 42));
        assert!(state.frame(7, 42).is_none());
        assert!(!state.detach(
            7,
            &Detach {
                token: "wrong".into(),
                generation: 1
            }
        ));
        assert!(state.detach(
            7,
            &Detach {
                token: "process-token".into(),
                generation: 1
            }
        ));
    }

    #[test]
    fn frame_page_and_zero_length_outstanding_budgets_settle_once() {
        let mut attach = attachment();
        attach.limits = Limits {
            max_message_bytes: 4,
            frame_inflight_bytes: 4,
            page_inflight_bytes: 4,
            frame_outstanding: 2,
            page_outstanding: 2,
            ..Limits::default()
        };
        let mut state = State::new(PROTOCOL_VERSION, "process-token".into());
        state.attach(7, attach).expect("attach");
        let seq = state
            .clear_document(7, 42, "https://example.com", true)
            .expect("doc");
        assert_eq!(
            state.reserve_post(7, 42, seq, "zephium-webext-isolated", 5, 0),
            Err(StateError::Oversized)
        );
        let first = state
            .reserve_post(7, 42, seq, "zephium-webext-isolated", 4, 0)
            .expect("post");
        assert_eq!(
            state.reserve_post(7, 42, seq, "zephium-webext-isolated", 1, 0),
            Err(StateError::InflightLimited)
        );
        let second = state
            .reserve_post(7, 42, seq, "zephium-webext-isolated", 0, 0)
            .expect("empty post");
        assert_eq!(
            state.reserve_post(7, 42, seq, "zephium-webext-isolated", 0, 0),
            Err(StateError::InflightLimited)
        );
        state.settle_post(first);
        state.settle_post(first);
        state.settle_post(second);
        assert!(state
            .reserve_post(7, 42, seq, "zephium-webext-isolated", 4, 0)
            .is_ok());
    }

    #[test]
    fn rates_refill_monotonically_and_are_shared_across_frames() {
        let mut attach = attachment();
        attach.limits.frame_burst = 1;
        attach.limits.page_burst = 1;
        attach.limits.frame_messages_per_second = 1;
        attach.limits.page_messages_per_second = 1;
        let mut state = State::new(PROTOCOL_VERSION, "process-token".into());
        state.attach(7, attach).expect("attach");
        let first = state
            .clear_document(7, 42, "https://example.com", true)
            .expect("doc");
        let second = state
            .clear_document(7, 43, "https://example.com", false)
            .expect("doc");
        let id = state
            .reserve_post(7, 42, first, "zephium-webext-isolated", 1, 1000)
            .expect("post");
        state.settle_post(id);
        assert_eq!(
            state.reserve_post(7, 43, second, "zephium-webext-isolated", 1, 1000),
            Err(StateError::RateLimited)
        );
        assert_eq!(
            state.reserve_post(7, 42, first, "zephium-webext-isolated", 1, 500),
            Err(StateError::RateLimited)
        );
        assert!(state
            .reserve_post(7, 43, second, "zephium-webext-isolated", 1, 2000)
            .is_ok());
    }

    #[test]
    fn document_navigation_keeps_pending_bytes_charged_until_completion() {
        let mut attach = attachment();
        attach.limits = Limits {
            max_message_bytes: 4,
            frame_inflight_bytes: 4,
            page_inflight_bytes: 4,
            ..Limits::default()
        };
        let mut state = State::new(PROTOCOL_VERSION, "process-token".into());
        state.attach(7, attach).expect("attach");
        let seq = state
            .clear_document(7, 42, "https://example.com", true)
            .expect("doc");
        let pending = state
            .reserve_post(7, 42, seq, "zephium-webext-isolated", 4, 0)
            .expect("pending");
        let new_seq = state
            .clear_document(7, 42, "https://example.com/new", true)
            .expect("new doc");
        assert_eq!(
            state.reserve_post(7, 42, new_seq, "zephium-webext-isolated", 1, 0),
            Err(StateError::InflightLimited)
        );
        state.settle_post(pending);
        assert!(state
            .reserve_post(7, 42, new_seq, "zephium-webext-isolated", 4, 0)
            .is_ok());
    }

    #[test]
    fn detached_generations_cannot_evade_the_process_inflight_budget() {
        let mut state = State::new(PROTOCOL_VERSION, "process-token".into());
        let mut pending = Vec::new();
        for generation in 1..=4 {
            let mut attach = attachment();
            attach.generation = generation;
            attach.role = PageRole::ExtensionOwned;
            attach.limits.max_message_bytes = 32 * 1024 * 1024;
            attach
                .extension_origins
                .push("chrome-extension://aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into());
            state.attach(7, attach).expect("attach");
            let seq = state
                .clear_document(
                    7,
                    42,
                    "chrome-extension://aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/page.html",
                    true,
                )
                .expect("doc");
            pending.push(
                state
                    .reserve_post(7, 42, seq, "", 32 * 1024 * 1024, 0)
                    .expect("pending"),
            );
            state.remove_page(7);
        }
        let mut attach = attachment();
        attach.generation = 5;
        state
            .attach(7, attach)
            .expect("reattach with next generation");
        let seq = state
            .clear_document(7, 42, "https://example.com", true)
            .expect("doc");
        assert_eq!(
            state.reserve_post(7, 42, seq, "zephium-webext-isolated", 1, 0),
            Err(StateError::InflightLimited)
        );
        state.settle_post(pending[0]);
        let current = state
            .reserve_post(7, 42, seq, "zephium-webext-isolated", 1, 0)
            .expect("settled capacity");
        for id in pending {
            state.settle_post(id);
        }
        assert_eq!(state.inflight_bytes, 1);
        assert_eq!(state.pages.get(&7).expect("page").outstanding, 1);
        state.settle_post(current);
        assert_eq!(state.inflight_bytes, 0);
    }

    #[test]
    fn registry_and_report_queue_stay_bounded() {
        let (mut state, _) = attached();
        for frame in 1..MAX_FRAMES_PER_PAGE as u64 {
            if frame != 42 {
                state
                    .clear_document(7, frame, "https://example.com", false)
                    .expect("frame");
            }
        }
        state
            .clear_document(7, 999, "https://example.com", false)
            .expect("last frame");
        assert_eq!(
            state.clear_document(7, 1000, "https://example.com", false),
            Err(StateError::Capacity)
        );
        for _ in 0..MAX_PENDING_FRAME_EVENTS + 1 {
            state.commit_frame(7, 42);
        }
        let page = state.pages.get(&7).expect("page");
        assert!(page.events.len() <= MAX_PENDING_FRAME_EVENTS);
        assert!(page
            .events
            .iter()
            .any(|event| matches!(event, FrameEvent::Reset)));
        assert!(state.drain_frame_events(7).len() <= MAX_BATCH_EVENTS);
    }

    #[test]
    fn attach_refuses_duplicate_worlds_noncanonical_origins_and_elevated_limits() {
        for change in 0..4 {
            let mut attach = attachment();
            match change {
                0 => attach.worlds.push(attach.worlds[0].clone()),
                1 => attach.worlds[0].matches.clear(),
                2 => attach.extension_origins.push("https://example.com".into()),
                _ => attach.limits.max_message_bytes = 32 * 1024 * 1024,
            }
            let mut state = State::new(PROTOCOL_VERSION, "process-token".into());
            assert_eq!(state.attach(1, attach), Err(StateError::InvalidAttach));
        }
    }

    #[test]
    fn large_owned_context_limit_does_not_raise_content_script_or_child_limits() {
        let mut attach = attachment();
        attach.role = PageRole::ExtensionOwned;
        attach.limits.max_message_bytes = 32 * 1024 * 1024;
        attach
            .extension_origins
            .push("chrome-extension://aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into());
        let mut state = State::new(PROTOCOL_VERSION, "process-token".into());
        state.attach(1, attach).expect("attach");
        state
            .clear_document(
                1,
                1,
                "chrome-extension://aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/page.html",
                true,
            )
            .expect("top");
        state
            .clear_document(
                1,
                2,
                "chrome-extension://aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/child.html",
                false,
            )
            .expect("child");
        let seq = state
            .clear_document(1, 3, "https://example.com", false)
            .expect("content");
        assert_eq!(state.message_cap(1, 1, ""), Some(32 * 1024 * 1024));
        assert_eq!(state.message_cap(1, 2, ""), Some(8 * 1024 * 1024));
        assert_eq!(
            state.reserve_post(1, 3, seq, "zephium-webext-isolated", 8 * 1024 * 1024 + 1, 0),
            Err(StateError::Oversized)
        );
    }
}
