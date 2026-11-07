//! The sidebar assistant: one conversation per Space that follows the
//! person across tabs. Rust owns the conversation, every page read, the model
//! loop and every effect; the panel renders blocks and sends the person's
//! choices. Tabs cross as their `TabView::id`, Spaces as their id.
use serde::{Deserialize, Serialize};
use specta::Type;
pub use zephium_core::work::runtime::{WorkConfirmCategoryV1, WorkModelFaultV1};

/// Longest message the panel may send, in UTF-8 bytes.
pub const PAGE_ASSIST_MAX_QUESTION_BYTES: usize = 8 * 1024;
/// Longest excerpt a citation, selection or picked element carries, in characters.
pub const PAGE_ASSIST_MAX_EXCERPT_CHARS: usize = 80;
/// Most attachments one message carries.
pub const PAGE_ASSIST_MAX_ATTACHMENTS: usize = 6;
/// Citation marks inside a text block: `⟦` the 1-based citation number `⟧`.
pub const PAGE_ASSIST_CITE_OPEN: char = '\u{27E6}';
/// Closes a citation mark.
pub const PAGE_ASSIST_CITE_CLOSE: char = '\u{27E7}';

/// One-click starts Rust offers for the page in front. The panel names them.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, Eq, PartialEq, Type)]
#[serde(rename_all = "snake_case")]
pub enum PageAssistSuggestionV1 {
    Summarize,
    KeyPoints,
    KeyNumbers,
    /// "What's the catch?": fine print, conditions, costs.
    Catch,
    /// Plain-language explanation of a dense page.
    Explain,
    /// The steps a how-to or recipe asks for.
    Steps,
    /// Compare the options a product or pricing page lists.
    Compare,
    /// The few items on a list page worth opening.
    Picks,
    /// A fresh, coherent look for this site.
    Redesign,
    /// A darker version of this site.
    Darker,
    /// Larger, easier-to-read text on this site.
    Readable,
}

/// Whether the page in front can be read now. Anything but `ready` means the
/// page is not attached; the conversation itself still works.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, Eq, PartialEq, Type)]
#[serde(rename_all = "snake_case")]
pub enum PageAssistAvailabilityV1 {
    Ready,
    /// AI is switched off in Settings.
    AiOff,
    /// Private windows never send pages to a model.
    Private,
    /// Not a web page: internal pages, new tab, files, extension pages, PDFs.
    NotWeb,
    /// Banks, payments, health and government sites are never read.
    Sensitive,
    /// No model is set up: no Zephium account and no key.
    NoModel,
    /// The page is still loading; ask again when it settles.
    Loading,
    /// This platform cannot read pages yet.
    Unsupported,
    /// The tab is gone or the service is not running.
    Unavailable,
}

/// The model the conversation uses, as the composer names it.
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq, Type)]
#[serde(deny_unknown_fields)]
pub struct PageAssistModelV1 {
    /// Catalog id, for the model picker.
    pub id: String,
    /// Display name, such as GPT-6 Luna.
    pub name: String,
    /// Zephium AI (credits) rather than the person's own key.
    pub hosted: bool,
    /// The effort it runs at, such as "high"; null when it has none.
    pub effort: Option<String>,
}

/// Why a message could not start or finish. Closed; never provider or page text.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, Eq, PartialEq, Type)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum PageAssistFaultV1 {
    AiOff,
    Private,
    NotWeb,
    Sensitive,
    NoModel,
    Loading,
    Unsupported,
    /// The conversation is still answering.
    Busy,
    /// The message is empty, too long, or names something that is gone.
    Invalid,
    /// Nothing is selected on the page.
    NoSelection,
    /// The page could not be read.
    Unreadable,
    /// The page navigated or closed while the reply needed it.
    PageChanged,
    /// The model answered with nothing usable.
    NoAnswer,
    /// A page step did not take effect as expected.
    StepFailed,
    /// The reply reached its limit of steps or time.
    TooLong,
    /// The model call failed. `retry_after_ms` is set for `rate_limited`
    /// (and hosted limits) when the provider or server said when to retry.
    Model {
        fault: WorkModelFaultV1,
        retry_after_ms: Option<u32>,
    },
    Unavailable,
}

/// What the reply is doing now, for its working line.
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq, Type)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum PageAssistPhaseV1 {
    /// Reading a page; `site` such as news.ycombinator.com.
    Reading {
        site: String,
    },
    Thinking,
    /// Running steps on the page.
    Acting,
    Opening,
    Styling,
    /// Preparing what Work would bring along.
    Preparing,
}

/// The tab in front, as the composer's page chip shows it.
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq, Type)]
#[serde(deny_unknown_fields)]
pub struct PageAssistPageV1 {
    pub tab: String,
    /// Host shown on the chip, such as news.ycombinator.com.
    pub host: String,
    /// The page's title (untrusted, bounded).
    pub title: String,
}

/// Something the person attached to a message, as stored and shown.
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq, Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PageAssistAttachmentV1 {
    /// A page; `tab` may since have closed.
    Page {
        tab: String,
        host: String,
        title: String,
    },
    /// An element the person picked on a page, such as "Story list".
    Element {
        tab: String,
        pick: String,
        label: String,
        host: String,
    },
    /// The start of the text the person selected.
    Selection {
        tab: String,
        excerpt: String,
        host: String,
    },
}

/// What the panel asks Rust to attach; Rust reads and admits each again.
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq, Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PageAssistAttachV1 {
    Page { tab: String },
    Element { tab: String, pick: String },
    Selection { tab: String },
}

/// A picked element, ready to attach. It stays outlined on the page until
/// it is sent, detached or the page navigates.
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq, Type)]
#[serde(deny_unknown_fields)]
pub struct PageAssistPickV1 {
    pub tab: String,
    pub pick: String,
    /// A readable name, such as "Story list" or "Pricing table".
    pub label: String,
    pub host: String,
}

/// `page_assist_pick` result: the picked element, or none when the person
/// cancelled (`fault` null) or picking could not start.
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq, Type)]
#[serde(deny_unknown_fields)]
pub struct PageAssistPickResponseV1 {
    pub pick: Option<PageAssistPickV1>,
    pub fault: Option<PageAssistFaultV1>,
}

/// One numbered source on a page. `excerpt` is the page's own words
/// (≤ 80 characters), never the model's.
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq, Type)]
#[serde(deny_unknown_fields)]
pub struct PageAssistCiteV1 {
    /// 1-based within the reply, as it appears between the cite marks; pass
    /// it to `page_assist_cite`.
    pub n: u8,
    pub excerpt: String,
    /// The tab it is on, to bring it forward before jumping.
    pub tab: String,
    pub host: String,
}

/// One item of an item list: a story, a product, a result. Its number is
/// also the mark beside it on the page while the reply is the latest.
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq, Type)]
#[serde(deny_unknown_fields)]
pub struct PageAssistItemV1 {
    /// 1-based, as the badge on the page shows it.
    pub n: u8,
    pub title: String,
    /// Short facts, such as "#2", "749 points", "541 comments" (≤ 4).
    pub meta: Vec<String>,
    /// Where the item leads, from the page read; null when it has no link.
    pub url: Option<String>,
    /// Its mark is on the page now.
    pub marked: bool,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, Eq, PartialEq, Type)]
#[serde(rename_all = "snake_case")]
pub enum PageAssistOpenStateV1 {
    Queued,
    Opening,
    Opened,
    Failed,
}

/// One tab the reply opens, for the opening checklist.
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq, Type)]
#[serde(deny_unknown_fields)]
pub struct PageAssistOpenedV1 {
    pub index: u8,
    /// Shown as the row's name, such as github.com.
    pub host: String,
    pub url: String,
    /// The new tab once it exists, for its icon and a click to show it.
    pub tab: Option<String>,
    pub state: PageAssistOpenStateV1,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, Eq, PartialEq, Type)]
#[serde(rename_all = "snake_case")]
pub enum PageAssistStepStateV1 {
    Running,
    /// Waits on the person: Approve or Decline through `page_assist_decide`.
    Confirm,
    Done,
    Declined,
    Failed,
    /// Not run because an earlier step stopped the reply.
    Skipped,
}

/// Why a step did not run or did not take.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, Eq, PartialEq, Type)]
#[serde(rename_all = "snake_case")]
pub enum PageAssistStepReasonV1 {
    /// A password, card or code field, or a link to another site.
    Refused,
    /// What it names is not a control this step can use.
    Unsuitable,
    /// The page changed before it ran.
    PageChanged,
    /// It ran, but the page did not change as expected.
    NoEffect,
}

/// One step of a light page action.
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq, Type)]
#[serde(deny_unknown_fields)]
pub struct PageAssistStepV1 {
    pub index: u8,
    /// What the step does, naming the exact control, such as
    /// `Click "Place order"`.
    pub label: String,
    /// Set when the step commits something and so needs the person's confirm.
    pub category: Option<WorkConfirmCategoryV1>,
    pub state: PageAssistStepStateV1,
    /// Set when `state` is `failed`.
    pub reason: Option<PageAssistStepReasonV1>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, Eq, PartialEq, Type)]
#[serde(rename_all = "snake_case")]
pub enum PageStyleStateV1 {
    /// Waits for the person's Apply (`page_assist_decide` Approve).
    Proposed,
    Applied,
    Declined,
    /// Nothing the model wrote passed the style rules; nothing applies.
    Rejected,
    /// The site already holds as many styles as it can (8, or their combined
    /// size); the person removes one in Settings or the panel to add another.
    Full,
}

/// A page style the reply proposed.
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq, Type)]
#[serde(deny_unknown_fields)]
pub struct PageStyleProposalV1 {
    /// The stored style's id once applied; null before.
    pub id: Option<String>,
    /// Short description, such as "Stories as cards".
    pub summary: String,
    /// Registrable site it applies to, such as ycombinator.com.
    pub site: String,
    /// Rules that passed validation.
    pub rules: u16,
    pub state: PageStyleStateV1,
}

/// What a Work handoff brings along.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, Eq, PartialEq, Type)]
#[serde(rename_all = "snake_case")]
pub enum PageAssistBringKindV1 {
    Task,
    Note,
    Tab,
    Repository,
    Folder,
    /// The conversation so far.
    Conversation,
}

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq, Type)]
#[serde(deny_unknown_fields)]
pub struct PageAssistBringV1 {
    pub kind: PageAssistBringKindV1,
    /// Such as "Set up status.zephium.app", "zephium-browser/Zephium" or
    /// "~/Dev/zephium-infra".
    pub label: String,
    /// The quiet word on the right, such as "This week", "Open tab",
    /// "Releases" or "Local folder".
    pub detail: Option<String>,
    /// A tab item, for its icon.
    pub tab: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq, Type)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum PageAssistHandoffStateV1 {
    /// Waits for Continue in Work.
    Offered,
    Starting,
    /// Work is running; `work` opens it.
    Started {
        work: String,
    },
    Dismissed,
    Failed {
        fault: PageAssistFaultV1,
    },
}

/// A task better done in Work, as the reply proposes it. Nothing runs before
/// the person continues.
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq, Type)]
#[serde(deny_unknown_fields)]
pub struct PageAssistHandoffV1 {
    pub title: String,
    /// One line on what Work will do.
    pub summary: String,
    pub brings: Vec<PageAssistBringV1>,
    pub state: PageAssistHandoffStateV1,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, Eq, PartialEq, Type)]
#[serde(rename_all = "snake_case")]
pub enum PageAssistPauseV1 {
    /// The person used the page; Continue re-reads it (`page_assist_resume`).
    TookOver,
}

/// One block of a reply, in order.
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq, Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PageAssistBlockV1 {
    /// Markdown (paragraphs, emphasis, lists, inline code, http links) with
    /// cite marks; grows through `text` events while the reply streams.
    Text {
        text: String,
        cites: Vec<PageAssistCiteV1>,
    },
    /// Item cards; `tab` is the page their marks are on.
    Items {
        tab: String,
        items: Vec<PageAssistItemV1>,
    },
    Opened {
        tabs: Vec<PageAssistOpenedV1>,
    },
    Steps {
        steps: Vec<PageAssistStepV1>,
    },
    Search {
        destination: String,
        query: String,
        state: PageAssistStepStateV1,
    },
    Paused {
        reason: PageAssistPauseV1,
    },
    Style {
        style: PageStyleProposalV1,
    },
    Handoff {
        handoff: PageAssistHandoffV1,
    },
}

/// What a reply cost.
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq, Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PageAssistCostV1 {
    /// Zephium AI. `credits` is a decimal string such as "0.3"; null while
    /// the charge is being confirmed.
    Hosted {
        model: String,
        credits: Option<String>,
    },
    /// The person's own key. `usd` is an estimate as a decimal string such
    /// as "0.004"; null when no price is known.
    Byok {
        model: String,
        provider: String,
        usd: Option<String>,
    },
    /// A model on this device or network; no charge.
    Local { model: String },
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, Eq, PartialEq, Type)]
#[serde(rename_all = "snake_case")]
pub enum PageAssistReplyStateV1 {
    Working,
    /// A confirm, a style Apply or a handoff waits on the person.
    Waiting,
    Paused,
    Done,
    Failed,
    Cancelled,
}

/// One message of a conversation.
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq, Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PageAssistMessageV1 {
    Person {
        id: u32,
        text: String,
        suggestion: Option<PageAssistSuggestionV1>,
        attachments: Vec<PageAssistAttachmentV1>,
    },
    Reply {
        id: u32,
        state: PageAssistReplyStateV1,
        phase: Option<PageAssistPhaseV1>,
        blocks: Vec<PageAssistBlockV1>,
        cost: Option<PageAssistCostV1>,
        fault: Option<PageAssistFaultV1>,
        /// The pages it read changed since; its cites no longer jump.
        inert: bool,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq, Type)]
#[serde(deny_unknown_fields)]
pub struct PageAssistConversationV1 {
    pub id: String,
    /// Named from its first message; null until then.
    pub title: Option<String>,
    pub messages: Vec<PageAssistMessageV1>,
    /// The last event sequence folded into this snapshot.
    pub seq: u32,
}

/// Everything the panel needs for a Space and the tab in front. Read on
/// open, on tab or Space switch, and after `invalidated`.
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq, Type)]
#[serde(deny_unknown_fields)]
pub struct PageAssistStatusV1 {
    pub version: u16,
    pub space: String,
    pub tab: Option<String>,
    /// Whether the tab in front can be attached.
    pub availability: PageAssistAvailabilityV1,
    pub page: Option<PageAssistPageV1>,
    pub model: Option<PageAssistModelV1>,
    pub suggestions: Vec<PageAssistSuggestionV1>,
    /// The start of what is selected on the page, when anything is.
    pub selection: Option<String>,
    /// Elements can be picked on this page.
    pub can_pick: bool,
    /// At least one of this site's page styles is on.
    pub styled: bool,
    pub conversation: PageAssistConversationV1,
}

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq, Type)]
#[serde(deny_unknown_fields)]
pub struct PageAssistAskV1 {
    pub space: String,
    /// The conversation the panel shows; a stale id is refused as invalid.
    pub conversation: String,
    /// The person's words; may be empty when `suggestion` is set.
    pub text: String,
    pub suggestion: Option<PageAssistSuggestionV1>,
    pub attachments: Vec<PageAssistAttachV1>,
}

/// Admission only: the reply arrives as events.
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq, Type)]
#[serde(deny_unknown_fields)]
pub struct PageAssistAskResponseV1 {
    pub reply: Option<u32>,
    pub fault: Option<PageAssistFaultV1>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, Eq, PartialEq, Type)]
#[serde(rename_all = "snake_case")]
pub enum PageAssistDecisionV1 {
    Approve,
    Decline,
}

/// "Continue in Work" result: the new Work's id, or why it did not start.
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq, Type)]
#[serde(deny_unknown_fields)]
pub struct PageAssistContinueV1 {
    pub work: Option<String>,
    pub fault: Option<PageAssistFaultV1>,
}

/// One past conversation, for the history list.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Type)]
#[serde(deny_unknown_fields)]
pub struct PageAssistPastV1 {
    pub id: String,
    pub title: String,
    pub space: String,
    /// Milliseconds since the Unix epoch.
    pub updated_ms: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Type)]
#[serde(deny_unknown_fields)]
pub struct PageAssistHistoryV1 {
    pub version: u16,
    /// Newest first.
    pub conversations: Vec<PageAssistPastV1>,
}

/// One stored style for a site.
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq, Type)]
#[serde(deny_unknown_fields)]
pub struct PageStyleV1 {
    pub id: String,
    pub summary: String,
    pub enabled: bool,
}

/// The styles of the site in a tab.
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq, Type)]
#[serde(deny_unknown_fields)]
pub struct PageStylesV1 {
    pub version: u16,
    pub tab: String,
    pub site: Option<String>,
    pub styles: Vec<PageStyleV1>,
}

/// One site's styles, for the Settings manager.
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq, Type)]
#[serde(deny_unknown_fields)]
pub struct PageSiteStylesV1 {
    pub site: String,
    pub styles: Vec<PageStyleV1>,
}

/// Every site's styles in a profile, sites in name order.
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq, Type)]
#[serde(deny_unknown_fields)]
pub struct PageStylesAllV1 {
    pub version: u16,
    pub profile: String,
    pub sites: Vec<PageSiteStylesV1>,
}

/// One change to a conversation, in `seq` order. On `invalidated`, read
/// `page_assist_status` again.
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq, Type)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PageAssistEventV1 {
    /// A message joined the conversation: the person's, then the reply's
    /// starting snapshot.
    Message {
        message: Box<PageAssistMessageV1>,
    },
    Phase {
        phase: Option<PageAssistPhaseV1>,
    },
    /// Appends to the text block at `block`, creating it when it is next.
    Text {
        block: u16,
        delta: String,
    },
    /// The text block's cites so far.
    Cites {
        block: u16,
        cites: Vec<PageAssistCiteV1>,
    },
    /// Sets the block at `block`: replaces it, or appends it when it is next.
    Block {
        block: u16,
        value: PageAssistBlockV1,
    },
    State {
        state: PageAssistReplyStateV1,
    },
    /// The reply finished. `cost` may still be confirming; a later `cost`
    /// event replaces it.
    Done {
        cost: Option<PageAssistCostV1>,
    },
    Cost {
        cost: PageAssistCostV1,
    },
    Failed {
        fault: PageAssistFaultV1,
    },
    Cancelled,
    /// Replies up to and including `reply` no longer jump to their pages.
    Inert,
    /// The conversation was titled from its first message.
    Titled {
        title: String,
    },
    /// The conversation changed shape (new, opened, deleted, AI off).
    Invalidated,
}

/// Payload of `zephium:page-assist-changed`.
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq, Type)]
#[serde(deny_unknown_fields)]
pub struct PageAssistChangedV1 {
    pub space: String,
    pub conversation: String,
    /// The message the event belongs to; 0 for conversation-wide events.
    pub reply: u32,
    pub seq: u32,
    pub kind: PageAssistEventV1,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_serialize_with_a_type_tag_inside_kind() {
        let event = PageAssistChangedV1 {
            space: "s".into(),
            conversation: "c1".into(),
            reply: 2,
            seq: 5,
            kind: PageAssistEventV1::Block {
                block: 1,
                value: PageAssistBlockV1::Items {
                    tab: "t".into(),
                    items: vec![PageAssistItemV1 {
                        n: 1,
                        title: "eMudhra to exit".into(),
                        meta: vec!["#2".into(), "749 points".into()],
                        url: Some("https://emudhra.com/".into()),
                        marked: true,
                    }],
                },
            },
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["kind"]["type"], "block");
        assert_eq!(json["kind"]["value"]["kind"], "items");
        let fault = serde_json::to_value(PageAssistFaultV1::Model {
            fault: WorkModelFaultV1::DailyLimit,
            retry_after_ms: None,
        })
        .unwrap();
        assert_eq!(fault["reason"], "model");
        assert_eq!(fault["fault"], "daily_limit");
        let state =
            serde_json::to_value(PageAssistHandoffStateV1::Started { work: "w".into() }).unwrap();
        assert_eq!(state["state"], "started");
    }
}
