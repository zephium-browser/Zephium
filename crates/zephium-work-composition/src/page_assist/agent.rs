//! One reply, start to finish: read what the person attached, then call the
//! model and run the tools it asks for until it answers. Text streams as it
//! comes, cited and paced; every tool's effect is admitted against the reads
//! of this reply. Every wait races the reply's cancellation.
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, oneshot};
use zephium_app::work_models::WorkCallSource;
use zephium_core::ids::ProfileId;
use zephium_core::work::model::{
    work_model_cache_key, WorkModelEvent, WorkModelMessage, WorkModelPart, WorkModelRequest,
    WorkModelStop, WorkModelSystemBlock, WorkModelToolChoice, WorkModelToolResult, WorkModelUsage,
};
use zephium_ipc::page_assist::*;
use zephium_page_assist::actions::{Step, Verb};
use zephium_page_assist::outline::Outline;
use zephium_page_assist::pacer::Pacer;
use zephium_page_assist::refs::{self, Ref};
use zephium_page_assist::stream::CiteRewriter;
use zephium_page_assist::{css, guard, prompt, suggest, text};

use super::effect::{self, Control, Observed};
use super::port::{
    BringRequest, Glow, ReadFailure, Readout, ResolvedModel, Role, StepEffect, StepFailure,
    StepRequest, WorkStart,
};
use super::tools::{self, Call, ItemCall};
use super::{
    fault, host_of, readout, site_of, Admitted, Cancel, CancelReason, PageAssist, PendingStyle,
    ReadRecord, MAX_LINKS,
};

const READ_TIMEOUT: Duration = Duration::from_secs(8);
const OUTLINE_TIMEOUT: Duration = Duration::from_secs(3);
const CALL_IDLE_TIMEOUT: Duration = Duration::from_secs(90);
const CALL_OVERALL_TIMEOUT: Duration = Duration::from_secs(600);
/// Longest a new tab is watched for its first load.
const OPEN_TIMEOUT: Duration = Duration::from_secs(12);
/// Longest a new tab may stay unknown to the shell before it counts as open.
const UNLISTED_TIMEOUT: Duration = Duration::from_millis(1_500);
const TEXT_INTERVAL_MS: u64 = 50;
/// When, after a step ran, the page is read again to see its effect.
const EFFECT_READS_MS: [u64; 4] = [0, 250, 700, 1_500];
/// Further reads, only when the page was busy after the step (loading, or
/// its reads changing): a late effect still counts. The last is the cap for
/// the same document.
const EFFECT_BUSY_READS_MS: [u64; 3] = [2_200, 3_000, 4_000];
/// Longest a step's effect is watched for, a new document included.
const EFFECT_NAVIGATION: Duration = Duration::from_secs(10);
const EFFECT_POLL: Duration = Duration::from_millis(50);
/// Most views `find` scrolls down to reach its words.
const FIND_SCROLLS: usize = 4;
/// Passages either side of each one `find` returns.
const FIND_AROUND: usize = 2;
/// Most lines `find` and an act's changes return.
const FIND_LINES: usize = 30;
/// Time a scrolled page gets before it is read again.
const SCROLL_SETTLE: Duration = Duration::from_millis(150);
/// Most model calls one reply makes.
const MAX_CALLS: usize = 16;
/// Most tool calls one reply runs.
const MAX_TOOLS: usize = 40;
/// Most pages one reply reads.
const MAX_READS: usize = 12;
/// Longest transcript handed to Work, in characters.
const MAX_TRANSCRIPT_CHARS: usize = 6_000;

pub(super) struct Reply {
    pub(super) service: PageAssist,
    pub(super) conversation: String,
    pub(super) space: String,
    pub(super) profile: ProfileId,
    pub(super) reply: u32,
    pub(super) words: String,
    pub(super) suggestion: Option<PageAssistSuggestionV1>,
    pub(super) attachments: Vec<Admitted>,
    pub(super) cancel: Arc<Cancel>,
    pub(super) leased: Arc<AtomicBool>,
    pub(super) took_over: Arc<AtomicBool>,
    pub(super) redirect: Arc<super::Redirect>,
    pub(super) ended: Arc<tokio::sync::Notify>,
    /// Tabs lit while this reply works on them.
    pub(super) glowing: std::sync::Mutex<Vec<String>>,
    /// What its model calls cost so far.
    pub(super) spent: std::sync::Mutex<Option<PageAssistCostV1>>,
    pub(super) spoken: bool,
}

enum End {
    Done(Option<PageAssistCostV1>, Option<PageAssistEndV1>),
    Failed(PageAssistFaultV1),
    Cancelled(CancelReason),
}

/// The open text block and what streams into it.
struct Text {
    block: Option<u16>,
    rewriter: CiteRewriter,
    pacer: Pacer<String>,
    cites: Vec<PageAssistCiteV1>,
    sent_cites: usize,
}

impl Default for Text {
    fn default() -> Self {
        Self {
            block: None,
            rewriter: CiteRewriter::default(),
            pacer: Pacer::new(TEXT_INTERVAL_MS),
            cites: Vec::new(),
            sent_cites: 0,
        }
    }
}

type ToolOutcome = Result<String, String>;

impl Reply {
    fn apply(&self, kind: PageAssistEventV1) -> bool {
        self.service.apply(&self.conversation, self.reply, kind)
    }

    fn note(&self, fact: std::fmt::Arguments<'_>) {
        self.service.ports().diagnostics.note(format_args!(
            "assist: conversation={} reply={} {fact}",
            self.conversation, self.reply
        ));
    }

    fn glow(&self, tab: &str, glow: Glow) {
        let mut glowing = self.glowing.lock().unwrap_or_else(|p| p.into_inner());
        if glow != Glow::Off && !glowing.iter().any(|known| known == tab) {
            glowing.push(tab.to_owned());
        }
        drop(glowing);
        self.service.ports().page.glow(tab, glow);
    }

    fn unglow(&self) {
        let tabs = std::mem::take(&mut *self.glowing.lock().unwrap_or_else(|p| p.into_inner()));
        for tab in tabs {
            self.service.ports().page.glow(&tab, Glow::Off);
        }
    }

    fn phase(&self, phase: Option<PageAssistPhaseV1>) {
        self.apply(PageAssistEventV1::Phase { phase });
    }

    fn now(&self) -> u64 {
        self.service.ports().clock.now_ms()
    }

    fn with_data<T>(&self, apply: impl FnOnce(&mut super::ReplyData) -> T) -> Option<T> {
        let mut state = self.service.state();
        state
            .live
            .get_mut(&self.conversation)
            .and_then(|live| live.replies.get_mut(&self.reply))
            .map(apply)
    }

    /// Whether the model has written anything to the person in this reply.
    fn said(&self) -> bool {
        let state = self.service.state();
        state
            .live
            .get(&self.conversation)
            .and_then(|live| live.conversation.blocks(self.reply))
            .is_some_and(|blocks| {
                blocks.iter().any(
                    |block| matches!(block, PageAssistBlockV1::Text { text, .. } if !text.trim().is_empty()),
                )
            })
    }

    fn next_block(&self) -> u16 {
        let state = self.service.state();
        state
            .live
            .get(&self.conversation)
            .map_or(0, |live| live.conversation.next_block(self.reply))
    }

    fn add_block(&self, text: &mut Text, value: PageAssistBlockV1) -> u16 {
        self.flush(text);
        text.block = None;
        text.cites.clear();
        text.sent_cites = 0;
        let block = self.next_block();
        self.apply(PageAssistEventV1::Block { block, value });
        block
    }

    fn set_block(&self, block: u16, value: PageAssistBlockV1) {
        self.apply(PageAssistEventV1::Block { block, value });
    }

    fn admit_links(&self, links: impl IntoIterator<Item = String>) {
        let mut state = self.service.state();
        if let Some(live) = state.live.get_mut(&self.conversation) {
            for link in links {
                if live.links.len() >= MAX_LINKS {
                    break;
                }
                if zephium_page_assist::policy::is_web_url(&link) {
                    live.links.insert(link);
                }
            }
        }
    }

    fn handle(&self, tab: &str) -> String {
        self.with_data(|data| {
            let index = match data.handles.iter().position(|known| known == tab) {
                Some(index) => index,
                None => {
                    data.handles.push(tab.to_owned());
                    data.handles.len() - 1
                }
            };
            format!("t{}", index + 1)
        })
        .unwrap_or_default()
    }

    fn tab_of(&self, handle: &str) -> Option<String> {
        let index: usize = handle.trim().strip_prefix('t')?.parse().ok()?;
        self.with_data(|data| data.handles.get(index.checked_sub(1)?).cloned())
            .flatten()
    }

    fn record(&self, letters: &str) -> Option<(usize, ReadRecord)> {
        let index = usize::from(refs::parse(&format!("{letters}1"))?.read);
        self.with_data(|data| data.reads.get(index).cloned().map(|read| (index, read)))
            .flatten()
    }

    fn record_of(&self, reference: Ref) -> Option<ReadRecord> {
        self.with_data(|data| data.reads.get(usize::from(reference.read)).cloned())
            .flatten()
    }

    // ------------------------------------------------------------------ text

    fn deliver(&self, text: &mut Text, chunks: Vec<String>) {
        let delta: String = chunks.concat();
        if !delta.is_empty() {
            let block = *text.block.get_or_insert_with(|| self.next_block());
            self.apply(PageAssistEventV1::Text { block, delta });
        }
        if let Some(block) = text.block {
            if text.cites.len() != text.sent_cites {
                text.sent_cites = text.cites.len();
                self.apply(PageAssistEventV1::Cites {
                    block,
                    cites: text.cites.clone(),
                });
            }
        }
    }

    fn take(&self, text: &mut Text, chunk: &str) {
        let cites = &mut text.cites;
        let rewritten = text.rewriter.push(chunk, &mut |reference| {
            let cite = self
                .with_data(|data| {
                    let read = data.reads.get(usize::from(reference.read))?;
                    let (page, tab, epoch) = (read.page.clone(), read.tab.clone(), read.epoch);
                    data.book.cite(reference, &page, &tab, epoch)
                })
                .flatten()?;
            if !cites.iter().any(|known| known.n == cite.n) {
                cites.push(cite.clone());
            }
            Some(cite.n)
        });
        if !rewritten.is_empty() {
            let now = self.now();
            let due = text.pacer.push(rewritten, now);
            self.deliver(text, due);
        }
    }

    fn flush(&self, text: &mut Text) {
        let rest = text.rewriter.finish();
        if !rest.is_empty() {
            text.pacer.push(rest, self.now());
        }
        let due = text.pacer.flush();
        self.deliver(text, due);
    }

    // ----------------------------------------------------------------- reads

    fn other_sites(&self, target: &str) -> Vec<String> {
        self.with_data(|data| {
            data.read_sites
                .iter()
                .map(|(_, site)| site)
                .filter(|site| site.as_str() != target)
                .cloned()
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect()
        })
        .unwrap_or_default()
    }

    async fn read_tab(&self, tab: &str) -> Result<usize, PageAssistFaultV1> {
        let ports = self.service.ports();
        let facts = ports
            .page
            .facts(tab)
            .filter(|facts| facts.profile == self.profile)
            .ok_or(PageAssistFaultV1::Unavailable)?;
        if let Some(fault) =
            zephium_page_assist::policy::refusal(self.service.availability(Some(&facts)).await)
        {
            return Err(fault);
        }
        let url = facts.url.clone().unwrap_or_default();
        let host = host_of(&url);
        let site = site_of(Some(&url)).ok_or(PageAssistFaultV1::NotWeb)?;
        let index = self
            .with_data(|data| data.reads.len())
            .ok_or(PageAssistFaultV1::Unavailable)?;
        if index >= MAX_READS {
            return Err(PageAssistFaultV1::TooLong);
        }
        let letters = refs::letters(u16::try_from(index).unwrap_or(u16::MAX))
            .ok_or(PageAssistFaultV1::TooLong)?;
        self.phase(Some(PageAssistPhaseV1::Reading { site: host.clone() }));
        let readout =
            match tokio::time::timeout(READ_TIMEOUT, ports.page.read(tab, facts.epoch)).await {
                Ok(Ok(readout)) if readout.epoch == facts.epoch => readout,
                Ok(Ok(_)) | Ok(Err(ReadFailure::PageChanged)) => {
                    return Err(PageAssistFaultV1::PageChanged)
                }
                Ok(Err(ReadFailure::NotReady)) => return Err(PageAssistFaultV1::Loading),
                Ok(Err(ReadFailure::Unavailable)) | Err(_) => {
                    return Err(PageAssistFaultV1::Unreadable)
                }
            };
        let page = readout::page_read(
            &letters,
            &site,
            &host,
            &readout,
            &zephium_agentic::contains_recognized_secret,
        );
        if page.fragments.is_empty() {
            return Err(PageAssistFaultV1::Unreadable);
        }
        let traits = page.traits;
        let links: Vec<String> = page
            .fragments
            .values()
            .filter_map(|f| f.href.clone())
            .collect();
        {
            let mut state = self.service.state();
            state.traits.insert(tab.to_owned(), (facts.epoch, traits));
            if let Some(live) = state.live.get_mut(&self.conversation) {
                for link in links {
                    if live.links.len() >= MAX_LINKS {
                        break;
                    }
                    live.links.insert(link);
                }
            }
        }
        self.with_data(|data| {
            data.read_sites.insert((tab.to_owned(), site));
            data.reads.push(ReadRecord {
                tab: tab.to_owned(),
                epoch: facts.epoch,
                readout,
                page,
                acted: data.acted,
            })
        });
        self.handle(tab);
        Ok(index)
    }

    fn page_block(&self, index: usize) -> String {
        self.with_data(|data| {
            let read = data.reads.get(index)?;
            let letters = refs::letters(u16::try_from(index).ok()?)?;
            Some(prompt::page_block(&letters, &read.page))
        })
        .flatten()
        .unwrap_or_default()
    }

    async fn outline(&self, index: usize) -> Option<Outline> {
        if let Some(outline) = self
            .with_data(|data| data.outlines.get(&index).cloned())
            .flatten()
        {
            return Some(outline);
        }
        let read = self
            .with_data(|data| data.reads.get(index).cloned())
            .flatten()?;
        let outline = tokio::time::timeout(
            OUTLINE_TIMEOUT,
            self.service.ports().page.outline(&read.tab, read.epoch),
        )
        .await
        .ok()?
        .ok()
        .map(Outline::admit)?;
        self.with_data(|data| data.outlines.insert(index, outline.clone()));
        Some(outline)
    }

    // ------------------------------------------------------------- the model

    /// Reads an attached page; the first one is where the reply works.
    async fn read_first(&self, tab: &str) -> Result<usize, PageAssistFaultV1> {
        self.with_data(|data| {
            data.working.get_or_insert_with(|| tab.to_owned());
        });
        self.read_tab(tab).await
    }

    async fn context(&self) -> Vec<String> {
        let mut parts = Vec::new();
        let mut first_read = None;
        for attachment in &self.attachments {
            match attachment {
                Admitted::Page { tab } => match self.read_first(tab).await {
                    Ok(index) => {
                        first_read.get_or_insert(index);
                        parts.push(self.page_block(index));
                    }
                    Err(_) => {
                        let host = self
                            .service
                            .ports()
                            .page
                            .facts(tab)
                            .and_then(|facts| facts.url)
                            .map(|url| host_of(&url))
                            .unwrap_or_default();
                        parts.push(format!("(The page {host} could not be read just now.)"));
                    }
                },
                Admitted::Selection { tab, site, text } => {
                    self.with_data(|data| data.read_sites.insert((tab.clone(), site.clone())));
                    let letters = self
                        .with_data(|data| data.reads.iter().position(|read| &read.tab == tab))
                        .flatten()
                        .and_then(|index| refs::letters(u16::try_from(index).ok()?))
                        .unwrap_or_else(|| "?".to_owned());
                    parts.push(prompt::selection_block(&letters, text));
                }
                Admitted::Element {
                    tab,
                    site,
                    host,
                    label,
                    pick,
                } => {
                    self.with_data(|data| data.read_sites.insert((tab.clone(), site.clone())));
                    parts.push(prompt::element_block(
                        host,
                        label,
                        pick.selector.as_deref(),
                        &pick.text,
                    ));
                }
            }
        }
        let wants_style = self.suggestion.is_some_and(suggest::restyles);
        if let (true, Some(index)) = (wants_style, first_read) {
            if let Some(outline) = self.outline(index).await {
                parts.push(prompt::outline_block(&outline));
            }
        }
        parts.push(prompt::request(&self.words, self.suggestion));
        parts
    }

    fn history(&self) -> Vec<WorkModelMessage> {
        let messages = {
            let mut state = self.service.state();
            let Some(live) = state.live.get_mut(&self.conversation) else {
                return Vec::new();
            };
            let messages = live.conversation.messages().to_vec();
            let mut sites = std::collections::BTreeSet::new();
            let mut unknown = false;
            for message in &messages {
                if let PageAssistMessageV1::Person { attachments, .. } = message {
                    for attachment in attachments {
                        let (tab, host) = match attachment {
                            PageAssistAttachmentV1::Page { tab, host, .. }
                            | PageAssistAttachmentV1::Selection { tab, host, .. }
                            | PageAssistAttachmentV1::Element { tab, host, .. } => (tab, host),
                        };
                        if let Some(site) = site_of(Some(&format!("https://{host}"))) {
                            sites.insert((tab.clone(), site));
                        } else {
                            unknown = true;
                        }
                    }
                }
                if let PageAssistMessageV1::Reply { id, .. } = message {
                    if *id == self.reply {
                        continue;
                    }
                    if let Some(data) = live.replies.get(id) {
                        sites.extend(data.read_sites.iter().cloned());
                        unknown |= data.unknown_read_sites;
                    } else {
                        unknown = true;
                    }
                }
            }
            if let Some(data) = live.replies.get_mut(&self.reply) {
                data.read_sites.extend(sites);
                data.unknown_read_sites |= unknown;
            }
            messages
        };
        let mut model = Vec::new();
        for exchange in prompt::history(&messages) {
            model.push(WorkModelMessage::User(vec![WorkModelPart::Text(
                exchange.person,
            )]));
            model.push(WorkModelMessage::Assistant(vec![WorkModelPart::Text(
                exchange.reply,
            )]));
        }
        model
    }

    async fn call(
        &self,
        model: &ResolvedModel,
        request: WorkModelRequest,
        text: &mut Text,
        hosted: bool,
    ) -> Result<zephium_core::work::model::WorkModelOutcome, End> {
        self.call_limited(
            model,
            request,
            text,
            hosted,
            CALL_IDLE_TIMEOUT,
            CALL_OVERALL_TIMEOUT,
        )
        .await
    }

    async fn call_limited(
        &self,
        model: &ResolvedModel,
        request: WorkModelRequest,
        text: &mut Text,
        hosted: bool,
        idle_timeout: Duration,
        overall_timeout: Duration,
    ) -> Result<zephium_core::work::model::WorkModelOutcome, End> {
        let (sender, mut receiver) = mpsc::unbounded_channel::<Option<String>>();
        let events = move |event: WorkModelEvent| {
            let chunk = match event {
                WorkModelEvent::Text(chunk) if chunk.is_empty() => return,
                WorkModelEvent::Text(chunk) => Some(chunk),
                _ => None,
            };
            let _ = sender.send(chunk);
        };
        let call = tokio::time::timeout(overall_timeout, model.client.call(request, &events));
        tokio::pin!(call);
        let idle = tokio::time::sleep(idle_timeout);
        tokio::pin!(idle);
        let outcome = loop {
            let wait = text
                .pacer
                .deadline()
                .map(|due| Duration::from_millis(due.saturating_sub(self.now())));
            tokio::select! {
                biased;
                _ = &mut idle => break None,
                outcome = &mut call => break Some(outcome),
                () = self.redirect.requested() => {
                    return Err(End::Cancelled(CancelReason::Redirected));
                }
                Some(chunk) = receiver.recv() => {
                    idle.as_mut().reset(tokio::time::Instant::now() + idle_timeout);
                    if let Some(chunk) = chunk {
                        self.take(text, &chunk);
                    }
                },
                _ = tokio::time::sleep(wait.unwrap_or_default()), if wait.is_some() => {
                    let due = text.pacer.poll(self.now());
                    self.deliver(text, due);
                }
            }
        };
        while let Ok(chunk) = receiver.try_recv() {
            if let Some(chunk) = chunk {
                self.take(text, &chunk);
            }
        }
        match outcome {
            Some(Ok(Ok(outcome))) => Ok(outcome),
            Some(Ok(Err(error))) => Err(End::Failed(fault::model_fault(error, hosted))),
            Some(Err(_)) | None => Err(End::Failed(PageAssistFaultV1::TimedOut)),
        }
    }

    #[cfg(test)]
    pub(super) async fn call_for_test(
        &self,
        model: &ResolvedModel,
        request: WorkModelRequest,
        idle_timeout: Duration,
        overall_timeout: Duration,
    ) -> Option<PageAssistFaultV1> {
        match self
            .call_limited(
                model,
                request,
                &mut Text::default(),
                false,
                idle_timeout,
                overall_timeout,
            )
            .await
        {
            Ok(_) => None,
            Err(End::Failed(fault)) => Some(fault),
            Err(_) => Some(PageAssistFaultV1::Unavailable),
        }
    }
}

fn add_usage(total: &mut WorkModelUsage, usage: &WorkModelUsage) {
    total.input_tokens = total.input_tokens.saturating_add(usage.input_tokens);
    total.cached_input_tokens = total
        .cached_input_tokens
        .saturating_add(usage.cached_input_tokens);
    total.cache_write_input_tokens = total
        .cache_write_input_tokens
        .saturating_add(usage.cache_write_input_tokens);
    total.output_tokens = total.output_tokens.saturating_add(usage.output_tokens);
    total.reasoning_tokens = total
        .reasoning_tokens
        .saturating_add(usage.reasoning_tokens);
    total.cost_micros = match (total.cost_micros, usage.cost_micros) {
        (Some(a), Some(b)) => Some(a.saturating_add(b)),
        (a, b) => a.or(b),
    };
}

/// Runs one admitted reply to its end.
pub(super) async fn run(reply: Reply) {
    let end = tokio::select! {
        biased;
        reason = reply.cancel.cancelled() => End::Cancelled(reason),
        end = body(&reply) => end,
    };
    reply.leased.store(false, Ordering::Release);
    reply.unglow();
    reply.note(format_args!(
        "end {}",
        match &end {
            End::Done(_, end) => format!("done end={end:?}"),
            End::Cancelled(CancelReason::Redirected) => "redirected".to_owned(),
            End::Failed(fault) => format!("failed fault={fault:?}"),
            End::Cancelled(reason) => format!("cancelled reason={reason:?}"),
        }
    ));
    match end {
        End::Done(cost, end) => {
            reply.apply(PageAssistEventV1::Done { cost, end });
        }
        End::Failed(fault) => {
            reply.apply(PageAssistEventV1::Failed { fault });
        }
        End::Cancelled(CancelReason::Stopped) => {
            reply.apply(PageAssistEventV1::Cancelled);
        }
        End::Cancelled(CancelReason::Redirected) => {
            reply.phase(None);
            let cost = reply
                .spent
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .clone();
            reply.apply(PageAssistEventV1::Done {
                cost,
                end: Some(PageAssistEndV1::Redirected),
            });
        }
        End::Cancelled(CancelReason::Gone) => {}
    }
    {
        let mut state = reply.service.state();
        if state
            .running
            .get(&reply.conversation)
            .is_some_and(|running| running.reply == reply.reply)
        {
            state.running.remove(&reply.conversation);
        }
    }
    reply.ended.notify_waiters();
    reply.service.persist(&reply.conversation);
}

async fn body(reply: &Reply) -> End {
    let ports = reply.service.ports();
    let source = WorkCallSource::Page {
        conversation: reply.conversation.clone(),
        reply: reply.reply,
    };
    reply.phase(Some(PageAssistPhaseV1::Thinking));
    let model = match ports.models.resolve(&reply.profile, source).await {
        Ok(model) => model,
        Err(error) => return End::Failed(fault::model_fault(error, false)),
    };
    let hosted = model.entry.model.provider == zephium_core::work::model::WorkModelProvider::Cloud;
    reply.note(format_args!(
        "start model={} effort={:?} tools={} attachments={} suggestion={:?}",
        model.entry.id,
        model.effort.map(super::sidebar_effort),
        model.entry.supports.tools,
        reply.attachments.len(),
        reply.suggestion
    ));
    let parts = reply.context().await;
    let mut messages = reply.history();
    messages.push(WorkModelMessage::User(
        parts.into_iter().map(WorkModelPart::Text).collect(),
    ));
    let mut system = vec![WorkModelSystemBlock {
        text: prompt::SYSTEM.to_owned(),
        cache: true,
    }];
    if reply.spoken {
        system.push(WorkModelSystemBlock {
            text: prompt::SPOKEN.to_owned(),
            cache: true,
        });
    }
    let tools = if model.entry.supports.tools {
        tools::definitions()
    } else {
        Vec::new()
    };
    let cache_key = work_model_cache_key(&system, &tools);
    let mut text = Text::default();
    let mut usage = WorkModelUsage::default();
    let mut ran = 0usize;
    let mut asked = false;
    let mut calls_left = MAX_CALLS;
    while calls_left > 0 {
        if reply.redirect.asked() {
            reply.flush(&mut text);
            return End::Cancelled(CancelReason::Redirected);
        }
        if text.block.is_none() {
            reply.phase(Some(PageAssistPhaseV1::Thinking));
        }
        let request = WorkModelRequest {
            image_fence: None,
            cache_key: Some(cache_key.clone()),
            model: model.entry.model.clone(),
            system: system.clone(),
            tools: tools.clone(),
            messages: messages.clone(),
            max_output_tokens: prompt::MAX_OUTPUT_TOKENS.min(model.entry.max_output.max(8_192)),
            reasoning: model
                .effort
                .filter(|_| model.entry.supports.reasoning)
                .map(super::sidebar_effort),
            native_search: false,
            parallel_tools: false,
            tool_choice: WorkModelToolChoice::Auto,
            volatile_tail: 0,
        };
        let outcome = match reply.call(&model, request, &mut text, hosted).await {
            Ok(outcome) => outcome,
            Err(end) => {
                reply.flush(&mut text);
                return end;
            }
        };
        add_usage(&mut usage, &outcome.usage);
        *reply.spent.lock().unwrap_or_else(|p| p.into_inner()) =
            Some(fault::cost(&model.entry, &usage));
        reply.note(format_args!(
            "call stop={:?} in={} cached={} out={} reasoning={} text_blocks={}",
            outcome.stop,
            outcome.usage.input_tokens,
            outcome.usage.cached_input_tokens,
            outcome.usage.output_tokens,
            outcome.usage.reasoning_tokens,
            reply.next_block()
        ));
        let exhausted = outcome.stop == WorkModelStop::MaxTokens;
        let calls: Vec<_> = outcome
            .assistant
            .iter()
            .filter_map(|part| match part {
                WorkModelPart::ToolCall(call) => Some(call.clone()),
                _ => None,
            })
            .collect();
        messages.push(WorkModelMessage::Assistant(outcome.assistant));
        if exhausted && calls.is_empty() && reply.next_block() == 0 && text.block.is_none() {
            reply.flush(&mut text);
            return End::Failed(PageAssistFaultV1::TooLong);
        }
        if outcome.stop != WorkModelStop::ToolUse || calls.is_empty() {
            break;
        }
        reply.flush(&mut text);
        let mut results = Vec::with_capacity(calls.len());
        let mut stop = false;
        for call in calls {
            if reply.redirect.asked() {
                reply.flush(&mut text);
                return End::Cancelled(CancelReason::Redirected);
            }
            ran += 1;
            let outcome = if ran > MAX_TOOLS {
                Err("Too many tool calls in one reply; answer with what you have.".to_owned())
            } else if asked {
                Err("You already asked the person; wait for their answer.".to_owned())
            } else {
                match tools::parse(&call.name, &call.arguments) {
                    Some(Call::AskPerson { question }) => {
                        if text.block.is_some() {
                            reply.take(&mut text, "\n\n");
                        }
                        reply.take(&mut text, &question);
                        asked = true;
                        stop = true;
                        Ok("Asked; the reply ends here.".to_owned())
                    }
                    Some(parsed) => {
                        stop |= matches!(parsed, Call::HandOff { .. });
                        match run_tool(reply, &mut text, parsed).await {
                            Ok(outcome) => outcome,
                            Err(end) => return end,
                        }
                    }
                    None => Err(format!(
                        "{} was called with arguments it does not take.",
                        call.name
                    )),
                }
            };
            let (content, is_error) = match outcome {
                Ok(content) => (content, false),
                Err(content) => (content, true),
            };
            reply.note(format_args!(
                "tool name={} ok={} result_bytes={}",
                tools::known(&call.name),
                !is_error,
                content.len()
            ));
            results.push(WorkModelToolResult {
                call: call.id,
                content,
                is_error,
            });
        }
        messages.push(WorkModelMessage::ToolResults(results));
        let finished = reply
            .with_data(|data| std::mem::take(&mut data.finished))
            .unwrap_or(false);
        if stop || (finished && reply.said()) {
            break;
        }
        calls_left -= 1;
        if calls_left == 1 {
            messages.push(WorkModelMessage::User(vec![WorkModelPart::Text(
                "Stop using tools now. In two or three sentences tell the person what you did and what is left.".to_owned(),
            )]));
        }
    }
    reply.flush(&mut text);
    reply.phase(None);
    if reply.next_block() == 0 {
        return End::Failed(PageAssistFaultV1::NoAnswer);
    }
    End::Done(
        Some(fault::cost(&model.entry, &usage)),
        asked.then_some(PageAssistEndV1::Asked),
    )
}

async fn run_tool(reply: &Reply, text: &mut Text, call: Call) -> Result<ToolOutcome, End> {
    Ok(match call {
        Call::ListTabs => Ok(list_tabs(reply)),
        Call::ReadPage { tab } => read_page(reply, &tab).await,
        Call::ShowItems { items } => show_items(reply, text, items).await,
        Call::OpenTabs { targets } => open_tabs(reply, text, targets).await,
        Call::GoTo { target, new_tab } => go_to(reply, text, &target, new_tab).await,
        Call::Act { steps, finish } => return act(reply, text, steps, finish).await,
        Call::Find { page, words } => find(reply, &page, &words).await,
        Call::InspectStyle { page } => inspect_style(reply, &page).await,
        Call::Restyle { page, summary, css } => restyle(reply, text, &page, summary, &css).await,
        Call::HandOff {
            title,
            summary,
            bring,
        } => hand_off(reply, text, title, summary, bring).await,
        Call::AskPerson { .. } => Err("ask_person ends the reply.".to_owned()),
    })
}

fn list_tabs(reply: &Reply) -> String {
    let tabs = reply
        .service
        .ports()
        .tabs
        .list(&reply.profile, &reply.space);
    reply.admit_links(tabs.iter().filter_map(|tab| tab.url.clone()));
    let lines: Vec<String> = tabs
        .iter()
        .take(60)
        .map(|tab| {
            let host = tab.url.as_deref().map(host_of).unwrap_or_default();
            let site = site_of(tab.url.as_deref()).unwrap_or_else(|| {
                if host.is_empty() {
                    "?".to_owned()
                } else {
                    host.clone()
                }
            });
            reply.with_data(|data| data.read_sites.insert((tab.id.clone(), site)));
            let title = text::bounded(&text::clean(&tab.title), 90).replace('"', "'");
            format!(
                "{} · {} · \"{}\"",
                reply.handle(&tab.id),
                host,
                prompt::fence(&title)
            )
        })
        .collect();
    if lines.is_empty() {
        "No tabs are open in this Space.".to_owned()
    } else {
        lines.join("\n")
    }
}

async fn read_page(reply: &Reply, handle: &str) -> ToolOutcome {
    let tab = reply
        .tab_of(handle)
        .ok_or_else(|| format!("{handle} is not a tab handle from list_tabs."))?;
    match reply.read_tab(&tab).await {
        Ok(index) => Ok(reply.page_block(index)),
        Err(PageAssistFaultV1::Sensitive) => {
            Err("That tab is a sensitive site and is never read.".to_owned())
        }
        Err(PageAssistFaultV1::TooLong) => {
            Err("No more pages can be read in this reply.".to_owned())
        }
        Err(_) => Err(
            "That tab is asleep or still loading. go_to its address from list_tabs to open it fresh."
                .to_owned(),
        ),
    }
}

async fn show_items(reply: &Reply, text: &mut Text, calls: Vec<ItemCall>) -> ToolOutcome {
    let mut items = Vec::with_capacity(calls.len());
    let mut marks = Vec::new();
    let mut page: Option<ReadRecord> = None;
    for (index, call) in calls.into_iter().enumerate() {
        let n = u8::try_from(index + 1).unwrap_or(u8::MAX);
        let reference = call.reference.as_deref().and_then(refs::parse);
        let record = reference.and_then(|reference| reply.record_of(reference));
        let fragment = reference
            .zip(record.as_ref())
            .and_then(|(reference, record)| record.page.fragments.get(&reference.passage).cloned());
        if let (Some(reference), Some(record), Some(_)) = (reference, &record, &fragment) {
            let same = page.as_ref().is_none_or(|page| {
                page.page.payload == record.page.payload && page.tab == record.tab
            });
            if same {
                page.get_or_insert_with(|| record.clone());
                marks.push((n, reference.passage));
            }
        }
        items.push(PageAssistItemV1 {
            n,
            title: call.title,
            meta: call.meta,
            url: fragment.and_then(|fragment| fragment.href),
            marked: false,
        });
    }
    let tab = page
        .as_ref()
        .map(|page| page.tab.clone())
        .unwrap_or_default();
    let block = reply.add_block(
        text,
        PageAssistBlockV1::Items {
            tab: tab.clone(),
            items: items.clone(),
        },
    );
    let Some(page) = page else {
        return Ok(format!(
            "Showed {} items; none could be marked on a page.",
            items.len()
        ));
    };
    let placed = reply
        .service
        .ports()
        .page
        .marks(&page.tab, page.epoch, page.readout.read, marks)
        .await;
    let previous = {
        let mut state = reply.service.state();
        state.marks.insert(
            page.tab.clone(),
            (reply.conversation.clone(), reply.reply, block),
        )
    };
    if let Some((conversation, earlier, earlier_block)) = previous {
        if (conversation.as_str(), earlier, earlier_block)
            != (reply.conversation.as_str(), reply.reply, block)
        {
            reply.service.unmark(&conversation, earlier, earlier_block);
        }
    }
    for item in &mut items {
        item.marked = placed.contains(&item.n);
    }
    reply.set_block(
        block,
        PageAssistBlockV1::Items {
            tab,
            items: items.clone(),
        },
    );
    Ok(format!(
        "Showed {} items as cards; {} marked on the page. Don't repeat them in text.",
        items.len(),
        placed.len()
    ))
}

async fn open_tabs(reply: &Reply, text: &mut Text, targets: Vec<String>) -> ToolOutcome {
    let mut urls: Vec<String> = Vec::new();
    let mut refused = Vec::new();
    let links = {
        let state = reply.service.state();
        state
            .live
            .get(&reply.conversation)
            .map(|live| live.links.clone())
            .unwrap_or_default()
    };
    for target in targets {
        let (url, source) = match refs::parse(&target) {
            Some(reference) => match reply.record_of(reference) {
                Some(record) => (
                    record
                        .page
                        .fragments
                        .get(&reference.passage)
                        .and_then(|fragment| fragment.href.clone()),
                    Some(record.page.site),
                ),
                None => (None, None),
            },
            None => (Some(target.clone()).filter(|url| links.contains(url)), None),
        };
        match url {
            Some(url) if !urls.contains(&url) => {
                approve_navigation(reply, text, &url, source.as_deref()).await?;
                urls.push(url);
            }
            Some(_) => {}
            None => refused.push(target),
        }
        if urls.len() == tools::MAX_OPEN {
            break;
        }
    }
    if urls.is_empty() {
        return Err(
            "None of those targets is a link from a page you read or an address the person wrote."
                .to_owned(),
        );
    }
    reply.phase(Some(PageAssistPhaseV1::Opening));
    let mut rows: Vec<PageAssistOpenedV1> = urls
        .iter()
        .enumerate()
        .map(|(index, url)| PageAssistOpenedV1 {
            index: u8::try_from(index).unwrap_or(u8::MAX),
            host: host_of(url),
            url: url.clone(),
            tab: None,
            state: PageAssistOpenStateV1::Queued,
        })
        .collect();
    let block = reply.add_block(text, PageAssistBlockV1::Opened { tabs: rows.clone() });
    let ports = reply.service.ports().clone();
    let mut after = reply
        .with_data(|data| data.handles.first().cloned())
        .flatten();
    for index in 0..rows.len() {
        rows[index].state = PageAssistOpenStateV1::Opening;
        reply.set_block(block, PageAssistBlockV1::Opened { tabs: rows.clone() });
        let opened = ports
            .tabs
            .open(
                &reply.profile,
                &reply.space,
                &rows[index].url,
                after.as_deref(),
                false,
            )
            .await;
        match opened {
            Some(tab) => {
                rows[index].tab = Some(tab.clone());
                reply.set_block(block, PageAssistBlockV1::Opened { tabs: rows.clone() });
                settled(&ports, &tab).await;
                rows[index].state = PageAssistOpenStateV1::Opened;
                after = Some(tab);
            }
            None => rows[index].state = PageAssistOpenStateV1::Failed,
        }
        reply.set_block(block, PageAssistBlockV1::Opened { tabs: rows.clone() });
    }
    reply.phase(Some(PageAssistPhaseV1::Thinking));
    let opened: Vec<&str> = rows
        .iter()
        .filter(|row| row.state == PageAssistOpenStateV1::Opened)
        .map(|row| row.host.as_str())
        .collect();
    let mut summary = format!(
        "Opened {} of {} in background tabs: {}.",
        opened.len(),
        rows.len(),
        opened.join(", ")
    );
    if !refused.is_empty() {
        summary.push_str(&format!(
            " Not opened, not from a page you read: {}.",
            refused.join(", ")
        ));
    }
    Ok(summary)
}

/// A read link is data, not permission to send information to another site.
/// Missing provenance is conservative: it cannot prove a same-site move.
async fn approve_navigation(
    reply: &Reply,
    text: &mut Text,
    url: &str,
    source: Option<&str>,
) -> Result<(), String> {
    if super::written_links(&reply.words).any(|written| written == url) {
        return Ok(());
    }
    let inherited_source = if source.is_none() {
        let state = reply.service.state();
        let sites: std::collections::BTreeSet<String> = state
            .live
            .get(&reply.conversation)
            .into_iter()
            .flat_map(|live| live.replies.values())
            .flat_map(|data| &data.reads)
            .filter(|record| {
                record
                    .page
                    .fragments
                    .values()
                    .any(|fragment| fragment.href.as_deref() == Some(url))
            })
            .map(|record| record.page.site.clone())
            .collect();
        (sites.len() == 1)
            .then(|| sites.into_iter().next())
            .flatten()
    } else {
        None
    };
    let source = source.or(inherited_source.as_deref());
    let destination = site_of(Some(url));
    let cross_site = source
        .zip(destination.as_deref())
        .is_none_or(|(from, to)| from != to);
    let holds_other_data = reply
        .with_data(|data| {
            data.unknown_read_sites
                || data
                    .read_sites
                    .iter()
                    .any(|(_, site)| Some(site.as_str()) != source)
        })
        .unwrap_or(true);
    if !cross_site || !holds_other_data {
        return Ok(());
    }
    if url.chars().count() > 2048 || url != text::clean(url) || destination.is_none() {
        return Err("That destination cannot be safely reviewed.".into());
    }
    let proposal = |state| PageAssistBlockV1::Steps {
        steps: vec![PageAssistStepV1 {
            index: 0,
            label: format!(
                "Allow opening {url}\nFrom {}",
                source.unwrap_or("an unknown source")
            ),
            category: Some(WorkConfirmCategoryV1::Communication),
            state,
            reason: None,
        }],
    };
    let block = reply.add_block(text, proposal(PageAssistStepStateV1::Confirm));
    let state = match decision(reply, block).await {
        Ok(PageAssistDecisionV1::Approve) if reply.cancel.reason().is_none() => {
            PageAssistStepStateV1::Done
        }
        Ok(PageAssistDecisionV1::Decline) | Err(CancelReason::Redirected) => {
            PageAssistStepStateV1::Declined
        }
        _ => PageAssistStepStateV1::Skipped,
    };
    reply.set_block(block, proposal(state));
    if state == PageAssistStepStateV1::Done {
        Ok(())
    } else {
        Err("The destination was not approved. Do not send its information another way.".into())
    }
}

/// Waits for a new tab's first load, bounded.
async fn settled(ports: &super::Ports, tab: &str) {
    let started = tokio::time::Instant::now();
    while started.elapsed() < OPEN_TIMEOUT {
        match ports.page.facts(tab) {
            Some(facts) if !facts.loading => break,
            None if started.elapsed() > UNLISTED_TIMEOUT => break,
            _ => tokio::time::sleep(Duration::from_millis(150)).await,
        }
    }
}

/// One page in front, read for the model: a read link, an address the
/// person wrote or a listed tab's, or a search on the person's own engine.
async fn go_to(reply: &Reply, text: &mut Text, target: &str, new_tab: bool) -> ToolOutcome {
    let ports = reply.service.ports().clone();
    let target = target.trim();
    let mut search = None;
    let mut source = None;
    let url = if let Some(query) = target
        .strip_prefix("search:")
        .map(str::trim)
        .filter(|query| !query.is_empty())
    {
        if query.chars().count() > 512
            || query != text::clean(query)
            || zephium_agentic::contains_recognized_secret(query)
        {
            return Err(
                "That search cannot be safely reviewed. Use a shorter query without credentials."
                    .into(),
            );
        }
        let url = ports
            .tabs
            .search_url(&reply.profile, query)
            .ok_or_else(|| "That search cannot be made.".to_owned())?;
        let destination = zephium_agentic::SemanticOrigin::parse(&url)
            .map_err(|_| "That search destination cannot be safely reviewed.".to_owned())?
            .as_url()
            .origin()
            .ascii_serialization();
        let proposal = |state| PageAssistBlockV1::Search {
            destination: destination.clone(),
            query: query.to_owned(),
            state,
        };
        let block = reply.add_block(text, proposal(PageAssistStepStateV1::Confirm));
        let state = match decision(reply, block).await {
            Ok(PageAssistDecisionV1::Approve) if reply.cancel.reason().is_none() => {
                PageAssistStepStateV1::Running
            }
            Ok(PageAssistDecisionV1::Decline) | Err(CancelReason::Redirected) => {
                PageAssistStepStateV1::Declined
            }
            _ => PageAssistStepStateV1::Skipped,
        };
        reply.set_block(block, proposal(state));
        if state != PageAssistStepStateV1::Running {
            return Err("The search was not approved. Do not send the query another way.".into());
        }
        search = Some((block, destination, query.to_owned()));
        url
    } else if let Some(reference) = refs::parse(target) {
        reply
            .record_of(reference)
            .and_then(|record| {
                source = Some(record.page.site);
                record.page.fragments.get(&reference.passage)?.href.clone()
            })
            .ok_or_else(|| format!("{target} is not a link of a page you read."))?
    } else {
        let known = {
            let state = reply.service.state();
            state
                .live
                .get(&reply.conversation)
                .is_some_and(|live| live.links.contains(target))
        };
        if !known {
            return Err("Not an address the person wrote, a link you read or a listed tab. Use `search: words` instead.".to_owned());
        }
        target.to_owned()
    };
    if search.is_none() {
        approve_navigation(reply, text, &url, source.as_deref()).await?;
    }
    let working = reply.with_data(|data| data.working.clone()).flatten();
    let result = match working.filter(|_| !new_tab) {
        Some(tab) => move_tab(reply, text, &ports, &tab, &url).await,
        None => open_in_front(reply, text, &ports, &url).await,
    };
    if let Some((block, destination, query)) = search {
        reply.set_block(
            block,
            PageAssistBlockV1::Search {
                destination,
                query,
                state: if result.is_ok() {
                    PageAssistStepStateV1::Done
                } else {
                    PageAssistStepStateV1::Failed
                },
            },
        );
    }
    result
}

/// Follows `url` in the tab the reply works in, as a step on that page.
async fn move_tab(
    reply: &Reply,
    text: &mut Text,
    ports: &super::Ports,
    tab: &str,
    url: &str,
) -> ToolOutcome {
    let host = host_of(url);
    let before = ports.page.facts(tab).map(|facts| facts.epoch);
    let mut row = PageAssistStepV1 {
        index: 0,
        label: format!("Go to {host}"),
        category: None,
        state: PageAssistStepStateV1::Running,
        reason: None,
    };
    reply.phase(Some(PageAssistPhaseV1::Acting));
    reply.glow(tab, Glow::Working);
    let block = reply.add_block(
        text,
        PageAssistBlockV1::Steps {
            steps: vec![row.clone()],
        },
    );
    if !ports.tabs.navigate(&reply.profile, tab, url) {
        row.state = PageAssistStepStateV1::Failed;
        row.reason = Some(PageAssistStepReasonV1::Refused);
        reply.set_block(block, PageAssistBlockV1::Steps { steps: vec![row] });
        return Err(format!(
            "{host} could not be opened in this tab; try new_tab."
        ));
    }
    let started = tokio::time::Instant::now();
    while started.elapsed() < OPEN_TIMEOUT {
        match ports.page.facts(tab) {
            Some(facts) if Some(facts.epoch) != before && !facts.loading => break,
            None => break,
            _ => tokio::time::sleep(Duration::from_millis(150)).await,
        }
    }
    row.state = PageAssistStepStateV1::Done;
    reply.set_block(block, PageAssistBlockV1::Steps { steps: vec![row] });
    read_after_open(reply, tab, &host, "this tab").await
}

/// Opens `url` in a new tab in front; the reply works there from now on.
async fn open_in_front(
    reply: &Reply,
    text: &mut Text,
    ports: &super::Ports,
    url: &str,
) -> ToolOutcome {
    reply.phase(Some(PageAssistPhaseV1::Opening));
    let after = reply
        .with_data(|data| {
            data.working
                .clone()
                .or_else(|| data.handles.first().cloned())
        })
        .flatten();
    let host = host_of(url);
    let mut row = PageAssistOpenedV1 {
        index: 0,
        host: host.clone(),
        url: url.to_owned(),
        tab: None,
        state: PageAssistOpenStateV1::Opening,
    };
    let block = reply.add_block(
        text,
        PageAssistBlockV1::Opened {
            tabs: vec![row.clone()],
        },
    );
    let Some(tab) = ports
        .tabs
        .open(&reply.profile, &reply.space, url, after.as_deref(), true)
        .await
    else {
        row.state = PageAssistOpenStateV1::Failed;
        reply.set_block(block, PageAssistBlockV1::Opened { tabs: vec![row] });
        return Err(format!("{host} could not be opened."));
    };
    row.tab = Some(tab.clone());
    reply.set_block(
        block,
        PageAssistBlockV1::Opened {
            tabs: vec![row.clone()],
        },
    );
    settled(ports, &tab).await;
    row.state = PageAssistOpenStateV1::Opened;
    reply.set_block(block, PageAssistBlockV1::Opened { tabs: vec![row] });
    reply.with_data(|data| data.working = Some(tab.clone()));
    read_after_open(reply, &tab, &host, "a new tab in front").await
}

async fn read_after_open(reply: &Reply, tab: &str, host: &str, place: &str) -> ToolOutcome {
    let handle = reply.handle(tab);
    let mut read = reply.read_tab(tab).await;
    if matches!(read, Err(PageAssistFaultV1::Loading)) {
        tokio::time::sleep(Duration::from_millis(800)).await;
        read = reply.read_tab(tab).await;
    }
    reply.phase(Some(PageAssistPhaseV1::Thinking));
    match read {
        Ok(index) => Ok(format!("Opened {host} in {place} ({handle}).\n{}", reply.page_block(index))),
        Err(PageAssistFaultV1::Sensitive) => Ok(format!(
            "Opened {host} in {place} ({handle}). It is a sensitive site and is never read; the person continues there."
        )),
        Err(_) => Ok(format!(
            "Opened {host} in {place} ({handle}), but it cannot be read yet; try read_page {handle} once."
        )),
    }
}

async fn inspect_style(reply: &Reply, page: &str) -> ToolOutcome {
    let (index, _) = reply
        .record(page)
        .ok_or_else(|| format!("{page} is not a page read in this reply."))?;
    match reply.outline(index).await {
        Some(outline) => Ok(prompt::outline_block(&outline)),
        None => Ok("No outline is available; use plain tags.".to_owned()),
    }
}

async fn restyle(
    reply: &Reply,
    text: &mut Text,
    page: &str,
    summary: String,
    sheet: &str,
) -> ToolOutcome {
    let (index, record) = reply
        .record(page)
        .ok_or_else(|| format!("{page} is not a page read in this reply."))?;
    let site = record.page.site.clone();
    let summary = text::bounded(&summary, PAGE_ASSIST_MAX_EXCERPT_CHARS);
    let proposal = |state, rules, id| PageAssistBlockV1::Style {
        style: PageStyleProposalV1 {
            id,
            summary: summary.clone(),
            site: site.clone(),
            rules,
            state,
        },
    };
    if !reply.service.site_has_room(&reply.profile, &site, 0) {
        reply.add_block(text, proposal(PageStyleStateV1::Full, 0, None));
        return Ok("This site already holds as many styles as it can; the person can remove one in Settings.".to_owned());
    }
    reply.phase(Some(PageAssistPhaseV1::Styling));
    let outline = reply.outline(index).await;
    let validated = match css::validate(sheet) {
        Ok(validated) => validated,
        Err(rejection) => {
            reply.note(format_args!(
                "restyle rejected={rejection:?} bytes={}",
                sheet.len()
            ));
            return Err(format!(
                "The stylesheet was refused ({rejection:?}). Write it again within the rules."
            ));
        }
    };
    let Some(guarded) = guard::guard(&validated, outline.as_ref()) else {
        return Err(
            "Nothing in the stylesheet passed the rules. Write it again within the rules."
                .to_owned(),
        );
    };
    if !reply
        .service
        .site_has_room(&reply.profile, &site, guarded.css.len())
    {
        reply.add_block(text, proposal(PageStyleStateV1::Full, guarded.rules, None));
        return Ok("The stylesheet does not fit beside this site's other styles.".to_owned());
    }
    let rules = guarded.rules;
    let block = reply.add_block(text, proposal(PageStyleStateV1::Proposed, rules, None));
    reply.with_data(|data| {
        data.styles.insert(
            block,
            PendingStyle {
                profile: reply.profile,
                site: site.clone(),
                summary: summary.clone(),
                css: guarded.css,
                rules,
            },
        )
    });
    reply.phase(Some(PageAssistPhaseV1::Thinking));
    Ok(format!("Proposed “{summary}” for {site}: {rules} rules passed. The person applies it from the card."))
}

fn transcript(reply: &Reply) -> String {
    let messages = {
        let state = reply.service.state();
        state
            .live
            .get(&reply.conversation)
            .map(|live| live.conversation.messages().to_vec())
            .unwrap_or_default()
    };
    let mut lines: Vec<String> = prompt::history(&messages)
        .into_iter()
        .map(|exchange| format!("Person: {}\nZephium: {}", exchange.person, exchange.reply))
        .collect();
    lines.push(format!(
        "Person: {}",
        prompt::request(&reply.words, reply.suggestion)
    ));
    let joined = lines.join("\n\n");
    let skip = joined.chars().count().saturating_sub(MAX_TRANSCRIPT_CHARS);
    joined.chars().skip(skip).collect()
}

async fn hand_off(
    reply: &Reply,
    text: &mut Text,
    title: String,
    summary: String,
    bring: BringRequest,
) -> ToolOutcome {
    reply.phase(Some(PageAssistPhaseV1::Preparing));
    let mut tabs: Vec<String> = bring
        .tabs
        .iter()
        .filter_map(|handle| reply.tab_of(handle))
        .collect();
    for attachment in &reply.attachments {
        if let Admitted::Page { tab } = attachment {
            if !tabs.contains(tab) {
                tabs.insert(0, tab.clone());
            }
        }
    }
    let request = BringRequest { tabs, ..bring };
    let brings = reply
        .service
        .ports()
        .work
        .prepare(&reply.profile, &reply.space, request)
        .await;
    let objective = format!(
        "{title}\n\n{summary}\n\nFrom the sidebar conversation:\n{}",
        transcript(reply)
    );
    let block = reply.add_block(
        text,
        PageAssistBlockV1::Handoff {
            handoff: PageAssistHandoffV1 {
                title: title.clone(),
                summary: summary.clone(),
                brings: brings.clone(),
                state: PageAssistHandoffStateV1::Offered,
            },
        },
    );
    reply.with_data(|data| {
        data.handoffs.insert(
            block,
            WorkStart {
                title,
                objective,
                brings,
            },
        )
    });
    Ok(
        "Offered to the person as a Work card. Stop here: write at most one short sentence."
            .to_owned(),
    )
}

// -------------------------------------------------------------------- steps

fn category(effect: StepEffect) -> Option<WorkConfirmCategoryV1> {
    match effect {
        StepEffect::Commit(category) => Some(category),
        StepEffect::Read | StepEffect::Draft => None,
    }
}

/// Refuses, from the step's own read, steps the engine must never run:
/// secrets, protected fields and links away from the site are refused, anything a
/// verb cannot use is unsuitable.
fn admissible(record: &ReadRecord, step: &StepRequest) -> Result<(), StepFailure> {
    if step.verb == Verb::Fill
        && step
            .value
            .as_deref()
            .is_some_and(zephium_agentic::contains_recognized_secret)
    {
        return Err(StepFailure::Refused);
    }
    let Some(target) = step.target else {
        return if step.verb == Verb::Scroll {
            Ok(())
        } else {
            Err(StepFailure::Unsuitable)
        };
    };
    let Some(passage) = record
        .readout
        .passages
        .iter()
        .find(|passage| passage.id == target)
    else {
        return Err(StepFailure::Unsuitable);
    };
    if matches!(passage.role, Role::Field(kind) if kind.protected()) {
        return Err(StepFailure::Refused);
    }
    if matches!(
        (step.verb, passage.role),
        (Verb::Open | Verb::Click, Role::Link { same_site: false })
    ) {
        return Err(StepFailure::Refused);
    }
    let fits = match (step.verb, passage.role) {
        (Verb::Fill, Role::Field(_)) => true,
        (Verb::Fill, _) => false,
        (Verb::Submit, Role::Field(_)) => true,
        (Verb::Submit, _) => false,
        (Verb::Open, Role::Link { same_site }) => same_site,
        (Verb::Open, _) => false,
        (Verb::Select, Role::Select) => true,
        (Verb::Select, _) => false,
        (Verb::Check | Verb::Uncheck, Role::Checkbox { .. }) => true,
        (Verb::Check | Verb::Uncheck, _) => false,
        (Verb::Click, Role::Link { same_site }) => same_site,
        (Verb::Click | Verb::Expand, Role::Button | Role::Checkbox { .. } | Role::Other) => true,
        (Verb::Click | Verb::Expand, _) => false,
        (Verb::Scroll, _) => true,
    };
    if fits {
        Ok(())
    } else {
        Err(StepFailure::Unsuitable)
    }
}

struct Plan {
    tab: String,
    epoch: u64,
    /// A step's effect was a new document.
    moved: bool,
    record: ReadRecord,
    /// The page as last read: the plan's read, then each step's effect read.
    latest: Readout,
    /// Where the document was seen changing on its own before the steps.
    volatile: effect::Volatile,
    /// Two reads of the document with no step between them were compared.
    paired: bool,
    requests: Vec<StepRequest>,
    rows: Vec<PageAssistStepV1>,
    /// The engine's label once a step resolved, to recognise it after a pause.
    resolved: Vec<Option<String>>,
    block: u16,
}

impl Plan {
    fn show(&self, reply: &Reply) {
        reply.set_block(
            self.block,
            PageAssistBlockV1::Steps {
                steps: self.rows.clone(),
            },
        );
    }

    fn fail(&mut self, reply: &Reply, index: usize, failure: StepFailure) {
        self.fail_with(
            reply,
            index,
            match failure {
                StepFailure::Refused => PageAssistStepReasonV1::Refused,
                StepFailure::Unsuitable => PageAssistStepReasonV1::Unsuitable,
                StepFailure::PageChanged => PageAssistStepReasonV1::PageChanged,
                StepFailure::Failed => PageAssistStepReasonV1::Unconfirmed,
            },
        );
    }

    fn fail_with(&mut self, reply: &Reply, index: usize, reason: PageAssistStepReasonV1) {
        if let Some(row) = self.rows.get_mut(index) {
            row.state = PageAssistStepStateV1::Failed;
            row.reason = Some(reason);
        }
        self.skip_from(reply, index + 1);
    }

    fn mark(
        &mut self,
        reply: &Reply,
        index: usize,
        label: &str,
        effect: StepEffect,
        state: PageAssistStepStateV1,
    ) {
        if let Some(slot) = self.resolved.get_mut(index) {
            *slot = Some(label.to_owned());
        }
        if let Some(row) = self.rows.get_mut(index) {
            if !label.is_empty() {
                row.label = text::bounded(&text::clean(label), 120);
                let sites = reply.other_sites(&self.record.page.site);
                let unknown = reply
                    .with_data(|data| data.unknown_read_sites)
                    .unwrap_or(true);
                if let Some(value) = self.requests.get(index).and_then(|step| {
                    (step.verb == Verb::Fill && (!sites.is_empty() || unknown))
                        .then_some(step.value.as_deref())
                        .flatten()
                }) {
                    let sources = if sites.is_empty() {
                        String::new()
                    } else {
                        format!("{} → ", sites.join(", "))
                    };
                    row.label = format!(
                        "{value:?} → {}\n{sources}{}",
                        row.label, self.record.page.site
                    );
                }
            }
            if category(effect).is_some() {
                row.category = category(effect);
            }
            row.state = state;
        }
        self.show(reply);
    }

    fn skip_from(&mut self, reply: &Reply, from: usize) {
        for row in self.rows.iter_mut().skip(from) {
            row.state = PageAssistStepStateV1::Skipped;
        }
        self.show(reply);
    }

    fn summary(&self) -> String {
        let rows: Vec<String> = self
            .rows
            .iter()
            .map(|row| {
                let state = match (row.state, row.reason) {
                    (PageAssistStepStateV1::Failed, Some(PageAssistStepReasonV1::Refused)) => {
                        "refused: a secret, a protected field or a link to another site"
                    }
                    (PageAssistStepStateV1::Failed, Some(PageAssistStepReasonV1::Unsuitable)) => {
                        "failed: that passage is not a control this verb can use; act on the control's own passage (a button, field, checkbox, select or same-site link)"
                    }
                    (PageAssistStepStateV1::Failed, Some(PageAssistStepReasonV1::PageChanged)) => {
                        "failed: the page changed before it ran; use the references of the page as it is now"
                    }
                    (PageAssistStepStateV1::Failed, Some(PageAssistStepReasonV1::NoEffect)) => {
                        "ran, but nothing on the page changed: it did not take"
                    }
                    (PageAssistStepStateV1::Failed, _) => {
                        "ran, but whether it took could not be confirmed"
                    }
                    (PageAssistStepStateV1::Done, _) => "done",
                    (PageAssistStepStateV1::Declined, _) => "declined by the person",
                    (PageAssistStepStateV1::Skipped, _) => "skipped",
                    (PageAssistStepStateV1::Running | PageAssistStepStateV1::Confirm, _) => {
                        "stopped"
                    }
                };
                format!("{} — {state}", row.label)
            })
            .collect();
        rows.join("\n")
    }
}

/// A step's row before the engine judged it, named from the read.
fn planned_label(step: &Step, record: &ReadRecord) -> String {
    let name = step
        .target
        .and_then(|target| {
            record
                .readout
                .passages
                .iter()
                .find(|passage| passage.id == target.passage)
        })
        .map(|passage| text::bounded(&text::clean(&passage.text), 48))
        .filter(|name| !name.is_empty());
    let value = step
        .value
        .as_deref()
        .map(|value| text::bounded(&text::clean(value), 40));
    let quoted = |name: &str| format!("“{name}”");
    match (step.verb, name.as_deref(), value.as_deref()) {
        (Verb::Fill, Some(name), Some(value)) => {
            format!("Type {} into {}", quoted(value), quoted(name))
        }
        (Verb::Fill, None, Some(value)) => format!("Type {}", quoted(value)),
        (Verb::Select, Some(name), Some(value)) => {
            format!("Choose {} in {}", quoted(value), quoted(name))
        }
        (Verb::Submit, Some(name), _) => format!("Press Return in {}", quoted(name)),
        (Verb::Submit, None, _) => "Press Return".to_owned(),
        (Verb::Scroll, None, _) => match step.direction {
            Some(zephium_page_assist::actions::Direction::Up) => "Scroll up".to_owned(),
            _ => "Scroll down".to_owned(),
        },
        (verb, name, _) => {
            let word = match verb {
                Verb::Click => "Click",
                Verb::Fill => "Type",
                Verb::Select => "Choose",
                Verb::Check => "Check",
                Verb::Uncheck => "Uncheck",
                Verb::Scroll => "Scroll to",
                Verb::Expand => "Expand",
                Verb::Open => "Open",
                Verb::Submit => "Press Return",
            };
            match name {
                Some(name) => format!("{word} {}", quoted(name)),
                None => word.to_owned(),
            }
        }
    }
}

async fn decision(reply: &Reply, block: u16) -> Result<PageAssistDecisionV1, CancelReason> {
    let (sender, receiver) = oneshot::channel();
    reply
        .service
        .running_reply(&reply.conversation, reply.reply, |running| {
            running.decision = Some((block, sender))
        });
    reply.apply(PageAssistEventV1::State {
        state: PageAssistReplyStateV1::Waiting,
    });
    let decided = tokio::select! {
        reason = reply.cancel.cancelled() => Err(reason),
        () = reply.redirect.requested() => Err(CancelReason::Redirected),
        decision = receiver => decision.map_err(|_| CancelReason::Gone),
    };
    if decided.is_err() {
        reply
            .service
            .running_reply(&reply.conversation, reply.reply, |running| {
                running.decision = None
            });
    }
    reply.apply(PageAssistEventV1::State {
        state: PageAssistReplyStateV1::Working,
    });
    decided
}

async fn act(
    reply: &Reply,
    text: &mut Text,
    steps: Vec<Step>,
    finish: bool,
) -> Result<ToolOutcome, End> {
    let first = steps
        .iter()
        .find_map(|step| step.target)
        .map(|target| target.read)
        .unwrap_or(0);
    if steps
        .iter()
        .any(|step| step.target.is_some_and(|target| target.read != first))
    {
        return Ok(Err(
            "All steps of one act call must be on one page.".to_owned()
        ));
    }
    let Some(record) = reply
        .with_data(|data| data.reads.get(usize::from(first)).cloned())
        .flatten()
    else {
        return Ok(Err("Read the page before acting on it.".to_owned()));
    };
    let repeated = reply
        .with_data(|data| {
            steps.iter().any(|step| {
                let control = step
                    .target
                    .and_then(|target| Control::of(&record.readout, target.passage));
                data.untaken.iter().any(|(tab, epoch, verb, untaken)| {
                    *tab == record.tab
                        && *epoch == record.epoch
                        && *verb == step.verb
                        && control.as_ref() == Some(untaken)
                })
            })
        })
        .unwrap_or(false);
    if repeated {
        return Ok(Err(
            "That step already ran in this reply and did not take; running it \
             again could do it twice. Do not repeat it: tell the person it did not work."
                .to_owned(),
        ));
    }
    let requests: Vec<StepRequest> = steps
        .iter()
        .map(|step| StepRequest {
            verb: step.verb,
            read: record.readout.read,
            target: step.target.map(|target| target.passage),
            direction: step.direction,
            value: step.value.clone(),
        })
        .collect();
    let last = requests
        .iter()
        .position(|step| step.verb == Verb::Open)
        .map_or(requests.len(), |open| open + 1);
    let rows: Vec<PageAssistStepV1> = steps
        .iter()
        .take(last)
        .enumerate()
        .map(|(index, step)| PageAssistStepV1 {
            index: u8::try_from(index).unwrap_or(u8::MAX),
            label: planned_label(step, &record),
            category: None,
            state: PageAssistStepStateV1::Running,
            reason: None,
        })
        .collect();
    let block = reply.add_block(
        text,
        PageAssistBlockV1::Steps {
            steps: rows.clone(),
        },
    );
    let (volatile, paired) = reply
        .with_data(|data| volatile(&data.reads, &record))
        .unwrap_or_default();
    let mut plan = Plan {
        tab: record.tab.clone(),
        epoch: record.epoch,
        moved: false,
        latest: record.readout.clone(),
        volatile,
        paired,
        record,
        requests,
        rows,
        resolved: Vec::new(),
        block,
    };
    plan.requests.truncate(last);
    plan.resolved = vec![None; plan.requests.len()];
    reply
        .service
        .running_reply(&reply.conversation, reply.reply, |running| {
            running.acting = Some(plan.tab.clone())
        });
    reply.leased.store(true, Ordering::Release);
    reply.phase(Some(PageAssistPhaseV1::Acting));
    reply.with_data(|data| data.working = Some(plan.tab.clone()));
    reply.glow(&plan.tab, Glow::Working);
    let result = run_steps(reply, text, &mut plan).await;
    reply.leased.store(false, Ordering::Release);
    reply
        .service
        .running_reply(&reply.conversation, reply.reply, |running| {
            running.acting = None
        });
    reply.phase(Some(PageAssistPhaseV1::Thinking));
    match result {
        Ok(()) if finish => {
            reply.with_data(|data| data.finished = true);
            Ok(Ok(plan.summary()))
        }
        Ok(()) => Ok(Ok(format!(
            "{}{}",
            plan.summary(),
            after_steps(reply, &plan).await
        ))),
        Err(Some(end)) => Err(end),
        Err(None) => {
            let stale = plan.rows.iter().any(|row| {
                matches!(
                    row.reason,
                    Some(PageAssistStepReasonV1::PageChanged | PageAssistStepReasonV1::Unsuitable)
                )
            });
            let fresh = if stale || plan.moved {
                after_steps(reply, &plan).await
            } else {
                String::new()
            };
            Ok(Err(format!("{}{fresh}", plan.summary())))
        }
    }
}

/// Where `record`'s document changed between reads of it this reply took
/// with no step between them, and whether any two such reads exist. Reads
/// only what the reply already has.
fn volatile(reads: &[ReadRecord], record: &ReadRecord) -> (effect::Volatile, bool) {
    let mut volatile = effect::Volatile::default();
    let mut paired = false;
    let same: Vec<&ReadRecord> = reads
        .iter()
        .filter(|read| read.tab == record.tab && read.epoch == record.epoch)
        .collect();
    for pair in same.windows(2) {
        if pair[0].acted == pair[1].acted {
            volatile.learn(&pair[0].readout, &pair[1].readout);
            paired = true;
        }
    }
    (volatile, paired)
}

/// One read just before a press whose planning read has no step-free
/// partner. No step ran since the planning read, so the two pair across the
/// model's thinking time and show what the page changes on its own. False
/// when the control the step names is gone from the page or renamed; a
/// failed, late or stale read changes nothing.
async fn read_before(reply: &Reply, plan: &mut Plan, step: &StepRequest) -> bool {
    let page = reply.service.ports().page.clone();
    let Ok(Ok(now)) = tokio::time::timeout(READ_TIMEOUT, page.read(&plan.tab, plan.epoch)).await
    else {
        return true;
    };
    if now.epoch != plan.epoch {
        return true;
    }
    plan.paired = true;
    let control = step
        .target
        .and_then(|target| Control::of(&plan.record.readout, target));
    if control.is_some_and(|control| control.find(&now).is_none()) {
        return false;
    }
    plan.volatile.learn(&plan.record.readout, &now);
    plan.latest = now;
    true
}

/// The page after an act, so the model need not read it again: the whole
/// new document when it moved on, else only the passages that changed.
async fn after_steps(reply: &Reply, plan: &Plan) -> String {
    let stale = plan.rows.iter().any(|row| {
        matches!(
            row.reason,
            Some(PageAssistStepReasonV1::PageChanged | PageAssistStepReasonV1::Unsuitable)
        )
    });
    if !plan.moved && !stale && plan.latest.passages == plan.record.readout.passages {
        return String::new();
    }
    let Ok(index) = reply.read_tab(&plan.tab).await else {
        return String::new();
    };
    let Some((letters, read)) = reply
        .with_data(|data| {
            let read = data.reads.get(index)?.clone();
            Some((refs::letters(u16::try_from(index).ok()?)?, read))
        })
        .flatten()
    else {
        return String::new();
    };
    if plan.moved || stale || read.epoch != plan.epoch {
        return format!("\n\nThe page now:\n{}", reply.page_block(index));
    }
    let body = |line: &str| {
        line.split_once(' ')
            .map_or(String::new(), |(_, rest)| rest.to_owned())
    };
    let mut before: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for line in plan.record.page.payload.lines() {
        *before.entry(body(line)).or_default() += 1;
    }
    let changed: Vec<&str> = read
        .page
        .payload
        .lines()
        .filter(|line| match before.get_mut(&body(line)) {
            Some(count) if *count > 0 => {
                *count -= 1;
                false
            }
            _ => true,
        })
        .collect();
    if changed.is_empty() {
        return String::new();
    }
    if changed.len() > FIND_LINES {
        return format!("\n\nThe page now:\n{}", reply.page_block(index));
    }
    format!(
        "\n\nChanged on the page, read again as {letters}; other passages are as before:\n<<<PAGE {letters}\nhost: {}\n{}\nPAGE>>>",
        prompt::fence(&read.page.host),
        prompt::fence(&changed.join("\n"))
    )
}

/// Whether `hay` holds `word` on its own, not inside a longer word or number.
fn holds_word(hay: &str, word: &str) -> bool {
    hay.match_indices(word).any(|(at, _)| {
        let before = hay[..at].chars().next_back();
        let after = hay[at + word.len()..].chars().next();
        !before.is_some_and(char::is_alphanumeric) && !after.is_some_and(char::is_alphanumeric)
    })
}

/// The lines of read `index` holding every one of `words`, with their
/// neighbours, under the read's letters.
fn found(reply: &Reply, index: usize, words: &[String]) -> Option<String> {
    let (letters, read) = reply
        .with_data(|data| {
            let read = data.reads.get(index)?.clone();
            Some((refs::letters(u16::try_from(index).ok()?)?, read))
        })
        .flatten()?;
    let lines: Vec<&str> = read.page.payload.lines().collect();
    let hits: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| {
            let line = line.to_lowercase();
            let text = line.split_once(' ').map_or("", |(_, rest)| rest);
            words.iter().all(|word| holds_word(text, word))
        })
        .map(|(at, _)| at)
        .collect();
    if hits.is_empty() {
        return None;
    }
    let mut keep: Vec<usize> = hits
        .iter()
        .flat_map(|&at| at.saturating_sub(FIND_AROUND)..=(at + FIND_AROUND).min(lines.len() - 1))
        .collect();
    keep.sort_unstable();
    keep.dedup();
    keep.truncate(FIND_LINES);
    let shown: Vec<&str> = keep.iter().map(|&at| lines[at]).collect();
    Some(format!(
        "{} found on page {letters}:\n<<<PAGE {letters}\nhost: {}\n{}\nPAGE>>>",
        hits.len(),
        prompt::fence(&read.page.host),
        prompt::fence(&shown.join("\n"))
    ))
}

/// Looks for `words` on a page read in this reply, scrolling further down a
/// view at a time, bounded, when the latest read of it does not hold them.
async fn find(reply: &Reply, page: &str, words: &str) -> ToolOutcome {
    let Some((first, record)) = reply.record(page.trim()) else {
        return Err(format!(
            "{page} is not a page you read. Use a page's letter, such as a."
        ));
    };
    let wanted: Vec<String> = words.split_whitespace().map(str::to_lowercase).collect();
    if wanted.is_empty() {
        return Err("Give the words to look for.".to_owned());
    }
    let mut at = reply
        .with_data(|data| {
            data.reads
                .iter()
                .rposition(|read| read.tab == record.tab && read.epoch == record.epoch)
        })
        .flatten()
        .unwrap_or(first);
    let page_port = reply.service.ports().page.clone();
    for scrolled in 0..=FIND_SCROLLS {
        if let Some(found) = found(reply, at, &wanted) {
            return Ok(found);
        }
        if scrolled == FIND_SCROLLS {
            break;
        }
        let Some(current) = reply
            .with_data(|data| data.reads.get(at).cloned())
            .flatten()
        else {
            break;
        };
        let step = StepRequest {
            verb: Verb::Scroll,
            read: current.readout.read,
            target: None,
            direction: Some(zephium_page_assist::actions::Direction::Down),
            value: None,
        };
        if page_port
            .resolve(&current.tab, current.epoch, &step)
            .is_err()
        {
            break;
        }
        reply.glow(&current.tab, Glow::Working);
        reply.with_data(|data| data.acted += 1);
        if !matches!(
            tokio::time::timeout(
                READ_TIMEOUT,
                page_port.act(&current.tab, current.epoch, &step)
            )
            .await,
            Ok(Ok(()))
        ) {
            break;
        }
        tokio::time::sleep(SCROLL_SETTLE).await;
        let Ok(next) = reply.read_tab(&current.tab).await else {
            break;
        };
        let same = reply
            .with_data(|data| {
                data.reads.get(next).map(|read| {
                    read.epoch != current.epoch
                        || read
                            .readout
                            .passages
                            .iter()
                            .map(|p| (p.role, &p.text))
                            .eq(current.readout.passages.iter().map(|p| (p.role, &p.text)))
                })
            })
            .flatten()
            .unwrap_or(true);
        at = next;
        if same {
            if let Some(found) = found(reply, at, &wanted) {
                return Ok(found);
            }
            break;
        }
    }
    Err(format!(
        "“{}” is not on that page, as far down as it was read. Try other words, or tell the person it is not there.",
        text::bounded(words, 80)
    ))
}

/// `Err(None)` ends the steps and tells the model; `Err(Some)` ends the reply.
async fn run_steps(reply: &Reply, text: &mut Text, plan: &mut Plan) -> Result<(), Option<End>> {
    let page = reply.service.ports().page.clone();
    let mut index = 0;
    while index < plan.requests.len() {
        if reply.redirect.asked() {
            plan.skip_from(reply, index);
            return Err(Some(End::Cancelled(CancelReason::Redirected)));
        }
        if reply.took_over.swap(false, Ordering::AcqRel) {
            take_over(reply, text, plan, index).await?;
            continue;
        }
        let step = plan.requests[index].clone();
        let resolved = admissible(&plan.record, &step)
            .and_then(|()| page.resolve(&plan.tab, plan.epoch, &step));
        let mut resolved = match resolved {
            Ok(resolved) => resolved,
            Err(failure) => {
                plan.fail(reply, index, failure);
                return Err(None);
            }
        };
        if step.verb == Verb::Fill
            && (!reply.other_sites(&plan.record.page.site).is_empty()
                || reply
                    .with_data(|data| data.unknown_read_sites)
                    .unwrap_or(true))
        {
            resolved.effect = StepEffect::Commit(WorkConfirmCategoryV1::Type);
        }
        if let StepEffect::Commit(_) = resolved.effect {
            plan.mark(
                reply,
                index,
                &resolved.label,
                resolved.effect,
                PageAssistStepStateV1::Confirm,
            );
            match decision(reply, plan.block).await {
                Ok(PageAssistDecisionV1::Approve) => {}
                Ok(PageAssistDecisionV1::Decline) => {
                    plan.mark(
                        reply,
                        index,
                        &resolved.label,
                        resolved.effect,
                        PageAssistStepStateV1::Declined,
                    );
                    plan.skip_from(reply, index + 1);
                    return Err(None);
                }
                Err(CancelReason::Redirected) => {
                    plan.mark(
                        reply,
                        index,
                        &resolved.label,
                        resolved.effect,
                        PageAssistStepStateV1::Declined,
                    );
                    plan.skip_from(reply, index + 1);
                    return Err(Some(End::Cancelled(CancelReason::Redirected)));
                }
                Err(reason) => return Err(Some(End::Cancelled(reason))),
            }
        }
        if reply.took_over.load(Ordering::Acquire) {
            continue;
        }
        if reply.redirect.asked() {
            plan.skip_from(reply, index);
            return Err(Some(End::Cancelled(CancelReason::Redirected)));
        }
        let unpaired = presses(step.verb)
            && !plan.paired
            && reply.with_data(|data| data.acted) == Some(plan.record.acted);
        if unpaired && !read_before(reply, plan, &step).await {
            // The plan no longer matches the page: as when the control is
            // gone at dispatch, it never runs on the old plan.
            plan.fail(reply, index, StepFailure::Unsuitable);
            return Err(None);
        }
        plan.mark(
            reply,
            index,
            &resolved.label,
            resolved.effect,
            PageAssistStepStateV1::Running,
        );
        reply.with_data(|data| data.acted += 1);
        if let Err(failure) = page.act(&plan.tab, plan.epoch, &step).await {
            plan.fail(reply, index, failure);
            return Err(None);
        }
        let settled = observe(reply, plan, &step).await;
        let navigated = settled == Settled::Navigated;
        plan.moved |= navigated;
        let reason = match settled {
            Settled::Took(after) => {
                if let Some(after) = after {
                    plan.latest = after;
                }
                None
            }
            Settled::Navigated if presses(step.verb) => None,
            Settled::NoEffect => Some(PageAssistStepReasonV1::NoEffect),
            Settled::Navigated | Settled::Unconfirmed => Some(PageAssistStepReasonV1::Unconfirmed),
        };
        if let Some(reason) = reason {
            if let Some(control) = step
                .target
                .and_then(|target| Control::of(&plan.record.readout, target))
            {
                let untaken = (plan.tab.clone(), plan.epoch, step.verb, control);
                reply.with_data(|data| data.untaken.push(untaken));
            }
            plan.fail_with(reply, index, reason);
            return Err(None);
        }
        plan.mark(
            reply,
            index,
            &resolved.label,
            resolved.effect,
            PageAssistStepStateV1::Done,
        );
        index += 1;
        if navigated && index < plan.requests.len() {
            plan.skip_from(reply, index);
            return Err(None);
        }
    }
    Ok(())
}

/// Steps whose effect may be the document itself moving on.
fn presses(verb: Verb) -> bool {
    matches!(verb, Verb::Click | Verb::Open | Verb::Submit | Verb::Expand)
}

#[derive(Debug, PartialEq)]
enum Settled {
    /// Its effect is on the page; the read that shows it, when one did.
    Took(Option<Readout>),
    /// The tab committed a new document.
    Navigated,
    /// The page read the same after it.
    NoEffect,
    /// It could not be told either way.
    Unconfirmed,
}

/// Watches the page after one step ran, bounded, and never runs it again: a
/// new document, or reads of the same one against the read before it. A
/// quiet page is judged on the first reads; one that was busy is read on to
/// the same-document cap, and is never called unchanged while it was busy.
/// Busy is timing evidence only: it decides how long to look, not what was
/// seen.
async fn observe(reply: &Reply, plan: &Plan, step: &StepRequest) -> Settled {
    if step.verb == Verb::Scroll {
        return Settled::Took(None);
    }
    let Some(control) = step
        .target
        .and_then(|target| Control::of(&plan.record.readout, target))
    else {
        return Settled::Unconfirmed;
    };
    let page = reply.service.ports().page.clone();
    let started = tokio::time::Instant::now();
    let mut reads = 0;
    let mut last: Option<Observed> = None;
    let mut previous: Option<Readout> = None;
    let mut busy = false;
    while let Some(facts) = page.facts(&plan.tab) {
        if facts.epoch != plan.epoch {
            return Settled::Navigated;
        }
        let left = EFFECT_NAVIGATION.saturating_sub(started.elapsed());
        if left.is_zero() {
            break;
        }
        busy |= facts.loading;
        if !facts.loading {
            let due = EFFECT_READS_MS.get(reads).or_else(|| {
                busy.then(|| EFFECT_BUSY_READS_MS.get(reads - EFFECT_READS_MS.len()))
                    .flatten()
            });
            let Some(due) = due else {
                break;
            };
            if started.elapsed() >= Duration::from_millis(*due) {
                reads += 1;
                if let Ok(Ok(after)) =
                    tokio::time::timeout(left.min(READ_TIMEOUT), page.read(&plan.tab, plan.epoch))
                        .await
                {
                    if after.epoch == plan.epoch {
                        let observed = effect::judge(
                            step.verb,
                            step.value.as_deref(),
                            &control,
                            &plan.latest,
                            &after,
                            &plan.volatile,
                        );
                        if observed == Observed::Took {
                            return Settled::Took(Some(after));
                        }
                        busy |= effect::changed(previous.as_ref().unwrap_or(&plan.latest), &after);
                        last = Some(observed);
                        previous = Some(after);
                    }
                }
                continue;
            }
        }
        tokio::time::sleep(EFFECT_POLL).await;
    }
    match last {
        Some(Observed::Nothing) if !busy => Settled::NoEffect,
        _ => Settled::Unconfirmed,
    }
}

/// The person used the page: wait for Continue, then read it again and go
/// on only if every remaining step still names the same control.
async fn take_over(
    reply: &Reply,
    text: &mut Text,
    plan: &mut Plan,
    from: usize,
) -> Result<(), Option<End>> {
    reply.add_block(
        text,
        PageAssistBlockV1::Paused {
            reason: PageAssistPauseV1::TookOver,
        },
    );
    reply.apply(PageAssistEventV1::State {
        state: PageAssistReplyStateV1::Paused,
    });
    reply.glow(&plan.tab, Glow::Paused);
    let (sender, receiver) = oneshot::channel();
    reply
        .service
        .running_reply(&reply.conversation, reply.reply, |running| {
            running.resume = Some(sender)
        });
    tokio::select! {
        reason = reply.cancel.cancelled() => return Err(Some(End::Cancelled(reason))),
        () = reply.redirect.requested() => {
            return Err(Some(End::Cancelled(CancelReason::Redirected)));
        }
        resumed = receiver => resumed.map_err(|_| Some(End::Cancelled(CancelReason::Gone)))?,
    }
    reply.glow(&plan.tab, Glow::Working);
    reply.apply(PageAssistEventV1::State {
        state: PageAssistReplyStateV1::Working,
    });
    let labels: Vec<Option<String>> = plan.resolved.iter().skip(from).cloned().collect();
    let index = reply.read_tab(&plan.tab).await.map_err(|_| None)?;
    let Some(fresh) = reply
        .with_data(|data| data.reads.get(index).cloned())
        .flatten()
    else {
        return Err(None);
    };
    reply.phase(Some(PageAssistPhaseV1::Acting));
    for step in plan.requests.iter_mut().skip(from) {
        step.read = fresh.readout.read;
    }
    if fresh.epoch != plan.epoch {
        plan.volatile = effect::Volatile::default();
        plan.paired = false;
    }
    plan.epoch = fresh.epoch;
    plan.latest = fresh.readout.clone();
    plan.record = fresh;
    let page = reply.service.ports().page.clone();
    for (step, label) in plan.requests.iter().skip(from).zip(labels) {
        match (page.resolve(&plan.tab, plan.epoch, step), label) {
            (Ok(resolved), Some(label)) if resolved.label == label => {}
            (Ok(_), None) => {}
            _ => return Err(None),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_counters_saturate_across_replies() {
        let usage = WorkModelUsage {
            input_tokens: u64::MAX,
            cached_input_tokens: u64::MAX,
            cache_write_input_tokens: u64::MAX,
            output_tokens: u64::MAX,
            reasoning_tokens: u64::MAX,
            cost_micros: Some(u64::MAX),
        };
        let mut total = usage;
        add_usage(&mut total, &usage);
        assert_eq!(total, usage);
    }
}
