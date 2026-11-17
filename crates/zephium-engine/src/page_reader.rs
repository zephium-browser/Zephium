//! The Ask panel's live Browse-tab reader: what the shell may ask of a tab's
//! document and the shapes of the answers. Reads, the selection, a marked
//! quote, numbered marks, the person's picks and the style outline all run
//! in a private world of the tab's current document through closed
//! invocations; nothing page-derived is ever executed, and every answer is
//! decoded as hostile data.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use zephium_core::ids::ItemId;

/// An sRGB colour.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Rgb(pub u8, pub u8, pub u8);

/// What a region of the page is, as the outline judged it.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum PageOutlineKind {
    /// A page header or banner.
    Header,
    /// Navigation.
    Navigation,
    /// The main landmark.
    Main,
    /// An article.
    Article,
    /// A sidebar.
    Aside,
    /// A page footer.
    Footer,
    /// A heading.
    Heading,
    /// Repeated items such as cards or comments.
    Content,
    /// A button or field.
    Control,
    /// Anything else.
    Other,
}

/// One region of the page.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PageOutlineNode {
    /// `tag`, one optional `#id`, up to three `.class` tokens.
    pub selector: String,
    /// What it is.
    pub kind: PageOutlineKind,
    /// How many elements match.
    pub count: u16,
    /// Its background, when opaque.
    pub background: Option<Rgb>,
    /// Its text colour.
    pub color: Option<Rgb>,
    /// Its font size.
    pub font_size_px: Option<u16>,
}

/// The page's structure and palette, for a style turn. Untrusted: the shell
/// admits it before use.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PageOutline {
    /// Regions, most important first.
    pub nodes: Vec<PageOutlineNode>,
    /// The page's background.
    pub background: Option<Rgb>,
    /// Its body text colour.
    pub text: Option<Rgb>,
    /// Its link colour.
    pub link: Option<Rgb>,
    /// Its accent.
    pub accent: Option<Rgb>,
    /// Its font families, most used first.
    pub fonts: Vec<String>,
    /// The page is already dark.
    pub dark: bool,
}

/// A form field's kind.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PageFieldKind {
    /// Ordinary text.
    Text,
    /// A search box.
    Search,
    /// A number.
    Number,
    /// A password: its value is never read.
    Password,
}

/// What a passage is.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PageRole {
    /// A heading of the given level.
    Heading(u8),
    /// Running text.
    Text,
    /// A list item.
    ListItem,
    /// A table cell.
    Cell,
    /// A link; `same_site` when it stays on the page's site.
    Link {
        /// The destination is on the page's own host.
        same_site: bool,
    },
    /// A button.
    Button,
    /// A text field.
    Field(PageFieldKind),
    /// A checkbox or switch.
    Checkbox {
        /// Its current state.
        checked: bool,
    },
    /// A drop-down or list box.
    Select,
    /// Anything else.
    Other,
}

/// One passage of a read.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PagePassage {
    /// The read fragment it came from (`rN`).
    pub id: u16,
    /// What it is.
    pub role: PageRole,
    /// The visible text or accessible name.
    pub text: String,
    /// `text` is exactly what the page shows, complete.
    pub verbatim: bool,
    /// A field's current value; never set for a password.
    pub value: Option<String>,
    /// 0 the main content, 1 beside it, 2 the page's chrome.
    pub region: u8,
    /// A link's destination: absolute, `http` or `https`, without user
    /// information, at most 2048 bytes. The only addresses a tab may be
    /// opened to from a read.
    pub href: Option<String>,
}

/// One numbered mark: badge `n` around the item holding passage `passage`
/// of a read.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PageMark {
    /// The number in its badge.
    pub n: u8,
    /// The passage (`rN`) of the read it marks.
    pub passage: u16,
}

/// An element the person picked, as it reads now.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PagePick {
    /// The pick's number.
    pub pick: u64,
    /// What it is, for a chip ("Story list"); at most 64 characters.
    pub label: String,
    /// Its visible text in reading order; never a form value; at most 4000
    /// characters.
    pub text: String,
    /// A selector matching it and at most 99 others, for scoping a restyle.
    pub selector: Option<String>,
}

/// Why a pick did not happen.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PagePickFailure {
    /// Escape, a cancel, another pick or two idle minutes ended it.
    Cancelled,
    /// The tab navigated or closed.
    PageChanged,
    /// The page cannot be picked from.
    Unavailable,
}

/// The light along a page's edges that says the assistant is acting there.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PageGlow {
    /// None.
    #[default]
    Off,
    /// The assistant is acting: the light travels.
    Working,
    /// The person took over: the light holds still, dimmer.
    Paused,
}

impl PageGlow {
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Working => "working",
            Self::Paused => "paused",
        }
    }
}

/// The light along the front page's edges that says voice is live there.
/// Listening outranks the assistant's glow; speaking yields to it.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PageVoice {
    /// None.
    #[default]
    Off,
    /// The microphone is live: the edge breathes with the person's voice.
    Listening,
    /// The assistant speaks: a calmer edge follows its voice.
    Speaking,
}

impl PageVoice {
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Listening => "listening",
            Self::Speaking => "speaking",
        }
    }
}

static DIAGNOSTICS: std::sync::OnceLock<fn(std::fmt::Arguments<'_>)> = std::sync::OnceLock::new();

/// Where the reader's closed facts go (the app's diagnostic log); until set,
/// standard error. Never page content.
pub fn set_diagnostics(sink: fn(std::fmt::Arguments<'_>)) {
    let _ = DIAGNOSTICS.set(sink);
}

#[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
pub(crate) fn note(fact: std::fmt::Arguments<'_>) {
    match DIAGNOSTICS.get() {
        Some(sink) => sink(fact),
        None => {
            use std::io::Write as _;
            let mut stderr = std::io::stderr().lock();
            let _ = stderr.write_fmt(fact);
            let _ = stderr.write_all(b"\n");
        }
    }
}

/// One read of a tab's document.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PageReadout {
    /// The committed navigation the read belongs to.
    pub epoch: u64,
    /// This read's identity: steps planned on it name it, and only it.
    pub read: u64,
    /// The page's title (untrusted).
    pub title: Option<String>,
    /// Passages in reading order.
    pub passages: Vec<PagePassage>,
}

/// Why a reader request did not happen.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PageReadFailure {
    /// The document is not ready, or predates the reader.
    NotReady,
    /// The tab navigated or closed.
    PageChanged,
    /// The page could not be read.
    Unavailable,
}

/// When the reader may install: the shell's switch and its site policy.
#[derive(Clone)]
pub struct PageReaderPolicy {
    /// Whether an address is on a site the reader never runs on.
    pub sensitive: Arc<dyn Fn(&str) -> bool + Send + Sync>,
    /// The person used a tab while steps held it: mouse, keys or scroll.
    pub human_input: Arc<dyn Fn(ItemId) + Send + Sync>,
}

impl std::fmt::Debug for PageReaderPolicy {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("PageReaderPolicy")
    }
}

#[derive(Clone, Copy, Default)]
struct TabState {
    epoch: u64,
    ready: bool,
}

static TABS: Mutex<Option<HashMap<ItemId, TabState>>> = Mutex::new(None);

fn with_tabs<T>(apply: impl FnOnce(&mut HashMap<ItemId, TabState>) -> T) -> T {
    let mut tabs = TABS.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    apply(tabs.get_or_insert_with(HashMap::new))
}

/// The committed navigation epoch of `tab`'s document, when it has one.
/// Epochs are process-unique and never reused.
pub fn committed_epoch(tab: ItemId) -> Option<u64> {
    with_tabs(|tabs| tabs.get(&tab).map(|state| state.epoch))
}

/// Whether `tab`'s current document can be read now.
pub fn readable(tab: ItemId) -> bool {
    with_tabs(|tabs| tabs.get(&tab).is_some_and(|state| state.ready))
}

#[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
pub(crate) fn publish_commit(tab: ItemId, epoch: u64) {
    with_tabs(|tabs| {
        if tabs.len() >= 4_096 && !tabs.contains_key(&tab) {
            tabs.clear();
        }
        tabs.insert(
            tab,
            TabState {
                epoch,
                ready: false,
            },
        );
    });
}

#[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
pub(crate) fn publish_ready(tab: ItemId, epoch: u64, ready: bool) {
    with_tabs(|tabs| {
        if let Some(state) = tabs.get_mut(&tab).filter(|state| state.epoch == epoch) {
            state.ready = ready;
        }
    });
}

#[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
pub(crate) fn forget(tab: ItemId) {
    with_tabs(|tabs| {
        tabs.remove(&tab);
    });
    with_reads(|reads| {
        reads.remove(&tab);
    });
}

/// Why a step did not run or did not take.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PageActFailure {
    /// The tab navigated or closed, or the step was planned on another page.
    PageChanged,
    /// The control is not one this step can use, or it changed.
    Unsuitable,
    /// A protected field, another site or a sensitive page.
    Refused,
    /// It ran without the expected effect, or could not run.
    Failed,
}

/// The read a step is planned on: its passages' nodes, as observed.
pub struct PageRead {
    id: u64,
    epoch: u64,
    observation: zephium_agentic::SemanticObservation,
    frame: zephium_agentic::SemanticFrameJoin,
    references: Vec<(u16, zephium_agentic::SemanticReferenceId)>,
}

impl PageRead {
    /// Its identity, as its readout carries it.
    pub fn id(&self) -> u64 {
        self.id
    }

    /// The committed navigation it belongs to.
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// The observation the passages came from.
    pub fn observation(&self) -> &zephium_agentic::SemanticObservation {
        &self.observation
    }

    /// Its main frame's authority.
    pub fn frame(&self) -> &zephium_agentic::SemanticFrameJoin {
        &self.frame
    }

    /// The observed node behind passage `rN`.
    pub fn reference(&self, passage: u16) -> Option<zephium_agentic::SemanticReferenceId> {
        self.references
            .iter()
            .find(|(id, _)| *id == passage)
            .map(|(_, reference)| *reference)
    }
}

/// Reads kept per tab: a plan stays bound to the exact read it was made
/// on, however many reads of the page happen meanwhile.
const MAX_KEPT_READS: usize = 8;

type KeptReads = HashMap<ItemId, std::collections::VecDeque<Arc<PageRead>>>;

static READS: Mutex<Option<KeptReads>> = Mutex::new(None);
static NEXT_READ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn with_reads<T>(apply: impl FnOnce(&mut KeptReads) -> T) -> T {
    let mut reads = READS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    apply(reads.get_or_insert_with(HashMap::new))
}

/// The read of `tab` named `id`, while it is kept. Another read never
/// stands in for it.
pub fn planned_read(tab: ItemId, id: u64) -> Option<Arc<PageRead>> {
    with_reads(|reads| reads.get(&tab)?.iter().find(|read| read.id == id).cloned())
}

/// Keeps `read` for the steps that may be planned on it; returns its id.
pub(crate) fn publish_read(tab: ItemId, mut read: PageRead) -> u64 {
    let id = NEXT_READ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    read.id = id;
    with_reads(|reads| {
        if reads.len() >= 512 && !reads.contains_key(&tab) {
            reads.clear();
        }
        let kept = reads.entry(tab).or_default();
        if kept.len() >= MAX_KEPT_READS {
            kept.pop_front();
        }
        kept.push_back(Arc::new(read));
    });
    id
}

#[cfg(feature = "agentic-browser")]
pub(crate) mod read {
    //! A runtime snapshot projected into passages through the semantic read
    //! policy: public content only, the standard budget, verbatim only when
    //! exact, and never a password's value.

    use zephium_agentic::{
        read_semantic_observation, ContextCapabilities, ContextCapability, ContextId,
        ContextIdentity, ContextJoin, ContextKind, ContextOperationId, ContextRegistry,
        ContextRunId, ContextSettlement, FrameId, SemanticCaptureInstant, SemanticFrameJoin,
        SemanticFrameTrust, SemanticInvocationId, SemanticObservationAssembler,
        SemanticObservationBudget, SemanticObservationId, SemanticObservationRequest,
        SemanticOrigin, SemanticReadAuthority, SemanticReadBudget, SemanticReadContent,
        SemanticReadField, SemanticReadSensitivityLimit, SemanticRole, SemanticRuntimeBudget,
        SemanticRuntimeInvocation, SemanticSnapshot, SemanticSnapshotGeneration, SemanticState,
    };

    use std::collections::HashMap;

    use super::{PageFieldKind, PagePassage, PageReadFailure, PageRole};

    /// One request in flight: its observation request and frame authority,
    /// kept to assemble the answer.
    pub(crate) struct PendingRead {
        request: SemanticObservationRequest,
        frame: SemanticFrameJoin,
        site: Option<String>,
    }

    /// A private, never-shared context identity for one reader. It carries
    /// no capability beyond typed bookkeeping for observation decoding.
    pub(crate) fn reader_context() -> Option<ContextJoin> {
        let identity = ContextIdentity::new(
            ContextId::generate(),
            ContextRunId::generate(),
            1_u128.into(),
            ContextKind::Owned,
        );
        let mut registry = ContextRegistry::new();
        registry
            .reserve(
                identity,
                ContextCapabilities::try_new(ContextKind::Owned, &[ContextCapability::Observe])
                    .ok()?,
            )
            .ok()?;
        let operation = registry
            .begin_context(identity.id(), ContextOperationId::new(1)?)
            .ok()?;
        registry
            .settle_construction(identity.id(), operation, ContextSettlement::Applied)
            .ok()?;
        registry.join(identity.id()).ok()
    }

    /// One whole-page observation of the main frame at `url`.
    pub(crate) fn invocation(
        context: ContextJoin,
        url: &str,
        sequence: u64,
    ) -> Option<(SemanticRuntimeInvocation, PendingRead)> {
        let origin = SemanticOrigin::parse(url).ok()?;
        let frame = SemanticFrameJoin::try_new(
            context,
            FrameId::MAIN,
            context.frame_generation(),
            origin,
            SemanticFrameTrust::SameOrigin,
        )
        .ok()?;
        let request = SemanticObservationRequest::initial(
            SemanticObservationId::new(sequence)?,
            context,
            SemanticObservationBudget::WHOLE_PAGE,
        );
        let invocation = zephium_agentic::encode_semantic_runtime_invocation(
            &request,
            frame.clone(),
            SemanticInvocationId::new(sequence)?,
            SemanticSnapshotGeneration::new(sequence)?,
            SemanticRuntimeBudget::WHOLE_PAGE.with_link_url_state(),
        )
        .ok()?;
        let site = zephium_agentic::ContextNavigationTarget::parse(url)
            .ok()
            .and_then(|target| zephium_agentic::registrable_site(&target));
        Some((
            invocation,
            PendingRead {
                request,
                frame,
                site,
            },
        ))
    }

    fn same_site(site: Option<&str>, target: &zephium_agentic::ContextNavigationTarget) -> bool {
        site.is_some_and(|site| zephium_agentic::registrable_site(target).as_deref() == Some(site))
    }

    /// Projects one runtime snapshot into passages in reading order.
    /// Passages, and the read kept for the steps that may follow it.
    pub(crate) fn passages(
        pending: PendingRead,
        snapshot: SemanticSnapshot,
        captured_at_ms: u64,
        epoch: u64,
    ) -> Result<(Vec<PagePassage>, super::PageRead), PageReadFailure> {
        let observation = SemanticObservationAssembler::new(pending.request, snapshot)
            .and_then(|assembler| assembler.finish())
            .map_err(|_| PageReadFailure::Unavailable)?;
        let read = read_semantic_observation(
            &observation,
            SemanticReadAuthority::Initial,
            SemanticCaptureInstant::from_millis(captured_at_ms),
            SemanticReadSensitivityLimit::PublicOnly,
            SemanticReadBudget::EXTRACTION,
        )
        .map_err(|_| PageReadFailure::Unavailable)?;
        let mut passages: Vec<(PagePassage, zephium_agentic::SemanticReferenceId)> = Vec::new();
        for fragment in read.fragments() {
            let reference = fragment.provenance().reference();
            let Ok(node) = observation.resolve_node(reference, &pending.frame) else {
                continue;
            };
            let role = page_role(node, pending.site.as_deref());
            let field = matches!(role, PageRole::Field(_));
            let protected = matches!(role, PageRole::Field(PageFieldKind::Password));
            let previous = passages
                .last_mut()
                .filter(|(_, last)| *last == reference)
                .map(|(passage, _)| passage);
            match (fragment.field(), fragment.content()) {
                (
                    SemanticReadField::VisibleText | SemanticReadField::AccessibleName,
                    SemanticReadContent::Text(text),
                ) => {
                    if let Some(previous) = previous.filter(|p| p.text.is_empty()) {
                        previous.text = text.as_str().to_owned();
                        previous.verbatim = fragment.verbatim_text().is_some();
                        continue;
                    }
                    passages.push((
                        PagePassage {
                            id: fragment.id().get(),
                            role,
                            text: text.as_str().to_owned(),
                            verbatim: fragment.verbatim_text().is_some(),
                            value: None,
                            region: fragment.region(),
                            href: link_href(node),
                        },
                        reference,
                    ));
                }
                (SemanticReadField::TextValue, SemanticReadContent::ValuePreview(preview))
                    if field && !protected =>
                {
                    let value = preview.text().to_owned();
                    match previous {
                        Some(previous) => previous.value = Some(value),
                        None => passages.push((
                            PagePassage {
                                id: fragment.id().get(),
                                role,
                                text: String::new(),
                                verbatim: false,
                                value: Some(value),
                                region: fragment.region(),
                                href: None,
                            },
                            reference,
                        )),
                    }
                }
                (SemanticReadField::BooleanValue, SemanticReadContent::Boolean(checked)) => {
                    match previous {
                        Some(previous) if matches!(previous.role, PageRole::Checkbox { .. }) => {
                            previous.role = PageRole::Checkbox { checked };
                        }
                        Some(_) => {}
                        None if matches!(role, PageRole::Checkbox { .. }) => {
                            passages.push((
                                unnamed(
                                    fragment.id().get(),
                                    PageRole::Checkbox { checked },
                                    fragment.region(),
                                ),
                                reference,
                            ));
                        }
                        None => {}
                    }
                }
                (SemanticReadField::OrdinalValue, SemanticReadContent::Ordinal(ordinal))
                    if role == PageRole::Select =>
                {
                    let value = chosen(&observation, node, ordinal);
                    match previous {
                        Some(previous) => previous.value = value,
                        None => {
                            let mut passage = unnamed(fragment.id().get(), role, fragment.region());
                            passage.value = value;
                            passages.push((passage, reference));
                        }
                    }
                }
                _ => {}
            }
        }
        let mut passages: Vec<(PagePassage, zephium_agentic::SemanticReferenceId)> = passages
            .into_iter()
            .filter(|(passage, _)| {
                !passage.text.is_empty() || passage.value.is_some() || control(passage.role)
            })
            .collect();
        add_unread_fields(&observation, pending.site.as_deref(), &mut passages);
        let references = passages
            .iter()
            .map(|(passage, reference)| (passage.id, *reference))
            .collect();
        drop(read);
        Ok((
            passages.into_iter().map(|(passage, _)| passage).collect(),
            super::PageRead {
                id: 0,
                epoch,
                observation,
                frame: pending.frame,
                references,
            },
        ))
    }

    fn page_role(node: &zephium_agentic::SemanticNode, site: Option<&str>) -> PageRole {
        match node.role() {
            SemanticRole::Heading => {
                PageRole::Heading(node.heading_level().map_or(2, |level| level.get()))
            }
            SemanticRole::Paragraph | SemanticRole::Document | SemanticRole::Status => {
                PageRole::Text
            }
            SemanticRole::ListItem => PageRole::ListItem,
            SemanticRole::Cell | SemanticRole::CellHeader => PageRole::Cell,
            SemanticRole::Link => PageRole::Link {
                same_site: node
                    .link_destination()
                    .is_some_and(|target| same_site(site, target)),
            },
            SemanticRole::Button | SemanticRole::MenuItem | SemanticRole::Tab => PageRole::Button,
            SemanticRole::Textbox => PageRole::Field(PageFieldKind::Text),
            SemanticRole::Searchbox => PageRole::Field(PageFieldKind::Search),
            SemanticRole::Spinbutton => PageRole::Field(PageFieldKind::Number),
            SemanticRole::Password => PageRole::Field(PageFieldKind::Password),
            SemanticRole::Checkbox | SemanticRole::Radio => PageRole::Checkbox {
                checked: node.states().contains(SemanticState::Checked),
            },
            SemanticRole::Combobox | SemanticRole::Listbox => PageRole::Select,
            _ => PageRole::Other,
        }
    }

    /// A control the person could use, which reads even without a name.
    fn control(role: PageRole) -> bool {
        matches!(
            role,
            PageRole::Field(PageFieldKind::Text | PageFieldKind::Search | PageFieldKind::Number)
                | PageRole::Checkbox { .. }
                | PageRole::Select
        )
    }

    /// The name of the option `select` shows: the one marked selected, else
    /// the one at its selected index.
    fn chosen(
        observation: &zephium_agentic::SemanticObservation,
        select: &zephium_agentic::SemanticNode,
        ordinal: u16,
    ) -> Option<String> {
        let nodes = observation.frames().first()?.nodes();
        let index = nodes
            .iter()
            .position(|node| node.reference() == select.reference())?;
        let mut inside = vec![index];
        let mut options = Vec::new();
        for (at, node) in nodes.iter().enumerate().skip(index + 1) {
            if !node
                .parent()
                .is_some_and(|parent| inside.contains(&usize::from(parent)))
            {
                continue;
            }
            inside.push(at);
            if node.role() == SemanticRole::Option {
                options.push(node);
            }
        }
        let option = options
            .iter()
            .find(|node| node.states().contains(SemanticState::Selected))
            .or_else(|| options.get(usize::from(ordinal)))?;
        let name = option.name()?.as_str().trim();
        (!name.is_empty()).then(|| name.to_owned())
    }

    fn unnamed(id: u16, role: PageRole, region: u8) -> PagePassage {
        PagePassage {
            id,
            role,
            text: String::new(),
            verbatim: false,
            value: None,
            region,
            href: None,
        }
    }

    /// Fields a read may gain beyond its own fragments.
    const MAX_UNREAD_FIELDS: usize = 16;

    /// Adds the enabled, public fields of the main frame the read had
    /// nothing to say about (an empty field the page left unnamed), in
    /// document order, under ids past the read's own.
    fn add_unread_fields(
        observation: &zephium_agentic::SemanticObservation,
        site: Option<&str>,
        passages: &mut Vec<(PagePassage, zephium_agentic::SemanticReferenceId)>,
    ) {
        let Some(frame) = observation.frames().first() else {
            return;
        };
        let nodes = frame.nodes();
        let indices: HashMap<_, _> = nodes
            .iter()
            .enumerate()
            .map(|(index, node)| (node.reference(), index))
            .collect();
        let mut order: Vec<usize> = passages
            .iter()
            .map(|(_, reference)| indices.get(reference).copied().unwrap_or(0))
            .collect();
        let read: std::collections::HashSet<_> =
            passages.iter().map(|(_, reference)| *reference).collect();
        let mut next = passages
            .iter()
            .map(|(passage, _)| passage.id)
            .max()
            .unwrap_or(0);
        let mut added = 0;
        for (index, node) in nodes.iter().enumerate() {
            if added == MAX_UNREAD_FIELDS {
                break;
            }
            let role = page_role(node, site);
            if !control(role)
                || node.sensitivity() != zephium_agentic::SemanticSensitivity::Public
                || node.states().contains(SemanticState::Disabled)
                || read.contains(&node.reference())
            {
                continue;
            }
            let Some(id) = next.checked_add(1) else {
                return;
            };
            next = id;
            added += 1;
            let at = order
                .iter()
                .rposition(|&i| i < index)
                .map_or(0, |at| at + 1);
            let region = at
                .checked_sub(1)
                .map_or(0, |before| passages[before].0.region);
            order.insert(at, index);
            passages.insert(at, (unnamed(id, role, region), node.reference()));
        }
    }

    /// A link's destination as a tab may be opened to it: the observed
    /// public address, `http` or `https` only, user information removed.
    fn link_href(node: &zephium_agentic::SemanticNode) -> Option<String> {
        if node.role() != SemanticRole::Link {
            return None;
        }
        let mut url = node.link_destination()?.as_url().clone();
        if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
            return None;
        }
        url.set_username("").ok()?;
        url.set_password(None).ok()?;
        let href = String::from(url);
        (href.len() <= 2_048).then_some(href)
    }

    fn mark_kind(role: SemanticRole) -> &'static str {
        match role {
            SemanticRole::Heading => "heading",
            SemanticRole::Link => "link",
            SemanticRole::Button | SemanticRole::MenuItem | SemanticRole::Tab => "button",
            SemanticRole::ListItem => "item",
            SemanticRole::Cell | SemanticRole::CellHeader => "cell",
            SemanticRole::Textbox
            | SemanticRole::Searchbox
            | SemanticRole::Spinbutton
            | SemanticRole::Password
            | SemanticRole::Combobox
            | SemanticRole::Listbox => "field",
            SemanticRole::Checkbox | SemanticRole::Radio => "check",
            SemanticRole::Image => "image",
            _ => "text",
        }
    }

    /// What the read observed of passage `passage`, so the program can find
    /// its element again: kind, text, name, destination, and which of the
    /// read's passages with the same facts it is. None when the read never
    /// had it.
    pub(crate) fn mark_target(
        stored: &super::PageRead,
        n: u8,
        passage: u16,
    ) -> Option<crate::platform::page_recipe::MarkTarget> {
        type Facts = (&'static str, Option<String>, Option<String>, Option<String>);
        let facts = |reference| -> Option<Facts> {
            let node = stored
                .observation
                .resolve_node(reference, &stored.frame)
                .ok()?;
            let owned = |text: Option<&zephium_agentic::SemanticText>| {
                text.map(|text| text.as_str().to_owned())
                    .filter(|text| !text.is_empty())
            };
            Some((
                mark_kind(node.role()),
                owned(node.text()),
                owned(node.name()),
                link_href(node),
            ))
        };
        let index = stored
            .references
            .iter()
            .position(|(id, _)| *id == passage)?;
        let own = facts(stored.references[index].1)?;
        if own.1.is_none() && own.2.is_none() {
            return None;
        }
        let occurrence = stored.references[..index]
            .iter()
            .filter(|(_, reference)| facts(*reference).as_ref() == Some(&own))
            .count();
        let (kind, text, name, href) = own;
        Some(crate::platform::page_recipe::MarkTarget {
            n,
            kind,
            text,
            name,
            href,
            occurrence: u16::try_from(occurrence).unwrap_or(u16::MAX),
        })
    }

    /// One person-directed step bound to the read it was planned on and
    /// prepared against `current`, the page as it is right now.
    pub(crate) fn person_action(
        stored: &super::PageRead,
        current: &SemanticSnapshot,
        proposal: zephium_agentic::SemanticActionProposal,
        attempt: u64,
        requested_at_ms: u64,
    ) -> Result<
        (
            zephium_agentic::PersonDirectedAction,
            zephium_agentic::SemanticActionNativeRequest,
        ),
        super::PageActFailure,
    > {
        use super::PageActFailure;
        let batch = zephium_agentic::SemanticActionBatch::bind(
            zephium_agentic::SemanticActionBatchId::new(attempt).ok_or(PageActFailure::Failed)?,
            &stored.observation,
            std::slice::from_ref(&stored.frame),
            vec![proposal],
        )
        .map_err(|_| PageActFailure::Unsuitable)?;
        let prepared = batch
            .actions()
            .first()
            .ok_or(PageActFailure::Failed)?
            .prepare(current)
            .map_err(|_| PageActFailure::Unsuitable)?;
        zephium_agentic::PersonDirectedAction::prepare(
            &prepared,
            zephium_agentic::SemanticActionAttemptId::new(attempt).ok_or(PageActFailure::Failed)?,
            zephium_agentic::SemanticActionExecutionInstant::from_millis(requested_at_ms),
        )
        .map_err(|_| PageActFailure::Failed)
    }

    /// Return in a field: a step whose taking may be the document leaving,
    /// as a form submits. Once it was handed to the page, that navigation
    /// is its effect, not a changed page.
    pub(crate) fn submits(proposal: &zephium_agentic::SemanticActionProposal) -> bool {
        matches!(
            proposal.intent(),
            zephium_agentic::SemanticActionIntent::Press {
                key: zephium_agentic::SemanticPressKey::Enter,
                ..
            }
        )
    }

    /// Why a prepared step did not take.
    pub(crate) fn act_failure(
        failure: zephium_agentic::SemanticActionFailure,
    ) -> super::PageActFailure {
        use zephium_agentic::SemanticActionFailure as Failure;
        match failure {
            Failure::CredentialBoundary | Failure::BlockedOrigin => super::PageActFailure::Refused,
            Failure::TargetChanged
            | Failure::TargetDisabled
            | Failure::TargetOccluded
            | Failure::UnsupportedInteraction => super::PageActFailure::Unsuitable,
            Failure::StaleReference => super::PageActFailure::PageChanged,
            _ => super::PageActFailure::Failed,
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use zephium_agentic::{decode_semantic_snapshot, SemanticDecodeContext};

        fn snapshot(pending: &PendingRead, invocation: u64, wire: &str) -> SemanticSnapshot {
            decode_semantic_snapshot(
                SemanticDecodeContext::new(
                    SemanticInvocationId::new(invocation).expect("invocation"),
                    pending.frame.clone(),
                    SemanticSnapshotGeneration::new(invocation).expect("generation"),
                ),
                wire.as_bytes(),
            )
            .expect("snapshot")
        }

        #[test]
        fn passages_follow_the_public_read_and_withhold_passwords() {
            let context = reader_context().expect("context");
            let (invocation, pending) =
                super::invocation(context, "https://news.example.com/story?id=1", 3)
                    .expect("invocation");
            assert!(!invocation.as_str().is_empty());
            let wire = r#"{"v":1,"i":3,"g":3,"c":"complete","n":[
                {"k":1,"r":"document","o":16},
                {"k":2,"p":0,"r":"heading","l":1,"n":"Rivers rise"},
                {"k":3,"p":0,"r":"paragraph","t":"The river rose two metres overnight."},
                {"k":4,"p":0,"r":"password","n":"Password","q":"secret","v":{"k":"redacted"}},
                {"k":5,"p":0,"r":"textbox","n":"Email","q":"sensitive"}
            ]}"#;
            let (passages, kept) =
                passages(pending, snapshot_for(&invocation, wire), 1, 7).expect("read");
            assert_eq!(kept.epoch(), 7);
            assert!(passages.iter().all(|p| kept.reference(p.id).is_some()));
            let texts: Vec<&str> = passages.iter().map(|p| p.text.as_str()).collect();
            assert!(texts.contains(&"Rivers rise"), "{texts:?}");
            assert!(texts.contains(&"The river rose two metres overnight."));
            assert!(passages.iter().all(|p| p.value.is_none()));
            assert!(
                !texts.contains(&"Email"),
                "sensitive fields stay out of a public read"
            );
            let heading = passages
                .iter()
                .find(|p| p.text == "Rivers rise")
                .expect("heading");
            assert_eq!(heading.role, PageRole::Heading(1));
            assert!(heading.verbatim);
        }

        #[test]
        fn links_carry_their_destination_and_marks_their_observed_facts() {
            let context = reader_context().expect("context");
            let (invocation, pending) =
                super::invocation(context, "https://news.example.com/", 4).expect("invocation");
            let wire = r#"{"v":1,"i":4,"g":4,"c":"complete","n":[
                {"k":1,"r":"document","o":16},
                {"k":2,"p":0,"r":"link","n":"hide","u":"https://news.example.com/hide?id=1"},
                {"k":3,"p":0,"r":"paragraph","t":"A story"},
                {"k":4,"p":0,"r":"link","n":"hide","u":"https://news.example.com/hide?id=1"},
                {"k":5,"p":0,"r":"link","n":"Elsewhere","u":"https://other.example/"}
            ]}"#;
            let (passages, kept) =
                passages(pending, snapshot_for(&invocation, wire), 1, 7).expect("read");
            let hrefs: Vec<Option<&str>> = passages.iter().map(|p| p.href.as_deref()).collect();
            assert_eq!(
                hrefs,
                [
                    Some("https://news.example.com/hide?id=1"),
                    None,
                    Some("https://news.example.com/hide?id=1"),
                    Some("https://other.example/")
                ]
            );
            let second = mark_target(&kept, 3, passages[2].id).expect("target");
            assert_eq!((second.kind, second.occurrence), ("link", 1));
            assert_eq!(second.name.as_deref(), Some("hide"));
            assert_eq!(
                second.href.as_deref(),
                Some("https://news.example.com/hide?id=1")
            );
            let text = mark_target(&kept, 1, passages[1].id).expect("target");
            assert_eq!((text.kind, text.occurrence), ("text", 0));
            assert!(
                mark_target(&kept, 1, 999).is_none(),
                "a passage the read never had"
            );
        }

        #[test]
        fn a_plan_stays_bound_to_its_own_read() {
            let context = reader_context().expect("context");
            let tab = zephium_core::ids::ItemId::generate();
            let publish = |sequence: u64, text: &str| {
                let (invocation, pending) =
                    super::invocation(context, "https://news.example.com/", sequence)
                        .expect("invocation");
                let wire = format!(
                    r#"{{"v":1,"i":{sequence},"g":{sequence},"c":"complete","n":[{{"k":1,"r":"document","o":16}},{{"k":2,"p":0,"r":"paragraph","t":"{text}"}}]}}"#
                );
                let (_, kept) =
                    passages(pending, snapshot_for(&invocation, &wire), 1, 7).expect("read");
                super::super::publish_read(tab, kept)
            };
            let planned = publish(1, "First look");
            let later = publish(2, "A status refresh");
            assert_ne!(planned, later);
            let bound = super::super::planned_read(tab, planned).expect("bound read");
            assert_eq!(bound.id(), planned);
            for sequence in 3..=9 {
                publish(sequence, "More reads");
            }
            assert!(
                super::super::planned_read(tab, planned).is_none(),
                "a read no longer kept ends its plan as a changed page"
            );
            assert!(super::super::planned_read(tab, later).is_some());
        }

        fn snapshot_for(invocation: &SemanticRuntimeInvocation, wire: &str) -> SemanticSnapshot {
            invocation.decode_result(wire.as_bytes()).expect("decode")
        }

        #[test]
        fn unnamed_empty_controls_still_read_and_resolve() {
            let context = reader_context().expect("context");
            let (invocation, pending) =
                super::invocation(context, "https://news.example.com/", 5).expect("invocation");
            let wire = r#"{"v":1,"i":5,"g":5,"c":"complete","n":[
                {"k":1,"r":"document","o":16},
                {"k":2,"p":0,"r":"paragraph","t":"Search:"},
                {"k":3,"p":0,"r":"textbox","v":{"k":"text","value":""},"o":2},
                {"k":4,"p":0,"r":"checkbox","v":{"k":"boolean","value":true},"o":1},
                {"k":5,"p":0,"r":"combobox","v":{"k":"ordinal","value":0}},
                {"k":6,"p":0,"r":"textbox","s":8,"v":{"k":"text","value":""}},
                {"k":7,"p":0,"r":"textbox","n":"Email","q":"sensitive","v":{"k":"text","value":""}},
                {"k":8,"p":0,"r":"paragraph","t":"Footer"}
            ]}"#;
            let (passages, kept) =
                passages(pending, snapshot_for(&invocation, wire), 1, 7).expect("read");
            let roles: Vec<PageRole> = passages.iter().map(|p| p.role).collect();
            assert_eq!(
                roles,
                [
                    PageRole::Text,
                    PageRole::Field(PageFieldKind::Text),
                    PageRole::Checkbox { checked: true },
                    PageRole::Select,
                    PageRole::Text,
                ],
                "disabled and sensitive fields stay out; the rest keep document order"
            );
            let mut ids: Vec<u16> = passages.iter().map(|p| p.id).collect();
            assert!(passages.iter().all(|p| kept.reference(p.id).is_some()));
            ids.sort_unstable();
            ids.dedup();
            assert_eq!(ids.len(), passages.len(), "ids stay unique");
        }

        #[test]
        fn a_select_reads_as_the_option_it_shows() {
            let context = reader_context().expect("context");
            let (invocation, pending) =
                super::invocation(context, "https://shop.example.com/", 6).expect("invocation");
            let wire = r#"{"v":1,"i":6,"g":6,"c":"complete","n":[
                {"k":1,"r":"document","o":16},
                {"k":2,"p":0,"r":"combobox","n":"Topic","v":{"k":"ordinal","value":1},"o":13},
                {"k":3,"p":1,"r":"option","n":"General","v":{"k":"ordinal","value":0},"o":9},
                {"k":4,"p":1,"r":"option","n":"Billing","v":{"k":"ordinal","value":1},"o":9},
                {"k":5,"p":0,"r":"combobox","n":"Size","v":{"k":"ordinal","value":0},"o":13},
                {"k":6,"p":4,"r":"option","n":"Small","v":{"k":"ordinal","value":0},"o":9},
                {"k":7,"p":4,"r":"option","n":"Large","v":{"k":"ordinal","value":1},"s":2,"o":9}
            ]}"#;
            let (passages, _) =
                passages(pending, snapshot_for(&invocation, wire), 1, 7).expect("read");
            let selects: Vec<(&str, Option<&str>)> = passages
                .iter()
                .filter(|p| p.role == PageRole::Select)
                .map(|p| (p.text.as_str(), p.value.as_deref()))
                .collect();
            assert_eq!(
                selects,
                [("Topic", Some("Billing")), ("Size", Some("Large"))],
                "the option marked selected wins over the index"
            );
        }

        #[test]
        fn links_know_their_site_and_internal_pages_are_never_read() {
            let site = Some("example.co.uk");
            let url =
                |value: &str| zephium_agentic::ContextNavigationTarget::parse(value).expect("url");
            assert!(same_site(site, &url("https://example.co.uk/a")));
            assert!(same_site(site, &url("https://shop.example.co.uk/")));
            assert!(!same_site(site, &url("https://other.co.uk/")));
            let context = reader_context().expect("context");
            for refused in ["about:blank", "file:///etc/hosts", "zephium://settings"] {
                assert!(
                    super::invocation(context, refused, 1).is_none(),
                    "{refused}"
                );
            }
            let _ = snapshot;
        }
    }
}
