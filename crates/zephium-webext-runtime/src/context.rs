//! Who is calling. [`Authority`] is the only place a [`Sender`] is made.
//!
//! Every input to a binding function is a host fact: the profile and
//! incognito class are fixed when the authority is created, runtime-owned
//! views exist only because the host opened them through [`Authority`], tab
//! and frame identities come from the engine view that delivered a message,
//! and content-script worlds count only after the host recorded injecting
//! them into that exact document. Nothing an extension or page sends is
//! consulted, and envelopes carry no identity at all.

use std::collections::{BTreeSet, HashMap};
use std::fmt;
use std::num::NonZeroU64;
use std::sync::Arc;

use url::Url;
use zephium_core::extensions::ExtensionRuntimeGeneration;
use zephium_core::ids::{ExtensionInstallId, ItemId, ProfileId};

use crate::errors::ApiError;
use crate::pattern::{ContentMatchError, ContentScriptMatcher, DocumentOrigin, FrameDocument};

pub use zephium_webext::ExtensionId;

/// Most injection records one profile keeps (frames times installs).
pub const MAX_INJECTION_RECORDS_PER_PROFILE: usize = 65_536;
/// Most bytes in a `userScripts` world id.
pub const MAX_USER_SCRIPT_WORLD_ID_BYTES: usize = 256;

/// The kind of JavaScript context a call came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ContextKind {
    /// The background (service worker emulation or host page).
    Background,
    /// The action popup.
    Popup,
    /// The options page in its own view.
    Options,
    /// A top-level extension page in an ordinary tab.
    ExtensionTab,
    /// An offscreen document.
    Offscreen,
    /// An extension page framed inside a web page. It shares the web page's
    /// renderer, so it gets less than other extension pages.
    ExtensionFrame,
    /// A content script in the install's isolated world.
    ContentScript,
    /// A `userScripts` script in a `USER_SCRIPT` world.
    UserScript,
    /// A web page allowed by `externally_connectable`.
    ExternalPage,
}

impl ContextKind {
    /// Every kind, in declaration order.
    pub const ALL: [Self; 9] = [
        Self::Background,
        Self::Popup,
        Self::Options,
        Self::ExtensionTab,
        Self::Offscreen,
        Self::ExtensionFrame,
        Self::ContentScript,
        Self::UserScript,
        Self::ExternalPage,
    ];

    const fn bit(self) -> u16 {
        1 << self as u16
    }

    /// Whether the document is the extension's own page, as opposed to
    /// script running in a web page.
    pub const fn is_extension_page(self) -> bool {
        ContextMask::EXTENSION_PAGES.contains(self)
    }

    /// Whether the context lives in a runtime-owned view or its own
    /// top-level tab document, rather than in a web page's renderer.
    pub const fn is_runtime_owned(self) -> bool {
        ContextMask::PRIVILEGED_PAGES.contains(self)
    }
}

/// A set of [`ContextKind`]s.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ContextMask(u16);

impl ContextMask {
    /// No context.
    pub const NONE: Self = Self(0);
    /// Every context.
    pub const ALL: Self = Self((1 << ContextKind::ALL.len()) - 1);
    /// Extension pages outside web renderers.
    pub const PRIVILEGED_PAGES: Self = Self::of(ContextKind::Background)
        .with(ContextKind::Popup)
        .with(ContextKind::Options)
        .with(ContextKind::ExtensionTab)
        .with(ContextKind::Offscreen);
    /// Every extension page, framed ones included.
    pub const EXTENSION_PAGES: Self = Self::PRIVILEGED_PAGES.with(ContextKind::ExtensionFrame);
    /// Scripts the extension runs inside web pages.
    pub const PAGE_SCRIPTS: Self =
        Self::of(ContextKind::ContentScript).with(ContextKind::UserScript);

    /// The set holding only `kind`.
    pub const fn of(kind: ContextKind) -> Self {
        Self(kind.bit())
    }

    /// This set plus `kind`.
    pub const fn with(self, kind: ContextKind) -> Self {
        Self(self.0 | kind.bit())
    }

    /// This set minus `kind`.
    pub const fn without(self, kind: ContextKind) -> Self {
        Self(self.0 & !kind.bit())
    }

    /// Both sets together.
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// What both sets share.
    pub const fn intersection(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }

    /// Whether `kind` is in the set.
    pub const fn contains(self, kind: ContextKind) -> bool {
        self.0 & kind.bit() != 0
    }

    /// Whether the set is empty.
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl fmt::Debug for ContextMask {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_set()
            .entries(ContextKind::ALL.iter().filter(|kind| self.contains(**kind)))
            .finish()
    }
}

/// A process-local, non-wrapping identity for one committed document, minted
/// by the host at commit. A new document in the same frame gets a new value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DocumentGeneration(NonZeroU64);

impl DocumentGeneration {
    /// A nonzero generation.
    pub const fn new(value: u64) -> Option<Self> {
        match NonZeroU64::new(value) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }

    /// The raw value, for host correlation only.
    pub const fn get(self) -> u64 {
        self.0.get()
    }
}

/// Chrome's `frameId`: 0 is a tab's main frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FrameId(u64);

impl FrameId {
    /// The main frame.
    pub const MAIN: Self = Self(0);

    /// A frame id assigned by the host's frame registry.
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// The raw value Chrome APIs report.
    pub const fn get(self) -> u64 {
        self.0
    }

    /// Whether this is the main frame.
    pub const fn is_main(self) -> bool {
        self.0 == 0
    }
}

/// A `userScripts` world id. The empty id is the default world.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct UserScriptWorldId(Arc<str>);

impl UserScriptWorldId {
    /// Validates a world id. Chrome reserves ids starting with `_`.
    pub fn parse(value: &str) -> Option<Self> {
        (value.len() <= MAX_USER_SCRIPT_WORLD_ID_BYTES
            && !value.starts_with('_')
            && !value.chars().any(char::is_control))
        .then(|| Self(value.into()))
    }

    /// The id.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The kinds of view the runtime itself owns.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ViewKind {
    /// The background.
    Background,
    /// The action popup.
    Popup,
    /// The options page.
    Options,
    /// An offscreen document.
    Offscreen,
}

impl ViewKind {
    const fn context(self) -> ContextKind {
        match self {
            Self::Background => ContextKind::Background,
            Self::Popup => ContextKind::Popup,
            Self::Options => ContextKind::Options,
            Self::Offscreen => ContextKind::Offscreen,
        }
    }

    const fn is_singleton(self) -> bool {
        matches!(self, Self::Background | Self::Offscreen)
    }
}

/// The URL scheme extension origins use in this profile.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ExtensionScheme(&'static str);

impl ExtensionScheme {
    /// `chrome-extension`, preferred for compatibility.
    pub const CHROME_EXTENSION: Self = Self("chrome-extension");
    /// `zephium-extension`, the proven fallback.
    pub const ZEPHIUM_EXTENSION: Self = Self("zephium-extension");

    /// The scheme name.
    pub const fn as_str(self) -> &'static str {
        self.0
    }

    /// The extension id whose origin `url` is in, if any.
    pub fn extension_of(self, url: &Url) -> Option<ExtensionId> {
        if url.scheme() != self.0 || url.port().is_some() {
            return None;
        }
        ExtensionId::parse(url.host_str()?)
    }
}

/// A document the engine committed, as the host observed it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommittedDocument {
    /// The host-minted generation of this document.
    pub generation: DocumentGeneration,
    /// The committed URL.
    pub url: Url,
    /// The committed security origin.
    pub origin: DocumentOrigin,
}

/// A committed document in one frame of an ordinary tab.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabDocument {
    /// The tab, identified by which engine view delivered the message.
    pub tab: ItemId,
    /// The frame, from the host's frame registry.
    pub frame: FrameId,
    /// The frame's committed document.
    pub committed: CommittedDocument,
}

/// Which script world a tab message says it came from. This is
/// renderer-reported, so binding checks it against host records.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClaimedWorld {
    /// The page's own world.
    Page,
    /// An install's isolated world, decoded from the world name.
    Isolated(ExtensionInstallId),
    /// An install's `userScripts` world.
    UserScript {
        /// The owning install.
        install: ExtensionInstallId,
        /// The world id.
        world: UserScriptWorldId,
    },
}

/// A world the host injected an install's scripts into.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum InjectedWorld {
    /// The install's isolated content-script world.
    Isolated,
    /// One of the install's `userScripts` worlds.
    UserScript(UserScriptWorldId),
}

/// The pages `externally_connectable.matches` lets message an extension.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExternallyConnectable {
    matcher: ContentScriptMatcher,
}

impl ExternallyConnectable {
    /// Parses `externally_connectable.matches`.
    pub fn parse<S: AsRef<str>>(matches: &[S]) -> Result<Self, ContentMatchError> {
        let none: &[S] = &[];
        Ok(Self {
            matcher: ContentScriptMatcher::new(matches, none, none, none, Default::default())?,
        })
    }

    fn admits(&self, committed: &CommittedDocument) -> bool {
        matches!(committed.url.scheme(), "http" | "https")
            && committed.origin.is_tuple_of(&committed.url)
            && self.matcher.matches(
                &FrameDocument {
                    url: &committed.url,
                    origin: &committed.origin,
                    creator_url: None,
                },
                false,
            )
    }
}

/// What the host knows about a running install when it starts a runtime
/// generation for it.
#[derive(Clone, Debug)]
pub struct InstallAdmission {
    /// The profile-scoped install.
    pub install: ExtensionInstallId,
    /// Its extension id, which is its origin host.
    pub extension_id: ExtensionId,
    /// The runtime generation being started.
    pub generation: ExtensionRuntimeGeneration,
    /// Development installation timing, from the browser's install record.
    pub unpacked: bool,
    /// Whether the person allowed it in incognito.
    pub allowed_in_incognito: bool,
    /// Its `externally_connectable` pages, if it declared any.
    pub externally_connectable: Option<ExternallyConnectable>,
}

/// Why a context could not be bound to a [`Sender`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BindError {
    /// No running install matches.
    UnknownInstall,
    /// The install's runtime generation moved on, or the view was closed.
    Stale,
    /// The view or document belongs to another profile's authority.
    WrongProfile,
    /// The install may not run in this incognito profile.
    IncognitoNotAllowed,
    /// Another install already has this extension id.
    DuplicateExtensionId,
    /// The generation is not newer than the running one.
    GenerationNotNewer,
    /// Only one view of this kind may exist per install.
    AlreadyOpen(ViewKind),
    /// The document is not the install's own extension page.
    NotExtensionDocument,
    /// The extension page is sandboxed and gets no API.
    Sandboxed,
    /// The page world of a web page has no extension API.
    NoExtensionContext,
    /// The host has not injected this world into this document.
    NotInjected,
    /// The page is not allowed to message the extension.
    NotExternallyConnectable,
    /// The profile's injection record budget is exhausted.
    TooManyInjections,
}

impl BindError {
    /// The error a call from an unbindable context settles with.
    pub fn to_api_error(&self) -> ApiError {
        match self {
            Self::Stale | Self::UnknownInstall => ApiError::context_invalidated(),
            _ => ApiError::unknown_method(),
        }
    }
}

/// A runtime-owned view the host opened through [`Authority::open_view`].
/// It cannot be cloned, so exactly one host object stands for the view.
#[derive(Debug, PartialEq, Eq)]
pub struct ExtensionView {
    serial: u64,
    profile: ProfileId,
    install: ExtensionInstallId,
    generation: ExtensionRuntimeGeneration,
    kind: ViewKind,
}

impl ExtensionView {
    /// The install that owns the view.
    pub fn install(&self) -> ExtensionInstallId {
        self.install
    }

    /// What the view is for.
    pub fn kind(&self) -> ViewKind {
        self.kind
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum TabRole {
    ExtensionPage,
    ContentScript,
    UserScript(UserScriptWorldId),
    ExternalPage,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Placement {
    View {
        serial: u64,
        kind: ViewKind,
        document: DocumentGeneration,
    },
    Tab {
        tab: ItemId,
        frame: FrameId,
        document: DocumentGeneration,
        role: TabRole,
    },
}

/// The identity of one calling context, for routing and per-context limits.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ContextKey {
    install: ExtensionInstallId,
    generation: ExtensionRuntimeGeneration,
    placement: Placement,
}

impl ContextKey {
    /// The install the context calls into.
    pub fn install(&self) -> ExtensionInstallId {
        self.install
    }
}

/// The authenticated caller of one API call, port message or event ack.
///
/// Only [`Authority`]'s binding functions make one. Each snippet below is
/// well formed except for the property it shows; the passing one first
/// proves the same shapes compile for types that allow them.
///
/// ```
/// use zephium_webext_runtime::{ExtensionId, Sender};
/// fn needs_clone<T: Clone>() {}
/// needs_clone::<Sender>();
/// fn parse(bytes: &[u8]) -> Option<ExtensionId> {
///     serde_json::from_slice(bytes).ok()
/// }
/// assert!(parse(b"\"abcdefghijklmnopabcdefghijklmnop\"").is_some());
/// ```
///
/// No struct literal, even one copying every field from a real sender:
///
/// ```compile_fail
/// fn forge(real: zephium_webext_runtime::Sender) -> zephium_webext_runtime::Sender {
///     zephium_webext_runtime::Sender { ..real }
/// }
/// ```
///
/// No deserialization from bytes:
///
/// ```compile_fail
/// fn forge(bytes: &[u8]) -> Option<zephium_webext_runtime::Sender> {
///     serde_json::from_slice(bytes).ok()
/// }
/// ```
///
/// No default value:
///
/// ```compile_fail
/// fn needs_default<T: Default>() {}
/// needs_default::<zephium_webext_runtime::Sender>();
/// ```
///
/// A runtime-owned view handle cannot be duplicated or made up either:
///
/// ```compile_fail
/// fn needs_clone<T: Clone>() {}
/// needs_clone::<zephium_webext_runtime::ExtensionView>();
/// ```
///
/// ```compile_fail
/// fn forge(real: zephium_webext_runtime::ExtensionView) -> zephium_webext_runtime::ExtensionView {
///     zephium_webext_runtime::ExtensionView { ..real }
/// }
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sender {
    profile: ProfileId,
    incognito: bool,
    scheme: ExtensionScheme,
    install: ExtensionInstallId,
    extension_id: ExtensionId,
    generation: ExtensionRuntimeGeneration,
    unpacked: bool,
    placement: Placement,
    url: Url,
    origin: DocumentOrigin,
}

impl Sender {
    /// The profile the context runs in.
    pub fn profile(&self) -> ProfileId {
        self.profile
    }

    /// Whether that profile is incognito.
    pub fn incognito(&self) -> bool {
        self.incognito
    }

    /// The install whose API the context calls. For an external page this
    /// is the extension it messages.
    pub fn install(&self) -> ExtensionInstallId {
        self.install
    }

    /// That install's extension id.
    pub fn extension_id(&self) -> &ExtensionId {
        &self.extension_id
    }

    /// `MessageSender.id`: the sending extension, absent for web pages.
    pub fn sender_extension_id(&self) -> Option<&ExtensionId> {
        (self.kind() != ContextKind::ExternalPage).then_some(&self.extension_id)
    }

    /// The runtime generation the context was bound under.
    pub fn generation(&self) -> ExtensionRuntimeGeneration {
        self.generation
    }

    /// Whether the browser admitted this install as unpacked development code.
    pub fn unpacked(&self) -> bool {
        self.unpacked
    }

    /// The extension scheme of this profile.
    pub fn scheme(&self) -> ExtensionScheme {
        self.scheme
    }

    /// The kind of context.
    pub fn kind(&self) -> ContextKind {
        match &self.placement {
            Placement::View { kind, .. } => kind.context(),
            Placement::Tab { frame, role, .. } => match role {
                TabRole::ExtensionPage if frame.is_main() => ContextKind::ExtensionTab,
                TabRole::ExtensionPage => ContextKind::ExtensionFrame,
                TabRole::ContentScript => ContextKind::ContentScript,
                TabRole::UserScript(_) => ContextKind::UserScript,
                TabRole::ExternalPage => ContextKind::ExternalPage,
            },
        }
    }

    /// The tab, for contexts inside one.
    pub fn tab(&self) -> Option<ItemId> {
        match &self.placement {
            Placement::View { .. } => None,
            Placement::Tab { tab, .. } => Some(*tab),
        }
    }

    /// The frame, for contexts inside a tab.
    pub fn frame(&self) -> Option<FrameId> {
        match &self.placement {
            Placement::View { .. } => None,
            Placement::Tab { frame, .. } => Some(*frame),
        }
    }

    /// The document generation the context was bound to.
    pub fn document(&self) -> DocumentGeneration {
        match &self.placement {
            Placement::View { document, .. } | Placement::Tab { document, .. } => *document,
        }
    }

    /// The `userScripts` world, for user scripts.
    pub fn user_script_world(&self) -> Option<&UserScriptWorldId> {
        match &self.placement {
            Placement::Tab {
                role: TabRole::UserScript(world),
                ..
            } => Some(world),
            _ => None,
        }
    }

    /// The committed document URL (`MessageSender.url`).
    pub fn url(&self) -> &Url {
        &self.url
    }

    /// The committed document origin (`MessageSender.origin`).
    pub fn origin(&self) -> &DocumentOrigin {
        &self.origin
    }

    /// The routing and accounting identity of the context.
    pub fn context_key(&self) -> ContextKey {
        ContextKey {
            install: self.install,
            generation: self.generation,
            placement: self.placement.clone(),
        }
    }
}

#[derive(Debug)]
struct InstallEntry {
    extension_id: ExtensionId,
    generation: ExtensionRuntimeGeneration,
    unpacked: bool,
    externally_connectable: Option<ExternallyConnectable>,
}

#[derive(Debug)]
struct OpenView {
    install: ExtensionInstallId,
    kind: ViewKind,
}

#[derive(Debug, Default)]
struct FrameInjections {
    document: Option<DocumentGeneration>,
    worlds: BTreeSet<(ExtensionInstallId, InjectedWorld)>,
}

/// One profile's binding authority.
#[derive(Debug)]
pub struct Authority {
    profile: ProfileId,
    incognito: bool,
    scheme: ExtensionScheme,
    installs: HashMap<ExtensionInstallId, InstallEntry>,
    by_extension_id: HashMap<ExtensionId, ExtensionInstallId>,
    views: HashMap<u64, OpenView>,
    next_view: u64,
    injections: HashMap<(ItemId, FrameId), FrameInjections>,
    injection_records: usize,
}

impl Authority {
    /// An authority for one profile. `incognito` is the profile's partition
    /// class, which the host fixed when it created the profile's context.
    pub fn new(profile: ProfileId, incognito: bool, scheme: ExtensionScheme) -> Self {
        Self {
            profile,
            incognito,
            scheme,
            installs: HashMap::new(),
            by_extension_id: HashMap::new(),
            views: HashMap::new(),
            next_view: 1,
            injections: HashMap::new(),
            injection_records: 0,
        }
    }

    /// The profile.
    pub fn profile(&self) -> ProfileId {
        self.profile
    }

    /// Starts a runtime generation for an install. A newer generation of a
    /// running install replaces it: its views go stale and its injection
    /// records are dropped.
    pub fn admit(&mut self, admission: InstallAdmission) -> Result<(), BindError> {
        if self.incognito && !admission.allowed_in_incognito {
            return Err(BindError::IncognitoNotAllowed);
        }
        if let Some(owner) = self.by_extension_id.get(&admission.extension_id) {
            if *owner != admission.install {
                return Err(BindError::DuplicateExtensionId);
            }
        }
        if let Some(running) = self.installs.get(&admission.install) {
            if admission.generation <= running.generation
                || running.extension_id != admission.extension_id
            {
                return Err(BindError::GenerationNotNewer);
            }
            self.retire(admission.install);
        }
        self.by_extension_id
            .insert(admission.extension_id.clone(), admission.install);
        self.installs.insert(
            admission.install,
            InstallEntry {
                extension_id: admission.extension_id,
                generation: admission.generation,
                unpacked: admission.unpacked,
                externally_connectable: admission.externally_connectable,
            },
        );
        Ok(())
    }

    /// Stops an install: every view and injection record it had goes stale.
    pub fn retire(&mut self, install: ExtensionInstallId) {
        if let Some(entry) = self.installs.remove(&install) {
            self.by_extension_id.remove(&entry.extension_id);
        }
        self.views.retain(|_, view| view.install != install);
        let mut removed = 0;
        for frame in self.injections.values_mut() {
            let before = frame.worlds.len();
            frame.worlds.retain(|(owner, _)| *owner != install);
            removed += before - frame.worlds.len();
        }
        self.injections.retain(|_, frame| !frame.worlds.is_empty());
        self.injection_records -= removed;
    }

    /// Records that the host is creating a runtime-owned view.
    pub fn open_view(
        &mut self,
        install: ExtensionInstallId,
        kind: ViewKind,
    ) -> Result<ExtensionView, BindError> {
        let entry = self
            .installs
            .get(&install)
            .ok_or(BindError::UnknownInstall)?;
        if kind.is_singleton()
            && self
                .views
                .values()
                .any(|view| view.install == install && view.kind == kind)
        {
            return Err(BindError::AlreadyOpen(kind));
        }
        let serial = self.next_view;
        self.next_view = serial.checked_add(1).ok_or(BindError::Stale)?;
        self.views.insert(serial, OpenView { install, kind });
        Ok(ExtensionView {
            serial,
            profile: self.profile,
            install,
            generation: entry.generation,
            kind,
        })
    }

    /// Records that the host destroyed a view.
    pub fn close_view(&mut self, view: ExtensionView) {
        if view.profile == self.profile {
            self.views.remove(&view.serial);
        }
    }

    /// Binds a message delivered by a runtime-owned view.
    pub fn bind_view(
        &self,
        view: &ExtensionView,
        document: &CommittedDocument,
    ) -> Result<Sender, BindError> {
        if view.profile != self.profile {
            return Err(BindError::WrongProfile);
        }
        if !self.views.contains_key(&view.serial) {
            return Err(BindError::Stale);
        }
        let entry = self.current(view.install, view.generation)?;
        self.check_extension_document(entry, document)?;
        Ok(self.sender(
            view.install,
            entry,
            Placement::View {
                serial: view.serial,
                kind: view.kind,
                document: document.generation,
            },
            document,
        ))
    }

    /// Binds a message delivered by an ordinary tab's view, from the frame
    /// and world the host attributes it to.
    pub fn bind_tab(
        &self,
        document: &TabDocument,
        world: &ClaimedWorld,
    ) -> Result<Sender, BindError> {
        let committed = &document.committed;
        let (install, role) = match world {
            ClaimedWorld::Page => {
                let extension = self
                    .scheme
                    .extension_of(&committed.url)
                    .ok_or(BindError::NoExtensionContext)?;
                let install = *self
                    .by_extension_id
                    .get(&extension)
                    .ok_or(BindError::UnknownInstall)?;
                (install, TabRole::ExtensionPage)
            }
            ClaimedWorld::Isolated(install) => {
                self.injected(document, *install, &InjectedWorld::Isolated)?;
                (*install, TabRole::ContentScript)
            }
            ClaimedWorld::UserScript { install, world } => {
                self.injected(
                    document,
                    *install,
                    &InjectedWorld::UserScript(world.clone()),
                )?;
                (*install, TabRole::UserScript(world.clone()))
            }
        };
        let entry = self
            .installs
            .get(&install)
            .ok_or(BindError::UnknownInstall)?;
        if role == TabRole::ExtensionPage {
            self.check_extension_document(entry, committed)?;
        }
        Ok(self.sender(
            install,
            entry,
            Placement::Tab {
                tab: document.tab,
                frame: document.frame,
                document: committed.generation,
                role,
            },
            committed,
        ))
    }

    /// Binds a web page's `runtime.sendMessage`/`connect` to `target`.
    /// The page names the target, so it is checked against the target's
    /// own `externally_connectable` declaration.
    pub fn bind_external_page(
        &self,
        document: &TabDocument,
        target: &ExtensionId,
    ) -> Result<Sender, BindError> {
        let install = *self
            .by_extension_id
            .get(target)
            .ok_or(BindError::NotExternallyConnectable)?;
        let entry = self
            .installs
            .get(&install)
            .ok_or(BindError::NotExternallyConnectable)?;
        let admitted = entry
            .externally_connectable
            .as_ref()
            .is_some_and(|allowed| allowed.admits(&document.committed));
        if !admitted {
            return Err(BindError::NotExternallyConnectable);
        }
        Ok(self.sender(
            install,
            entry,
            Placement::Tab {
                tab: document.tab,
                frame: document.frame,
                document: document.committed.generation,
                role: TabRole::ExternalPage,
            },
            &document.committed,
        ))
    }

    /// Records that the host injected `install`'s scripts into `world` of
    /// this exact document.
    pub fn record_injection(
        &mut self,
        tab: ItemId,
        frame: FrameId,
        document: DocumentGeneration,
        install: ExtensionInstallId,
        world: InjectedWorld,
    ) -> Result<(), BindError> {
        if !self.installs.contains_key(&install) {
            return Err(BindError::UnknownInstall);
        }
        self.document_committed(tab, frame, document);
        if self.injection_records >= MAX_INJECTION_RECORDS_PER_PROFILE {
            return Err(BindError::TooManyInjections);
        }
        let entry = self.injections.entry((tab, frame)).or_default();
        entry.document = Some(document);
        if entry.worlds.insert((install, world)) {
            self.injection_records += 1;
        }
        Ok(())
    }

    /// The frame committed `document`; records for earlier documents go.
    pub fn document_committed(
        &mut self,
        tab: ItemId,
        frame: FrameId,
        document: DocumentGeneration,
    ) {
        if let Some(entry) = self.injections.get(&(tab, frame)) {
            if entry.document != Some(document) {
                self.frame_removed(tab, frame);
            }
        }
    }

    /// The frame is gone.
    pub fn frame_removed(&mut self, tab: ItemId, frame: FrameId) {
        if let Some(entry) = self.injections.remove(&(tab, frame)) {
            self.injection_records -= entry.worlds.len();
        }
    }

    /// The tab is gone.
    pub fn tab_closed(&mut self, tab: ItemId) {
        let mut removed = 0;
        self.injections.retain(|(owner, _), entry| {
            let keep = *owner != tab;
            if !keep {
                removed += entry.worlds.len();
            }
            keep
        });
        self.injection_records -= removed;
    }

    fn current(
        &self,
        install: ExtensionInstallId,
        generation: ExtensionRuntimeGeneration,
    ) -> Result<&InstallEntry, BindError> {
        let entry = self
            .installs
            .get(&install)
            .ok_or(BindError::UnknownInstall)?;
        if entry.generation != generation {
            return Err(BindError::Stale);
        }
        Ok(entry)
    }

    fn check_extension_document(
        &self,
        entry: &InstallEntry,
        document: &CommittedDocument,
    ) -> Result<(), BindError> {
        if self.scheme.extension_of(&document.url).as_ref() != Some(&entry.extension_id) {
            return Err(BindError::NotExtensionDocument);
        }
        if !document.origin.is_tuple_of(&document.url) {
            return Err(BindError::Sandboxed);
        }
        Ok(())
    }

    fn injected(
        &self,
        document: &TabDocument,
        install: ExtensionInstallId,
        world: &InjectedWorld,
    ) -> Result<(), BindError> {
        let recorded = self
            .injections
            .get(&(document.tab, document.frame))
            .is_some_and(|entry| {
                entry.document == Some(document.committed.generation)
                    && entry.worlds.contains(&(install, world.clone()))
            });
        if recorded {
            Ok(())
        } else {
            Err(BindError::NotInjected)
        }
    }

    fn sender(
        &self,
        install: ExtensionInstallId,
        entry: &InstallEntry,
        placement: Placement,
        document: &CommittedDocument,
    ) -> Sender {
        Sender {
            profile: self.profile,
            incognito: self.incognito,
            scheme: self.scheme,
            install,
            extension_id: entry.extension_id.clone(),
            generation: entry.generation,
            unpacked: entry.unpacked,
            placement,
            url: document.url.clone(),
            origin: document.origin.clone(),
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) const EXTENSION: &str = "abcdefghijklmnopabcdefghijklmnop";
    pub(crate) const OTHER: &str = "ponmlkjihgfedcbaponmlkjihgfedcba";

    pub(crate) fn profile() -> ProfileId {
        ProfileId::from(1)
    }

    pub(crate) fn install() -> ExtensionInstallId {
        ExtensionInstallId::from(10)
    }

    pub(crate) fn generation(value: u64) -> ExtensionRuntimeGeneration {
        ExtensionRuntimeGeneration::new(value).expect("nonzero")
    }

    pub(crate) fn doc(value: u64) -> DocumentGeneration {
        DocumentGeneration::new(value).expect("nonzero")
    }

    pub(crate) fn committed(url: &str, generation: u64) -> CommittedDocument {
        let url = Url::parse(url).expect("URL");
        CommittedDocument {
            generation: doc(generation),
            origin: DocumentOrigin::of_url(&url),
            url,
        }
    }

    pub(crate) fn admission(install: ExtensionInstallId, id: &str, value: u64) -> InstallAdmission {
        InstallAdmission {
            install,
            extension_id: ExtensionId::parse(id).expect("id"),
            generation: generation(value),
            unpacked: false,
            allowed_in_incognito: false,
            externally_connectable: None,
        }
    }

    pub(crate) fn authority() -> Authority {
        let mut authority = Authority::new(profile(), false, ExtensionScheme::CHROME_EXTENSION);
        authority
            .admit(admission(install(), EXTENSION, 1))
            .expect("admit");
        authority
    }

    pub(crate) fn background_sender() -> Sender {
        let mut authority = authority();
        let view = authority
            .open_view(install(), ViewKind::Background)
            .expect("view");
        authority
            .bind_view(
                &view,
                &committed(&format!("chrome-extension://{EXTENSION}/bg.html"), 1),
            )
            .expect("bind")
    }

    pub(crate) fn content_sender(url: &str) -> Sender {
        let mut authority = authority();
        let tab = ItemId::from(5);
        authority
            .record_injection(
                tab,
                FrameId::MAIN,
                doc(3),
                install(),
                InjectedWorld::Isolated,
            )
            .expect("record");
        authority
            .bind_tab(
                &TabDocument {
                    tab,
                    frame: FrameId::MAIN,
                    committed: committed(url, 3),
                },
                &ClaimedWorld::Isolated(install()),
            )
            .expect("bind")
    }

    fn extension_url(path: &str) -> String {
        format!("chrome-extension://{EXTENSION}/{path}")
    }

    #[test]
    fn development_timing_is_bound_to_the_admitted_install_generation() {
        let mut authority = authority();
        let page = committed(&extension_url("bg.html"), 1);
        let view = authority
            .open_view(install(), ViewKind::Background)
            .unwrap();
        assert!(!authority.bind_view(&view, &page).unwrap().unpacked());
        let mut replacement = admission(install(), EXTENSION, 2);
        replacement.unpacked = true;
        authority.admit(replacement).unwrap();
        assert_eq!(authority.bind_view(&view, &page), Err(BindError::Stale));
        let view = authority
            .open_view(install(), ViewKind::Background)
            .unwrap();
        assert!(authority.bind_view(&view, &page).unwrap().unpacked());
    }

    #[test]
    fn a_view_binds_only_to_its_own_extension_origin() {
        let mut authority = authority();
        let popup = authority
            .open_view(install(), ViewKind::Popup)
            .expect("popup");
        let sender = authority
            .bind_view(&popup, &committed(&extension_url("popup.html"), 2))
            .expect("bind");
        assert_eq!(sender.kind(), ContextKind::Popup);
        assert_eq!(sender.tab(), None);
        assert_eq!(sender.frame(), None);
        assert_eq!(sender.document(), doc(2));
        assert_eq!(
            sender.origin().serialize(),
            format!("chrome-extension://{EXTENSION}")
        );
        assert_eq!(
            sender.sender_extension_id().map(ExtensionId::as_str),
            Some(EXTENSION)
        );

        for url in [
            "https://example.com/".to_owned(),
            format!("chrome-extension://{OTHER}/popup.html"),
            format!("zephium-extension://{EXTENSION}/popup.html"),
            format!("chrome-extension://{EXTENSION}:1/popup.html"),
        ] {
            assert_eq!(
                authority.bind_view(&popup, &committed(&url, 3)),
                Err(BindError::NotExtensionDocument),
                "{url}"
            );
        }
    }

    #[test]
    fn sandboxed_extension_pages_get_no_api() {
        let mut authority = authority();
        let view = authority
            .open_view(install(), ViewKind::Options)
            .expect("options");
        let mut document = committed(&extension_url("sandbox.html"), 4);
        document.origin = DocumentOrigin::opaque(Some(&document.url));
        assert_eq!(
            authority.bind_view(&view, &document),
            Err(BindError::Sandboxed)
        );
    }

    #[test]
    fn closed_views_and_old_generations_are_stale() {
        let mut authority = authority();
        let view = authority
            .open_view(install(), ViewKind::Background)
            .expect("view");
        assert_eq!(
            authority.open_view(install(), ViewKind::Background),
            Err(BindError::AlreadyOpen(ViewKind::Background))
        );
        let page = committed(&extension_url("bg.html"), 1);
        authority
            .admit(admission(install(), EXTENSION, 2))
            .expect("newer generation");
        assert_eq!(authority.bind_view(&view, &page), Err(BindError::Stale));
        assert_eq!(
            authority.admit(admission(install(), EXTENSION, 2)),
            Err(BindError::GenerationNotNewer)
        );

        let fresh = authority
            .open_view(install(), ViewKind::Background)
            .expect("view");
        assert_eq!(
            authority.bind_view(&fresh, &page).map(|s| s.generation()),
            Ok(generation(2))
        );
        authority.close_view(fresh);
        let reopened = authority
            .open_view(install(), ViewKind::Background)
            .expect("reopen after close");
        authority.retire(install());
        assert_eq!(authority.bind_view(&reopened, &page), Err(BindError::Stale));
    }

    #[test]
    fn views_do_not_cross_profiles() {
        let mut first = authority();
        let mut second =
            Authority::new(ProfileId::from(2), false, ExtensionScheme::CHROME_EXTENSION);
        second
            .admit(admission(install(), EXTENSION, 1))
            .expect("admit");
        let view = first.open_view(install(), ViewKind::Popup).expect("view");
        let page = committed(&extension_url("popup.html"), 1);
        assert_eq!(second.bind_view(&view, &page), Err(BindError::WrongProfile));
    }

    #[test]
    fn extension_pages_in_tabs_are_tab_or_frame_by_the_host_frame() {
        let authority = authority();
        let tab = ItemId::from(5);
        let main = TabDocument {
            tab,
            frame: FrameId::MAIN,
            committed: committed(&extension_url("options.html"), 7),
        };
        let sender = authority.bind_tab(&main, &ClaimedWorld::Page).expect("tab");
        assert_eq!(sender.kind(), ContextKind::ExtensionTab);
        assert_eq!(sender.tab(), Some(tab));

        let framed = TabDocument {
            frame: FrameId::new(9),
            ..main.clone()
        };
        let sender = authority
            .bind_tab(&framed, &ClaimedWorld::Page)
            .expect("frame");
        assert_eq!(sender.kind(), ContextKind::ExtensionFrame);
        assert_eq!(sender.frame(), Some(FrameId::new(9)));

        let web = TabDocument {
            committed: committed("https://example.com/", 8),
            ..main
        };
        assert_eq!(
            authority.bind_tab(&web, &ClaimedWorld::Page),
            Err(BindError::NoExtensionContext)
        );
    }

    #[test]
    fn content_scripts_need_a_host_injection_record_for_that_document() {
        let mut authority = authority();
        let tab = ItemId::from(5);
        let frame = FrameId::new(2);
        let document = TabDocument {
            tab,
            frame,
            committed: committed("https://example.com/", 3),
        };
        let claim = ClaimedWorld::Isolated(install());
        assert_eq!(
            authority.bind_tab(&document, &claim),
            Err(BindError::NotInjected)
        );

        authority
            .record_injection(tab, frame, doc(3), install(), InjectedWorld::Isolated)
            .expect("record");
        let sender = authority.bind_tab(&document, &claim).expect("bind");
        assert_eq!(sender.kind(), ContextKind::ContentScript);
        assert_eq!((sender.tab(), sender.frame()), (Some(tab), Some(frame)));

        let user = ClaimedWorld::UserScript {
            install: install(),
            world: UserScriptWorldId::parse("w").expect("world"),
        };
        assert_eq!(
            authority.bind_tab(&document, &user),
            Err(BindError::NotInjected)
        );

        let other = ExtensionInstallId::from(11);
        assert_eq!(
            authority.bind_tab(&document, &ClaimedWorld::Isolated(other)),
            Err(BindError::NotInjected)
        );

        let navigated = TabDocument {
            committed: committed("https://example.com/", 4),
            ..document.clone()
        };
        assert_eq!(
            authority.bind_tab(&navigated, &claim),
            Err(BindError::NotInjected)
        );
        authority.document_committed(tab, frame, doc(4));
        assert_eq!(
            authority.bind_tab(&document, &claim),
            Err(BindError::NotInjected)
        );
        assert_eq!(authority.injection_records, 0);
    }

    #[test]
    fn injection_records_are_released_with_frames_tabs_and_installs() {
        let mut authority = authority();
        let tab = ItemId::from(5);
        for frame in 0..4 {
            authority
                .record_injection(
                    tab,
                    FrameId::new(frame),
                    doc(1),
                    install(),
                    InjectedWorld::Isolated,
                )
                .expect("record");
        }
        authority.frame_removed(tab, FrameId::new(0));
        assert_eq!(authority.injection_records, 3);
        authority.tab_closed(tab);
        assert_eq!(authority.injection_records, 0);
        authority
            .record_injection(
                tab,
                FrameId::MAIN,
                doc(1),
                install(),
                InjectedWorld::Isolated,
            )
            .expect("record");
        authority.retire(install());
        assert_eq!(authority.injection_records, 0);
        assert_eq!(
            authority.record_injection(
                tab,
                FrameId::MAIN,
                doc(1),
                install(),
                InjectedWorld::Isolated
            ),
            Err(BindError::UnknownInstall)
        );
    }

    #[test]
    fn external_pages_need_the_targets_declaration() {
        let mut authority = Authority::new(profile(), false, ExtensionScheme::CHROME_EXTENSION);
        let mut declared = admission(install(), EXTENSION, 1);
        declared.externally_connectable =
            Some(ExternallyConnectable::parse(&["https://*.example.com/*"]).expect("matches"));
        authority.admit(declared).expect("admit");
        authority
            .admit(admission(ExtensionInstallId::from(11), OTHER, 1))
            .expect("admit");
        let page = TabDocument {
            tab: ItemId::from(5),
            frame: FrameId::MAIN,
            committed: committed("https://www.example.com/app", 1),
        };
        let target = ExtensionId::parse(EXTENSION).expect("id");
        let sender = authority.bind_external_page(&page, &target).expect("bind");
        assert_eq!(sender.kind(), ContextKind::ExternalPage);
        assert_eq!(sender.sender_extension_id(), None);
        assert_eq!(sender.install(), install());

        let undeclared = ExtensionId::parse(OTHER).expect("id");
        assert_eq!(
            authority.bind_external_page(&page, &undeclared),
            Err(BindError::NotExternallyConnectable)
        );
        let elsewhere = TabDocument {
            committed: committed("https://evil.test/", 2),
            ..page.clone()
        };
        assert_eq!(
            authority.bind_external_page(&elsewhere, &target),
            Err(BindError::NotExternallyConnectable)
        );
        let mut sandboxed = page;
        sandboxed.committed.origin = DocumentOrigin::opaque(Some(&sandboxed.committed.url));
        assert_eq!(
            authority.bind_external_page(&sandboxed, &target),
            Err(BindError::NotExternallyConnectable)
        );
    }

    #[test]
    fn incognito_and_duplicate_ids_are_refused_at_admission() {
        let mut incognito = Authority::new(profile(), true, ExtensionScheme::CHROME_EXTENSION);
        assert_eq!(
            incognito.admit(admission(install(), EXTENSION, 1)),
            Err(BindError::IncognitoNotAllowed)
        );
        let mut allowed = admission(install(), EXTENSION, 1);
        allowed.allowed_in_incognito = true;
        incognito.admit(allowed).expect("allowed");

        let mut authority = authority();
        assert_eq!(
            authority.admit(admission(ExtensionInstallId::from(11), EXTENSION, 1)),
            Err(BindError::DuplicateExtensionId)
        );
    }

    #[test]
    fn masks_hold_the_section_six_partition() {
        assert!(!ContextMask::PRIVILEGED_PAGES.contains(ContextKind::ExtensionFrame));
        assert!(ContextMask::EXTENSION_PAGES.contains(ContextKind::ExtensionFrame));
        assert!(ContextKind::ExtensionTab.is_runtime_owned());
        assert!(!ContextKind::ContentScript.is_extension_page());
        assert_eq!(
            ContextMask::ALL,
            ContextMask::EXTENSION_PAGES
                .union(ContextMask::PAGE_SCRIPTS)
                .with(ContextKind::ExternalPage)
        );
        assert!(ContextMask::ALL
            .without(ContextKind::Popup)
            .intersection(ContextMask::of(ContextKind::Popup))
            .is_empty());
    }

    #[test]
    fn user_script_world_ids_follow_chrome_rules() {
        assert!(UserScriptWorldId::parse("").is_some());
        assert!(UserScriptWorldId::parse("_reserved").is_none());
        assert!(UserScriptWorldId::parse(&"w".repeat(257)).is_none());
    }
}
