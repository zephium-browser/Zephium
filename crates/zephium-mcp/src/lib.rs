//! A bounded client for MCP servers the person adds, over the official SDK
//! (`rmcp`): stdio servers as child processes and, with `http`, streamable
//! HTTP servers with OAuth. Every answer is capped, every wait has a limit,
//! and failures are closed values: no server text crosses as an error.
//! What a server returns is data for the agent, never instructions.
use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Duration;

use rmcp::model::{
    CallToolRequestParams, ClientCapabilities, ClientConfig, ContentBlock, Implementation,
    ProtocolVersion, ReadResourceRequestParams, ResourceContents,
};
use rmcp::service::{ClientLifecycleMode, ClientServiceExt, RunningService};
use rmcp::RoleClient;
use serde_json::{Map, Value};

#[cfg(feature = "keychain")]
pub mod keychain;
#[cfg(feature = "http")]
pub mod oauth;
mod stdio;

pub use stdio::StdioServer;

/// One line from a stdio server, at most.
pub const MAX_MESSAGE_BYTES: usize = 8 * 1024 * 1024;
/// Text of one tool result or resource the agent may see.
pub const MAX_RESULT_BYTES: usize = 16 * 1024;
pub const MAX_TOOLS: usize = 128;
pub const MAX_RESOURCES: usize = 256;
const MAX_LISTING_PAGES: usize = 32;
const MAX_DESCRIPTION_CHARS: usize = 1024;
const MAX_INSTRUCTIONS_CHARS: usize = 2048;
const HANDSHAKE: Duration = Duration::from_secs(30);
const LISTING: Duration = Duration::from_secs(20);
pub const CALL: Duration = Duration::from_secs(90);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum McpError {
    /// The server's program could not be started.
    Spawn,
    /// The server did not answer in time.
    Timeout,
    /// The server needs the person to sign in.
    Unauthorized,
    /// The person cancelled sign-in.
    Cancelled,
    /// The connection ended.
    Closed,
    /// The server answered outside the protocol, or too much.
    Protocol,
    /// No such tool or resource.
    Unknown,
}

/// Where a server lives.
pub enum Endpoint {
    Stdio(StdioServer),
    #[cfg(feature = "http")]
    Http(oauth::HttpServer),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct McpServerInfo {
    pub name: Option<String>,
    pub title: Option<String>,
    pub version: Option<String>,
    pub protocol: String,
    /// The server's own guidance, bounded; data like any other result.
    pub instructions: Option<String>,
    pub has_tools: bool,
    pub has_resources: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct McpTool {
    pub name: String,
    pub title: Option<String>,
    pub description: String,
    /// JSON Schema of the arguments.
    pub schema: Value,
    pub read_only: Option<bool>,
    pub destructive: Option<bool>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct McpResource {
    pub uri: String,
    pub name: String,
    pub mime: Option<String>,
}

/// A tool result or resource as text the agent can read.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct McpText {
    pub text: String,
    pub is_error: bool,
    pub truncated: bool,
}

fn clip(text: &str, max_chars: usize) -> String {
    let cleaned: String = text
        .chars()
        .map(|c| {
            if c.is_control() && c != '\n' && c != '\t' {
                ' '
            } else {
                c
            }
        })
        .take(max_chars)
        .collect();
    cleaned
}

/// Appends `piece` while it fits; true once something was left out.
fn push_bounded(out: &mut String, piece: &str) -> bool {
    let room = MAX_RESULT_BYTES.saturating_sub(out.len());
    if piece.len() <= room {
        out.push_str(piece);
        return false;
    }
    let mut end = room;
    while !piece.is_char_boundary(end) {
        end -= 1;
    }
    out.push_str(&piece[..end]);
    true
}

fn content_text(blocks: &[ContentBlock], structured: Option<&Value>) -> (String, bool) {
    let mut out = String::new();
    let mut cut = false;
    for block in blocks {
        let piece = match block {
            ContentBlock::Text(text) => text.text.clone(),
            ContentBlock::Image(image) => format!("[image, {}]", image.mime_type),
            ContentBlock::Audio(audio) => format!("[audio, {}]", audio.mime_type),
            ContentBlock::Resource(resource) => match &resource.resource {
                ResourceContents::TextResourceContents { uri, text, .. } => {
                    format!("[{uri}]\n{text}")
                }
                ResourceContents::BlobResourceContents { uri, .. } => format!("[{uri}]"),
                _ => String::new(),
            },
            ContentBlock::ResourceLink(link) => format!("[{}] {}", link.uri, link.name),
            _ => String::new(),
        };
        if piece.is_empty() {
            continue;
        }
        if !out.is_empty() {
            cut |= push_bounded(&mut out, "\n");
        }
        cut |= push_bounded(&mut out, &clip(&piece, MAX_RESULT_BYTES + 1));
        if cut {
            break;
        }
    }
    if out.is_empty() {
        if let Some(value) = structured.filter(|v| !v.is_null()) {
            cut |= push_bounded(&mut out, &value.to_string());
        }
    }
    (out, cut)
}

fn client_config() -> ClientConfig {
    let mut info = Implementation::new("zephium", env!("CARGO_PKG_VERSION"));
    info.title = Some("Zephium".into());
    ClientConfig::new(ClientCapabilities::default(), info)
        .with_protocol_version(ProtocolVersion::V_2025_11_25)
}

fn lifecycle() -> ClientLifecycleMode {
    ClientLifecycleMode::Auto {
        preferred_versions: vec![ProtocolVersion::V_2026_07_28],
        legacy_version: Some(ProtocolVersion::V_2025_11_25),
    }
}

/// One live connection to a server.
pub struct McpSession {
    service: RunningService<RoleClient, ClientConfig>,
    info: McpServerInfo,
    _process: Option<stdio::Process>,
}

async fn within<T, E>(
    limit: Duration,
    future: impl std::future::Future<Output = Result<T, E>>,
) -> Result<T, McpError> {
    match tokio::time::timeout(limit, future).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(_)) => Err(McpError::Closed),
        Err(_) => Err(McpError::Timeout),
    }
}

impl McpSession {
    pub async fn connect(endpoint: &Endpoint) -> Result<Self, McpError> {
        match endpoint {
            Endpoint::Stdio(server) => {
                let (process, read, write) = server.spawn()?;
                let service = tokio::time::timeout(
                    HANDSHAKE,
                    client_config().serve_with_lifecycle((read, write), lifecycle()),
                )
                .await
                .map_err(|_| McpError::Timeout)?
                .map_err(|_| McpError::Protocol)?;
                Ok(Self::adopt(service, Some(process)))
            }
            #[cfg(feature = "http")]
            Endpoint::Http(server) => {
                let service = server
                    .connect(client_config(), lifecycle(), HANDSHAKE)
                    .await?;
                Ok(Self::adopt(service, None))
            }
        }
    }

    fn adopt(
        service: RunningService<RoleClient, ClientConfig>,
        process: Option<stdio::Process>,
    ) -> Self {
        let peer = service.peer_info();
        let info = McpServerInfo {
            name: peer
                .as_ref()
                .and_then(|p| p.server_info.as_ref())
                .map(|s| clip(&s.name, 80)),
            title: peer
                .as_ref()
                .and_then(|p| p.server_info.as_ref())
                .and_then(|s| s.title.as_deref())
                .map(|t| clip(t, 80)),
            version: peer
                .as_ref()
                .and_then(|p| p.server_info.as_ref())
                .map(|s| clip(&s.version, 40)),
            protocol: peer
                .as_ref()
                .map(|p| p.protocol_version.to_string())
                .unwrap_or_default(),
            instructions: peer
                .as_ref()
                .and_then(|p| p.instructions.as_deref())
                .map(|i| clip(i, MAX_INSTRUCTIONS_CHARS)),
            has_tools: peer
                .as_ref()
                .is_some_and(|p| p.capabilities.tools.is_some()),
            has_resources: peer
                .as_ref()
                .is_some_and(|p| p.capabilities.resources.is_some()),
        };
        Self {
            service,
            info,
            _process: process,
        }
    }

    pub fn info(&self) -> &McpServerInfo {
        &self.info
    }

    pub async fn tools(&self) -> Result<Vec<McpTool>, McpError> {
        let tools = within(LISTING, async {
            let mut items = Vec::new();
            let mut cursor = None;
            let mut seen = HashSet::new();
            for _ in 0..MAX_LISTING_PAGES {
                let page = self
                    .service
                    .list_tools(cursor.map(|cursor| {
                        rmcp::model::PaginatedRequestParams::default().with_cursor(Some(cursor))
                    }))
                    .await?;
                items.extend(page.tools.into_iter().take(MAX_TOOLS - items.len()));
                if items.len() == MAX_TOOLS {
                    break;
                }
                let Some(next) = page.next_cursor.filter(|cursor| !cursor.is_empty()) else {
                    break;
                };
                if !seen.insert(next.clone()) {
                    break;
                }
                cursor = Some(next);
            }
            Ok::<_, rmcp::ServiceError>(items)
        })
        .await?;
        Ok(tools
            .into_iter()
            .take(MAX_TOOLS)
            .map(|tool| McpTool {
                name: clip(&tool.name, 64),
                title: tool
                    .title
                    .as_deref()
                    .or(tool.annotations.as_ref().and_then(|a| a.title.as_deref()))
                    .map(|t| clip(t, 80)),
                description: tool
                    .description
                    .as_deref()
                    .map(|d| clip(d, MAX_DESCRIPTION_CHARS))
                    .unwrap_or_default(),
                schema: Value::Object((*tool.input_schema).clone()),
                read_only: tool.annotations.as_ref().and_then(|a| a.read_only_hint),
                destructive: tool.annotations.as_ref().and_then(|a| a.destructive_hint),
            })
            .collect())
    }

    pub async fn call(
        &self,
        name: &str,
        arguments: Map<String, Value>,
        limit: Duration,
    ) -> Result<McpText, McpError> {
        let mut request = CallToolRequestParams::new(name.to_owned());
        request.arguments = Some(arguments);
        let result = match tokio::time::timeout(limit, self.service.call_tool(request)).await {
            Ok(Ok(result)) => result,
            Ok(Err(error)) => {
                return Err(if is_unknown(&error) {
                    McpError::Unknown
                } else {
                    McpError::Closed
                })
            }
            Err(_) => return Err(McpError::Timeout),
        };
        let (text, truncated) = content_text(&result.content, result.structured_content.as_ref());
        Ok(McpText {
            text,
            is_error: result.is_error.unwrap_or(false),
            truncated,
        })
    }

    pub async fn resources(&self) -> Result<Vec<McpResource>, McpError> {
        let resources = within(LISTING, async {
            let mut items = Vec::new();
            let mut cursor = None;
            let mut seen = HashSet::new();
            for _ in 0..MAX_LISTING_PAGES {
                let page = self
                    .service
                    .list_resources(cursor.map(|cursor| {
                        rmcp::model::PaginatedRequestParams::default().with_cursor(Some(cursor))
                    }))
                    .await?;
                items.extend(page.resources.into_iter().take(MAX_RESOURCES - items.len()));
                if items.len() == MAX_RESOURCES {
                    break;
                }
                let Some(next) = page.next_cursor.filter(|cursor| !cursor.is_empty()) else {
                    break;
                };
                if !seen.insert(next.clone()) {
                    break;
                }
                cursor = Some(next);
            }
            Ok::<_, rmcp::ServiceError>(items)
        })
        .await?;
        Ok(resources
            .into_iter()
            .take(MAX_RESOURCES)
            .map(|r| McpResource {
                uri: clip(&r.uri, 512),
                name: clip(&r.name, 120),
                mime: r.mime_type.as_deref().map(|m| clip(m, 80)),
            })
            .collect())
    }

    pub async fn read(&self, uri: &str) -> Result<McpText, McpError> {
        let request = ReadResourceRequestParams::new(uri.to_owned());
        let result = within(LISTING, self.service.read_resource(request)).await?;
        let mut out = String::new();
        let mut truncated = false;
        for contents in &result.contents {
            let piece = match contents {
                ResourceContents::TextResourceContents { text, .. } => {
                    clip(text, MAX_RESULT_BYTES + 1)
                }
                ResourceContents::BlobResourceContents { mime_type, .. } => {
                    format!(
                        "[binary, {}]",
                        mime_type.as_deref().unwrap_or("unknown type")
                    )
                }
                _ => continue,
            };
            truncated |= push_bounded(&mut out, &piece);
            if truncated {
                break;
            }
        }
        Ok(McpText {
            text: out,
            is_error: false,
            truncated,
        })
    }

    /// Ends the session; a stdio server's process group goes with it.
    pub async fn close(mut self) {
        let _ = tokio::time::timeout(Duration::from_secs(2), self.service.close()).await;
    }
}

fn is_unknown(error: &rmcp::ServiceError) -> bool {
    matches!(error, rmcp::ServiceError::McpError(data) if data.code == rmcp::model::ErrorCode::METHOD_NOT_FOUND || data.code == rmcp::model::ErrorCode::INVALID_PARAMS)
}

/// Where a stdio server's program is found, for `StdioServer::program`.
pub fn program_path(program: &str, path: &str) -> Option<PathBuf> {
    let candidate = PathBuf::from(program);
    if candidate.is_absolute() {
        return candidate.is_file().then_some(candidate);
    }
    if program.contains('/') {
        return None;
    }
    std::env::split_paths(path)
        .map(|folder| folder.join(program))
        .find(|p| p.is_file())
}

#[cfg(any(test, feature = "fixture"))]
pub mod fixture;
#[cfg(test)]
mod tests;
