//! Whether a post's renderer claims are consistent with host state.
//!
//! The view that delivered a post is a host fact. Its frame id, document
//! sequence, world name and URI are claims made by the renderer; they only
//! select among records the host already holds and never add to them.

use url::Url;
use zephium_core::ids::ExtensionInstallId;
use zephium_webext_runtime::context::InjectedWorld;
use zephium_webext_runtime::pattern::DocumentOrigin;
use zephium_webkit_web_process_protocol::protocol::{document_origin, Post};

use super::frames::{Frame, FrameTable};
use super::plan::ResolvedPlan;

/// Why a post produced no sender. Every refusal is counted, never guessed
/// around.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Refusal {
    /// The view has no live attachment.
    Unattached,
    /// The post names an attachment generation that is not current.
    StaleGeneration,
    /// The world is not one the host attached for this view.
    ForgedWorld,
    /// An extension-owned view claimed a script world.
    ExtensionViewWorld,
    /// No frame record arrived for the claimed frame in time.
    UnknownFrame,
    /// The claimed document was already replaced.
    StaleDocument,
    /// A subframe of an extension-owned view.
    ExtensionViewSubframe,
    /// The claimed URI does not parse.
    InvalidUri,
    /// The claimed URI is not in the recorded document's origin.
    CrossOriginUri,
    /// The world is not admitted for the document's origin.
    NotAdmitted,
    /// The main document disagrees with what the host committed.
    HostMismatch,
    /// The post did not fit the wait buffer.
    BufferFull,
    /// The payload exceeds the context's message limit.
    Oversized,
    /// The host has no injection record for that world and document.
    NotInjected,
    /// The authority refused to bind the context.
    Unbound,
    /// The view is shutting down or gone.
    Closed,
    /// The transport message was not a well-formed event.
    Malformed,
}

/// What the post is waiting for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Pending {
    Frame,
    Host,
}

impl Pending {
    pub fn refusal(self) -> Refusal {
        match self {
            Self::Frame => Refusal::UnknownFrame,
            Self::Host => Refusal::HostMismatch,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum AttributedWorld {
    Page,
    Script {
        install: ExtensionInstallId,
        world: InjectedWorld,
    },
}

#[derive(Debug)]
pub(crate) struct Attributed<'a> {
    pub frame: &'a Frame,
    pub world: AttributedWorld,
    pub url: Url,
    pub origin: DocumentOrigin,
}

#[derive(Debug)]
pub(crate) enum Verdict<'a> {
    Attributed(Attributed<'a>),
    Wait(Pending),
    Refused(Refusal),
}

/// The host state a post is checked against.
pub(crate) struct ClaimContext<'a> {
    pub owned: bool,
    /// The generation of the attachment the module holds, if any.
    pub generation: Option<u64>,
    /// The plan that attachment carried.
    pub attached: Option<&'a ResolvedPlan>,
    /// The host's current plan, which may be narrower.
    pub plan: &'a ResolvedPlan,
    pub frames: &'a FrameTable,
    /// The main-frame URL the host saw commit, if any.
    pub host_main: Option<&'a Url>,
}

pub(crate) fn assess<'a>(context: &ClaimContext<'a>, post: &Post) -> Verdict<'a> {
    let (Some(generation), Some(attached)) = (context.generation, context.attached) else {
        return Verdict::Refused(Refusal::Unattached);
    };
    if post.generation != generation {
        return Verdict::Refused(Refusal::StaleGeneration);
    }
    let page_world = post.world.is_empty();
    if !page_world {
        if context.owned {
            return Verdict::Refused(Refusal::ExtensionViewWorld);
        }
        if !attached.worlds.contains_key(&post.world)
            || !context.plan.worlds.contains_key(&post.world)
        {
            return Verdict::Refused(Refusal::ForgedWorld);
        }
    }
    let Some(frame) = context.frames.get(post.frame_id) else {
        return Verdict::Wait(Pending::Frame);
    };
    if post.document_seq > frame.document.seq {
        return Verdict::Wait(Pending::Frame);
    }
    if post.document_seq < frame.document.seq {
        return Verdict::Refused(Refusal::StaleDocument);
    }
    if context.owned && !frame.main {
        return Verdict::Refused(Refusal::ExtensionViewSubframe);
    }
    let Ok(url) = Url::parse(&post.uri) else {
        return Verdict::Refused(Refusal::InvalidUri);
    };
    let Some(origin) = frame.document.origin.as_deref() else {
        return Verdict::Refused(Refusal::NotAdmitted);
    };
    if document_origin(url.as_str()).as_deref() != Some(origin) {
        return Verdict::Refused(Refusal::CrossOriginUri);
    }
    let (world, document_origin_value) = if page_world {
        let (Some(_), Some(pages)) = (
            attached.admits_page(origin),
            context.plan.admits_page(origin),
        ) else {
            return Verdict::Refused(Refusal::NotAdmitted);
        };
        let sandboxed = pages.sandboxed.contains(url.path());
        let value = if sandboxed {
            DocumentOrigin::opaque(Some(&url))
        } else {
            DocumentOrigin::of_url(&url)
        };
        (AttributedWorld::Page, value)
    } else {
        let (Some(_), Some(grant)) = (
            attached.admits_world(&post.world, origin),
            context.plan.admits_world(&post.world, origin),
        ) else {
            return Verdict::Refused(Refusal::NotAdmitted);
        };
        (
            AttributedWorld::Script {
                install: grant.install,
                world: grant.world.clone(),
            },
            DocumentOrigin::of_url(&url),
        )
    };
    if frame.main {
        let host = context
            .host_main
            .and_then(|committed| document_origin(committed.as_str()));
        match host {
            Some(host) if host == origin => {}
            Some(_) => return Verdict::Wait(Pending::Host),
            None if context.owned => return Verdict::Wait(Pending::Host),
            None => {}
        }
    }
    Verdict::Attributed(Attributed {
        frame,
        world,
        url,
        origin: document_origin_value,
    })
}
