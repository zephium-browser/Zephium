//! Bounded programmatic JavaScript and CSS injections into exact documents.

use serde_json::{json, Value};
use zephium_webext::manifest::normalize_resource;

use super::web_navigation::{self, BrowserFrame};
use crate::browser::{self, BrowserPort, BrowserRequest};
use crate::context::DocumentGeneration;
use crate::dispatch::{ApiTable, Invocation, RegistrationError};
use crate::scripting_registry::ExecutionWorld;
use crate::ApiError;

pub(crate) const METHODS: &[&str] = &[
    "scripting.executeScript",
    "scripting.insertCSS",
    "scripting.removeCSS",
];
/// Maximum UTF-8 bytes in injected code or serialized function arguments.
pub const MAX_SOURCE_BYTES: usize = 1024 * 1024;
/// Maximum files referenced by one injection.
pub const MAX_FILES: usize = 256;

/// A tab and exact document generations admitted for this injection.
#[derive(Clone, Debug)]
pub struct InjectionTarget {
    /// Chrome tab identity.
    pub tab_id: i32,
    /// Main document fencing activeTab and navigation races.
    pub main_document: DocumentGeneration,
    /// Native-authenticated, permission-checked destination frames.
    pub frames: Vec<BrowserFrame>,
}

/// Script input. Adapters serialize arguments as JSON and never concatenate raw values.
#[derive(Clone, Debug)]
pub enum ScriptSource {
    /// Ordered package resources. Adapters validate UTF-8, MIME, syntax and a
    /// combined MAX_SOURCE_BYTES byte limit before any injection starts.
    Files(Vec<String>),
    /// Binding-serialized function expression and JSON arguments; no closure state.
    Function {
        /// Function source to parse as an expression in the selected world.
        source: String,
        /// JSON arguments passed positionally; await any returned promise.
        args: Vec<Value>,
    },
}

/// CSS identity is install, source and origin; removing an absent identity succeeds.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CssSource {
    /// Ordered package resources, localized by the adapter.
    Files(Vec<String>),
    /// Exact UTF-8 stylesheet text.
    Code(String),
}

/// Native stylesheet cascade origin.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StyleOrigin {
    /// Author origin, the default.
    Author,
    /// User origin.
    User,
}

/// Injection operation; adapters recheck every frame generation and permission at execution.
#[derive(Clone, Debug)]
pub enum Request {
    /// Execute in the selected world, which must not expose extension APIs in MAIN.
    Execute {
        /// Exact admitted destinations.
        target: InjectionTarget,
        /// Code to execute.
        source: ScriptSource,
        /// Isolated by install, or the page world without extension authority.
        world: ExecutionWorld,
        /// Run immediately instead of waiting for document idle.
        inject_immediately: bool,
    },
    /// Insert or remove CSS using an install-scoped source/origin identity.
    Css {
        /// Exact admitted destinations.
        target: InjectionTarget,
        /// Exact stylesheet identity.
        source: CssSource,
        /// Cascade origin, included in removal identity.
        origin: StyleOrigin,
        /// True removes a previous insertion; an absent insertion is a no-op.
        remove: bool,
    },
}

impl Request {
    /// Exact documents admitted by the handler.
    pub fn target(&self) -> &InjectionTarget {
        match self {
            Self::Execute { target, .. } | Self::Css { target, .. } => target,
        }
    }
}

/// An engine result tied to the exact requested document.
#[derive(Clone, Debug)]
pub struct FrameResult {
    /// Native frame identity and document generation actually executed.
    pub frame: BrowserFrame,
    /// JSON result, null for JavaScript undefined; an engine error is preserved.
    pub result: Result<Value, ApiError>,
}

/// Shapes settled results, with the main frame first and individual multi-frame failures omitted.
pub fn reply(request: &Request, mut results: Vec<FrameResult>) -> Result<Value, ApiError> {
    let target = request.target();
    if results.len() != target.frames.len() {
        return Err(ApiError::context_invalidated());
    }
    let mut seen = std::collections::BTreeSet::new();
    for result in &results {
        if !seen.insert(result.frame.frame.get())
            || !target.frames.iter().any(|frame| {
                frame.tab == result.frame.tab
                    && frame.frame == result.frame.frame
                    && frame.document == result.frame.document
                    && frame.document_id == result.frame.document_id
            })
        {
            return Err(ApiError::context_invalidated());
        }
    }
    if results.len() == 1 {
        if let Some(result) = results.first() {
            if let Err(error) = &result.result {
                return Err(error.clone());
            }
        }
    }
    if matches!(request, Request::Css { .. }) {
        return Ok(Value::Null);
    }
    results.sort_by_key(|result| result.frame.frame.get());
    let values: Vec<_> = results.into_iter().filter_map(|result| result.result.ok().map(|value| json!({"frameId": result.frame.frame.get(), "documentId": result.frame.document_id, "result": value}))).collect();
    let value = Value::Array(values);
    if serde_json::to_vec(&value)
        .map_err(|_| ApiError::malformed_arguments())?
        .len()
        > MAX_SOURCE_BYTES
    {
        return Err(ApiError::message_too_large());
    }
    Ok(value)
}

/// Installs generated programmatic scripting contracts.
pub fn register<S: BrowserPort + 'static>(
    table: &mut ApiTable<S>,
) -> Result<(), RegistrationError> {
    for &name in METHODS {
        table.register_generated(name, |host: &mut S, call| match prepare(host, &call) {
            Ok(request) => host.request(BrowserRequest::Scripting(request), call),
            Err(error) => call.reply(Err(error)),
        })?;
    }
    Ok(())
}

fn files(
    host: &impl BrowserPort,
    call: &Invocation<'_>,
    value: &Value,
) -> Result<Vec<String>, ApiError> {
    let values = value.as_array().ok_or_else(ApiError::malformed_arguments)?;
    if values.is_empty() {
        return Err(ApiError::other("At least one file must be specified."));
    }
    if values.len() > MAX_FILES {
        return Err(ApiError::too_many_pending());
    }
    let mut files = Vec::new();
    let mut unique = std::collections::BTreeSet::new();
    for path in values {
        let path = path.as_str().ok_or_else(ApiError::malformed_arguments)?;
        let normalized = normalize_resource(path)
            .filter(|path| !path.is_empty())
            .ok_or_else(|| ApiError::other(format!("Could not load file: '{path}'.")))?;
        if !unique.insert(normalized.clone()) {
            return Err(ApiError::other(format!(
                "Duplicate file specified: '{path}'."
            )));
        }
        host.validate_resource(call.sender(), &normalized)?;
        files.push(normalized);
    }
    Ok(files)
}

fn target(
    host: &impl BrowserPort,
    call: &Invocation<'_>,
    value: &Value,
) -> Result<InjectionTarget, ApiError> {
    let tab_id = value["tabId"]
        .as_i64()
        .ok_or_else(ApiError::malformed_arguments)? as i32;
    let frame_ids = value.get("frameIds").and_then(Value::as_array);
    let document_ids = value.get("documentIds").and_then(Value::as_array);
    let all = value
        .get("allFrames")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if frame_ids.is_some() && document_ids.is_some() {
        return Err(ApiError::other(
            "Cannot specify both 'frameIds' and 'documentIds'.",
        ));
    }
    if all && (frame_ids.is_some() || document_ids.is_some()) {
        return Err(ApiError::other(
            "Cannot specify 'allFrames' if either 'frameIds' or 'documentIds' is specified.",
        ));
    }
    if frame_ids.is_some_and(|ids| ids.len() > web_navigation::MAX_FRAMES)
        || document_ids.is_some_and(|ids| ids.len() > web_navigation::MAX_FRAMES)
    {
        return Err(ApiError::too_many_pending());
    }
    let snapshot = browser::snapshot(host, call.sender())?;
    let tab = snapshot
        .tab(tab_id)
        .map_err(|_| ApiError::other(format!("No tab with id: {tab_id}")))?;
    let available = web_navigation::frames(host, call.sender(), tab)?;
    let mut selected = Vec::new();
    if let Some(ids) = frame_ids {
        for id in ids {
            let id = id.as_i64().ok_or_else(ApiError::malformed_arguments)?;
            let frame = available
                .iter()
                .find(|frame| frame.frame.get() as i64 == id)
                .ok_or_else(|| {
                    ApiError::other(format!("No frame with id {id} in tab with id {tab_id}"))
                })?;
            selected.push(frame.clone());
        }
    } else if let Some(ids) = document_ids {
        for id in ids {
            let id = id
                .as_str()
                .filter(|id| !id.is_empty() && id.len() <= 256)
                .ok_or_else(ApiError::malformed_arguments)?;
            let frame = available
                .iter()
                .find(|frame| frame.document_id == id)
                .ok_or_else(|| {
                    ApiError::other(format!("No document with id {id} in tab with id {tab_id}"))
                })?;
            selected.push(frame.clone());
        }
    } else {
        selected.extend(
            available
                .into_iter()
                .filter(|frame| all || frame.frame == crate::context::FrameId::MAIN),
        );
    }
    if frame_ids.is_none()
        && document_ids.is_none()
        && !selected
            .iter()
            .any(|frame| frame.frame == crate::context::FrameId::MAIN)
    {
        return Err(ApiError::other(format!(
            "No frame with id 0 in tab with id {tab_id}"
        )));
    }
    selected.sort_by_key(|frame| frame.frame.get());
    selected.dedup_by_key(|frame| frame.frame.get());
    if all {
        let main = selected
            .iter()
            .find(|frame| frame.frame == crate::context::FrameId::MAIN)
            .ok_or_else(ApiError::context_invalidated)?;
        if main.document_lifecycle != "active" {
            return Err(ApiError::cannot_access_page());
        }
        main.check_access(call.sender(), call.permissions(), tab)?;
        selected.retain(|frame| {
            frame.document_lifecycle == "active"
                && frame
                    .check_access(call.sender(), call.permissions(), tab)
                    .is_ok()
        });
    } else {
        for frame in &selected {
            if frame.document_lifecycle != "active" {
                return Err(ApiError::cannot_access_page());
            }
            frame.check_access(call.sender(), call.permissions(), tab)?;
        }
    }
    Ok(InjectionTarget {
        tab_id,
        main_document: tab.document,
        frames: selected,
    })
}

fn prepare(host: &impl BrowserPort, call: &Invocation<'_>) -> Result<Request, ApiError> {
    let details = &call.args()[0];
    let is_script = call.name() == "scripting.executeScript";
    let source = details
        .get(if is_script { "func" } else { "css" })
        .or_else(|| {
            if is_script {
                details.get("function")
            } else {
                None
            }
        });
    let has_files = details.get("files");
    if is_script && details.get("func").is_some() && details.get("function").is_some() {
        return Err(ApiError::other(
            "Both 'func' and 'function' were specified. Only 'func' should be used.",
        ));
    }
    if source.is_some() == has_files.is_some() {
        return Err(ApiError::other(if is_script {
            "Exactly one of 'func' and 'files' must be specified"
        } else {
            "Exactly one of 'css' and 'files' must be specified."
        }));
    }
    if has_files.is_some() && details.get("args").is_some() {
        return Err(ApiError::other(
            "'args' may not be used with file injections.",
        ));
    }
    let target = target(host, call, &details["target"])?;
    if is_script {
        let source = if let Some(paths) = has_files {
            ScriptSource::Files(files(host, call, paths)?)
        } else {
            let source = source
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(ApiError::malformed_arguments)?;
            let args = details
                .get("args")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if source.len() > MAX_SOURCE_BYTES
                || serde_json::to_vec(&args)
                    .map_err(|_| ApiError::malformed_arguments())?
                    .len()
                    > MAX_SOURCE_BYTES
            {
                return Err(ApiError::message_too_large());
            }
            ScriptSource::Function {
                source: source.into(),
                args,
            }
        };
        Ok(Request::Execute {
            target,
            source,
            world: if details["world"] == "MAIN" {
                ExecutionWorld::Main
            } else {
                ExecutionWorld::Isolated
            },
            inject_immediately: details["injectImmediately"].as_bool().unwrap_or(false),
        })
    } else {
        let source = if let Some(paths) = has_files {
            CssSource::Files(files(host, call, paths)?)
        } else {
            let source = source
                .and_then(Value::as_str)
                .ok_or_else(ApiError::malformed_arguments)?;
            if source.len() > MAX_SOURCE_BYTES {
                return Err(ApiError::message_too_large());
            }
            CssSource::Code(source.into())
        };
        Ok(Request::Css {
            target,
            source,
            origin: if details["origin"] == "USER" {
                StyleOrigin::User
            } else {
                StyleOrigin::Author
            },
            remove: call.name() == "scripting.removeCSS",
        })
    }
}

#[cfg(test)]
pub(crate) mod tests;
