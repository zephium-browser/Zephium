//! Parts: a helper does one purpose of the job with its own prompt, tools
//! and model, places what it found at the end of its row, and returns a
//! digest to the lead. Up to four run at once; their pages share the run's
//! three live agent pages.
use std::future::Future;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;
use zephium_core::work::{artifact::public_host, model::*, parts::*, runtime::*, *};
use zephium_ipc::work::WorkActivityV1;

use super::call::{self, clip, CallFailure, LeadModel};
use super::hands::{self, Hands, Request};
use super::lead::Lead;
use super::objects;
use super::prompt;
use super::tools::{LeadHelper, LeadScope, LeadToolContext, LeadToolSet};
use crate::work_agent::{WorkAgentBrowseRequest, WorkBrowserOutcome};
use crate::work_runtime::WorkAttemptProbe;

/// Parts that run at once; the rest wait for a slot.
pub(crate) const PARALLEL_PARTS: usize = 4;
/// One part's ceiling, inside the run's remaining budget.
const PART_MAX: WorkExecutionLimits = WorkExecutionLimits {
    model_tokens: 450_000,
    cost_micro_usd: 900_000,
    operations: 96,
    timeout_seconds: 1800,
    max_workers: 3,
};
const PART_TURNS: u8 = 10;
/// The least a part starts with when the run has it: one search's
/// reservation and one page task's cost.
const PART_MIN_TOKENS: u32 = zephium_core::work::search::PUBLIC_SEARCH_TOKEN_RESERVATION + 16_384;
const PART_MIN_COST: u32 = 150_000;
/// Replies a part's ceiling allows for: forty calls of a usual tool turn.
const PART_REPLY_TOKENS: u64 = 40 * 2_048;
/// The note a page step ends with when its page asked to sign in.
const SIGN_IN_NOTE: &str = "The page asked to sign in";
/// Searches one part may run; past them it reports what it has.
const PART_SEARCHES: usize = 4;
/// Searches a part runs before each further one is weighed against its goal.
const PART_FREE_SEARCHES: usize = 2;
/// What of a part's search answers the goal check reads.
const FOUND_CHARS: usize = 6_000;

pub(crate) struct PartSpec {
    pub title: String,
    pub helper: WorkHelperV1,
    pub goal: String,
    pub brief: String,
    pub service: Option<WorkPartServiceV1>,
    pub records: Vec<String>,
    /// What a browser part searches, as typed fields for a site's results page.
    pub search: Option<super::recipes::SiteSearch>,
    /// The person's connection for the service, when they chose its website.
    pub declined: Option<super::route::Offer>,
    /// A canvas draft the part sends or posts exactly as it stands.
    pub draft: Option<String>,
    /// The part does not run: the person's answer to the way question sent
    /// them to Settings or away from the service.
    pub halt: Option<WorkPartNeedV1>,
}

fn now_ms() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
        .to_string()
}

fn text_arg(args: &Value, key: &str) -> Option<String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// A `start_part` call's arguments, admitted or refused with the rule.
pub(crate) fn spec(args: &Value) -> Result<PartSpec, String> {
    let title = text_arg(args, "title").ok_or("title is required")?;
    if title.chars().count() > 24 || title.contains('\n') {
        return Err("title is one line of at most 24 characters: Stay, Flights, Entry".into());
    }
    let helper = match args.get("helper").and_then(Value::as_str) {
        Some("browser") => WorkHelperV1::Browser,
        Some("research") => WorkHelperV1::Research,
        Some("computer") => WorkHelperV1::Computer,
        Some("connection") => WorkHelperV1::Connection,
        _ => return Err("helper is one of browser, research, computer, connection".into()),
    };
    // A part sent with its search or brief but no goal still says what it
    // is for.
    let goal = text_arg(args, "goal")
        .or_else(|| derived_goal(args, &title))
        .ok_or("goal is required")?;
    // A goal said at length keeps every word in the brief and a short
    // first line on the canvas, rather than costing the lead a turn.
    let (goal, overflow) = if goal.chars().count() > 200 || goal.contains('\n') {
        let first = goal
            .split(['\n', '.', ';'])
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or_default();
        (clip(first, 200), Some(goal.clone()))
    } else {
        (goal, None)
    };
    let brief = text_arg(args, "brief");
    if brief.as_ref().is_some_and(|b| b.chars().count() > 1200) {
        return Err("brief is at most 1200 characters".into());
    }
    let brief = match (overflow, brief) {
        (Some(whole), Some(brief)) => format!("{}\n{brief}", clip(&whole, 600)),
        (Some(whole), None) => clip(&whole, 600),
        (None, brief) => brief.unwrap_or_default(),
    };
    let service = text_arg(args, "service").and_then(|service| {
        let host = service
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .trim_start_matches("www.")
            .split('/')
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        if public_host(&host) {
            Some(WorkPartServiceV1 {
                host: Some(host),
                connection: None,
            })
        } else if service.chars().count() <= 40 && !service.contains('\n') {
            Some(WorkPartServiceV1 {
                host: None,
                connection: Some(service),
            })
        } else {
            None
        }
    });
    let records = args
        .get("records")
        .and_then(Value::as_array)
        .map(|fields| {
            fields
                .iter()
                .filter_map(Value::as_str)
                .map(|f| clip(f.trim(), 40))
                .take(12)
                .collect()
        })
        .unwrap_or_default();
    let search = super::recipes::parse(args.get("search"))?;
    Ok(PartSpec {
        title,
        helper,
        goal,
        brief,
        service,
        records,
        search,
        declined: None,
        draft: text_arg(args, "draft"),
        halt: None,
    })
}

/// A goal from a part's search or brief, when the lead left it out.
fn derived_goal(args: &Value, title: &str) -> Option<String> {
    let search = |key: &str| {
        args.get("search")
            .and_then(|search| search.get(key))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
    };
    let site = text_arg(args, "service").unwrap_or_else(|| title.to_owned());
    let goal = if let Some(query) = search("query") {
        format!("Find {query} on {site}")
    } else if let (Some(from), Some(to)) = (search("from"), search("to")) {
        format!("Find flights from {from} to {to} on {site}")
    } else if let Some(place) = search("place") {
        format!("Find stays in {place} on {site}")
    } else {
        let brief = text_arg(args, "brief")?;
        let first = brief
            .split(['\n', '.'])
            .map(str::trim)
            .find(|line| !line.is_empty())?;
        first.to_owned()
    };
    Some(clip(&goal, 200))
}

/// How a part ended, for its fact and the lead.
pub(crate) struct PartReport {
    pub state: WorkPartStateV1,
    pub summary: Option<String>,
    pub digest: String,
    pub objects: Vec<WorkArtifactId>,
    /// What the person can do so the part can do its job.
    pub need: Option<WorkPartNeedV1>,
    /// Its helper read what its pages gave and reported nothing usable.
    pub judged_empty: bool,
}
impl PartReport {
    fn ended(state: WorkPartStateV1, digest: &str, objects: Vec<WorkArtifactId>) -> Self {
        Self {
            state,
            summary: None,
            digest: digest.into(),
            objects,
            need: None,
            judged_empty: false,
        }
    }
}

/// The digest with the commit lines the helper's own words left out.
fn with_committed(mut digest: String, committed: &[String]) -> String {
    for line in committed {
        if !digest.contains(line.as_str()) {
            digest.push('\n');
            digest.push_str(line);
        }
    }
    digest
}

/// A browser part this run started on a site, and how it ended.
pub(crate) struct SitePart {
    pub site: String,
    pub title: String,
    pub ended: Option<SiteEnd>,
}
pub(crate) struct SiteEnd {
    pub state: WorkPartStateV1,
    pub need: Option<WorkPartNeedV1>,
    pub summary: Option<String>,
    /// Pages the part opened on the site.
    pub pages: usize,
}
/// Whether another browser part may start on a site in this run.
#[derive(Debug, Eq, PartialEq)]
pub(crate) enum SiteTurn {
    Go,
    /// Once more, another way in: the words go into its brief.
    Retry(String),
    /// Not again: the words tell the lead why and what to do instead.
    Refuse(String),
}

/// What the parts that already ran on `site` in this request allow: a site
/// is read once per request; one that failed for a reason another way in
/// can fix is tried once more, differently; one that needs the person, or
/// failed twice, is not tried again.
pub(crate) fn site_turn(sites: &[SitePart], site: &str) -> SiteTurn {
    let mine: Vec<&SitePart> = sites.iter().filter(|p| p.site == site).collect();
    if let Some(running) = mine.iter().find(|p| p.ended.is_none()) {
        return SiteTurn::Refuse(format!(
            "Part {} is already working on {site}; its report comes back when it ends. Never start a second part on the same site.",
            running.title
        ));
    }
    let ended: Vec<(&SitePart, &SiteEnd)> = mine
        .iter()
        .filter_map(|p| p.ended.as_ref().map(|end| (*p, end)))
        .filter(|(_, end)| end.state != WorkPartStateV1::Stopped)
        .collect();
    if let Some((part, end)) = ended
        .iter()
        .find(|(_, end)| end.state == WorkPartStateV1::Done)
    {
        return SiteTurn::Refuse(format!(
            "Part {} already read {site} in this request{}: build the result from its digest and the objects on its row. A request reads a site once.",
            part.title,
            end.summary
                .as_deref()
                .map(|s| format!(" ({s})"))
                .unwrap_or_default()
        ));
    }
    if let Some((part, need)) = ended.iter().find_map(|(part, end)| {
        end.need
            .as_ref()
            .filter(|need| blocks(need))
            .map(|need| (part, need))
    }) {
        return SiteTurn::Refuse(format!(
            "{site} needs {}; part {}'s row shows the fix. Don't start another part there: build the result from what the other parts found, and say in the reply what {site} needs.",
            need_words(need),
            part.title
        ));
    }
    let pages: usize = ended.iter().map(|(_, end)| end.pages).sum();
    if ended.len() >= 2 || pages >= 2 {
        return SiteTurn::Refuse(format!(
            "{site} was tried {} times in this request and gave nothing. Don't try it again: build the result from what you have and say in the reply that {site} could not be read.",
            pages.max(ended.len())
        ));
    }
    match ended.first() {
        Some((_, end)) => SiteTurn::Retry(format!(
            "An earlier part on {site} could not do its job{}. Go in another way: browse the site's front page and use its own search with fewer words, or open the page that lists what the goal needs. If that fails as well, finish at once and say what {site} did not show.",
            end.need
                .as_ref()
                .map(|need| format!(" ({})", need_words(need)))
                .unwrap_or_default()
        )),
        None => SiteTurn::Go,
    }
}

/// A need another try on the same site cannot meet: the person must act.
fn blocks(need: &WorkPartNeedV1) -> bool {
    !matches!(
        need,
        WorkPartNeedV1::Retry {
            reason: None
                | Some(
                    WorkPartReasonV1::CouldntRead
                        | WorkPartReasonV1::NotFound
                        | WorkPartReasonV1::SiteError
                        | WorkPartReasonV1::NoAnswer
                ),
            ..
        }
    )
}

#[derive(Debug, PartialEq)]
enum BrowseWords<'a> {
    None,
    Text(&'a str),
    Draft(&'a str),
}

fn browse_words<'a>(name: &str, arguments: &'a Value) -> Result<BrowseWords<'a>, String> {
    if name != "browse" {
        return Ok(BrowseWords::None);
    }
    // Models fill every optional property of a non-strict schema, often with "".
    let given = |key: &str| {
        arguments
            .get(key)
            .and_then(Value::as_str)
            .filter(|words| !words.trim().is_empty())
    };
    match (given("draft"), given("text")) {
        (Some(_), Some(_)) => Err("give draft or text, not both".into()),
        (Some(id), None) => Ok(BrowseWords::Draft(id)),
        (None, Some(text)) => Ok(BrowseWords::Text(text)),
        (None, None) => Ok(BrowseWords::None),
    }
}

/// A goal about what anyone sees on a service's site (its pricing, plans,
/// features or docs), not the person's own workspace there: it reads the
/// public site, and the person's connection is no way in to it.
/// Whether exact words a page will type are the person's own: they appear
/// in something the person wrote, however its lines were wrapped.
fn person_wrote<'a>(text: &str, sources: impl IntoIterator<Item = &'a str>) -> bool {
    let flat = |words: &str| words.split_whitespace().collect::<Vec<_>>().join(" ");
    let text = flat(text);
    !text.is_empty()
        && sources
            .into_iter()
            .any(|source| flat(source).contains(&text))
}

fn public_goal(spec: &PartSpec) -> bool {
    const PUBLIC: [&str; 12] = [
        "pricing",
        "price",
        "plans",
        "per seat",
        "per user",
        "features",
        "documentation",
        "docs",
        "changelog",
        "careers",
        "blog",
        "compare",
    ];
    const OWN: [&str; 9] = [
        "my ",
        "mine",
        "inbox",
        "assigned",
        "notification",
        "unread",
        "person's",
        "user's",
        "our ",
    ];
    let text = format!("{} {} {}", spec.title, spec.goal, spec.brief).to_lowercase();
    PUBLIC.iter().any(|word| text.contains(word)) && !OWN.iter().any(|word| text.contains(word))
}

// This selects the task path; it never grants a session or an effect.
// Unknown account services need the same handoff-capable path as known apps.
fn public_source_retry(spec: &PartSpec) -> bool {
    !account_goal(spec)
        && spec.declined.is_none()
        && spec.helper != WorkHelperV1::Connection
        && (public_goal(spec)
            || super::route::brand(spec.service.as_ref(), &spec.title)
                .is_none_or(|brand| super::route::site(&brand).is_none()))
}

fn account_goal(spec: &PartSpec) -> bool {
    if public_goal(spec) {
        return false;
    }
    let text = format!("{} {} {}", spec.title, spec.goal, spec.brief).to_lowercase();
    [
        "my account",
        "test account",
        "your account",
        "person's account",
        "user's account",
        "my inbox",
        "my workspace",
        "our workspace",
        "signed-in",
        "signed in",
        "sign-in",
        "sign in",
        "mfa",
        "two-factor",
    ]
    .iter()
    .any(|phrase| text.contains(phrase))
}

fn account_read_route(spec: &PartSpec, kind: WorkStepKindV1) -> Result<WorkStepKindV1, String> {
    if account_goal(spec) && matches!(kind, WorkStepKindV1::Read { goal: None, .. }) {
        Err("Use browse with the original account URL and the requested goal for this account task. Plain read is a public-page fetch and cannot replace a sign-in handoff. Keep all normal permissions; never substitute help or marketing pages for account content.".into())
    } else {
        Ok(kind)
    }
}

/// The site a browser part works on, by its registrable domain.
fn part_site(spec: &PartSpec) -> Option<String> {
    if spec.helper != WorkHelperV1::Browser {
        return None;
    }
    let host = spec.service.as_ref()?.host.as_deref()?;
    crate::work_sites::site_of(&format!("https://{host}/")).or_else(|| Some(host.to_owned()))
}

/// Said to the lead once a page waited its whole turn for the browser.
const BROWSER_DOWN: &str = "Pages cannot open in this request now: the browser did not take the last page. Don't start browser parts or read pages: build the result from what was found, use web_search for what is missing, and say in the reply which sites could not be opened.";

enum Kit {
    Browser,
    Research,
    Files,
    Registered(Arc<dyn LeadHelper>),
}

impl<'a, B, Fut> Lead<'a, B>
where
    B: FnMut(WorkAttemptProbe, WorkAgentBrowseRequest) -> Fut + Send,
    Fut: Future<Output = Result<WorkBrowserOutcome, WorkError>> + Send,
{
    /// A browse that names a canvas draft carries its words, as the person
    /// last left them, as the exact text the page types.
    async fn with_draft(&self, name: &str, arguments: &Value) -> Result<Value, String> {
        let id = match browse_words(name, arguments)? {
            BrowseWords::None => return Ok(arguments.clone()),
            BrowseWords::Text(text) => {
                let projection = self.run.probe.runtime_projection().await.ok();
                let earlier = projection.iter().flat_map(|projection| {
                    projection
                        .executions
                        .iter()
                        .filter_map(|execution| execution.spec.request.as_deref())
                });
                if !person_wrote(
                    text,
                    std::iter::once(self.objective.as_str()).chain(earlier),
                ) {
                    return Err("text is only the person's own words, copied character for character from their request; these words are not in it. Copy them exactly, or use draft for a draft on the canvas.".into());
                }
                return Ok(arguments.clone());
            }
            BrowseWords::Draft(id) => id,
        };
        let projection = self
            .run
            .probe
            .runtime_projection()
            .await
            .map_err(|_| "The canvas could not be read".to_owned())?;
        let words = super::objects::draft_words(&projection, self.run.probe.execution(), id)?;
        let mut arguments = arguments.clone();
        if let Some(fields) = arguments.as_object_mut() {
            fields.remove("draft");
            fields.insert("text".into(), Value::String(words));
        }
        Ok(arguments)
    }

    /// Runs one part to its end and returns what the lead reads.
    pub(crate) async fn part(&self, mut spec: PartSpec) -> (String, bool) {
        if spec.helper == WorkHelperV1::Browser && self.run.browser_down() {
            return (BROWSER_DOWN.into(), true);
        }
        if let Some(service) = spec.service.as_mut() {
            if let Some(app) = service.host.as_deref().and_then(super::route::app_host) {
                service.host = Some(app.into());
            }
        }
        let site = part_site(&spec);
        if let Some(site) = &site {
            match self.claim_site(site, &spec.title) {
                SiteTurn::Refuse(why) => return (why, true),
                SiteTurn::Retry(note) => {
                    spec.brief = format!("{}\n{note}", spec.brief).trim().to_owned();
                    // Its results page is what gave nothing.
                    spec.search = None;
                }
                SiteTurn::Go => {}
            }
        }
        let title = spec.title.clone();
        let release_site = |state: WorkPartStateV1, need, summary, pages| {
            if let Some(site) = &site {
                self.end_site(
                    site,
                    &title,
                    SiteEnd {
                        state,
                        need,
                        summary,
                        pages,
                    },
                );
            }
        };
        if !self.claim_part() {
            release_site(WorkPartStateV1::Stopped, None, None, 0);
            return (
                format!("A run holds at most {MAX_WORK_PARTS} parts; finish the work with the parts you have"),
                true,
            );
        }
        self.route(&mut spec).await;
        let mut fact = WorkPartFactV1 {
            id: WorkPartId::generate(),
            title: spec.title.clone(),
            helper: spec.helper,
            service: spec.service.clone(),
            goal: spec.goal.clone(),
            state: WorkPartStateV1::Planned,
            started_ms: None,
            ended_ms: None,
            summary: None,
            need: None,
        };
        if self.run.part(fact.clone()).await.is_err() {
            release_site(WorkPartStateV1::Stopped, None, None, 0);
            return ("The part could not be recorded".into(), true);
        }
        if let Some(need) = spec.halt.clone() {
            self.release_part();
            fact.state = WorkPartStateV1::Failed;
            fact.need = Some(need.clone());
            fact.ended_ms = Some(now_ms());
            let _ = self.run.part(fact.clone()).await;
            release_site(WorkPartStateV1::Failed, Some(need.clone()), None, 0);
            return (
                format!(
                    "Part {} ({}) did not run\nIt needs {}; its row shows the fix. Make no object, figure, pick or task for it, and say in the reply in one sentence what the person does next.\n",
                    spec.title,
                    fact.id,
                    need_words(&need)
                ),
                true,
            );
        }
        let _slot = self.slots.acquire().await.ok();
        fact.state = WorkPartStateV1::Running;
        fact.started_ms = Some(now_ms());
        let _ = self.run.part(fact.clone()).await;
        self.run.activity(WorkActivityV1::Delegating);
        let mut report = self.helper(fact.id, &spec).await;
        // Records stand for the part unless its helper read them and found
        // nothing in them that answers the goal.
        if report.objects.is_empty()
            && report.state != WorkPartStateV1::Stopped
            && !report.judged_empty
            && matches!(self.kit(spec.helper), Kit::Browser | Kit::Research)
        {
            if let Some((id, count)) = self.place_found(fact.id, &spec).await {
                report.objects.push(id);
                if report.state == WorkPartStateV1::Failed && report.need.is_none() {
                    report.state = WorkPartStateV1::Done;
                    report.summary = Some(format!("{count} found"));
                }
            }
        }
        self.settle_need(fact.id, &spec, &mut report).await;
        // A page that still found the person signed out, though their profile
        // now holds a session there, met the sign-in they just made before it
        // settled: it reads again once, as their own "Try again" would.
        if let Some(WorkPartNeedV1::SignIn { host }) = report.need.clone() {
            let signed_in = crate::work_context::sessions_present(self.run.profile, vec![host])
                .await
                .first()
                .copied()
                .flatten()
                == Some(true);
            if signed_in && report.state != WorkPartStateV1::Stopped {
                report = self.helper(fact.id, &spec).await;
                self.settle_need(fact.id, &spec, &mut report).await;
            }
        }
        if let Kit::Registered(helper) = self.kit(spec.helper) {
            let context = LeadToolContext {
                run: self.run,
                part: Some(fact.id),
            };
            for (title, data) in helper.objects(context, &report.digest).await {
                match context.publish(&title, data).await {
                    Ok(id) => report.objects.push(id),
                    Err(_) => self.run.report(super::WorkLeadDiagnostic::ObjectRefused {
                        reason: Some(objects::ObjectRefusal::Shape),
                        at: None,
                    }),
                }
            }
        }
        self.release_part();
        fact.state = report.state;
        fact.need = report.need.clone();
        fact.ended_ms = Some(now_ms());
        fact.summary = report
            .summary
            .as_deref()
            .map(|s| clip(s.lines().next().unwrap_or(""), 78))
            .filter(|s| !s.trim().is_empty());
        let _ = self.run.part(fact.clone()).await;
        let pages = match &site {
            Some(site) => self
                .run
                .execution()
                .await
                .map(|execution| {
                    execution
                        .steps
                        .iter()
                        .filter(|s| s.part == Some(fact.id))
                        .filter(|s| match &s.kind {
                            WorkStepKindV1::Read { url, .. } => {
                                crate::work_sites::site_of(url).as_deref() == Some(site.as_str())
                            }
                            _ => false,
                        })
                        .count()
                })
                .unwrap_or(0),
            None => 0,
        };
        release_site(report.state, fact.need.clone(), fact.summary.clone(), pages);
        self.run.report(super::WorkLeadDiagnostic::PartEnded {
            helper: spec.helper,
            state: report.state,
            objects: report.objects.len(),
        });
        let mut out = format!(
            "Part {} ({}) {}",
            spec.title,
            fact.id,
            match report.state {
                WorkPartStateV1::Done => "done",
                WorkPartStateV1::Stopped => "stopped",
                _ => "could not do its job",
            }
        );
        if let Some(summary) = &fact.summary {
            out.push_str(&format!(": {summary}"));
        }
        out.push('\n');
        if let Some(need) = &fact.need {
            out.push_str(&format!(
                "It needs {}; its row shows the fix. Build the result only from what was found, make no object, figure, pick or task for what it could not do, and when nothing was found say so in the reply in one sentence: that the site could not be read, never that nothing there matches.\n",
                need_words(need)
            ));
        } else if report.state != WorkPartStateV1::Done {
            out.push_str("Build the result only from what was found; make no object, figure, pick or task for what it could not do.\n");
        }
        // A site whose search or listings would not read often still serves
        // its product and listing pages, which a search finds.
        let public_retry = matches!(
            fact.need,
            None | Some(WorkPartNeedV1::Retry {
                reason: None
                    | Some(
                        WorkPartReasonV1::CouldntRead
                            | WorkPartReasonV1::NotFound
                            | WorkPartReasonV1::NoAnswer
                    ),
                ..
            })
        );
        if let (Some(site), true, true, true) = (
            &site,
            report.objects.is_empty(),
            report.state == WorkPartStateV1::Failed,
            public_retry && public_source_retry(&spec) && !self.run.browser_down(),
        ) {
            out.push_str(&format!(
                "Before you settle for nothing from {site}: search the web once for what the part looked for there, and read the one to three best {site} pages the search lists; a product or listing page often reads where the site's search did not.\n"
            ));
        }
        if !report.objects.is_empty() {
            let projection = self.run.probe.runtime_projection().await.ok();
            let canvas = projection
                .as_ref()
                .map(|p| objects::canvas(p, self.run.probe.execution()))
                .unwrap_or_default();
            out.push_str("Objects it placed, which stand in the result as they are (never place another object of the same things; to line up offers of the same thing from several sites, create one new picks with them side by side and leave this one as it is):\n");
            out.push_str(&objects::view(
                &canvas
                    .into_iter()
                    .filter(|o| report.objects.contains(&o.artifact.id))
                    .collect::<Vec<_>>(),
            ));
        }
        let catalog = self
            .run
            .records
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .catalog(fact.id);
        if !catalog.is_empty() {
            out.push_str(
                "Native records (select r keys with data.records; values remain native). A field marked unknown was not shown by the page: say it was not shown, never that it is none or not set.\n",
            );
            out.push_str(&catalog);
        }
        out.push_str(&report.digest);
        (out, report.state != WorkPartStateV1::Done)
    }

    /// Places the records a browser part's pages gave as its picks, when it
    /// placed nothing itself; returns the object and how many it holds.
    async fn place_found(
        &self,
        part: WorkPartId,
        spec: &PartSpec,
    ) -> Option<(WorkArtifactId, usize)> {
        use super::found;
        if !self
            .run
            .records
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .catalog(part)
            .is_empty()
        {
            return None;
        }
        let execution = self.run.execution().await.ok()?;
        let rows = found::rows(&execution, part);
        if rows.is_empty() {
            return None;
        }
        let facet = found::facet(
            spec.search.as_ref(),
            &format!("{} {} {}", spec.title, spec.goal, spec.brief),
            &rows,
        );
        let query = spec.search.as_ref().and_then(|s| s.query.as_deref());
        let cheapest = found::wants_cheapest(&format!("{} {}", spec.goal, self.objective));
        let (data, sources) = found::picks(&rows, facet, query, cheapest, |link, page| {
            self.run.cite(link.clone(), "", Some(page))
        })?;
        let count = data["items"].as_array().map_or(0, Vec::len);
        let site = spec
            .service
            .as_ref()
            .and_then(|s| s.host.as_deref())
            .map(|host| {
                let label = host
                    .trim_start_matches("www.")
                    .split('.')
                    .next()
                    .unwrap_or(host);
                if spec.title.to_lowercase().contains(&label.to_lowercase()) {
                    spec.title.clone()
                } else {
                    crate::work_sites::service_name(host)
                }
            });
        let title = found::title(facet, site.as_deref(), &spec.title);
        let mut proposed =
            objects::propose(self.run, &[], "picks", &title, data, &sources, &[], true).ok()?;
        objects::hydrate_record_media(
            &mut proposed,
            std::slice::from_ref(&execution),
            self.run.profile,
            self.run.probe.work(),
        );
        let honesty = objects::Honesty {
            sourced: !proposed.evidence.is_empty(),
            failed: Vec::new(),
        };
        if objects::honest(&proposed.data, &honesty).is_err() {
            proposed.data = objects::mend(&proposed.data, &honesty)?.0;
        }
        let id = objects::publish(self.run, proposed, Some(part), None)
            .await
            .ok()?;
        self.run.activity(WorkActivityV1::ProducingArtifact);
        Some((id, count))
    }

    fn kit(&self, helper: WorkHelperV1) -> Kit {
        if let Some(registered) = self.helpers.iter().find(|h| h.kind() == helper) {
            return Kit::Registered(registered.clone());
        }
        match helper {
            WorkHelperV1::Research => Kit::Research,
            WorkHelperV1::Computer => Kit::Files,
            // Without an installed route, a connection works through its site.
            WorkHelperV1::Browser | WorkHelperV1::Connection => Kit::Browser,
        }
    }

    async fn helper(&self, part: WorkPartId, spec: &PartSpec) -> PartReport {
        if spec.helper == WorkHelperV1::Computer && self.run.files().is_none() {
            if let Ok(execution) = self.run.execution().await {
                if let Some(mut fact) = execution.parts.into_iter().find(|fact| fact.id == part) {
                    fact.state = WorkPartStateV1::Waiting;
                    fact.need = Some(WorkPartNeedV1::ChooseFolder);
                    let _ = self.run.part(fact).await;
                }
            }
            if !super::folders::choose(
                self.run,
                "Choose the folder this computer work should use.",
                Some(part),
            )
            .await
            {
                let mut report = PartReport::ended(
                    if self.run.cancelled().await {
                        WorkPartStateV1::Stopped
                    } else {
                        WorkPartStateV1::Failed
                    },
                    "No folder was chosen; no computer work was performed.",
                    Vec::new(),
                );
                report.need = Some(WorkPartNeedV1::ChooseFolder);
                return report;
            }
            if let Ok(execution) = self.run.execution().await {
                if let Some(mut fact) = execution.parts.into_iter().find(|fact| fact.id == part) {
                    fact.state = WorkPartStateV1::Running;
                    fact.need = None;
                    let _ = self.run.part(fact).await;
                }
            }
        }
        let kit = self.kit(spec.helper);
        let (prompt_text, role, max_turns) = match &kit {
            Kit::Browser => (prompt::BROWSER.to_owned(), WorkModelRole::Page, PART_TURNS),
            Kit::Research => (
                prompt::RESEARCH.to_owned(),
                WorkModelRole::Light,
                PART_TURNS,
            ),
            Kit::Files => (
                prompt::COMPUTER.to_owned(),
                WorkModelRole::Page,
                PART_TURNS + 6,
            ),
            Kit::Registered(helper) => (
                helper.prompt().to_owned(),
                helper.role(),
                helper.max_turns(),
            ),
        };
        let model = self.models.for_role(role);
        let share = self.part_share(model);
        let objective = format!("{}\n\nThe person's request: {}", spec.goal, self.objective);
        let hands = Hands::new(
            self.run,
            self.attempt,
            self.search,
            self.browser,
            Some(part),
            share,
            objective,
        )
        .await
        .personal(
            spec.helper == WorkHelperV1::Connection
                || spec.declined.is_some()
                || (!public_goal(spec)
                    && super::route::brand(spec.service.as_ref(), &spec.title)
                        .is_some_and(|brand| super::route::site(&brand).is_some())),
        );
        let mut tools: Vec<WorkModelTool> = match &kit {
            Kit::Browser => vec![
                prompt::browse_tool(),
                prompt::read_tool(),
                prompt::search_tool(),
            ],
            Kit::Research => vec![prompt::search_tool(), prompt::read_tool()],
            Kit::Files => prompt::file_tools(),
            Kit::Registered(helper) => helper.tools().tools(
                LeadScope::Helper(spec.helper),
                &super::tools::LeadRunView {
                    run: self.run,
                    service: spec.service.as_ref(),
                },
            ),
        };
        // Tool sets beside the lead's own offer tools to helpers too.
        let view = super::tools::LeadRunView {
            run: self.run,
            service: spec.service.as_ref(),
        };
        let mut sets: Vec<Arc<dyn LeadToolSet>> = Vec::new();
        if let Kit::Registered(helper) = &kit {
            sets.push(helper.tools());
        }
        for set in &self.extra {
            let offered = set.tools(LeadScope::Helper(spec.helper), &view);
            if !offered.is_empty() {
                tools.extend(
                    offered
                        .into_iter()
                        .filter(|t| !tools.iter().any(|known| known.name == t.name))
                        .collect::<Vec<_>>(),
                );
                sets.push(set.clone());
            }
        }
        tools.extend(prompt::helper_tools());
        let system = vec![
            WorkModelSystemBlock {
                text: prompt::HELPER.to_owned(),
                cache: false,
            },
            WorkModelSystemBlock {
                text: prompt_text,
                cache: true,
            },
        ];
        let mut brief = format!(
            "Part: {} ({})\nGoal: {}\n",
            spec.title,
            match spec.helper {
                WorkHelperV1::Browser => "browser",
                WorkHelperV1::Research => "research",
                WorkHelperV1::Computer => "computer",
                WorkHelperV1::Connection => "connection",
            },
            spec.goal
        );
        if !spec.brief.is_empty() {
            brief.push_str(&format!("Details: {}\n", spec.brief));
        }
        if matches!(kit, Kit::Browser) && account_goal(spec) {
            brief.push_str("This is an account task: use browse with its original URL and requested goal so sign-in/MFA can hand the page to the person. Existing effect approvals still apply. Continue in the browser through app-launch interstitials. Help, marketing and app-launch pages are not the requested account content.\n");
        }
        if let (Kit::Browser, Some(draft)) = (&kit, &spec.draft) {
            brief.push_str(&format!(
                "The draft to enter exactly as it stands is canvas object {draft}: call browse with draft set to it and a goal that only says what to do with it.\n"
            ));
        }
        if let Some(service) = &spec.service {
            if let Some(host) = &service.host {
                brief.push_str(&format!("Site: {host}\n"));
            }
            if let Some(connection) = &service.connection {
                brief.push_str(&format!("Service: {connection}\n"));
            }
        }
        if !spec.records.is_empty() {
            brief.push_str(&format!(
                "Return for each thing: {}\n",
                spec.records.join(", ")
            ));
        }
        let given = match kit {
            Kit::Browser => given_pages(&self.objective, spec),
            _ => Vec::new(),
        };
        if matches!(kit, Kit::Browser) && !account_goal(spec) {
            if let Some(collection) = records_collection(&spec.title, &spec.records) {
                let mut records = self.run.records.lock().unwrap_or_else(|e| e.into_inner());
                for url in &given {
                    if let Some(host) = crate::work_sites::host_of(url) {
                        records.remember_site(&host, &collection);
                    }
                }
                if let Some(host) = spec
                    .service
                    .as_ref()
                    .and_then(|service| service.host.as_deref())
                {
                    records.remember_site(host, &collection);
                }
            }
        }
        if !given.is_empty() {
            let collection = records_collection(&spec.title, &spec.records);
            let requests = given
                .iter()
                .enumerate()
                .map(|(index, url)| Request {
                    call: format!("given-{index}"),
                    kind: WorkStepKindV1::Read {
                        verbatim: None,
                        url: url.clone(),
                        collection: collection.clone(),
                        goal: None,
                    },
                    mine: false,
                    view: false,
                })
                .collect();
            match hands.run(requests).await {
                Ok(done) => {
                    for ((_, content, error), url) in done.into_iter().zip(&given) {
                        brief.push_str(&format!(
                            "The page the person gave, already read as given ({url}){}:\n{}\n",
                            if error { ", which failed" } else { "" },
                            clip(&content, 6_000)
                        ));
                    }
                    brief.push_str("Build from what it gave. Read or browse other pages only for what it lacks, and never search the site for what it already shows.\n");
                }
                Err(_) => {
                    return PartReport::ended(
                        WorkPartStateV1::Stopped,
                        "Stopped while its pages ran.",
                        Vec::new(),
                    )
                }
            }
        }
        if let (Kit::Browser, Some(search), true) = (&kit, &spec.search, given.is_empty()) {
            let pages = super::recipes::pages(
                search,
                spec.service
                    .as_ref()
                    .and_then(|service| service.host.as_deref()),
            );
            if !pages.is_empty() {
                brief.push_str("Results pages with the search already in them, the part's own site first; browse the first as start, its goal to read the results shown as records (open an item only for a field the list lacks). If it will not load, use the next one before any form:\n");
                for (name, url) in pages {
                    // Built by Rust from a fixed template on a known store,
                    // not an address the model wrote.
                    self.run.allow_url(&url);
                    brief.push_str(&format!("- {name}: {url}\n"));
                }
            }
        }
        let folders = self.run.folders();
        if matches!(kit, Kit::Files | Kit::Registered(_)) {
            if !folders.current.is_empty() {
                brief.push_str(&format!("Folders: {}\n", folders.current.join(", ")));
            }
            if !folders.available.is_empty() {
                brief.push_str(&format!(
                    "Also readable, only if the goal is about them: {}\n",
                    folders.available.join(", ")
                ));
            }
        }
        brief.push_str(&format!(
            "The person's request: {}\nNow: {}\nYou have at most {max_turns} turns.",
            self.objective,
            prompt::now_line()
        ));
        let mut messages = vec![WorkModelMessage::User(vec![WorkModelPart::Text(brief)])];
        let mut placed: Vec<WorkArtifactId> = Vec::new();
        let mut last_text = String::new();
        let mut committed: Vec<String> = Vec::new();
        let mut spent = WorkUsage::default();
        let mut warned = false;
        let mut idle = 0u8;
        let mut searches = 0usize;
        let mut found = String::new();
        for turn in 0..max_turns {
            let over = {
                let pages = hands.used().await;
                pages.cost_micro_usd.saturating_add(spent.cost_micro_usd) >= share.cost_micro_usd
                    || pages.model_tokens.saturating_add(spent.model_tokens) >= share.model_tokens
            };
            let last = turn + 1 == max_turns;
            if (over || last) && !warned {
                warned = true;
                messages.push(WorkModelMessage::User(vec![WorkModelPart::Text(
                    "This part's budget is spent: place what you found, then call finish now."
                        .into(),
                )]));
            } else if over {
                break;
            }
            // Long helpers, such as a computer part's forty turns, would
            // otherwise outgrow their model's context.
            super::lead::compact(
                &mut messages,
                super::lead::conversation_budget(&model.entry),
            );
            let request = WorkModelRequest {
                image_fence: None,
                model: model.entry.model.clone(),
                system: system.clone(),
                tools: tools.clone(),
                messages: messages.clone(),
                max_output_tokens: 8_000,
                reasoning: model
                    .entry
                    .supports
                    .reasoning
                    .then_some(WorkModelReasoning::Low),
                native_search: false,
                parallel_tools: true,
                tool_choice: WorkModelToolChoice::Auto,
                volatile_tail: 0,
                cache_key: Some(work_model_cache_key(&system, &tools)),
            };
            let (outcome, usage) = match call::call(self.run, model, request, role, None).await {
                Ok(done) => done,
                Err(CallFailure::Stopped) => {
                    return PartReport::ended(
                        WorkPartStateV1::Stopped,
                        "Stopped before it finished.",
                        placed,
                    )
                }
                Err(failure) => {
                    self.record_turn(
                        Some(part),
                        WorkStepStatus::Failed,
                        None,
                        failure,
                        model.entry.model.provider,
                    )
                    .await;
                    let mut report = PartReport::ended(
                        WorkPartStateV1::Failed,
                        "The helper's model could not be reached.",
                        placed,
                    );
                    report.need = Some(WorkPartNeedV1::Retry {
                        host: None,
                        reason: None,
                    });
                    return report;
                }
            };
            spent = add(spent, usage);
            let text = call::text(&outcome.assistant);
            if !text.is_empty() {
                last_text = text.clone();
            }
            self.turn_step(Some(part), usage, call::say_line(&text))
                .await;
            let calls = call::tool_calls(&outcome.assistant);
            messages.push(WorkModelMessage::Assistant(outcome.assistant));
            if calls.is_empty() {
                idle += 1;
                if idle >= 2 {
                    break;
                }
                messages.push(WorkModelMessage::User(vec![WorkModelPart::Text(
                    "Use your tools to do the part, or call finish.".into(),
                )]));
                continue;
            }
            idle = 0;
            let mut results: Vec<Option<WorkModelToolResult>> = vec![None; calls.len()];
            let mut requests = Vec::new();
            let mut finished: Option<(String, String, bool, Option<WorkPartNeedV1>)> = None;
            let mut enough: Option<bool> = None;
            for (index, tool_call) in calls.iter().enumerate() {
                let answer = |content: String, is_error: bool| WorkModelToolResult {
                    call: tool_call.id.clone(),
                    content,
                    is_error,
                };
                match tool_call.name.as_str() {
                    "web_search" | "read" | "browse" | "list" | "read_file" | "search_files"
                    | "write_file" | "edit_file" | "run_command"
                        if !matches!(kit, Kit::Registered(_)) =>
                    {
                        let arguments =
                            match self.with_draft(&tool_call.name, &tool_call.arguments).await {
                                Ok(arguments) => arguments,
                                Err(fault) => {
                                    results[index] = Some(answer(fault, true));
                                    continue;
                                }
                            };
                        match step_request(&tool_call.name, &arguments, self.run)
                            .and_then(|kind| if matches!(kit, Kit::Browser) { account_read_route(spec, kind) } else { Ok(kind) }) {
                            Ok(WorkStepKindV1::Search { .. }) if searches >= PART_SEARCHES => {
                                results[index] = Some(answer(
                                    "This part has used its searches: place what you found and finish, saying what is missing.".into(),
                                    true,
                                ))
                            }
                            Ok(WorkStepKindV1::Search { .. })
                                if searches >= PART_FREE_SEARCHES
                                    && *enough.get_or_insert(
                                        self.found_enough(&spec.goal, &found).await,
                                    ) =>
                            {
                                results[index] = Some(answer(
                                    "What your searches found already answers the goal: place what you found and finish.".into(),
                                    true,
                                ))
                            }
                            Ok(mut kind) => {
                                // Only a daily app has views: a store or a listings
                                // site read "as a view" would lose its page planner.
                                let view = reads_app(&kind)
                                    || (tool_call.arguments.get("view").and_then(Value::as_bool)
                                        == Some(true)
                                        && daily_app(&kind));
                                if let (true, WorkStepKindV1::Read { goal: Some(_), collection, .. }) = (view, &mut kind) {
                                    *collection = None;
                                }
                                searches += usize::from(matches!(kind, WorkStepKindV1::Search { .. }));
                                requests.push(Request {
                                    call: tool_call.id.clone(),
                                    kind,
                                    mine: tool_call
                                        .arguments
                                        .get("mine")
                                        .and_then(Value::as_bool)
                                        .unwrap_or(false),
                                    view,
                                })
                            }
                            Err(fault) => results[index] = Some(answer(fault, true)),
                        }
                    }
                    "create" => {
                        let (content, error) =
                            self.create(&tool_call.arguments, Some(part), true).await;
                        if !error {
                            if let Some(id) =
                                content.split_whitespace().find_map(WorkArtifactId::parse)
                            {
                                placed.push(id);
                            }
                        }
                        results[index] = Some(answer(content, error));
                    }
                    "finish" => {
                        let summary = text_arg(&tool_call.arguments, "summary").unwrap_or_default();
                        let digest = text_arg(&tool_call.arguments, "digest").unwrap_or_default();
                        let found = tool_call
                            .arguments
                            .get("found")
                            .and_then(Value::as_bool)
                            .unwrap_or(true);
                        let need = need_arg(tool_call.arguments.get("need"));
                        finished = Some((summary, digest, found, need));
                        results[index] = Some(answer("Reported to the lead.".into(), false));
                    }
                    name => match sets.iter().find(|set| {
                        set.tools(LeadScope::Helper(spec.helper), &view)
                            .iter()
                            .any(|tool| tool.name == name)
                    }) {
                        Some(set) => {
                            let context = LeadToolContext {
                                run: self.run,
                                part: Some(part),
                            };
                            let outcome = set.call(context, tool_call.clone()).await;
                            if super::run::tool_reads_private(&tool_call.name) {
                                self.run.mark_private();
                            }
                            if !outcome.is_error {
                                if let Some(id) = outcome
                                    .content
                                    .split_whitespace()
                                    .find_map(WorkArtifactId::parse)
                                {
                                    if outcome.content.starts_with("Placed project") {
                                        placed.push(id);
                                    }
                                }
                            }
                            results[index] = Some(answer(outcome.content, outcome.is_error));
                        }
                        None => {
                            results[index] = Some(answer(
                                format!("{} is not a tool of this part", tool_call.name),
                                true,
                            ))
                        }
                    },
                }
            }
            if !requests.is_empty() {
                let asked: Vec<(String, bool)> = requests
                    .iter()
                    .map(|r| {
                        (
                            r.call.clone(),
                            matches!(r.kind, WorkStepKindV1::Search { .. }),
                        )
                    })
                    .collect();
                match hands.run(requests).await {
                    Ok(done) => {
                        for (id, content, error) in done {
                            let search = asked.iter().any(|(call, search)| *call == id && *search);
                            for line in crate::work_agent::split_ledger(content.trim_end()).1 {
                                if !committed.iter().any(|seen| seen == line) {
                                    committed.push(line.into());
                                }
                            }
                            if search && !error && found.len() < FOUND_CHARS {
                                found.push_str(&clip(&content, 1_500));
                                found.push('\n');
                            }
                            if let Some(at) = calls.iter().position(|c| c.id == id) {
                                results[at] = Some(WorkModelToolResult {
                                    call: id,
                                    content,
                                    is_error: error,
                                });
                            }
                        }
                    }
                    Err(_) => {
                        return PartReport::ended(
                            WorkPartStateV1::Stopped,
                            "Stopped while its pages ran.",
                            placed,
                        )
                    }
                }
            }
            let results: Vec<WorkModelToolResult> = results
                .into_iter()
                .zip(&calls)
                .map(|(result, c)| {
                    result.unwrap_or_else(|| WorkModelToolResult {
                        call: c.id.clone(),
                        content: "Not run".into(),
                        is_error: true,
                    })
                })
                .collect();
            messages.push(WorkModelMessage::ToolResults(results));
            if let Some((summary, digest, found, need)) = finished {
                // What the page task committed is a fact of the run, not of
                // the helper's wording.
                let found = found || !committed.is_empty();
                return PartReport {
                    state: if found && need.is_none() {
                        WorkPartStateV1::Done
                    } else {
                        WorkPartStateV1::Failed
                    },
                    summary: Some(summary),
                    digest: with_committed(clip(&digest, 3_000), &committed),
                    judged_empty: !found,
                    objects: placed,
                    need,
                };
            }
        }
        PartReport {
            state: if placed.is_empty() {
                WorkPartStateV1::Failed
            } else {
                WorkPartStateV1::Done
            },
            summary: None,
            digest: with_committed(
                if last_text.is_empty() {
                    "It ran out of turns before it reported.".into()
                } else {
                    clip(&last_text, 2_000)
                },
                &committed,
            ),
            objects: placed,
            need: None,
            judged_empty: false,
        }
    }

    /// Whether a part's searches already answer its goal, by the search
    /// provider's own check; false when it cannot tell.
    async fn found_enough(&self, goal: &str, found: &str) -> bool {
        if found.trim().is_empty() {
            return false;
        }
        let scope = zephium_core::work::search::WorkPublicSearchScope {
            provider: self.run.grant.provider,
            model: self.run.grant.model.clone(),
            query: clip(goal, 400),
        };
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        match self
            .search
            .enough(&scope, found, self.run.remaining(), deadline)
            .await
        {
            Ok(decided) => {
                if decided.usage.model_tokens > 0 || decided.usage.cost_micro_usd > 0 {
                    self.run.charge(decided.usage);
                }
                decided.answers
            }
            Err(_) => false,
        }
    }

    /// A part about a service the person uses goes through their own
    /// connection when they have one and accept it (asked once per work),
    /// else through the website in their session.
    async fn route(&self, spec: &mut PartSpec) {
        if self.run.grant.private
            || !matches!(
                spec.helper,
                WorkHelperV1::Browser | WorkHelperV1::Connection
            )
        {
            return;
        }
        let Some(brand) = super::route::brand(spec.service.as_ref(), &spec.title) else {
            return;
        };
        if spec.helper == WorkHelperV1::Browser && public_goal(spec) {
            return;
        }
        // Reading a service's public site (its product or pricing pages) is
        // not the person's workspace: their connection is not offered.
        if spec.helper == WorkHelperV1::Browser
            && !super::route::reaches_own_app(
                &brand,
                spec.service.as_ref().and_then(|s| s.host.as_deref()),
            )
        {
            return;
        }
        let _routing = self.run.routing.lock().await;
        let profile = self.run.profile.to_string();
        let servers = crate::work_connections::store::shared()
            .and_then(|store| store.servers(&profile).ok())
            .unwrap_or_default();
        let offer = super::route::offer(
            &brand,
            crate::work_connections::helper::shared().gh_ready(),
            &servers,
        );
        let site = super::route::site(&brand)
            .and_then(|host| crate::work_sites::site_of(&format!("https://{host}/")));
        let way = self.choose(&brand, offer.as_ref(), site.as_deref()).await;
        let accepted = way == Some(super::route::Way::Connection);
        if let (Some(site), Some(way)) = (&site, way) {
            use super::route::Way;
            use crate::work_sites::EntryAnswer;
            // The answer covers the site the part names too, when the lead
            // named the app by a site it moved away from.
            let named = spec
                .service
                .as_ref()
                .and_then(|service| service.host.as_deref())
                .and_then(|host| crate::work_sites::site_of(&format!("https://{host}/")))
                .filter(|named| named != site);
            let enter = |answer| {
                self.run.set_entry(site, answer);
                if let Some(named) = &named {
                    self.run.set_entry(named, answer);
                }
            };
            match way {
                Way::Website => enter(EntryAnswer::Allow),
                Way::AlwaysWebsite => enter(EntryAnswer::Always),
                Way::NotNow => {
                    enter(EntryAnswer::NotNow);
                    spec.halt = Some(WorkPartNeedV1::AllowSite { host: site.clone() });
                }
                Way::Connect => {
                    spec.halt =
                        super::route::known_server(&brand).map(|name| WorkPartNeedV1::Connect {
                            connection: name.into(),
                        });
                }
                Way::Connection => {}
            }
        }
        if spec.halt.is_some() {
            return;
        }
        if !accepted && spec.helper == WorkHelperV1::Browser {
            spec.declined = offer.clone();
        }
        if let Some((helper, service)) =
            super::route::settle(spec.helper, &brand, offer.as_ref(), accepted)
        {
            spec.helper = helper;
            spec.service = Some(service);
        }
    }

    /// How the person has the service reached, asked once per work: with
    /// their connection or on the website when they have a connection,
    /// on the website or after connecting when the service has one they
    /// could add. None when the service has neither, or it is not asked:
    /// the part works as it always did.
    async fn choose(
        &self,
        brand: &str,
        offer: Option<&super::route::Offer>,
        site: Option<&str>,
    ) -> Option<super::route::Way> {
        use super::route::Way;
        let name = match offer {
            Some(offer) => offer.connection.clone(),
            None => super::route::known_server(brand)?.to_owned(),
        };
        // A service with no website of ours has only its connection.
        let Some(site) = site else {
            let offer = offer?;
            self.run.accept_connection(&offer.yes);
            return Some(Way::Connection);
        };
        let objective = self.objective.to_lowercase();
        if let Some(offer) = offer {
            let asked_for = self.run.accepted_connection(&offer.yes)
                || objective.contains(&offer.yes.to_lowercase())
                || objective
                    .split(|c: char| !c.is_alphanumeric())
                    .any(|word| matches!(word, "mcp" | "connection" | "connector"));
            if asked_for {
                self.run.accept_connection(&offer.yes);
                return Some(Way::Connection);
            }
        }
        let standing = crate::work_sites::standing(&self.run.handle, self.run.profile)
            .await
            .unwrap_or_default()
            .iter()
            .any(|entry| {
                entry.site == site
                    && entry.access == zephium_core::work::sites::WorkSiteAccessV1::Always
            });
        if standing {
            return Some(Way::Website);
        }
        let (prompt, options) = super::route::question(&name, offer.is_some());
        let earlier = self
            .run
            .probe
            .runtime_projection()
            .await
            .ok()
            .and_then(|projection| {
                projection.executions.iter().rev().find_map(|execution| {
                    execution
                        .steps
                        .iter()
                        .rev()
                        .find_map(|step| match &step.kind {
                            WorkStepKindV1::Ask {
                                prompt: asked,
                                answer: Some(answer),
                                purpose: Some(WorkAskPurposeV1::Connection),
                                ..
                            } if *asked == prompt => Some(super::route::way(answer)),
                            _ => None,
                        })
                })
            });
        let way = match (earlier, offer) {
            // They went to connect it: now that they have, it is used.
            (Some(Way::Connect), Some(_)) => Way::Connection,
            (Some(Way::Connect), None) | (None, _) => {
                match self
                    .run
                    .ask(WorkAskPurposeV1::Connection, prompt, options, None)
                    .await
                    .ok()
                    .flatten()
                {
                    Some(answer) => super::route::way(&answer),
                    None => Way::Website,
                }
            }
            (Some(way), _) => way,
        };
        // A connection the person did not add cannot be used.
        let way = match (way, offer) {
            (Way::Connection, None) => Way::Website,
            (way, _) => way,
        };
        if let (Way::Connection, Some(offer)) = (way, offer) {
            // The connection helper asks under the same words: it takes this answer.
            self.run.accept_connection(&offer.yes);
        }
        Some(way)
    }

    /// The need a part that could not do its job shows on its row, with the
    /// reason its steps show: the one its helper named when it holds, else
    /// what its steps show. A web part for a service the person has a
    /// connection for offers the connection.
    async fn settle_need(&self, part: WorkPartId, spec: &PartSpec, report: &mut PartReport) {
        let projection = self.run.probe.runtime_projection().await.ok();
        let execution = projection.as_ref().and_then(|projection| {
            projection
                .executions
                .iter()
                .find(|execution| execution.id == self.run.probe.execution())
        });
        let steps: Vec<&WorkStepFact> = execution
            .iter()
            .flat_map(|e| e.steps.iter())
            .filter(|s| s.part == Some(part))
            .collect();
        let persistent = projection.as_ref().and_then(|projection| {
            let blocked = crate::work_agent::WorkAgentService::site_blockers(projection);
            let sites = part_site(spec)
                .into_iter()
                .chain(steps.iter().filter_map(|step| match &step.kind {
                    WorkStepKindV1::Read { url, .. } => crate::work_sites::site_of(url),
                    _ => None,
                }));
            sites
                .into_iter()
                .find_map(|site| blocked.iter().find(|blocked| blocked.site == site).cloned())
        });
        if let Some(blocked) = persistent {
            // A help page or optimistic helper report does not resolve an
            // account wall. Preserve the typed need even across follow-ups.
            if report.state != WorkPartStateV1::Stopped {
                report.state = WorkPartStateV1::Failed;
            }
            report.need = Some(blocked.need());
            report.digest.push_str(&format!("\n{}", blocked.notice()));
            return;
        }
        if report.state == WorkPartStateV1::Done {
            report.need = None;
            return;
        }
        let service_host = spec.service.as_ref().and_then(|s| s.host.clone());
        let hosts: Vec<String> = service_host
            .iter()
            .cloned()
            .chain(steps.iter().filter_map(|s| match &s.kind {
                WorkStepKindV1::Read { url, .. } => crate::work_sites::site_of(url),
                _ => None,
            }))
            .collect();
        ground_handoff_report(report, &steps);
        let helper_reason = match &report.need {
            Some(
                WorkPartNeedV1::Retry { reason, .. } | WorkPartNeedV1::UseConnection { reason, .. },
            ) => *reason,
            _ => None,
        };
        let reason = step_reason(&steps, report.objects.is_empty(), helper_reason);
        let named = report.need.take().and_then(|need| {
            let need = match need {
                WorkPartNeedV1::SignIn { ref host }
                    if handoff_resolved(&steps, host, WorkPageHandoffKindV1::SignIn) =>
                {
                    return None
                }
                WorkPartNeedV1::Check { ref host }
                    if handoff_resolved(&steps, host, WorkPageHandoffKindV1::Check) =>
                {
                    return None
                }
                WorkPartNeedV1::ChooseFolder
                    if spec.helper == WorkHelperV1::Computer && self.run.files().is_none() =>
                {
                    WorkPartNeedV1::ChooseFolder
                }
                WorkPartNeedV1::ChooseFolder => return None,
                WorkPartNeedV1::AllowFolder { path } => {
                    let home = std::env::var_os("HOME").map(std::path::PathBuf::from)?;
                    let folder = super::folders::folder_for(std::path::Path::new(&path), &home)?;
                    let folder = folder.to_string_lossy().into_owned();
                    let (admitted, _) =
                        crate::work_files::WorkFileGrant::admit(std::slice::from_ref(&folder));
                    (!admitted.is_empty())
                        .then_some(WorkPartNeedV1::AllowFolder { path: folder })?
                }
                WorkPartNeedV1::Retry { host, .. } => WorkPartNeedV1::Retry {
                    host: host.or_else(|| hosts.first().cloned()),
                    reason,
                },
                WorkPartNeedV1::UseConnection { connection, .. } => {
                    if connected(self.run, &connection) {
                        WorkPartNeedV1::UseConnection { connection, reason }
                    } else if let Some(name) = super::route::brand(None, &connection)
                        .and_then(|brand| super::route::known_server(&brand))
                    {
                        WorkPartNeedV1::Connect {
                            connection: name.into(),
                        }
                    } else {
                        WorkPartNeedV1::Retry {
                            host: hosts.first().cloned().filter(|h| public_host(h)),
                            reason,
                        }
                    }
                }
                other => other,
            };
            need.validate().is_ok().then_some(need)
        });
        report.need = named.or_else(|| {
            let declined_entry = steps.iter().any(|s| match &s.kind {
                WorkStepKindV1::Ask {
                    purpose: Some(WorkAskPurposeV1::Entry),
                    answer: Some(answer),
                    ..
                } => hosts.first().is_some_and(|site| {
                    crate::work_sites::entry_answer(site, answer)
                        == crate::work_sites::EntryAnswer::NotNow
                }),
                _ => false,
            });
            if declined_entry {
                return hosts
                    .first()
                    .map(|host| WorkPartNeedV1::AllowSite { host: host.clone() });
            }
            let reads: Vec<&&WorkStepFact> = steps
                .iter()
                .filter(|s| matches!(s.kind, WorkStepKindV1::Read { .. }))
                .collect();
            let pages_failed = !reads.is_empty()
                && reads
                    .iter()
                    .all(|s| s.status != WorkStepStatus::Succeeded || s.artifacts.is_empty());
            if (pages_failed || reason.is_some()) && report.objects.is_empty() {
                return Some(WorkPartNeedV1::Retry {
                    host: hosts.first().cloned().filter(|h| public_host(h)),
                    reason,
                });
            }
            None
        });
        if let (Some(offer), Some(need)) = (&spec.declined, &report.need) {
            if !matches!(
                need,
                WorkPartNeedV1::AllowFolder { .. } | WorkPartNeedV1::ChooseFolder
            ) {
                let reason = match need {
                    WorkPartNeedV1::SignIn { .. } => Some(WorkPartReasonV1::SignedOut),
                    WorkPartNeedV1::Check { .. } => Some(WorkPartReasonV1::BlockedByCheck),
                    WorkPartNeedV1::Retry { reason, .. } => *reason,
                    _ => reason,
                };
                report.need = Some(WorkPartNeedV1::UseConnection {
                    connection: offer.connection.clone(),
                    reason,
                })
                .filter(|need| need.validate().is_ok())
                .or(report.need.take());
            }
        }
        // A request that asked for the service's connection, which this
        // person has not added, is told to connect it. A website that failed
        // is a website to try again, never a connection to add.
        if let Some(name) = super::route::brand(spec.service.as_ref(), &spec.title)
            .filter(|_| !public_goal(spec))
            .filter(|brand| !connected(self.run, brand))
            .and_then(|brand| super::route::known_server(&brand))
        {
            let asked = self
                .objective
                .to_lowercase()
                .split(|c: char| !c.is_alphanumeric())
                .any(|word| matches!(word, "mcp" | "connection" | "connector"));
            if asked
                && matches!(
                    report.need,
                    None | Some(
                        WorkPartNeedV1::Retry { .. } | WorkPartNeedV1::UseConnection { .. }
                    )
                )
            {
                report.need = Some(WorkPartNeedV1::Connect {
                    connection: name.into(),
                });
            }
        }
        if report.state == WorkPartStateV1::Stopped
            && !matches!(report.need, Some(WorkPartNeedV1::SignIn { .. }))
        {
            report.need = None;
        }
    }

    /// A share of what the run has left, so parts that start together can
    /// all finish and the lead keeps room to build the result.
    fn part_share(&self, model: &LeadModel) -> WorkExecutionLimits {
        let left = self.run.remaining();
        let (ceiling, floor) = part_money(&model.entry, &self.models.page.entry);
        // Parts past the run's parallel slots wait: they do not share now.
        let ways = (self.parts_running().min(PARALLEL_PARTS) as u32 + 1).max(2);
        WorkExecutionLimits {
            // A search reserves its whole context: a part holds room for one.
            model_tokens: (left.model_tokens / ways)
                .max(PART_MIN_TOKENS)
                .clamp(1, PART_MAX.model_tokens)
                .min(left.model_tokens),
            cost_micro_usd: (left.cost_micro_usd / ways)
                .max(floor)
                .clamp(1, ceiling)
                .min(left.cost_micro_usd),
            operations: (left.operations / ways).clamp(1, PART_MAX.operations),
            timeout_seconds: left.timeout_seconds,
            max_workers: PART_MAX.max_workers.min(left.max_workers).max(1),
        }
    }
}

/// A part's money ceiling and floor in its dearest model's prices: its own
/// or the page model its pages run on. The fixed sums fit the cheap models; a
/// pricier one gets the same tokens and calls, so it is not stopped early.
fn part_money(own: &WorkModelEntry, page: &WorkModelEntry) -> (u32, u32) {
    let dearest = |pick: fn(&WorkModelPrice) -> u64| {
        [own, page]
            .iter()
            .filter_map(|entry| entry.price.as_ref().map(pick))
            .max()
            .unwrap_or(0)
    };
    let cost = u64::from(PART_MAX.model_tokens) * dearest(|price| price.input)
        + PART_REPLY_TOKENS * dearest(|price| price.output);
    let ceiling = u32::try_from(cost / 1_000_000)
        .unwrap_or(u32::MAX)
        .max(PART_MAX.cost_micro_usd);
    let floor = u64::from(PART_MIN_COST) * u64::from(ceiling) / u64::from(PART_MAX.cost_micro_usd);
    (ceiling, u32::try_from(floor).unwrap_or(u32::MAX))
}

fn add(a: WorkUsage, b: WorkUsage) -> WorkUsage {
    WorkUsage {
        model_tokens: a.model_tokens.saturating_add(b.model_tokens),
        cost_micro_usd: a.cost_micro_usd.saturating_add(b.cost_micro_usd),
        operations: a.operations.saturating_add(b.operations),
        accounting: if b.accounting == WorkUsageAccounting::ConservativeReservation {
            b.accounting
        } else {
            a.accounting
        },
    }
}

/// A helper's page, search or file call as the step it becomes.
/// Whether the person has the connection a need names: gh, or an enabled
/// server of theirs by its name or id.
fn connected(run: &super::run::LeadRun, connection: &str) -> bool {
    let Some(brand) = super::route::brand(None, connection) else {
        return false;
    };
    let servers = crate::work_connections::store::shared()
        .and_then(|store| store.servers(&run.profile.to_string()).ok())
        .unwrap_or_default();
    super::route::offer(
        &brand,
        crate::work_connections::helper::shared().gh_ready(),
        &servers,
    )
    .is_some()
}

/// A page task that only reads one of the person's daily apps (what is new
/// in Slack, the Linear inbox): it is read from the app's own views first,
/// with no model call, whatever the helper set. A goal that asks to send,
/// post or change something is a task, not a read; a sentence that forbids
/// it ("do not change anything") asks nothing.
fn daily_app(kind: &WorkStepKindV1) -> bool {
    let WorkStepKindV1::Read { url, .. } = kind else {
        return false;
    };
    // Loopback replicas of the apps (.test sites) stand in for them in
    // qualification.
    url::Url::parse(url)
        .ok()
        .is_some_and(|url| match url.host() {
            // GitHub is an app only on the person's own views; its search,
            // repositories and code are public pages with their own planner.
            Some(url::Host::Domain(host)) => {
                (zephium_agentic::DailyApp::of(host).is_some()
                    && (host.trim_start_matches("www.") != "github.com"
                        || crate::work_sites::personal_page(url.as_str())))
                    || host.ends_with(".test")
            }
            Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
            _ => false,
        })
}

fn reads_app(kind: &WorkStepKindV1) -> bool {
    const ACTS: [&str; 22] = [
        "send", "post", "reply", "write", "create", "draft", "comment", "assign", "archive",
        "delete", "remove", "invite", "schedule", "book", "update", "move", "mark", "react",
        "forward", "edit", "submit", "fill",
    ];
    const NOT: [&str; 5] = ["do not", "don't", "never", "without", "no "];
    let WorkStepKindV1::Read {
        goal: Some(goal), ..
    } = kind
    else {
        return false;
    };
    if !daily_app(kind) {
        return false;
    }
    let goal = goal.to_lowercase();
    // GitHub is also the public web: a question about its public projects
    // (the most-starred frameworks, a repository's releases) is browsed by the
    // page planner. Only the person's own GitHub is read as a view.
    const OWN: [&str; 10] = [
        "my ",
        "mine",
        "notification",
        "inbox",
        "assigned",
        "review request",
        "mention",
        "user's",
        "person's",
        "unread",
    ];
    if matches!(kind, WorkStepKindV1::Read { url, .. }
        if url::Url::parse(url).ok().and_then(|url| url.host_str().and_then(zephium_agentic::DailyApp::of))
            == Some(zephium_agentic::DailyApp::GitHub))
        && !OWN.iter().any(|own| goal.contains(own))
    {
        return false;
    }
    !goal
        .split(['.', ';', '\n', '!'])
        .filter(|sentence| !NOT.iter().any(|not| sentence.contains(not)))
        .flat_map(|sentence| sentence.split(|c: char| !c.is_alphanumeric()))
        .any(|word| ACTS.contains(&word))
}

pub(crate) fn step_request(
    name: &str,
    args: &Value,
    run: &super::run::LeadRun,
) -> Result<WorkStepKindV1, String> {
    let text = |key: &str| text_arg(args, key);
    let kind = match name {
        "web_search" => {
            let query = text("query").ok_or("query is required")?;
            if query.contains(';') || query.contains(" | ") {
                return Err("query is one question: search each question in its own call".into());
            }
            WorkStepKindV1::Search {
                query: operator_last(&query),
            }
        }
        "read" => {
            let url = text("url").ok_or("url is required")?;
            // Work never loads plain http: the page is read, and cited, at
            // its https address, which is where its server would send it.
            let url = match url.strip_prefix("http://") {
                Some(rest) => format!("https://{rest}"),
                None => url,
            };
            WorkStepKindV1::Read {
                verbatim: None,
                url,
                collection: hands::collection(args.get("records"))?,
                goal: None,
            }
        }
        "browse" => {
            let start = text("start").ok_or("start is required")?;
            // A goal is one line of plain text: a helper writing it over
            // several lines must not cost the page its start.
            let goal = text("goal")
                .map(|goal| goal.split_whitespace().collect::<Vec<_>>().join(" "))
                .filter(|goal| !goal.is_empty())
                .ok_or("goal is required")?;
            if goal.len() > MAX_WORK_PAGE_GOAL_BYTES {
                return Err(
                    "goal is at most 600 bytes: give longer exact words as text or draft".into(),
                );
            }
            let verbatim = match args.get("text").and_then(Value::as_str) {
                Some(words) => {
                    let words = words.replace("\r\n", "\n").replace('\r', "\n");
                    let words = words.trim_end_matches('\n');
                    if words.trim().is_empty() {
                        None
                    } else if words.len() > MAX_WORK_PAGE_VERBATIM_BYTES {
                        return Err(format!(
                            "text is at most {MAX_WORK_PAGE_VERBATIM_BYTES} bytes"
                        ));
                    } else {
                        Some(words.to_owned())
                    }
                }
                None => None,
            };
            let url = if start.starts_with("https://") {
                start
            } else {
                let site = start
                    .trim_start_matches("http://")
                    .trim_start_matches("www.")
                    .trim_end_matches('/')
                    .to_ascii_lowercase();
                if !public_host(&site) {
                    return Err("start is an https page or a bare site such as airbnb.com".into());
                }
                format!(
                    "https://{}/",
                    super::route::app_host(&site).unwrap_or(&site)
                )
            };
            WorkStepKindV1::Read {
                verbatim,
                url,
                collection: hands::collection(args.get("records"))?,
                goal: Some(goal),
            }
        }
        "list" => WorkStepKindV1::List {
            path: text("path").ok_or("path is required")?,
            depth: args
                .get("depth")
                .and_then(Value::as_u64)
                .map(|d| d.clamp(1, 3) as u8),
        },
        "read_file" => WorkStepKindV1::ReadFile {
            path: text("path").ok_or("path is required")?,
            offset: args
                .get("offset")
                .and_then(Value::as_u64)
                .map(|v| v.max(1) as u32),
            limit: args
                .get("limit")
                .and_then(Value::as_u64)
                .map(|v| v.clamp(1, 2000) as u32),
        },
        "search_files" => WorkStepKindV1::SearchFiles {
            path: text("path").ok_or("path is required")?,
            query: text("query").ok_or("query is required")?,
            glob: text("glob"),
            regex: args.get("regex").and_then(Value::as_bool),
        },
        "write_file" => WorkStepKindV1::WriteFile {
            path: text("path").ok_or("path is required")?,
            content: args
                .get("content")
                .and_then(Value::as_str)
                .ok_or("content is required")?
                .to_owned(),
            decision: None,
        },
        "edit_file" => WorkStepKindV1::EditFile {
            path: text("path").ok_or("path is required")?,
            old: args
                .get("old")
                .and_then(Value::as_str)
                .ok_or("old is required")?
                .to_owned(),
            new: args
                .get("new")
                .and_then(Value::as_str)
                .ok_or("new is required")?
                .to_owned(),
            replacements: vec![],
            decision: None,
        },
        "run_command" => WorkStepKindV1::RunCommand {
            cwd: text("cwd").ok_or("cwd is required")?,
            command: text("command").ok_or("command is required")?,
            timeout_secs: args
                .get("timeout_secs")
                .and_then(Value::as_u64)
                .map(|v| v.clamp(1, 600) as u32),
            decision: None,
        },
        _ => return Err(format!("{name} is not a tool here")),
    };
    admit_kind(name, kind, run)
}

/// A search that opens with a `site:` operator reads as an address with a
/// `site` scheme, which a search never sends: the operator goes last, where
/// the engine reads it the same.
fn operator_last(query: &str) -> String {
    let mut words = query.split_whitespace();
    match words.next() {
        Some(first) if first.to_ascii_lowercase().starts_with("site:") => {
            let rest: Vec<&str> = words.collect();
            if rest.is_empty() {
                query.to_owned()
            } else {
                format!("{} {first}", rest.join(" "))
            }
        }
        _ => query.to_owned(),
    }
}

/// A new read of the address a page's first load was redirected to,
/// admitted by the same checks as a read the lead asked for itself.
pub(crate) fn redirected_read(
    kind: &WorkStepKindV1,
    destination: &str,
    run: &super::run::LeadRun,
) -> Result<WorkStepKindV1, String> {
    let WorkStepKindV1::Read {
        collection,
        goal,
        verbatim,
        ..
    } = kind
    else {
        return Err("only a page read follows a redirect".into());
    };
    admit_kind(
        "read",
        WorkStepKindV1::Read {
            verbatim: verbatim.clone(),
            url: destination.to_owned(),
            collection: collection.clone(),
            goal: goal.clone(),
        },
        run,
    )
}

fn admit_kind(
    name: &str,
    mut kind: WorkStepKindV1,
    run: &super::run::LeadRun,
) -> Result<WorkStepKindV1, String> {
    if let WorkStepKindV1::Read {
        url,
        collection,
        goal: None,
        ..
    } = &mut kind
    {
        if collection.is_none() {
            let records = run.records.lock().unwrap_or_else(|e| e.into_inner());
            *collection = records
                .collection_for_document(url)
                .or_else(|| records.collection_for_site(url));
        }
    }
    if let WorkStepKindV1::Search { query } = &kind {
        zephium_core::work::search::validate_public_search_query(query)
            .map_err(|_| "query is one line of at most 512 characters".to_owned())?;
    }
    if let WorkStepKindV1::Read {
        url, goal: None, ..
    } = &kind
    {
        if !run.known_url(url) && !plain_page(url) {
            return Err("read takes a url you were given (a source, a link a page showed, one in the request) or a site's own page address with no query, such as https://www.hetzner.com/cloud; search or browse the site to find others".into());
        }
    }
    let probe = WorkStepFact {
        model_fault: None,
        id: WorkStepId::from(1),
        turn: 1,
        kind: kind.clone(),
        status: WorkStepStatus::Running,
        usage: None,
        artifacts: vec![],
        evidence: None,
        note: None,
        measurements: None,
        local: None,
        account: None,
        part: None,
    };
    probe
        .validate()
        .map_err(|_| format!("{name}: the arguments are outside their limits"))?;
    Ok(kind)
}

/// A site's own page by its plain address: https, a public host, no query,
/// fragment or credentials, and a short path of plain words, such as a
/// pricing or docs page. Such an address carries nothing but its name.
fn plain_page(url: &str) -> bool {
    let Ok(parsed) = url::Url::parse(url) else {
        return false;
    };
    let segments: Vec<&str> = parsed.path().split('/').filter(|s| !s.is_empty()).collect();
    parsed.scheme() == "https"
        && parsed.query().is_none()
        && parsed.fragment().is_none()
        && parsed.username().is_empty()
        && parsed.password().is_none()
        && parsed.port().is_none()
        && parsed.host_str().is_some_and(public_host)
        && segments.len() <= 4
        && segments.iter().all(|s| {
            s.len() <= 40
                && s.chars().all(|c| {
                    c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_' | '.')
                })
        })
}

/// Pages the person gave in their request (or the lead put in the part's
/// goal) on the part's own site: a browser part reads them as given first.
/// Pages in the person's own apps go through the part's page tasks instead.
fn given_pages(objective: &str, spec: &PartSpec) -> Vec<String> {
    if account_goal(spec) {
        return Vec::new();
    }
    let site = spec
        .service
        .as_ref()
        .and_then(|service| service.host.as_deref())
        .and_then(|host| crate::work_sites::site_of(&format!("https://{host}/")));
    let mut pages: Vec<String> = Vec::new();
    for text in [objective, spec.goal.as_str(), spec.brief.as_str()] {
        for word in
            text.split(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '"' | '(' | ')'))
        {
            let word = word.trim_end_matches(['.', ',', ';', ':', '!', '?', '\'']);
            let Ok(url) = url::Url::parse(word) else {
                continue;
            };
            // A root URL plus an explicit search is an entry point, not a
            // source of matching records. Reading it first feeds unrelated
            // recommendations to the helper and suppresses the results route.
            if spec.search.is_some()
                && url.path() == "/"
                && url.query().is_none()
                && url.fragment().is_none()
            {
                continue;
            }
            let url = url.to_string();
            if !url.starts_with("https://")
                || url.len() > 1024
                || crate::work_sites::personal_page(&url)
                || site
                    .as_ref()
                    .is_some_and(|site| crate::work_sites::site_of(&url).as_ref() != Some(site))
                || pages.contains(&url)
            {
                continue;
            }
            pages.push(url);
        }
    }
    pages.truncate(2);
    pages
}

/// The fields a part returns for each thing, as the rows a page read
/// collects: a link, up to three pictures, the rest as text.
fn records_collection(
    title: &str,
    records: &[String],
) -> Option<zephium_core::work::collection::WorkBrowseCollection> {
    let mut columns: Vec<Value> = Vec::new();
    let mut names: Vec<String> = Vec::new();
    let (mut link, mut pictures) = (false, 0);
    for field in records {
        let name: String = field
            .trim()
            .to_ascii_lowercase()
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect();
        let name = name.trim_matches('_').to_owned();
        if name.is_empty() || matches!(name.as_str(), "name" | "title") || names.contains(&name) {
            continue;
        }
        let kind = if (name.contains("url") || name.contains("link")) && !link {
            link = true;
            "url"
        } else if ["photo", "image", "picture", "img"]
            .iter()
            .any(|w| name.contains(w))
        {
            if pictures == 3 {
                continue;
            }
            pictures += 1;
            "image_url"
        } else {
            "text"
        };
        names.push(name.clone());
        columns.push(serde_json::json!({"name": name, "value": {"kind": kind}, "required": kind == "url", "extraction": "generate"}));
    }
    if columns.is_empty() {
        return None;
    }
    let max_items = (256 / (columns.len() + 1)).min(12);
    let value =
        serde_json::json!({"title": clip(title, 60), "max_items": max_items, "columns": columns});
    hands::collection(Some(&value)).ok().flatten()
}

/// A helper's `need` argument: `{kind, target}`.
fn need_arg(value: Option<&Value>) -> Option<WorkPartNeedV1> {
    let value = value?;
    let target = value
        .get("target")
        .and_then(Value::as_str)
        .map(|t| t.trim().to_owned())
        .filter(|t| !t.is_empty());
    let host = |t: Option<String>| {
        t.map(|t| {
            t.trim_start_matches("https://")
                .trim_start_matches("http://")
                .trim_start_matches("www.")
                .split('/')
                .next()
                .unwrap_or("")
                .to_ascii_lowercase()
        })
    };
    let reason = match value.get("reason").and_then(Value::as_str) {
        Some("couldnt_read") => Some(WorkPartReasonV1::CouldntRead),
        Some("blocked_by_check") => Some(WorkPartReasonV1::BlockedByCheck),
        Some("not_found") => Some(WorkPartReasonV1::NotFound),
        Some("site_error") => Some(WorkPartReasonV1::SiteError),
        Some("no_answer") => Some(WorkPartReasonV1::NoAnswer),
        _ => None,
    };
    Some(match value.get("kind").and_then(Value::as_str)? {
        "sign_in" => WorkPartNeedV1::SignIn {
            host: host(target)?,
        },
        "check" => WorkPartNeedV1::Check {
            host: host(target)?,
        },
        "allow_site" => WorkPartNeedV1::AllowSite {
            host: host(target)?,
        },
        "choose_folder" => WorkPartNeedV1::ChooseFolder,
        "allow_folder" => WorkPartNeedV1::AllowFolder { path: target? },
        "use_connection" => WorkPartNeedV1::UseConnection {
            connection: target?,
            reason,
        },
        "retry" => WorkPartNeedV1::Retry {
            host: host(target).filter(|h| public_host(h)),
            reason,
        },
        _ => return None,
    })
}

/// The need in the lead's words.
fn need_words(need: &WorkPartNeedV1) -> String {
    match need {
        WorkPartNeedV1::SignIn { host } => format!("the person to sign in on {host}"),
        WorkPartNeedV1::Check { host } => format!("the person to pass the human check on {host}"),
        WorkPartNeedV1::AllowSite { host } => format!("the person to allow work on {host}"),
        WorkPartNeedV1::ChooseFolder => "the person to choose a folder".into(),
        WorkPartNeedV1::AllowFolder { path } => format!("the person to allow reading {path}"),
        WorkPartNeedV1::UseConnection { connection, .. } => {
            format!("the person to choose their {connection} connection instead of its website")
        }
        WorkPartNeedV1::Connect { connection } => format!(
            "the person to connect {connection} in Settings: there is no {connection} connection yet"
        ),
        WorkPartNeedV1::Retry { host, reason } => {
            let site = host.as_deref().unwrap_or("the site");
            match reason {
                Some(WorkPartReasonV1::CouldntRead) => {
                    format!("another try: {site} loaded but could not be read")
                }
                Some(WorkPartReasonV1::SignedOut) => {
                    format!("another try: {site} showed its signed-out view")
                }
                Some(WorkPartReasonV1::BlockedByCheck) => {
                    format!("the person to pass the human check on {site} before continuing")
                }
                Some(WorkPartReasonV1::NotFound) => {
                    format!("another try: {site} showed nothing that matched")
                }
                Some(WorkPartReasonV1::SiteError) => {
                    format!("another try: {site} failed on its side")
                }
                Some(WorkPartReasonV1::NoAnswer) => {
                    format!("another try: {site} did not answer in time")
                }
                None if host.is_some() => format!("another try on {site}"),
                None => "another try".into(),
            }
        }
    }
}

fn page_handoff(step: &WorkStepFact) -> Option<WorkPageHandoffKindV1> {
    match step.measurements.and_then(|m| m.outcome) {
        Some(WorkPageOutcomeV1::NeedsSignIn) => Some(WorkPageHandoffKindV1::SignIn),
        Some(WorkPageOutcomeV1::NeedsCheck) => Some(WorkPageHandoffKindV1::Check),
        Some(_) => None,
        None => match step.note.as_deref() {
            Some(SIGN_IN_NOTE) => Some(WorkPageHandoffKindV1::SignIn),
            Some(note) if read_note::is_human_check(note) => Some(WorkPageHandoffKindV1::Check),
            _ => None,
        },
    }
}

fn ground_handoff_report(report: &mut PartReport, steps: &[&WorkStepFact]) {
    let (host, kind) = match &report.need {
        Some(WorkPartNeedV1::SignIn { host }) => (host, WorkPageHandoffKindV1::SignIn),
        Some(WorkPartNeedV1::Check { host }) => (host, WorkPageHandoffKindV1::Check),
        _ => return,
    };
    if handoff_resolved(steps, host, kind) {
        report.need = None;
        return;
    }
    let site = crate::work_sites::site_of(&format!("https://{host}/"));
    let matching: Vec<_> = steps
        .iter()
        .copied()
        .filter(|step| {
            matches!(&step.kind, WorkStepKindV1::Read { url, .. }
            if site.is_some() && crate::work_sites::site_of(url) == site)
        })
        .collect();
    if matching.iter().any(|step| page_handoff(step) == Some(kind)) {
        return;
    }
    // A helper's explanation cannot turn an interaction or engine failure
    // into a sign-in/check wall. Native waits were preserved above this seam.
    let manual = matching
        .iter()
        .filter(|step| {
            matches!(
                step.status,
                WorkStepStatus::Failed | WorkStepStatus::OutcomeUnknown | WorkStepStatus::Cancelled
            )
        })
        .max_by_key(|step| crate::work_agent::read_settlement_order(step))
        .is_some_and(|step| {
            step.measurements.and_then(|m| m.outcome) == Some(WorkPageOutcomeV1::NeedsInteraction)
        });
    report.need = None;
    report.summary = Some(
        if manual {
            "Needs manual interaction"
        } else {
            "Page task unfinished"
        }
        .into(),
    );
    report.digest = if manual {
        "The page task needs manual interaction. Work could not complete the requested interaction automatically."
    } else {
        "Work could not verify completion of this page task. No sign-in or human-check handoff was recorded."
    }.into();
}

fn handoff_resolved(steps: &[&WorkStepFact], host: &str, kind: WorkPageHandoffKindV1) -> bool {
    let site = crate::work_sites::site_of(&format!("https://{host}/"));
    steps.iter().any(|prior| {
        let WorkStepKindV1::Read { url, .. } = &prior.kind else {
            return false;
        };
        let matching_wall = page_handoff(prior) == Some(kind);
        site.is_some()
            && crate::work_sites::site_of(url) == site
            && matching_wall
            && steps
                .iter()
                .any(|next| crate::work_agent::read_handoff_resolves(prior, next, kind))
    })
}

/// Why a part's steps show it could not do its job: the last failed page or
/// search in closed words, else what the helper named, else nothing found
/// where its pages read cleanly.
fn step_reason(
    steps: &[&WorkStepFact],
    nothing_placed: bool,
    named: Option<WorkPartReasonV1>,
) -> Option<WorkPartReasonV1> {
    let failed = steps.iter().rev().find_map(|step| {
        if !matches!(
            step.kind,
            WorkStepKindV1::Read { .. } | WorkStepKindV1::Search { .. }
        ) || !matches!(
            step.status,
            WorkStepStatus::Failed | WorkStepStatus::OutcomeUnknown
        ) {
            return None;
        }
        if let WorkStepKindV1::Read { .. } = &step.kind {
            let handoff = page_handoff(step);
            if handoff.is_some_and(|kind| {
                steps
                    .iter()
                    .any(|next| crate::work_agent::read_handoff_resolves(step, next, kind))
            }) {
                return None;
            }
        }
        Some(*step)
    });
    if let Some(reason) = failed.and_then(|step| match step.measurements.and_then(|m| m.outcome) {
        Some(WorkPageOutcomeV1::NeedsSignIn) => Some(WorkPartReasonV1::SignedOut),
        Some(WorkPageOutcomeV1::NeedsCheck) => Some(WorkPartReasonV1::BlockedByCheck),
        Some(WorkPageOutcomeV1::SiteError) => Some(WorkPartReasonV1::SiteError),
        Some(_) => Some(WorkPartReasonV1::CouldntRead),
        None => note_reason(
            step.note.as_deref(),
            matches!(step.kind, WorkStepKindV1::Search { .. }),
        ),
    }) {
        return Some(reason);
    }
    if named.is_some() {
        return named;
    }
    let read_cleanly = failed.is_none()
        && steps.iter().any(|s| {
            matches!(
                s.kind,
                WorkStepKindV1::Read { .. } | WorkStepKindV1::Search { .. }
            )
        });
    (read_cleanly && nothing_placed).then_some(WorkPartReasonV1::NotFound)
}

/// A failed step's closed note as the reason the canvas phrases.
fn note_reason(note: Option<&str>, search: bool) -> Option<WorkPartReasonV1> {
    use zephium_core::work::runtime::read_note;
    use WorkPartReasonV1 as R;
    let note = note.map_or("", |note| crate::work_agent::split_ledger(note).0);
    Some(match note {
        SIGN_IN_NOTE => R::SignedOut,
        read_note::HUMAN_CHECK => R::BlockedByCheck,
        read_note::CONSTRUCTION_TIMEOUT
        | read_note::SLOW_SITE
        | "The page took too long"
        | "The page could not be opened"
        | "The search could not be sent" => R::NoAnswer,
        "The search gave no usable sources" => R::NotFound,
        // This Mac's browser or the run's own search, not the site: no
        // reason to give.
        "The browser was not available"
        | "The browser was not ready for this page"
        | "The browsing profile was not available"
        | "Too many pages were already open"
        | read_note::WORK_STORAGE
        | read_note::STORAGE_UNCONFIRMED => return None,
        _ if read_note::is_human_check(note) => R::BlockedByCheck,
        _ if search => return None,
        _ => R::CouldntRead,
    })
}

/// A goal that acts on the site for the person (a cart, a booking, a form
/// to send) works in their session: what it changes is theirs.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_digest_states_every_commit_the_helper_left_out() {
        let committed = vec![
            "Saved on the page: Add task".to_owned(),
            "Sent on the page: Send".to_owned(),
        ];
        assert_eq!(
            with_committed(
                "Added the task.\nSaved on the page: Add task".into(),
                &committed
            ),
            "Added the task.\nSaved on the page: Add task\nSent on the page: Send"
        );
        assert_eq!(with_committed("Done".into(), &[]), "Done");
    }

    #[test]
    fn a_part_gets_the_same_calls_on_a_pricier_model() {
        let entry = |input: u64, output: u64| WorkModelEntry {
            id: "m".into(),
            model: WorkModelRef {
                provider: WorkModelProvider::Anthropic,
                wire: WorkModelWire::AnthropicMessages,
                model: "m".into(),
            },
            display_name: "m".into(),
            roles: vec![WorkModelRole::Page],
            recommended: false,
            context_window: 200_000,
            max_output: 32_000,
            supports: WorkModelSupports {
                tools: true,
                vision: true,
                prompt_cache: true,
                reasoning: true,
                native_search: false,
            },
            price: Some(WorkModelPrice {
                input,
                cached_input: input / 10,
                output,
            }),
        };
        // Micro-USD per million tokens.
        let luna = entry(100_000, 500_000);
        let opus = entry(5_000_000, 25_000_000);
        assert_eq!(
            part_money(&luna, &luna),
            (PART_MAX.cost_micro_usd, PART_MIN_COST)
        );
        let (ceiling, floor) = part_money(&luna, &opus);
        assert_eq!(ceiling, 4_298_000);
        assert!(floor > PART_MIN_COST && floor < ceiling);
        assert_eq!(part_money(&opus, &luna), (ceiling, floor));
        let unpriced = WorkModelEntry {
            price: None,
            ..luna.clone()
        };
        assert_eq!(part_money(&unpriced, &unpriced).0, PART_MAX.cost_micro_usd);
    }

    #[test]
    fn a_page_the_person_gave_is_read_as_given_by_the_part_for_its_site() {
        let spec = PartSpec {
            title: "Sets".into(),
            helper: WorkHelperV1::Browser,
            goal: "Pick three sets".into(),
            brief: String::new(),
            service: Some(WorkPartServiceV1 {
                host: Some("lego.com".into()),
                connection: None,
            }),
            records: vec![
                "price".into(),
                "pieces".into(),
                "url".into(),
                "photo".into(),
            ],
            search: None,
            declined: None,
            draft: None,
            halt: None,
        };
        assert_eq!(
            given_pages(
                "Open https://www.lego.com/en-us/themes/architecture, pick three sets; see https://example.com/x.",
                &spec
            ),
            ["https://www.lego.com/en-us/themes/architecture"]
        );
        let slack = PartSpec {
            service: None,
            ..spec
        };
        assert!(given_pages("Summarise https://app.slack.com/client/T1/C2", &slack).is_empty());
        let collection = records_collection("Sets", &slack.records).unwrap();
        assert_eq!(collection.columns.len(), 4);
        assert!(collection
            .columns
            .iter()
            .all(|c| c.required == (c.name == "url")));
    }

    #[test]
    fn a_typed_search_skips_only_the_bare_entry_page_and_keeps_specific_sources() {
        let mut part = PartSpec {
            title: "Catalog".into(),
            helper: WorkHelperV1::Browser,
            goal: "Find three matching items".into(),
            brief: String::new(),
            service: Some(WorkPartServiceV1 {
                host: Some("catalog.example".into()),
                connection: None,
            }),
            records: vec!["price".into(), "url".into()],
            search: Some(super::super::recipes::SiteSearch {
                query: Some("red boots".into()),
                ..Default::default()
            }),
            declined: None,
            draft: None,
            halt: None,
        };
        assert!(given_pages("Start at https://catalog.example/", &part).is_empty());
        for url in [
            "https://catalog.example/items",
            "https://catalog.example/?q=red",
            "https://catalog.example/#details",
        ] {
            assert_eq!(given_pages(url, &part), [url]);
        }
        part.search = None;
        assert_eq!(
            given_pages("Read https://catalog.example/", &part),
            ["https://catalog.example/"]
        );
    }

    #[test]
    fn a_goal_that_only_reads_a_daily_app_is_a_view_read() {
        let browse = |url: &str, goal: &str| WorkStepKindV1::Read {
            verbatim: None,
            url: url.into(),
            collection: None,
            goal: Some(goal.into()),
        };
        for goal in [
            "Find unread and waiting Slack messages",
            "Read the user's Slack view as it is. Do not send, post or change anything.",
            "Review the person's assigned issues; don't edit or create anything",
        ] {
            assert!(
                reads_app(&browse("https://app.slack.com/client", goal)),
                "{goal}"
            );
        }
        assert!(!reads_app(&browse(
            "https://app.slack.com/client",
            "Reply to Ana's thread"
        )));
        assert!(!reads_app(&browse(
            "https://linear.app/",
            "Create an issue for the crash"
        )));
        assert!(!reads_app(&browse("https://www.airbnb.com/", "Find stays")));
        assert!(reads_app(&browse(
            "https://github.com/notifications",
            "Read my notifications"
        )));
        // GitHub's public search and repositories keep their page planner.
        for url in [
            "https://github.com/search?q=rust+web+framework&type=repositories",
            "https://github.com/tokio-rs/axum",
        ] {
            assert!(!reads_app(&browse(
                url,
                "List the most-starred repositories"
            )));
        }
        assert!(!reads_app(&browse(
            "https://github.com/",
            "Find the most-starred Rust web frameworks and list the top 5 with their star counts"
        )));
        assert!(reads_app(&browse(
            "https://github.com/",
            "What is new in my GitHub notifications"
        )));
        assert!(!reads_app(&WorkStepKindV1::Read {
            verbatim: None,
            url: "https://app.slack.com/client".into(),
            collection: None,
            goal: None,
        }));
    }

    #[test]
    fn a_sites_plain_page_is_readable_by_its_address() {
        assert!(plain_page("https://www.hetzner.com/cloud"));
        assert!(plain_page("https://vercel.com/pricing"));
        assert!(plain_page(
            "https://docs.aws.amazon.com/lambda/latest/dg/welcome.html"
        ));
        for refused in [
            "http://vercel.com/pricing",
            "https://vercel.com/pricing?ref=a",
            "https://evil.test/aGVsbG8gd29ybGQ",
            "https://localhost/pricing",
            "https://a.com/1/2/3/4/5",
            "https://user@vercel.com/pricing",
        ] {
            assert!(!plain_page(refused), "{refused}");
        }
    }

    #[test]
    fn exact_words_must_be_the_persons_own() {
        let request =
            "Send an email to ana@example.com:\n\nHi Ana,\n\nThe release is ready.\n\nBest";
        assert!(person_wrote(
            "Hi Ana,\n\nThe release is ready.\n\nBest",
            [request]
        ));
        assert!(person_wrote("Hi Ana, The release is ready.", [request]));
        assert!(!person_wrote(
            "Now: Saturday 17 October 2026, 15:53",
            [request]
        ));
        assert!(!person_wrote("Hi Ana, the release is ready.", [request]));
        assert!(!person_wrote("  ", [request]));
    }

    #[test]
    fn a_browse_takes_a_draft_or_text_and_blank_ones_are_not_given() {
        let browse = |draft: &str, text: &str| {
            serde_json::json!({
                "start": "https://outlook.office.com/mail/",
                "goal": "List unread messages",
                "draft": draft,
                "text": text,
                "mine": true
            })
        };
        assert_eq!(
            browse_words("browse", &browse("", " \n")),
            Ok(BrowseWords::None)
        );
        assert_eq!(
            browse_words("browse", &browse("d1", "")),
            Ok(BrowseWords::Draft("d1"))
        );
        assert_eq!(
            browse_words("browse", &browse("  ", "Hi")),
            Ok(BrowseWords::Text("Hi"))
        );
        assert_eq!(
            browse_words("browse", &browse("d1", "Hi")),
            Err("give draft or text, not both".to_owned())
        );
        assert_eq!(
            browse_words("read", &browse("d1", "Hi")),
            Ok(BrowseWords::None)
        );
    }

    #[test]
    fn a_site_is_read_once_and_retried_once_another_way() {
        let part =
            |title: &str, ended: Option<(WorkPartStateV1, Option<WorkPartNeedV1>, usize)>| {
                SitePart {
                    site: "x-kom.pl".into(),
                    title: title.into(),
                    ended: ended.map(|(state, need, pages)| SiteEnd {
                        state,
                        need,
                        summary: Some("3 offers".into()),
                        pages,
                    }),
                }
            };
        let retry = |reason| {
            Some(WorkPartNeedV1::Retry {
                host: Some("x-kom.pl".into()),
                reason,
            })
        };
        assert_eq!(site_turn(&[], "x-kom.pl"), SiteTurn::Go);
        let refused =
            |sites: &[SitePart]| matches!(site_turn(sites, "x-kom.pl"), SiteTurn::Refuse(_));
        assert!(refused(&[part("x-kom", None)]));
        assert!(refused(&[part(
            "x-kom",
            Some((WorkPartStateV1::Done, None, 1))
        )]));
        assert!(matches!(
            site_turn(
                &[part(
                    "x-kom",
                    Some((
                        WorkPartStateV1::Failed,
                        retry(Some(WorkPartReasonV1::CouldntRead)),
                        1
                    ))
                )],
                "x-kom.pl"
            ),
            SiteTurn::Retry(_)
        ));
        // Tried twice already inside the part, or needs the person: not again.
        assert!(refused(&[part(
            "x-kom",
            Some((
                WorkPartStateV1::Failed,
                retry(Some(WorkPartReasonV1::CouldntRead)),
                2
            ))
        )]));
        assert!(refused(&[part(
            "x-kom",
            Some((
                WorkPartStateV1::Failed,
                retry(Some(WorkPartReasonV1::BlockedByCheck)),
                1
            ))
        )]));
        assert!(refused(&[part(
            "x-kom",
            Some((
                WorkPartStateV1::Failed,
                Some(WorkPartNeedV1::SignIn {
                    host: "x-kom.pl".into()
                }),
                1
            ))
        )]));
        // A stopped part never ran its course: the site is free again.
        assert_eq!(
            site_turn(
                &[part("x-kom", Some((WorkPartStateV1::Stopped, None, 1)))],
                "x-kom.pl"
            ),
            SiteTurn::Go
        );
        assert_eq!(
            site_turn(&[part("x-kom", None)], "allegro.pl"),
            SiteTurn::Go
        );
    }

    #[test]
    fn unknown_account_services_never_take_the_eager_public_read_path() {
        let mut part = PartSpec {
            title: "Test account".into(),
            helper: WorkHelperV1::Browser,
            goal: "Read my account inbox".into(),
            brief: String::new(),
            service: Some(WorkPartServiceV1 {
                host: Some("unlisted.example".into()),
                connection: None,
            }),
            records: vec![],
            search: None,
            declined: None,
            draft: None,
            halt: None,
        };
        let url = "https://unlisted.example/app/inbox";
        assert!(given_pages(url, &part).is_empty());
        let read = WorkStepKindV1::Read {
            verbatim: None,
            url: url.into(),
            collection: None,
            goal: None,
        };
        assert!(account_read_route(&part, read.clone()).is_err());
        let task = WorkStepKindV1::Read {
            verbatim: None,
            url: url.into(),
            collection: None,
            goal: Some("Read visible inbox items without changing anything".into()),
        };
        assert_eq!(account_read_route(&part, task.clone()).unwrap(), task);
        part.title = "Public documentation".into();
        part.goal = "Read the sign-in and MFA documentation".into();
        assert_eq!(given_pages(url, &part), [url]);
        assert_eq!(account_read_route(&part, read.clone()).unwrap(), read);
    }

    #[test]
    fn a_services_pricing_is_its_public_site_not_the_persons_app() {
        let part = |goal: &str| PartSpec {
            title: "Linear".into(),
            helper: WorkHelperV1::Browser,
            goal: goal.into(),
            brief: String::new(),
            service: Some(WorkPartServiceV1 {
                host: Some("linear.app".into()),
                connection: None,
            }),
            records: vec![],
            search: None,
            declined: None,
            draft: None,
            halt: None,
        };
        assert!(public_goal(&part(
            "Find Linear's pricing for a 10-person team"
        )));
        assert!(!public_goal(&part("What is assigned to me in Linear")));
        assert!(!public_goal(&part("Read my Linear inbox")));
        assert!(public_source_retry(&part("Read Linear's documentation")));
        assert!(!public_source_retry(&part(
            "What is assigned to me in Linear"
        )));
        assert!(!public_source_retry(&part("Read my Linear inbox")));
    }

    #[test]
    fn a_part_sent_without_a_goal_takes_its_search() {
        let spec = spec(&serde_json::json!({
            "title": "Allegro", "helper": "browser", "service": "allegro.pl",
            "search": {"query": "LEGO 42115"}
        }))
        .unwrap();
        assert_eq!(spec.goal, "Find LEGO 42115 on allegro.pl");
        assert!(super::spec(&serde_json::json!({"title": "Notes", "helper": "research"})).is_err());
        let stay = super::spec(&serde_json::json!({
            "title": "Lisbon hotels", "helper": "browser", "service": "booking.com",
            "search": {"place": "Lisbon", "checkin": "2026-12-04", "checkout": "2026-12-07"}
        }))
        .unwrap();
        assert_eq!(stay.goal, "Find stays in Lisbon on booking.com");
        let briefed = super::spec(&serde_json::json!({
            "title": "Entry", "helper": "research",
            "brief": "Visa rules for Polish citizens in the US. Passport validity too."
        }))
        .unwrap();
        assert_eq!(briefed.goal, "Visa rules for Polish citizens in the US");
        let long = format!(
            "Find Lisbon hotels for 4 to 7 December. {}",
            "Rated 8 or more. ".repeat(12)
        );
        let said = super::spec(&serde_json::json!({
            "title": "Hotels", "helper": "browser", "goal": long, "brief": "Two adults."
        }))
        .unwrap();
        assert_eq!(said.goal, "Find Lisbon hotels for 4 to 7 December");
        assert!(
            said.brief.starts_with("Find Lisbon hotels") && said.brief.ends_with("Two adults.")
        );
    }

    #[test]
    fn a_site_operator_goes_last_so_the_search_is_sent() {
        assert_eq!(
            operator_last("site:allegro.pl/oferta LEGO 42115 nowy"),
            "LEGO 42115 nowy site:allegro.pl/oferta"
        );
        assert_eq!(
            operator_last("LEGO 42115 site:x-kom.pl"),
            "LEGO 42115 site:x-kom.pl"
        );
        assert!(zephium_core::search::SearchEngine::DuckDuckGo
            .search("site:x-kom.pl LEGO 42115")
            .is_none());
        assert!(zephium_core::search::SearchEngine::DuckDuckGo
            .search(&operator_last("site:x-kom.pl LEGO 42115"))
            .is_some());
    }

    #[test]
    fn helper_wall_claims_require_matching_native_outcomes() {
        let read = |url: &str, outcome| WorkStepFact {
            id: WorkStepId::generate(),
            turn: 1,
            kind: WorkStepKindV1::Read {
                verbatim: None,
                url: url.into(),
                goal: None,
                collection: None,
            },
            status: WorkStepStatus::Failed,
            usage: Some(WorkUsage::default()),
            artifacts: vec![],
            evidence: None,
            note: Some(SIGN_IN_NOTE.into()),
            measurements: Some(WorkStepMeasurementsV1 {
                outcome: Some(outcome),
                ..Default::default()
            }),
            local: None,
            account: None,
            part: None,
            model_fault: None,
        };
        for (kind, native) in [
            (WorkPageHandoffKindV1::Check, WorkPageOutcomeV1::NeedsCheck),
            (
                WorkPageHandoffKindV1::SignIn,
                WorkPageOutcomeV1::NeedsSignIn,
            ),
        ] {
            let report = || {
                let mut report =
                    PartReport::ended(WorkPartStateV1::Failed, "A helper claimed a wall", vec![]);
                report.need = Some(if kind == WorkPageHandoffKindV1::Check {
                    WorkPartNeedV1::Check {
                        host: "example.com".into(),
                    }
                } else {
                    WorkPartNeedV1::SignIn {
                        host: "example.com".into(),
                    }
                });
                report
            };
            let mut valid = report();
            ground_handoff_report(&mut valid, &[&read("https://example.com/", native)]);
            assert!(valid.need.is_some());
            assert_eq!(valid.digest, "A helper claimed a wall");
            let mut unrelated = report();
            ground_handoff_report(&mut unrelated, &[&read("https://other.test/", native)]);
            assert!(unrelated.need.is_none());
            for outcome in [
                WorkPageOutcomeV1::NeedsInteraction,
                WorkPageOutcomeV1::TooManySteps,
                WorkPageOutcomeV1::Refused {
                    stage: WorkPageFailureStageV1::Action,
                },
                WorkPageOutcomeV1::Completed,
            ] {
                let mut unsupported = report();
                ground_handoff_report(&mut unsupported, &[&read("https://example.com/", outcome)]);
                assert!(unsupported.need.is_none());
                assert_ne!(unsupported.digest, "A helper claimed a wall");
                if outcome == WorkPageOutcomeV1::NeedsInteraction {
                    assert_eq!(
                        unsupported.summary.as_deref(),
                        Some("Needs manual interaction")
                    );
                    assert_eq!(unsupported.digest, "The page task needs manual interaction. Work could not complete the requested interaction automatically.");
                }
            }
            let mut legacy = read("https://example.com/", native);
            legacy.measurements = None;
            if kind == WorkPageHandoffKindV1::SignIn {
                let mut known = report();
                ground_handoff_report(&mut known, &[&legacy]);
                assert!(known.need.is_some());
            }
        }
    }

    #[test]
    fn named_handoff_need_requires_same_original_read_and_account_receipt() {
        let read = |url: &str, outcome| WorkStepFact {
            id: WorkStepId::generate(),
            turn: 1,
            kind: WorkStepKindV1::Read {
                verbatim: None,
                url: url.into(),
                goal: None,
                collection: None,
            },
            status: WorkStepStatus::Failed,
            usage: Some(WorkUsage::default()),
            artifacts: vec![],
            evidence: None,
            note: None,
            measurements: Some(WorkStepMeasurementsV1 {
                outcome: Some(outcome),
                ..Default::default()
            }),
            local: None,
            account: None,
            part: None,
            model_fault: None,
        };
        let mut wall = read(
            "https://mail.example.com/inbox",
            WorkPageOutcomeV1::NeedsSignIn,
        );
        let mut receipt = read(
            "https://docs.example.com/",
            WorkPageOutcomeV1::Refused {
                stage: WorkPageFailureStageV1::Extraction,
            },
        );
        wall.measurements.as_mut().unwrap().settled_revision = WorkRevision::new(10);
        receipt.measurements.as_mut().unwrap().settled_revision = WorkRevision::new(20);
        receipt.measurements.as_mut().unwrap().resolved_handoff =
            Some(WorkPageHandoffKindV1::SignInAndCheck);
        assert!(!handoff_resolved(
            &[&wall, &receipt],
            "example.com",
            WorkPageHandoffKindV1::SignIn
        ));
        if let WorkStepKindV1::Read { url, .. } = &mut receipt.kind {
            *url = "https://mail.example.com/inbox".into();
        }
        assert!(handoff_resolved(
            &[&wall, &receipt],
            "example.com",
            WorkPageHandoffKindV1::SignIn
        ));
        assert!(
            handoff_resolved(
                &[&receipt, &wall],
                "example.com",
                WorkPageHandoffKindV1::SignIn
            ),
            "Store settlement order wins even when receipt's Read began first"
        );
        wall.account = Some(Box::new(WorkPageAccountV1 {
            host: "mail.example.com".into(),
            badge: true,
        }));
        assert!(!handoff_resolved(
            &[&wall, &receipt],
            "example.com",
            WorkPageHandoffKindV1::SignIn
        ));
        receipt.account = wall.account.clone();
        wall.measurements = None;
        wall.note = Some(SIGN_IN_NOTE.into());
        assert!(
            !handoff_resolved(
                &[&wall, &receipt],
                "example.com",
                WorkPageHandoffKindV1::SignIn
            ),
            "unsequenced legacy outcomes do not prove ordering inside one execution"
        );
    }

    #[test]
    fn a_failed_step_says_why_in_closed_words() {
        use WorkPartReasonV1 as R;
        let reason = |note: &str| note_reason(Some(note), false);
        assert_eq!(
            reason("The page showed nothing usable for the request"),
            Some(R::CouldntRead)
        );
        assert_eq!(reason(SIGN_IN_NOTE), Some(R::SignedOut));
        assert_eq!(
            reason("state.gov asks for a human check; its page was skipped"),
            Some(R::BlockedByCheck)
        );
        assert_eq!(
            reason("Airbnb needs you to continue"),
            Some(R::BlockedByCheck)
        );
        assert_eq!(reason("The page took too long"), Some(R::NoAnswer));
        assert_eq!(reason("The browser was not ready for this page"), None);
        assert_eq!(note_reason(Some("The search could not be run"), true), None);
        assert_eq!(
            note_reason(Some("The search gave no usable sources"), true),
            Some(R::NotFound)
        );
    }
}
