//! Public-only gh reads after a distinct, explicit CLI choice. Legacy ambient
//! gh execution and cached generic service consent do not authorize this broker.
mod fetch;
use super::{gh, CallFact, ConnectionHost};
use crate::{
    work_commands::runner::{self, Failure},
    work_computer::ToolReply,
};
use serde_json::{json, Value};
use std::time::Duration;
use zephium_core::work::model::WorkModelTool;

#[derive(Clone, Debug, Eq, PartialEq)]
enum Kind {
    Repository,
    Issue(u32),
    PullRequest(u32),
}
#[derive(Clone, Debug, Eq, PartialEq)]
struct Request {
    repo: String,
    kind: Kind,
}
impl Request {
    fn parse(name: &str, args: &Value) -> Result<Self, &'static str> {
        if zephium_agentic::contains_recognized_secret(&args.to_string()) {
            return Err("Recognizable credential content was withheld");
        }
        let repo = args["repo"]
            .as_str()
            .ok_or("Give an explicit public repository as owner/name")?;
        let mut parts = repo.split('/');
        let valid = |part: Option<&str>| {
            part.is_some_and(|part| {
                !part.is_empty()
                    && part.len() <= 100
                    && !part.starts_with(['.', '-'])
                    && part
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))
            })
        };
        if !valid(parts.next()) || !valid(parts.next()) || parts.next().is_some() {
            return Err("Give an explicit public repository as owner/name");
        }
        let number = || {
            args["number"]
                .as_u64()
                .filter(|number| *number > 0 && *number <= u32::MAX as u64)
                .map(|number| number as u32)
                .ok_or("A positive issue or pull-request number is required")
        };
        let kind = match name {
            "github_cli_repository" => Kind::Repository,
            "github_cli_issue" => Kind::Issue(number()?),
            "github_cli_pr" => Kind::PullRequest(number()?),
            _ => return Err("This GitHub CLI operation is not supported"),
        };
        if args.as_object().is_none_or(|args| {
            args.keys()
                .any(|key| key != "repo" && (key != "number" || kind == Kind::Repository))
        }) {
            return Err("Unexpected GitHub CLI arguments");
        }
        Ok(Self {
            repo: repo.into(),
            kind,
        })
    }
    fn endpoint(&self) -> String {
        match self.kind {
            Kind::Repository => format!("/repos/{}", self.repo),
            Kind::Issue(number) => format!("/repos/{}/issues/{number}", self.repo),
            Kind::PullRequest(number) => format!("/repos/{}/pulls/{number}", self.repo),
        }
    }
    fn url(&self) -> String {
        match self.kind {
            Kind::Repository => format!("https://github.com/{}", self.repo),
            Kind::Issue(number) => format!("https://github.com/{}/issues/{number}", self.repo),
            Kind::PullRequest(number) => format!("https://github.com/{}/pull/{number}", self.repo),
        }
    }
}
pub(super) fn available() -> bool {
    runner::github::available()
}
pub(super) fn definitions() -> Vec<WorkModelTool> {
    [("github_cli_repository", "Read bounded public repository metadata through the qualified GitHub CLI broker, only after the person explicitly chooses CLI. Requires owner/name. No local repository inference, account credentials or private repositories.", false),
     ("github_cli_issue", "Read a public issue's title, state and body excerpt through the qualified GitHub CLI broker after explicit CLI consent. Comments are not included; no account or private-repository access.", true),
     ("github_cli_pr", "Read a public pull request's title, state, branches and body excerpt through the qualified GitHub CLI broker after explicit CLI consent. No diff, review/check contents or account access.", true)]
     .into_iter().map(|(name, description, numbered)| {
        let mut properties = json!({"repo":{"type":"string","maxLength":201,"description":"Explicit public owner/name"}});
        let mut required = vec!["repo"];
        if numbered { properties["number"] = json!({"type":"integer","minimum":1,"maximum":u32::MAX}); required.push("number"); }
        WorkModelTool { name:name.into(), description:description.into(), schema:json!({"type":"object","properties":properties,"required":required,"additionalProperties":false}), strict:false }
    }).collect()
}
pub(super) async fn call(host: &dyn ConnectionHost, name: &str, args: &Value) -> ToolReply {
    if !available() {
        return fault(Failure::GitHubUnavailable.note());
    }
    call_with_fetch(host, name, args, |request| async move {
        fetch::get(&request).await
    })
    .await
}
async fn call_with_fetch<F, FF>(
    host: &dyn ConnectionHost,
    name: &str,
    args: &Value,
    fetch: F,
) -> ToolReply
where
    F: FnOnce(Request) -> FF,
    FF: std::future::Future<Output = Result<Vec<u8>, Failure>>,
{
    if runner::cancellation_requested(&|| host.cancelled()).await {
        return fault("The run stopped");
    }
    let request = match Request::parse(name, args) {
        Ok(request) => request,
        Err(note) => return fault(note),
    };
    let prompt = format!(
        "{} Read public GitHub data without using account credentials.",
        gh::ASK
    );
    let answer = host.ask(&prompt, &[gh::USE, gh::DECLINE]).await;
    if !matches!(answer, Ok(Some(ref answer)) if answer == gh::USE) {
        return fault(
            "CLI was not chosen. Use the Browser helper; do not retry this choice unchanged.",
        );
    }
    if runner::cancellation_requested(&|| host.cancelled()).await {
        return fault("The run stopped");
    }
    let output = match runner::github::run(&|| host.cancelled(), || fetch(request.clone())).await {
        Ok(output) => output,
        Err(error) => return fault(failure_note(error)),
    };
    let value = match serde_json::from_str(&output.text) {
        Ok(value) => value,
        Err(_) => return fault("The GitHub CLI result could not be verified"),
    };
    let (text, mut fact) = match render(&request, &value) {
        Ok(result) => result,
        Err(note) => return fault(note),
    };
    if runner::cancellation_requested(&|| host.cancelled()).await {
        return fault("The run stopped");
    }
    fact.service = "github".into();
    fact.tool = name.into();
    fact.url = Some(request.url());
    if !matches!(
        tokio::time::timeout(
            Duration::from_millis(200),
            host.record(fact, true, Some(text.clone()))
        )
        .await,
        Ok(Ok(()))
    ) {
        return fault("The read finished, but its receipt could not be recorded");
    }
    ToolReply {
        content: text,
        is_error: false,
    }
}
fn field(value: &Value, key: &str, max: usize) -> String {
    let text = value[key].as_str().unwrap_or("");
    super::bounded(text, max).0
}
fn render(request: &Request, value: &Value) -> Result<(String, CallFact), &'static str> {
    if zephium_agentic::contains_recognized_secret(&value.to_string()) {
        return Err("Recognizable credential content was withheld");
    }
    let url = request.url();
    let (text, fact) = match request.kind {
        Kind::Repository => {
            if value["private"] != false
                || !value["full_name"]
                    .as_str()
                    .is_some_and(|name| name.eq_ignore_ascii_case(&request.repo))
            {
                return Err("The public repository result did not match the request");
            }
            (
                format!(
                    "Public repository {}\n{}\nDefault branch: {}\nDescription excerpt:\n{}",
                    request.repo,
                    url,
                    field(value, "default_branch", 128),
                    field(value, "description", 4000)
                ),
                CallFact {
                    verb: "repository".into(),
                    target: Some(request.repo.clone()),
                    ..Default::default()
                },
            )
        }
        Kind::Issue(number) | Kind::PullRequest(number) => {
            if value["number"].as_u64() != Some(u64::from(number)) {
                return Err("The public GitHub item did not match the requested number");
            }
            if matches!(request.kind, Kind::Issue(_)) && value.get("pull_request").is_some() {
                return Err(
                    "That number identifies a pull request; use the public pull-request CLI tool",
                );
            }
            let title = field(value, "title", 512).replace(['\n', '\r'], " ");
            let verb = if matches!(request.kind, Kind::Issue(_)) {
                "issue"
            } else {
                "pr"
            };
            let kind = if verb == "issue" {
                "issue"
            } else {
                "pull request"
            };
            let mut text = format!(
                "Public {kind} {}#{number} · {} · {title}\n{url}\nBody excerpt:\n{}",
                request.repo,
                field(value, "state", 32),
                field(value, "body", 4000)
            );
            if verb == "pr" {
                text.push_str(&format!(
                    "\nBranches: {} → {}",
                    field(&value["head"], "ref", 128),
                    field(&value["base"], "ref", 128)
                ));
            }
            if let Some(comments) = value["comments"].as_u64() {
                text.push_str(&format!("\n{comments} comments (not included)"));
            }
            (
                text,
                CallFact {
                    verb: verb.into(),
                    target: Some(format!("{}#{number}", request.repo)),
                    title: Some(title),
                    ..Default::default()
                },
            )
        }
    };
    Ok((text, fact))
}

fn fault(note: &str) -> ToolReply {
    ToolReply {
        content: note.into(),
        is_error: true,
    }
}

#[cfg(test)]
mod tests;

fn failure_note(error: Failure) -> &'static str {
    match error {
        Failure::Failed => "Public GitHub data could not be read without an account; use the website if it needs sign-in",
        Failure::Limit => "The public GitHub CLI call or response exceeded its bound",
        Failure::Sensitive => "Recognizable credential content in the GitHub response was withheld",
        Failure::Storage => "The confined GitHub CLI process could not be prepared",
        #[cfg(any(target_os = "macos", windows, test))]
        Failure::Launch => "The confined GitHub CLI process could not be prepared",
        other => other.note(),
    }
}
