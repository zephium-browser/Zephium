//! Connections: services the agent reaches through a tool the person already
//! has (a CLI such as `gh`) or an MCP server they added. The first use in a
//! work asks "Use GitHub (gh)?"; reading is free after that; anything that
//! writes as the person stops for a Confirm with a preview. What a service
//! returns is data for the agent, never instructions.
pub mod cli;
pub mod gh;
pub mod helper;
pub mod mcp;
pub mod store;

/// The MCP client, for the app's Settings commands.
pub use zephium_mcp as client;

use zephium_core::work::runtime::WorkConfirmV1;
use zephium_core::work::WorkError;

pub use crate::work_computer::{Decision, HostFuture};

/// One call as the canvas shows it: "Read issue #123", "Listed 4 PRs".
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CallFact {
    /// "github", or an MCP server's id.
    pub service: String,
    /// The namespaced tool: `github_issue`, `notion__search`.
    pub tool: String,
    /// What the frame writes the row from: `issue`, `issues`, `pr`, `comment`.
    pub verb: String,
    /// What it touched: "#123", "octo/app".
    pub target: Option<String>,
    /// A title it read, such as the issue's.
    pub title: Option<String>,
    /// Items it listed.
    pub count: Option<u32>,
    /// The page the call is about, when it has one.
    pub url: Option<String>,
}

/// Where a connection's calls go: the run that owns the part.
pub trait ConnectionHost: Send + Sync {
    /// Records a finished call on the part. `source` is the bounded result,
    /// kept with the step and out of the lead's context.
    fn record(
        &self,
        fact: CallFact,
        ok: bool,
        source: Option<String>,
    ) -> HostFuture<'_, Result<(), WorkError>>;
    /// Records a Confirm step and waits for the person.
    fn confirm(&self, confirm: WorkConfirmV1) -> HostFuture<'_, Result<Decision, WorkError>>;
    /// Asks once per work; an earlier answer in the same work is reused.
    fn ask<'a>(
        &'a self,
        prompt: &'a str,
        options: &'a [&'a str],
    ) -> HostFuture<'a, Result<Option<String>, WorkError>>;
}

/// Text from a service, bounded for the agent and cleaned of control bytes.
pub fn bounded(text: &str, max: usize) -> (String, bool) {
    let cleaned: String = text
        .chars()
        .map(|c| {
            if c.is_control() && c != '\n' && c != '\t' {
                ' '
            } else {
                c
            }
        })
        .collect();
    if cleaned.len() <= max {
        return (cleaned, false);
    }
    let mut end = max;
    while !cleaned.is_char_boundary(end) {
        end -= 1;
    }
    (cleaned[..end].to_owned(), true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_text_is_bounded_and_control_safe() {
        assert_eq!(bounded("a\u{1}b", 10), ("a b".into(), false));
        assert_eq!(bounded("héllo", 2), ("h".into(), true));
    }
}

#[cfg(test)]
mod live_tests;
