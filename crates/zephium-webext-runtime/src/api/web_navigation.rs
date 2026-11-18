//! Frame queries and authenticated native navigation observations.

use serde_json::{json, Value};
use url::Url;
use zephium_core::ids::ItemId;

use crate::browser::{self, BrowserPort, BrowserTab};
use crate::context::{DocumentGeneration, FrameId, Sender};
use crate::dispatch::{ApiTable, Invocation, RegistrationError};
use crate::events::EventFacts;
use crate::pattern::DocumentOrigin;
use crate::permissions::{InstallPermissions, Target};
use crate::ApiError;

pub(crate) const METHODS: &[&str] = &["webNavigation.getFrame", "webNavigation.getAllFrames"];
/// Maximum frames considered in one browser observation.
pub const MAX_FRAMES: usize = 4_096;

/// One native-authenticated ordinary-tab document. Never constructed from page claims.
#[derive(Clone, Debug)]
pub struct BrowserFrame {
    /// Logical native tab owning the frame.
    pub tab: ItemId,
    /// Stable Chrome frame identity, zero for the main frame.
    pub frame: FrameId,
    /// Current document generation in this frame.
    pub document: DocumentGeneration,
    /// Opaque browser-issued document identity.
    pub document_id: String,
    /// Committed document address.
    pub url: Url,
    /// Native-authenticated origin, including an opaque document's precursor.
    pub origin: DocumentOrigin,
    /// Parent frame, absent for the main frame.
    pub parent_frame_id: Option<FrameId>,
    /// Parent's document identity, absent for the main frame.
    pub parent_document_id: Option<String>,
    /// Chrome lifecycle, normally active.
    pub document_lifecycle: String,
    /// Chrome frame type: outermost_frame, sub_frame, or fenced_frame.
    pub frame_type: String,
    /// Whether the last navigation failed.
    pub error_occurred: bool,
}

impl BrowserFrame {
    /// Checks the frame's actual origin with the tab's main-document activeTab fence.
    pub fn check_access(
        &self,
        sender: &Sender,
        permissions: &InstallPermissions,
        tab: &BrowserTab,
    ) -> Result<(), ApiError> {
        if self.tab != tab.tab.id()
            || (self.frame == FrameId::MAIN && self.document != tab.document)
        {
            return Err(ApiError::context_invalidated());
        }
        permissions.check_target(
            sender,
            &Target {
                profile: sender.profile(),
                tab: self.tab,
                frame: self.frame,
                main_document: tab.document,
                url: &self.url,
                origin: &self.origin,
            },
        )
    }

    /// Chrome's frame details. Process identities are intentionally opaque to extensions.
    pub fn details(&self, include_frame: bool) -> Value {
        let mut result = json!({"errorOccurred": self.error_occurred, "url": self.url.as_str(), "parentFrameId": self.parent_frame_id.map_or(-1, |id| id.get() as i64), "documentId": self.document_id, "documentLifecycle": self.document_lifecycle, "frameType": self.frame_type});
        if let Some(parent) = &self.parent_document_id {
            result["parentDocumentId"] = parent.clone().into();
        }
        if include_frame {
            result["frameId"] = self.frame.get().into();
            result["processId"] = (-1).into();
        }
        result
    }
}

/// Validates a bounded, duplicate-free native observation for the exact ordinary tab.
pub fn frames(
    host: &impl BrowserPort,
    sender: &Sender,
    tab: &BrowserTab,
) -> Result<Vec<BrowserFrame>, ApiError> {
    let frames = host.frames(sender, tab.tab.id())?;
    if frames.len() > MAX_FRAMES {
        return Err(ApiError::too_many_pending());
    }
    let mut ids = std::collections::BTreeSet::new();
    let mut documents = std::collections::BTreeSet::new();
    for frame in &frames {
        if frame.tab != tab.tab.id()
            || frame.frame.get() > i32::MAX as u64
            || frame.document_id.is_empty()
            || frame.document_id.len() > 256
            || frame.document_id.contains('\0')
            || !matches!(
                frame.document_lifecycle.as_str(),
                "prerender" | "active" | "cached" | "pending_deletion"
            )
            || !matches!(
                frame.frame_type.as_str(),
                "outermost_frame" | "sub_frame" | "fenced_frame"
            )
            || (frame.frame == FrameId::MAIN
                && (frame.parent_frame_id.is_some() || frame.parent_document_id.is_some()))
            || frame
                .parent_frame_id
                .is_some_and(|id| id == frame.frame || id.get() > i32::MAX as u64)
            || !ids.insert(frame.frame.get())
            || !documents.insert(frame.document_id.as_str())
            || (frame.frame == FrameId::MAIN
                && (frame.document != tab.document || frame.document_id != tab.document_id))
        {
            return Err(ApiError::context_invalidated());
        }
    }
    Ok(frames)
}

/// Returns only current frames the install can access; reused by tab messaging.
pub fn authorized_frames(
    host: &impl BrowserPort,
    sender: &Sender,
    permissions: &InstallPermissions,
    tab: &BrowserTab,
) -> Result<Vec<BrowserFrame>, ApiError> {
    Ok(frames(host, sender, tab)?
        .into_iter()
        .filter(|frame| frame.check_access(sender, permissions, tab).is_ok())
        .collect())
}

/// Installs generated frame-query contracts.
pub fn register<S: BrowserPort + 'static>(
    table: &mut ApiTable<S>,
) -> Result<(), RegistrationError> {
    for &name in METHODS {
        table.register_generated(name, |host: &mut S, call| {
            let result = query(host, &call);
            call.reply(result)
        })?;
    }
    Ok(())
}

fn query(host: &impl BrowserPort, call: &Invocation<'_>) -> Result<Value, ApiError> {
    let snapshot = browser::snapshot(host, call.sender())?;
    let details = &call.args()[0];
    let tab_id = details.get("tabId").and_then(Value::as_i64);
    let frame_id = details.get("frameId").and_then(Value::as_u64);
    let document_id = details.get("documentId").and_then(Value::as_str);
    if call.name() == "webNavigation.getFrame"
        && document_id.is_none()
        && (tab_id.is_none() || frame_id.is_none())
    {
        return Err(ApiError::other(
            "Either documentId or both tabId and frameId must be specified.",
        ));
    }
    let mut found = Vec::new();
    for tab in snapshot
        .tabs
        .iter()
        .filter(|tab| tab_id.is_none_or(|id| i64::from(tab.id) == id))
    {
        for frame in authorized_frames(host, call.sender(), call.permissions(), tab)? {
            if call.name() == "webNavigation.getAllFrames"
                || (frame_id.is_none_or(|id| frame.frame.get() == id)
                    && document_id.is_none_or(|id| frame.document_id == id))
            {
                found.push(frame.details(call.name() == "webNavigation.getAllFrames"));
            }
        }
    }
    if call.name() == "webNavigation.getFrame" {
        Ok(found.into_iter().next().unwrap_or(Value::Null))
    } else if !snapshot
        .tabs
        .iter()
        .any(|tab| tab_id == Some(i64::from(tab.id)))
    {
        Ok(Value::Null)
    } else {
        Ok(found.into())
    }
}

/// Native navigation phase; adapters preserve native ordering within one document.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NavigationEvent {
    /// Navigation started.
    BeforeNavigate,
    /// Document committed.
    Committed,
    /// DOM construction completed.
    DomContentLoaded,
    /// Loading completed.
    Completed,
    /// Loading failed.
    ErrorOccurred,
    /// History API changed the address.
    HistoryStateUpdated,
    /// Fragment changed without a new document.
    ReferenceFragmentUpdated,
}

impl NavigationEvent {
    /// Generated event name.
    pub fn name(self) -> &'static str {
        match self {
            Self::BeforeNavigate => "webNavigation.onBeforeNavigate",
            Self::Committed => "webNavigation.onCommitted",
            Self::DomContentLoaded => "webNavigation.onDOMContentLoaded",
            Self::Completed => "webNavigation.onCompleted",
            Self::ErrorOccurred => "webNavigation.onErrorOccurred",
            Self::HistoryStateUpdated => "webNavigation.onHistoryStateUpdated",
            Self::ReferenceFragmentUpdated => "webNavigation.onReferenceFragmentUpdated",
        }
    }
}

/// Native navigation facts. Adapters supply actual transition and error details.
#[derive(Clone, Debug)]
pub struct NavigationObservation {
    /// Destination Chrome tab identity.
    pub tab_id: i32,
    /// Native document identity observed.
    pub frame: BrowserFrame,
    /// Native lifecycle phase.
    pub event: NavigationEvent,
    /// Unix milliseconds from the native observation clock.
    pub time_stamp: f64,
    /// Native Chrome transition type for committed/history/fragment events.
    pub transition_type: Option<String>,
    /// Native qualifiers, such as client_redirect or forward_back.
    pub transition_qualifiers: Vec<String>,
    /// Native failure text for an error event.
    pub error: Option<String>,
}

/// Shapes a native observation only after profile, ordinary-tab, generation and host checks.
pub fn observation(
    host: &impl BrowserPort,
    sender: &Sender,
    permissions: &InstallPermissions,
    observation: &NavigationObservation,
) -> Result<Value, ApiError> {
    let NavigationObservation {
        tab_id,
        frame,
        event,
        time_stamp,
        transition_type,
        transition_qualifiers,
        error,
    } = observation;
    let (tab_id, event, time_stamp) = (*tab_id, *event, *time_stamp);
    if !permissions.has_api("webNavigation") {
        return Err(ApiError::missing_permission());
    }
    if !time_stamp.is_finite() || time_stamp < 0.0 {
        return Err(ApiError::malformed_arguments());
    }
    let snapshot = browser::snapshot(host, sender)?;
    let tab = snapshot.tab(tab_id)?;
    frame.check_access(sender, permissions, tab)?;
    if !frames(host, sender, tab)?.iter().any(|current| {
        current.frame == frame.frame
            && current.document == frame.document
            && current.document_id == frame.document_id
    }) {
        return Err(ApiError::context_invalidated());
    }
    let mut details = frame.details(true);
    if let Some(object) = details.as_object_mut() {
        object.remove("errorOccurred");
        if event == NavigationEvent::BeforeNavigate {
            object.remove("documentId");
        }
    }
    details["tabId"] = tab_id.into();
    details["timeStamp"] = time_stamp.into();
    if matches!(
        event,
        NavigationEvent::Committed
            | NavigationEvent::HistoryStateUpdated
            | NavigationEvent::ReferenceFragmentUpdated
    ) {
        details["transitionType"] = transition_type
            .as_ref()
            .ok_or_else(ApiError::malformed_arguments)?
            .clone()
            .into();
        details["transitionQualifiers"] = json!(transition_qualifiers);
    }
    if event == NavigationEvent::ErrorOccurred {
        details["error"] = error
            .as_ref()
            .filter(|error| !error.is_empty() && error.len() <= 1024)
            .ok_or_else(ApiError::malformed_arguments)?
            .clone()
            .into();
    }
    Ok(json!([details]))
}

/// Filter facts derived from the authenticated frame, separate from event JSON.
pub fn event_facts(frame: &BrowserFrame, tab_id: i32) -> EventFacts<'_> {
    EventFacts {
        url: Some(&frame.url),
        tab_id: Some(i64::from(tab_id)),
        ..EventFacts::default()
    }
}

/// Native source and destination of a newly created navigation target.
#[derive(Clone, Debug)]
pub struct CreatedNavigationTarget {
    /// Chrome source tab identity.
    pub source_tab: i32,
    /// Native source document.
    pub source: BrowserFrame,
    /// Chrome destination tab identity.
    pub target_tab: i32,
    /// Native destination document.
    pub target: BrowserFrame,
    /// Native event time in Unix milliseconds.
    pub time_stamp: f64,
}

/// Shapes creation of an ordinary navigation target after checking both native documents.
pub fn created_navigation_target(
    host: &impl BrowserPort,
    sender: &Sender,
    permissions: &InstallPermissions,
    observation: &CreatedNavigationTarget,
) -> Result<Value, ApiError> {
    let CreatedNavigationTarget {
        source_tab,
        source,
        target_tab,
        target,
        time_stamp,
    } = observation;
    let (source_tab, target_tab, time_stamp) = (*source_tab, *target_tab, *time_stamp);
    let snapshot = browser::snapshot(host, sender)?;
    if !permissions.has_api("webNavigation") {
        return Err(ApiError::missing_permission());
    }
    if !time_stamp.is_finite() || time_stamp < 0.0 {
        return Err(ApiError::malformed_arguments());
    }
    for (id, frame) in [(source_tab, source), (target_tab, target)] {
        let tab = snapshot.tab(id)?;
        frame.check_access(sender, permissions, tab)?;
        if !frames(host, sender, tab)?.iter().any(|current| {
            current.frame == frame.frame
                && current.document == frame.document
                && current.document_id == frame.document_id
        }) {
            return Err(ApiError::context_invalidated());
        }
    }
    Ok(
        json!([{"sourceTabId": source_tab, "sourceFrameId": source.frame.get(), "sourceProcessId": -1, "tabId": target_tab, "url": target.url.as_str(), "timeStamp": time_stamp}]),
    )
}

/// Shapes a native tab replacement from an authenticated pre-replacement snapshot and current facts.
pub fn tab_replaced(
    host: &impl BrowserPort,
    sender: &Sender,
    permissions: &InstallPermissions,
    before: &crate::browser::BrowserSnapshot,
    replaced_tab: i32,
    new_tab: i32,
    time_stamp: f64,
) -> Result<Value, ApiError> {
    if !permissions.has_api("webNavigation") {
        return Err(ApiError::missing_permission());
    }
    if !time_stamp.is_finite() || time_stamp < 0.0 {
        return Err(ApiError::malformed_arguments());
    }
    before.authorize(sender, &host.extension_info(sender)?)?;
    let after = browser::snapshot(host, sender)?;
    before
        .tab(replaced_tab)?
        .check_access(sender, permissions)?;
    after.tab(new_tab)?.check_access(sender, permissions)?;
    Ok(json!([{"replacedTabId": replaced_tab, "tabId": new_tab, "timeStamp": time_stamp}]))
}

#[cfg(test)]
mod tests;
