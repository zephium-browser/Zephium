//! Page tasks in the person's session against two loopback sites. Each check
//! reads what the servers saw; no public site, and no page text leaves this
//! file's closed-fact lines.
use std::{
    io::{Read as _, Write as _},
    net::{TcpListener, TcpStream},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex, OnceLock,
    },
    time::Duration,
};
use zephium_app::{
    work_agent::{
        WorkAgentBrowseRequest, WorkAgentProviders, WorkAgentService, WorkBrowserOutcome,
    },
    work_sites::SiteSession,
};
use zephium_core::{
    ids::ProfileId,
    work::{agent::*, runtime::*, search::*, sites::WorkSiteAccessV1, synthesis::*, *},
};
use zephium_ipc::work::*;
use zephium_work_composition::NativeWorkComposition;

use super::work_durable::{browser_settings, WorkflowResult};

#[path = "work_site_human.rs"]
mod human_fixture;

#[path = "work_site_media.rs"]
mod media_fixture;

#[path = "work_site_journey.rs"]
mod journey_fixture;

#[path = "work_site_load.rs"]
mod load_fixture;

/// The person's site as the Work contract sees it, and another site.
const ACCOUNT: &str = "https://account.probe.test";
const OTHER: &str = "https://other.probe.test";
const THIRD: &str = "https://third.probe.test";
const BOARD: &str = "https://board.probe.test";
const SITE: &str = "account.probe.test";
const OTHER_SITE: &str = "other.probe.test";
const THIRD_SITE: &str = "third.probe.test";
const BOARD_SITE: &str = "board.probe.test";
const ACCOUNT_COOKIE: &str = "zaccount=signed-in-a";
/// The consent choice the other site saves, which Work's memory keeps.
const MEMORY_COOKIE: &str = "zmemory=no";
const INBOX_FACT: &str = "Quarterly report from Dana";
const REPORT_FACT: &str = "Reports total is 4,812";
const LANDED_FACT: &str = "Landing desk opens at nine";
const AUTOPOST_FACT: &str = "Autopost page lists three drafts";
const SEARCH_FACT: &str = "Brass lantern stock is 42 units";
const FILTER_FACT: &str = "Open orders number 7";
const VAULT_FACT: &str = "Vault balance is 318 credits";
const HOME_FACT: &str = "Front page lists five offers";
const CHURN_FACT: &str = "Churn list holds 12 items";
const CONSENT_FACT: &str = "Fares list shows 4 flights";
const DESIGN_FACT: &str = "Mira shipped the new onboarding flow";
const AGENDA_FACT: &str = "Design review at 14:30";
const LANDING_FACT: &str = "Your notes, docs and projects in one place";
const BOARD_FACT: &str = "Ticket ZP-42 is due Thursday";
const DAY_FACT: &str = "Standup at 09:15";
const SHELF_FACT: &str = "Shelf total is 9 products";
const KETTLE_FACT: &str = "Kettle price is 37 credits";
const WEBMAIL_GOAL: &str = "Compose a new email to dana@example.test with the subject Status, then send it. The body, where \\n stands for a real line break to type, is: Hi Dana,\\n\\nThe release is ready.\\nThanks, Hayk";
const WEBMAIL_THREAD_GOAL: &str = "Send a new email to dana@example.test with the subject Status. The body, where \\n stands for a real line break to type, is: Hi Dana,\\n\\nThe release is ready.\\nThanks, Hayk";
const WEBMAIL_SENT: &str =
    "dana@example.test|Status|Hi Dana,\n\nThe release is ready.\nThanks, Hayk";
const ROUTES_FACT: &str = "Dana: Quarterly report";
const DESK_FACT: &str = "Desk bell rings twice at noon";
const HOURS_FACT: &str = "Doors open at half past eight";
/// The vault's own sign-in, kept by the server: a tab sign-in stands for it.
static VAULT_OPEN: AtomicBool = AtomicBool::new(false);
static JOURNEY_FIELDS: AtomicUsize = AtomicUsize::new(0);
/// Page loads the person's tabs made on the site, as the engine would count them.
static TAB_LOADS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Check {
    /// Asked once; Allow opens the person's session and its cookie reaches the site.
    Session,
    /// Always is kept for the profile: the next run asks nothing.
    Always,
    /// Never and private runs keep the session closed.
    Never,
    /// A same-document route inside the page keeps the page.
    Route,
    /// A same-site redirect is followed to its landing page.
    Redirect,
    /// A redirect off the site ends the page without carrying the session.
    Leave,
    /// A page's own POST without any agent step is cancelled; the page stays.
    Autopost,
    /// A committing control is held back; nothing is posted.
    Held,
    /// A sign-in page stops for the person before any model call.
    Wall,
    /// Native retained human presentation and same-document wall clearing.
    Human(human_fixture::Scenario),
    /// A search form and a script filter load their results without asking.
    Search,
    /// Station suggestions and segmented numeric ARIA date/time inputs.
    Journey,
    /// An approved send runs once, exactly as previewed.
    Approve,
    /// A total that changes after approval spends the approval.
    Changed,
    /// A declined booking commits nothing.
    Decline,
    /// An autosaving editor asks once; the run-wide allowance covers the rest.
    Autosave,
    /// A signed-out start page is worked on without the entry question.
    SignedOut,
    /// A sign-in finished in a tab wakes the held page, which starts over once.
    TabSignIn,
    /// A page that reloads itself after a click neither fails its read nor
    /// holds the pages after it, in its run or the next.
    Churn,
    /// A page whose script stalls the page after a click is lost alone: the
    /// next page of its run and the next run's page both read.
    Freeze,
    /// A cookie banner that saves the refusal with a form POST, or with a
    /// script and a reload, is refused by Rust and the page reads.
    Consent,
    /// A single-page app whose sidebar re-renders its rows on every click,
    /// like Slack: opening a channel reads without a refusal loop.
    Spa,
    /// A product page that rewrites its address on load, then Back to the
    /// list it was opened from.
    Back,
    /// A page that reloads itself once on arrival, then a link on it.
    Reloaded,
    /// A link whose address the site sends to another site, which the
    /// session refuses: the page task goes on from the page it was on.
    Detour,
    /// A heavy calendar whose script holds the page for seconds after load:
    /// its first look waits for it and the day reads.
    Heavy,
    /// A mail app that redirects, sets `#inbox` and never finishes loading
    /// still reads once its document is committed.
    Webmail,
    /// Three signed-in apps read by three parts of one lead run: one entry
    /// question names all three and every page opens in the session.
    Apps,
    /// Six daily apps' main views (Slack, Gmail, Calendar, Linear, Notion,
    /// GitHub) read as records with no model call.
    Views,
    /// An app that shows its public landing page to a signed-out visitor: a
    /// task about the person's own account hands over for sign-in instead of
    /// reading the landing page.
    SignedOutApp,
    /// A message typed into a rich composer and sent with Enter, after the
    /// person approves it: the channel receives exactly that text.
    SlackWrite,
    /// The complete natural-language approved-send request through real small models.
    SlackRequest,
    /// The same message sent with Enter from the composer, not its button.
    SlackEnter,
    /// Declining the rich composer's send must leave the channel unchanged.
    SlackDecline,
    /// Facts painted in a canvas are available only through the visual path.
    Visual,
    /// Source-bound synthetic fetched images, local crops and negative gates.
    Media(media_fixture::Scenario),
    /// A new issue from a dialog of rich editors and a pointer-down picker.
    LinearWrite,
    /// A line added to a page of blocks that saves as it is typed.
    NotionWrite,
    /// A mail compose that moves to `#inbox?compose=new` and sends a
    /// multi-line body: the page loads nothing, the body keeps its lines.
    WebmailCompose,
    /// The same compose with Send replaced by an identical new node while
    /// the confirmation waits: the approved step runs once.
    WebmailSwap,
    WebmailAutosave,
    /// The same compose floating over an inbox longer than one look: after
    /// the approval the page task still sees the message and sends it.
    WebmailInbox,
    /// The same compose with a body that starts empty and takes a block per
    /// line from the browser's own editing, as a mail composer's does.
    WebmailBlocks,
    /// The same compose beside an open newsletter thread with its own Reply
    /// box and Send: the new message is sent, the thread is never answered.
    WebmailThread,
    /// The model's own Navigate to a page whose script routes itself with a
    /// fragment as soon as it commits: that move loads nothing, so the
    /// load settles on it.
    WebmailRoutes,
    /// A consent choice a page outside the person's session saved in one run
    /// reaches the site in the next run from Work's own memory; the profile's
    /// own store holds none.
    Memory,
}
/// Freeze leaves a lost page's debt, which the probe's exit reports as an
/// unclean shutdown; it runs on its own.
const ALL: [Check; 24] = [
    Check::Memory,
    Check::Session,
    Check::Always,
    Check::Never,
    Check::Route,
    Check::Redirect,
    Check::Leave,
    Check::Autopost,
    Check::Held,
    Check::Wall,
    Check::Search,
    Check::Approve,
    Check::Changed,
    Check::Decline,
    Check::Autosave,
    Check::SignedOut,
    Check::TabSignIn,
    Check::Churn,
    Check::Consent,
    Check::Spa,
    Check::Heavy,
    Check::Apps,
    Check::Views,
    Check::SignedOutApp,
];
static CHECKS: OnceLock<Vec<Check>> = OnceLock::new();
/// The engine, to ask which store holds a site's cookies.
static ENGINE: OnceLock<std::sync::Arc<zephium_engine::WebviewEngine>> = OnceLock::new();

pub(super) fn remember_engine(engine: std::sync::Arc<zephium_engine::WebviewEngine>) {
    let _ = ENGINE.set(engine);
}

/// The app's own session check and tab-load count, as the desktop installs
/// them: the profile's real store answers.
#[cfg(target_os = "macos")]
pub(super) fn install_app_presence(engine: std::sync::Arc<zephium_engine::WebviewEngine>) {
    remember_engine(engine.clone());
    let loads = engine.clone();
    zephium_app::work_context::install_session_presence(std::sync::Arc::new(
        move |profile, hosts| engine.work_sessions_present(profile, hosts),
    ));
    zephium_app::work_context::install_site_loads(std::sync::Arc::new(move |profile, site| {
        loads.work_site_loads(profile, site)
    }));
}

/// Clears Work's store as Settings does; whether it is empty afterwards.
#[cfg(target_os = "macos")]
pub(super) fn clear_work_memory(profile: ProfileId) -> Option<bool> {
    ENGINE
        .get()?
        .clear_work_website_data(profile)
        .recv_timeout(Duration::from_secs(15))
        .ok()
}

/// Whether the profile's own store and Work's store hold cookies for a host.
#[cfg(target_os = "macos")]
pub(super) fn stores_hold(profile: ProfileId, host: &str) -> Option<(bool, bool)> {
    let engine = ENGINE.get()?;
    let wait = Duration::from_secs(5);
    let own = engine
        .work_sessions_present(profile, vec![host.into()])
        .recv_timeout(wait)
        .ok()?;
    let work = engine
        .work_store_sessions_present(profile, vec![host.into()])
        .recv_timeout(wait)
        .ok()?;
    Some((own.first() == Some(&true), work.first() == Some(&true)))
}

/// Sites a person's profile has usually seen before they ask Work anything.
#[cfg(target_os = "macos")]
const EVERYDAY_SITES: &[&str] = &[
    "https://www.google.com/",
    "https://www.airbnb.com/",
    "https://www.booking.com/",
    "https://allegro.pl/",
    "https://www.amazon.com/",
    "https://www.ebay.com/",
    "https://travel.state.gov/",
    "https://www.state.gov/",
    "https://www.kayak.com/",
    "https://www.ycombinator.com/",
    "https://www.lego.com/",
    "https://www.x-kom.pl/",
    "https://en.wikipedia.org/",
    "https://github.com/",
];

/// `ZEPHIUM_PROBE_SEEDED_PROFILE` set: before the run, the profile's own
/// store visits everyday sites (or the space-separated addresses the
/// variable names), so the run meets the cookies a real profile holds.
/// Prints, per site, whether the profile now holds a session for it.
#[cfg(target_os = "macos")]
pub(super) async fn seed_profile(profile: ProfileId) -> Result<(), &'static str> {
    let Some(named) = std::env::var_os("ZEPHIUM_PROBE_SEEDED_PROFILE") else {
        return Ok(());
    };
    let named = named.to_string_lossy();
    let urls: Vec<String> = if named.contains("https://") {
        named.split_whitespace().map(str::to_owned).collect()
    } else {
        EVERYDAY_SITES.iter().map(|url| (*url).to_owned()).collect()
    };
    let hosts: Vec<String> = urls
        .iter()
        .filter_map(|url| zephium_app::work_sites::host_of(url))
        .collect();
    let engine = ENGINE.get().ok_or("seed_engine")?.clone();
    let started = std::time::Instant::now();
    let opened = engine
        .visit_in_profile(profile, urls.clone())
        .recv_timeout(Duration::from_secs(10))
        .map_err(|_| "seed_visit")?;
    // Every site's first load and its scripts' first cookies, bounded.
    let mut present = Vec::new();
    while started.elapsed() < Duration::from_secs(40) {
        tokio::time::sleep(Duration::from_secs(4)).await;
        present = engine
            .work_sessions_present(profile, hosts.clone())
            .recv_timeout(Duration::from_secs(5))
            .unwrap_or_default();
        if started.elapsed() >= Duration::from_secs(16) && present.iter().all(|held| *held) {
            break;
        }
    }
    let _ = engine
        .end_profile_visits()
        .recv_timeout(Duration::from_secs(10));
    for (host, held) in hosts
        .iter()
        .zip(present.iter().chain(std::iter::repeat(&false)))
    {
        let _ = writeln!(
            std::io::stdout().lock(),
            "seeded-profile: host={host} session_present={held}"
        );
    }
    let _ = writeln!(
        std::io::stdout().lock(),
        "seeded-profile: opened={opened} sites={} present={} elapsed_ms={}",
        hosts.len(),
        present.iter().filter(|held| **held).count(),
        started.elapsed().as_millis()
    );
    Ok(())
}

#[cfg(windows)]
pub(super) fn stores_hold(_: ProfileId, _: &str) -> Option<(bool, bool)> {
    // The Windows engine does not expose a separate Work-store cookie query.
    None
}

pub(super) fn slack_send_request(url: &str, message: &str) -> String {
    format!(
        "Open my disposable Slack test channel at {url} with the Browser helper. If sign-in or MFA is needed, hand the live page to me and resume after native verification. Stay in the browser through any app-launch page. Send exactly this message once: {} Prepare it and wait for the normal explicit confirmation before sending. Then read the channel to verify it appears and report the result honestly. Never resend after uncertain verification or extraction failure. Do not search, substitute help pages, use Computer, edit, delete, invite, or change other account data.\nExplain from recorded state whether the approved message was sent and what verification was obtained. Do not send or retry any action to explain the previous result.",
        message
    )
}

pub(super) fn run(which: &std::ffi::OsStr) -> Result<(), super::ProbeFailure> {
    #[cfg(windows)]
    if std::env::var_os("ZEPHIUM_PROBE_SEEDED_PROFILE").is_some() {
        return Err(super::ProbeFailure::Authority);
    }
    let checks = match which.to_str() {
        Some("all") => ALL.to_vec(),
        Some("session") => vec![Check::Session],
        Some("always") => vec![Check::Always],
        Some("never") => vec![Check::Never],
        Some("route") => vec![Check::Route],
        Some("redirect") => vec![Check::Redirect],
        Some("leave") => vec![Check::Leave],
        Some("autopost") => vec![Check::Autopost],
        Some("held") => vec![Check::Held],
        Some("wall") => vec![Check::Wall],
        Some("human-signin") => vec![Check::Human(human_fixture::Scenario::SignIn)],
        Some("human-signin-hash") => vec![Check::Human(human_fixture::Scenario::SignInHash)],
        Some("human-signin-sites") => vec![Check::Human(human_fixture::Scenario::SignInSites)],
        Some("human-email-code") => vec![Check::Human(human_fixture::Scenario::EmailCode)],
        Some("human-check") => vec![Check::Human(human_fixture::Scenario::Check)],
        Some("human-long-wait") => vec![Check::Human(human_fixture::Scenario::LongWait)],
        Some("human-unresolved") => vec![Check::Human(human_fixture::Scenario::Unresolved)],
        Some("human-cancel") => vec![Check::Human(human_fixture::Scenario::Cancel)],
        Some("human-stale-document") => vec![Check::Human(human_fixture::Scenario::StaleDocument)],
        Some("human") => human_fixture::SCENARIOS
            .iter()
            .copied()
            .map(Check::Human)
            .collect(),
        Some("search") => vec![Check::Search],
        Some("journey-form") => vec![Check::Journey],
        Some("approve") => vec![Check::Approve],
        Some("changed") => vec![Check::Changed],
        Some("decline") => vec![Check::Decline],
        Some("autosave") => vec![Check::Autosave],
        Some("signedout") => vec![Check::SignedOut],
        Some("tabsignin") => vec![Check::TabSignIn],
        Some("churn") => vec![Check::Churn],
        Some("freeze") => vec![Check::Freeze],
        Some("consent") => vec![Check::Consent],
        Some("spa") => vec![Check::Spa],
        Some("back") => vec![Check::Back],
        Some("reloaded") => vec![Check::Reloaded],
        Some("detour") => vec![Check::Detour],
        Some("heavy") => vec![Check::Heavy],
        Some("webmail") => vec![Check::Webmail],
        Some("apps") => vec![Check::Apps],
        Some("views") => vec![Check::Views],
        Some("signed-out-app") => vec![Check::SignedOutApp],
        Some("memory") => vec![Check::Memory],
        Some("slackwrite") => vec![Check::SlackWrite],
        Some("slack-request") => vec![Check::SlackRequest],
        Some("slackenter") => vec![Check::SlackEnter],
        Some("slackdecline") => vec![Check::SlackDecline],
        Some("slack-actions") => vec![Check::SlackWrite, Check::SlackEnter, Check::SlackDecline],
        Some("visual") => vec![Check::Visual],
        Some("media") => vec![
            Check::Media(media_fixture::Scenario::Mixed),
            Check::Media(media_fixture::Scenario::Anonymous),
            Check::Media(media_fixture::Scenario::Held),
        ],
        Some("media-anonymous") => vec![Check::Media(media_fixture::Scenario::Anonymous)],
        Some("media-crop") => vec![Check::Media(media_fixture::Scenario::Canvas)],
        Some("media-record") => vec![Check::Media(media_fixture::Scenario::Record)],
        Some("media-held") => vec![Check::Media(media_fixture::Scenario::Held)],
        Some("webmail-compose") => vec![Check::WebmailCompose],
        Some("webmail-swap") => vec![Check::WebmailSwap],
        Some("webmail-autosave") => vec![Check::WebmailAutosave],
        Some("webmail-inbox") => vec![Check::WebmailInbox],
        Some("webmail-blocks") => vec![Check::WebmailBlocks],
        Some("webmail-thread") => vec![Check::WebmailThread],
        Some("webmail-routes") => vec![Check::WebmailRoutes],
        Some("linearwrite") => vec![Check::LinearWrite],
        Some("notionwrite") => vec![Check::NotionWrite],
        Some("writes") => vec![
            Check::SlackWrite,
            Check::SlackEnter,
            Check::LinearWrite,
            Check::NotionWrite,
        ],
        _ => return Err(super::ProbeFailure::Authority),
    };
    let _ = CHECKS.set(checks);
    super::work_durable::run_loopback_site()
}

/// Only the explicit synthetic handoff checks expose a native human pane.
pub(super) fn needs_human_presentation() -> bool {
    CHECKS
        .get()
        .is_some_and(|checks| checks.iter().any(|check| matches!(check, Check::Human(_))))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Site {
    Account,
    Other,
    Third,
    Board,
}
struct Hit {
    site: Site,
    post: bool,
    path: String,
    cookie: String,
}

/// The app's session check against the real profile store, with each
/// contract site answered by the loopback host whose cookies stand for it.
pub(super) fn install_presence(engine: std::sync::Arc<zephium_engine::WebviewEngine>) {
    remember_engine(engine.clone());
    zephium_app::work_context::install_session_presence(std::sync::Arc::new(
        move |profile, sites: Vec<String>| {
            let hosts = sites
                .into_iter()
                .map(|site| {
                    match site.as_str() {
                        SITE | THIRD_SITE | BOARD_SITE => "127.0.0.1",
                        OTHER_SITE => "localhost",
                        _ => "fresh.probe.invalid",
                    }
                    .to_owned()
                })
                .collect();
            engine.work_sessions_present(profile, hosts)
        },
    ));
    zephium_app::work_context::install_site_loads(std::sync::Arc::new(|_, _| {
        let (reply, answer) = std::sync::mpsc::channel();
        let _ = reply.send(TAB_LOADS.load(Ordering::SeqCst));
        answer
    }));
}

/// Four HTTP/1.1 loopback servers that record method, path and cookie
/// only. The third and board sites share the account host, but use separate
/// origins. Each Windows Work origin receives its own real authentication seed.
struct Sites {
    account: u16,
    other: u16,
    third: u16,
    board: u16,
    hits: Arc<Mutex<Vec<Hit>>>,
    stop: Arc<AtomicBool>,
}
impl Sites {
    fn start() -> Result<Self, &'static str> {
        let hits = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let account = TcpListener::bind(("127.0.0.1", 0)).map_err(|_| "loopback_bind")?;
        let other = TcpListener::bind(("127.0.0.1", 0)).map_err(|_| "loopback_bind")?;
        let third = TcpListener::bind(("127.0.0.1", 0)).map_err(|_| "loopback_bind")?;
        let board = TcpListener::bind(("127.0.0.1", 0)).map_err(|_| "loopback_bind")?;
        let board_port = board.local_addr().map_err(|_| "loopback_addr")?.port();
        let ports = (
            account.local_addr().map_err(|_| "loopback_addr")?.port(),
            other.local_addr().map_err(|_| "loopback_addr")?.port(),
        );
        let third_port = third.local_addr().map_err(|_| "loopback_addr")?.port();
        for (listener, site) in [
            (account, Site::Account),
            (other, Site::Other),
            (third, Site::Third),
            (board, Site::Board),
        ] {
            listener
                .set_nonblocking(true)
                .map_err(|_| "loopback_nonblocking")?;
            let hits = hits.clone();
            let stop = stop.clone();
            std::thread::Builder::new()
                .name("loopback-site".into())
                .spawn(move || {
                    while !stop.load(Ordering::Relaxed) {
                        match listener.accept() {
                            Ok((stream, _)) => serve(stream, site, ports, &hits),
                            Err(_) => std::thread::sleep(Duration::from_millis(5)),
                        }
                    }
                })
                .map_err(|_| "loopback_thread")?;
        }
        Ok(Self {
            account: ports.0,
            other: ports.1,
            third: third_port,
            board: board_port,
            hits,
            stop,
        })
    }
    /// The loopback address behind a contract URL. The other site is named
    /// `localhost`, so the two sites never share a cookie jar entry.
    fn local(&self, url: &str) -> String {
        if let Some(path) = url.strip_prefix(ACCOUNT) {
            format!("http://127.0.0.1:{}{path}", self.account)
        } else if let Some(path) = url.strip_prefix(OTHER) {
            format!("http://localhost:{}{path}", self.other)
        } else if let Some(path) = url.strip_prefix(THIRD) {
            format!("http://127.0.0.1:{}{path}", self.third)
        } else if let Some(path) = url.strip_prefix(BOARD) {
            format!("http://127.0.0.1:{}{path}", self.board)
        } else {
            url.to_owned()
        }
    }
    fn hits(&self, site: Site, path: &str) -> Vec<(bool, String)> {
        self.hits
            .lock()
            .map(|hits| {
                hits.iter()
                    .filter(|hit| hit.site == site && hit.path.starts_with(path))
                    .map(|hit| (hit.post, hit.cookie.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }
    /// What a replica's commits posted under a path, in order.
    fn written(&self, path: &str) -> Vec<String> {
        self.hits
            .lock()
            .map(|hits| {
                hits.iter()
                    .filter(|hit| {
                        hit.site == Site::Account && hit.post && hit.path.starts_with(path)
                    })
                    .filter_map(|hit| super::work_app_editors::posted(&hit.path))
                    .collect()
            })
            .unwrap_or_default()
    }
    fn posts(&self) -> usize {
        self.hits
            .lock()
            .map(|hits| hits.iter().filter(|hit| hit.post).count())
            .unwrap_or(usize::MAX)
    }
}
impl Drop for Sites {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn serve(mut stream: TcpStream, site: Site, ports: (u16, u16), hits: &Mutex<Vec<Hit>>) {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut request = Vec::new();
    let mut chunk = [0u8; 4096];
    while !request.windows(4).any(|w| w == b"\r\n\r\n") && request.len() < 16 * 1024 {
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(read) => request.extend_from_slice(&chunk[..read]),
        }
    }
    let head = String::from_utf8_lossy(&request);
    let mut lines = head.lines();
    let mut first = lines.next().unwrap_or_default().split(' ');
    let method = first.next().unwrap_or_default().to_owned();
    let path = first.next().unwrap_or_default().to_owned();
    let cookie = lines
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.eq_ignore_ascii_case("cookie"))
        .map(|(_, value)| value.trim().to_owned())
        .unwrap_or_default();
    let signed_in = cookie.contains(ACCOUNT_COOKIE);
    if let Ok(mut hits) = hits.lock() {
        hits.push(Hit {
            site,
            post: method == "POST",
            path: path.clone(),
            cookie: cookie.clone(),
        });
    }
    if site == Site::Account && method == "GET" && load_fixture::never(&path) {
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(300));
            drop(stream);
        });
        return;
    }
    if site == Site::Account
        && method == "GET"
        && media_fixture::serve(&mut stream, &path, signed_in)
    {
        return;
    }
    let page = |title: &str, body: &str| {
        format!(
            "<!doctype html><html><head><title>{title}</title></head><body><main>{body}</main></body></html>"
        )
    };
    let consented = |name: &str| cookie.contains(&format!("{name}=no"));
    let banner = |form: bool| {
        let buttons = if form {
            "<form method=\"post\" action=\"/consent/save\"><button name=\"set\" value=\"reject\">Reject all</button><button name=\"set\" value=\"accept\">Accept all</button></form>"
        } else {
            "<button id=\"r\" type=\"button\">Reject all</button><button id=\"a\" type=\"button\">Accept all</button><script>document.getElementById('r').addEventListener('click',function(){this.disabled=true;fetch('/consent/jsave',{method:'POST'}).then(function(){location.reload();});});</script>"
        };
        format!(
            "<div role=\"dialog\" aria-modal=\"true\" aria-label=\"Before you continue\"><h2>Before you continue</h2><p>We use cookies and data to deliver and maintain our services.</p>{buttons}</div><main><p>Loading fares</p></main>"
        )
    };
    let (status, headers, body) = match (site, method.as_str(), path.as_str()) {
        (Site::Account, _, "/human-fixture/exit") => (
            "302 Found",
            format!(
                "Location: http://localhost:{}/human-fixture/hop\r\n",
                ports.1
            ),
            String::new(),
        ),
        (Site::Other, _, "/human-fixture/hop") => (
            "302 Found",
            format!(
                "Set-Cookie: zhop=1; Path=/; Max-Age=3600\r\nLocation: http://127.0.0.1:{}/human-fixture/signin-sites/2\r\n",
                ports.0
            ),
            String::new(),
        ),
        (Site::Account, "GET", path) if path.starts_with("/human-fixture/") => {
            ("200 OK", String::new(), human_fixture::response(path))
        }
        (Site::Account, "POST", "/consent/save") => (
            "303 See Other",
            "Set-Cookie: zconsent=no; Path=/; Max-Age=3600\r\nLocation: /gconsent\r\n".into(),
            String::new(),
        ),
        (Site::Account, "POST", "/consent/jsave") => (
            "200 OK",
            "Set-Cookie: zjconsent=no; Path=/; Max-Age=3600\r\n".into(),
            String::new(),
        ),
        (Site::Other, "POST", "/memory/save") => (
            "303 See Other",
            format!("Set-Cookie: {MEMORY_COOKIE}; Path=/; Max-Age=3600\r\nLocation: /memory\r\n"),
            String::new(),
        ),
        (_, "POST", _) => ("200 OK", String::new(), page("Done", "<p>Saved.</p>")),
        (Site::Account, _, "/gconsent") if consented("zconsent") => (
            "200 OK",
            String::new(),
            page("Fares", &format!("<h1>Fares</h1><p>{CONSENT_FACT}.</p>")),
        ),
        (Site::Account, _, "/jconsent") if consented("zjconsent") => (
            "200 OK",
            String::new(),
            page("Fares", &format!("<h1>Fares</h1><p>{CONSENT_FACT}.</p>")),
        ),
        (Site::Account, _, "/gconsent") => ("200 OK", String::new(), banner(true)),
        (Site::Account, _, "/jconsent") => ("200 OK", String::new(), banner(false)),
        (Site::Account | Site::Board | Site::Third, _, "/seed") => (
            "200 OK",
            format!("Set-Cookie: {ACCOUNT_COOKIE}; Path=/; Max-Age=3600; HttpOnly\r\n"),
            page(
                "Account",
                "<h1>Signed in</h1><p>Welcome back to the account site.</p>",
            ),
        ),
        (Site::Account, "GET", path) if load_fixture::response(path).is_some() => {
            load_fixture::response(path).unwrap_or_default()
        }
        (Site::Account, _, "/leave") => (
            "302 Found",
            format!("Location: http://localhost:{}/landing\r\n", ports.1),
            String::new(),
        ),
        (Site::Account, _, "/start") => {
            ("302 Found", "Location: /landed\r\n".into(), String::new())
        }
        (Site::Account, _, "/landed") => (
            "200 OK",
            String::new(),
            page(
                "Landed",
                &format!("<h1>Front desk</h1><p>{LANDED_FACT}.</p>"),
            ),
        ),
        (Site::Account, _, "/visual") => (
            "200 OK",
            String::new(),
            page(
                "Chart",
                r##"<h1>Inventory chart</h1><canvas id="chart" width="700" height="420" aria-label="Inventory bar chart"></canvas><script>
const c=document.getElementById('chart').getContext('2d');c.fillStyle='white';c.fillRect(0,0,700,420);c.font='28px sans-serif';
[['Red',17,'#b22222'],['Blue',43,'#1644bb'],['Green',29,'#16753e']].forEach(function(row,i){let y=55+i*115;c.fillStyle='black';c.fillText(row[0],20,y+28);c.fillStyle=row[2];c.fillRect(130,y,row[1]*10,45);c.fillStyle='black';c.fillText(String(row[1]),145+row[1]*10,y+30);});
</script>"##,
            ),
        ),
        (Site::Account, _, "/app") => (
            "200 OK",
            String::new(),
            page(
                "App",
                &format!(
                    "<h1>Dashboard</h1><button id=\"r\" type=\"button\">Reports</button><section id=\"view\"><p>Pick a view.</p></section><script>document.getElementById('r').addEventListener('click',function(){{history.pushState({{}},'','/app/reports');document.getElementById('view').innerHTML='<h2>Reports</h2><p>{REPORT_FACT}.</p>';}});</script>"
                ),
            ),
        ),
        (Site::Account, _, "/autopost") => (
            "200 OK",
            String::new(),
            page(
                "Autopost",
                &format!(
                    "<h1>Drafts</h1><p>{AUTOPOST_FACT}.</p><form id=\"f\" method=\"post\" action=\"/autopost/submit\"><input type=\"hidden\" name=\"x\" value=\"1\"></form><script>setTimeout(function(){{document.getElementById('f').submit();}},600);</script>"
                ),
            ),
        ),
        (Site::Account, _, "/compose") => (
            "200 OK",
            String::new(),
            page(
                "Compose",
                "<h1>Message #design</h1><label>Message <input id=\"m\" name=\"message\"></label><button id=\"s\" type=\"button\">Send</button><div id=\"log\" role=\"status\"></div><script>document.getElementById('s').addEventListener('click',function(){var m=document.getElementById('m');fetch('/send',{method:'POST',body:m.value});document.getElementById('log').textContent='Message sent: '+m.value;m.value='';});</script>",
            ),
        ),
        (Site::Account, _, path) if path.starts_with("/book") => {
            // /book/live raises its total every second, so a later look differs.
            let live = path.starts_with("/book/live");
            (
                "200 OK",
                String::new(),
                page(
                    "Cabin",
                    &format!(
                        "<h1>Cabin by the lake</h1><form method=\"post\" action=\"/book/confirm\" aria-label=\"Reserve\"><p>Dates: 3-5 May</p><p>Guests: 2</p><p>Total: <span id=\"t\">$1,240</span></p><button>Request to book</button></form>{}",
                        if live {
                            "<script>var n=1240,s=Date.now();setInterval(function(){document.getElementById('t').textContent='$'+(n+Math.floor((Date.now()-s)/1000)*10).toLocaleString('en-US');},250);</script>"
                        } else {
                            ""
                        }
                    ),
                ),
            )
        }
        (Site::Account, _, "/notes") => (
            "200 OK",
            String::new(),
            page(
                "Notes",
                "<h1>Meeting notes</h1><div id=\"n\" contenteditable=\"true\" role=\"textbox\" aria-label=\"Notes\" aria-multiline=\"true\" style=\"min-height:120px;border:1px solid #999\"></div><script>var t;document.getElementById('n').addEventListener('input',function(){clearTimeout(t);t=setTimeout(function(){fetch('/save',{method:'POST',body:document.getElementById('n').textContent});},300);});</script>",
            ),
        ),
        (Site::Account, _, "/home") => (
            "200 OK",
            String::new(),
            page(
                "Home",
                &format!(
                    "<header><nav><a href=\"/login\">Sign in</a></nav></header><h1>Welcome</h1><p>{HOME_FACT}.</p>"
                ),
            ),
        ),
        (Site::Account, _, "/churn") => (
            "200 OK",
            String::new(),
            page(
                "Churn",
                &format!(
                    "<h1>List</h1><button id=\"c\" type=\"button\">Continue</button><p id=\"p\">Press Continue to load the list.</p><script>var n=+(sessionStorage.getItem('churn')||0);if(n>0&&n<7){{document.getElementById('p').textContent='Loading';sessionStorage.setItem('churn',n+1);setTimeout(function(){{location.reload();}},350);}}else if(n>=7){{document.getElementById('p').textContent='{CHURN_FACT}.';}}document.getElementById('c').addEventListener('click',function(){{sessionStorage.setItem('churn',1);setTimeout(function(){{location.reload();}},120);}});</script>"
                ),
            ),
        ),
        (Site::Account, _, "/freeze") => (
            "200 OK",
            String::new(),
            page(
                "Freeze",
                "<h1>Report</h1><button id=\"l\" type=\"button\">Load</button><p id=\"p\">Press Load to fill the report.</p><script>document.getElementById('l').addEventListener('click',function(){document.getElementById('p').textContent='Loading the report';setTimeout(function(){var t=Date.now();while(Date.now()-t<40000){}},300);});</script>",
            ),
        ),
        (Site::Account, _, "/vault-login") => {
            VAULT_OPEN.store(true, Ordering::SeqCst);
            (
                "200 OK",
                String::new(),
                page("Signed in", "<p>Signed in.</p>"),
            )
        }
        (Site::Account, _, "/vault") if !VAULT_OPEN.load(Ordering::SeqCst) => (
            "200 OK",
            String::new(),
            page(
                "Sign in",
                "<h1>Sign in to your vault</h1><form method=\"post\" action=\"/login\"><label>Email <input type=\"email\" name=\"email\" autocomplete=\"email\"></label><label>Password <input type=\"password\" name=\"password\"></label><button>Sign in</button></form>",
            ),
        ),
        (Site::Account, _, "/vault") => (
            "200 OK",
            String::new(),
            page("Vault", &format!("<h1>Vault</h1><p>{VAULT_FACT}.</p>")),
        ),
        (Site::Account, _, "/lobby") => (
            "200 OK",
            String::new(),
            page(
                "Lobby",
                "<h1>Lobby</h1><p><a href=\"/lobby/desk\">Front desk</a></p><script>if(!sessionStorage.getItem('r')){sessionStorage.setItem('r','1');setTimeout(function(){location.reload();},200);}</script>",
            ),
        ),
        (Site::Account, _, "/hours") => (
            "200 OK",
            String::new(),
            page(
                "Help",
                "<h1>Help</h1><p><a href=\"/hours/open\">Opening hours</a></p><p><a href=\"/hours/contact\">Contact</a></p>",
            ),
        ),
        (Site::Account, _, "/hours/open") => (
            "302 Found",
            format!("Location: http://localhost:{}/hours\r\n", ports.1),
            String::new(),
        ),
        (Site::Account, _, "/hours/contact") => (
            "200 OK",
            String::new(),
            page("Contact", &format!("<h1>Contact</h1><p>{HOURS_FACT}.</p>")),
        ),
        (Site::Account, _, "/lobby/desk") => (
            "200 OK",
            String::new(),
            page("Desk", &format!("<h1>Front desk</h1><p>{DESK_FACT}.</p>")),
        ),
        (Site::Account, _, "/shelf") => (
            "200 OK",
            String::new(),
            page(
                "Shelf",
                &format!("<h1>Shelf</h1><p>{SHELF_FACT}.</p><ul><li><a href=\"/shelf/item-1\">Toaster</a></li><li><a href=\"/shelf/item-2\">Kettle</a></li></ul>"),
            ),
        ),
        (Site::Account, _, path) if path.starts_with("/shelf/item-") => (
            "200 OK",
            String::new(),
            page(
                "Product",
                &format!("<h1>Product</h1><p>{KETTLE_FACT}.</p><script>history.pushState({{}}, '', location.pathname + '?view=full');</script>"),
            ),
        ),
        (Site::Account, _, "/find") => (
            "200 OK",
            String::new(),
            page(
                "Catalog",
                "<h1>Catalog</h1><form role=\"search\" action=\"/results\" method=\"get\"><label>Search the catalog <input type=\"search\" name=\"q\"></label><button>Search</button></form>",
            ),
        ),
        (Site::Account, _, "/journey-form") => (
            "200 OK", String::new(), page("Rail journeys", include_str!("work_site_journey.html")),
        ),
        (Site::Account, _, path) if path.starts_with("/journey-results?") => {
            let values: std::collections::BTreeMap<_, _> = zephium_agentic::ContextNavigationTarget::parse(&format!("{ACCOUNT}{path}")).ok().map(|target| target.as_url().query_pairs().into_owned().collect()).unwrap_or_default();
            let matches = [values.get("from").is_some_and(|value| value == "Northport"), values.get("to").is_some_and(|value| value == "Lakeside")]
                .into_iter().chain([("day", 18), ("month", 11), ("year", 2026), ("hour", 9), ("minute", 0)].into_iter()
                    .map(|(key, expected)| values.get(key).and_then(|value| value.parse::<u16>().ok()) == Some(expected)));
            let fields = matches.enumerate().fold(0, |mask, (index, matches)| mask | (usize::from(matches) << index));
            JOURNEY_FIELDS.store(fields, Ordering::SeqCst);
            let correct = fields == 127;
            ("200 OK", String::new(), page("Results", if correct {
                "<h1>Northport to Lakeside, 18 November 2026 after 09:00</h1><p>Train Q17 departs 09:30, takes 75 minutes, no changes, fare EUR 25.</p>"
            } else { "<h1>No matching fixture journeys</h1><p>The station and date fields did not match the request.</p>" }))
        },
        (Site::Account, _, "/filters") => (
            "200 OK",
            String::new(),
            page(
                "Orders",
                "<h1>Orders</h1><button id=\"o\" type=\"button\">Show open orders</button><script>document.getElementById('o').addEventListener('click',function(){location.assign('/results?status=open');});</script>",
            ),
        ),
        (Site::Account, _, path) if path.starts_with("/results") && signed_in => {
            let found = if path.contains("lantern") {
                format!("<p>{SEARCH_FACT}.</p>")
            } else if path.contains("status=open") {
                format!("<p>{FILTER_FACT}.</p>")
            } else {
                "<p>No matches.</p>".to_owned()
            };
            (
                "200 OK",
                String::new(),
                page("Results", &format!("<h1>Results</h1>{found}")),
            )
        }
        (Site::Account, _, "/app/landing") => (
            "200 OK",
            String::new(),
            page(
                "Notes and docs for your team",
                &format!("<header><nav aria-label=\"Main\"><a href=\"/product\">Product</a><a href=\"/pricing\">Pricing</a><a href=\"/login\">Log in</a><a href=\"/signup\">Get started free</a></nav></header><h1>{LANDING_FACT}</h1><p>Write, plan and share in one workspace.</p>"),
            ),
        ),
        (Site::Account, _, _) if !signed_in => (
            "200 OK",
            String::new(),
            page(
                "Sign in",
                "<h1>Sign in to your account</h1><form method=\"post\" action=\"/login\"><label>Email <input type=\"email\" name=\"email\" autocomplete=\"email\"></label><label>Password <input type=\"password\" name=\"password\"></label><button>Sign in</button></form>",
            ),
        ),
        (Site::Account, _, path) if super::work_app_editors::editor(path).is_some() => (
            "200 OK",
            String::new(),
            super::work_app_editors::page(path).unwrap_or_default(),
        ),
        (Site::Account, _, path) if super::work_app_views::routed(path).is_some() => (
            "200 OK",
            String::new(),
            super::work_app_views::routed(path)
                .map(str::to_owned)
                .unwrap_or_default(),
        ),
        (Site::Account, _, path) if super::work_app_views::view(path).is_some() => (
            "200 OK",
            String::new(),
            super::work_app_views::view(path)
                .map(|view| view.html.to_owned())
                .unwrap_or_default(),
        ),
        (Site::Account, _, "/inbox") => (
            "200 OK",
            String::new(),
            page(
                "Inbox",
                &format!(
                    "<h1>Inbox</h1><ul><li>{INBOX_FACT} is due Friday.</li><li>Team lunch moved to Thursday.</li></ul>"
                ),
            ),
        ),
        // Slack-like: every click re-renders the sidebar's rows and replaces
        // the message pane, and the address follows the channel.
        (Site::Account, _, path) if path == "/spa" || path.starts_with("/spa/") => (
            "200 OK",
            String::new(),
            format!(
                "<!doctype html><html><head><title>Workspace</title></head><body><nav aria-label=\"Channels\"><div role=\"tree\" id=\"side\"></div></nav><main id=\"pane\"><h1>#general</h1><ul><li>Ola: coffee at ten?</li></ul></main><script>var chans=['general','design','random'];var msgs={{general:['Ola: coffee at ten?'],design:['Tom: can someone review the icons?','Ana: {DESIGN_FACT}.'],random:['Leo: new plant on the desk']}};function side(active){{var h='';for(var i=0;i<chans.length;i++){{var c=chans[i];h+='<div role=\"treeitem\" tabindex=\"0\" aria-selected=\"'+(c===active)+'\" data-c=\"'+c+'\">#'+c+'</div>';}}var el=document.getElementById('side');el.innerHTML=h;var rows=el.querySelectorAll('[data-c]');for(var j=0;j<rows.length;j++){{rows[j].addEventListener('click',function(){{var c=this.getAttribute('data-c');setTimeout(function(){{open(c);}},150);}});}}}}function open(c){{history.pushState({{}},'','/spa/'+c);document.title='#'+c+' - Workspace';side(c);var h='<h1>#'+c+'</h1><ul>';for(var k=0;k<msgs[c].length;k++){{h+='<li>'+msgs[c][k]+'</li>';}}document.getElementById('pane').innerHTML=h+'</ul>';}}side('general');</script></body></html>"
            ),
        ),
        // A heavy day view: a large grid, and a script that holds the page
        // for eight seconds right after load before it draws the day.
        (Site::Account, _, "/agenda") => {
            let mut cells = String::new();
            for hour in 0..24 {
                for slot in 0..120 {
                    cells.push_str(&format!("<div role=\"gridcell\" data-h=\"{hour}\" data-s=\"{slot}\"><span></span></div>"));
                }
            }
            (
                "200 OK",
                String::new(),
                format!(
                    "<!doctype html><html><head><title>Calendar</title></head><body><main><h1>Today</h1><ul id=\"day\"><li>Loading your day</li></ul><div role=\"grid\" aria-label=\"Week\" style=\"height:4000px;overflow:hidden\">{cells}</div></main><script>window.addEventListener('load',function(){{setTimeout(function(){{var t=Date.now();while(Date.now()-t<8000){{}}document.getElementById('day').innerHTML='<li>{DAY_FACT}</li><li>{AGENDA_FACT}</li><li>Gym at 18:00</li>';}},0);}});</script></body></html>"
                ),
            )
        }
        (Site::Account, _, path) => (
            "200 OK",
            String::new(),
            page(
                "Account page",
                &format!(
                    "<h1>Account page</h1><p>This signed-in page is {}.</p>",
                    path.trim_start_matches('/')
                ),
            ),
        ),
        (Site::Third | Site::Board, _, _) if !signed_in => (
            "200 OK",
            String::new(),
            page(
                "Sign in",
                "<h1>Sign in</h1><form method=\"post\" action=\"/login\"><label>Email <input type=\"email\" name=\"email\"></label><label>Password <input type=\"password\" name=\"password\"></label><button>Sign in</button></form>",
            ),
        ),
        (Site::Board, _, _) => (
            "200 OK",
            String::new(),
            page(
                "Board",
                &format!(
                    "<h1>My issues</h1><ul><li>{BOARD_FACT}.</li><li>ZP-17 is in review.</li></ul>"
                ),
            ),
        ),
        (Site::Third, _, _) => (
            "200 OK",
            String::new(),
            page(
                "Calendar",
                &format!(
                    "<h1>Today</h1><ul><li>{DAY_FACT}.</li><li>Lunch with Ana at 12:30.</li></ul>"
                ),
            ),
        ),
        (Site::Other, _, "/memory") if cookie.contains(MEMORY_COOKIE) => (
            "200 OK",
            String::new(),
            page("Fares", &format!("<h1>Fares</h1><p>{CONSENT_FACT}.</p>")),
        ),
        (Site::Other, _, "/memory") => (
            "200 OK",
            String::new(),
            String::from(
                "<div role=\"dialog\" aria-modal=\"true\" aria-label=\"Before you continue\"><h2>Before you continue</h2><p>We use cookies and data to deliver and maintain our services.</p><form method=\"post\" action=\"/memory/save\"><button name=\"set\" value=\"reject\">Reject all</button><button name=\"set\" value=\"accept\">Accept all</button></form></div><main><p>Loading fares</p></main>",
            ),
        ),
        (Site::Other, _, _) => (
            "200 OK",
            String::new(),
            page(
                "Other",
                "<h1>Other site</h1><p>Opening hours: nine to five.</p>",
            ),
        ),
    };
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\n{headers}Content-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

/// Fetches each turn in order, then places one knowledge note and finishes.
struct Script {
    turns: Vec<Vec<WorkAgentFetch>>,
    at: AtomicUsize,
    notices: Mutex<Vec<String>>,
}
impl Script {
    fn new(turns: Vec<Vec<WorkAgentFetch>>) -> Self {
        Self {
            turns,
            at: AtomicUsize::new(0),
            notices: Mutex::new(Vec::new()),
        }
    }
}
impl WorkAgentTurnProvider for Script {
    fn turn<'a>(
        &'a self,
        input: &'a WorkAgentTurnDisclosure,
        _: WorkSynthesisTrace,
    ) -> WorkAgentTurnFuture<'a> {
        Box::pin(async move {
            if let Ok(mut notices) = self.notices.lock() {
                notices.extend(input.context().notices.iter().cloned());
            }
            let at = self.at.fetch_add(1, Ordering::SeqCst);
            let fetch = self.turns.get(at).cloned().unwrap_or_default();
            let publish = at == self.turns.len();
            Ok(WorkAgentTurnResult {
                output: WorkAgentTurnOutput {
                    say: None,
                    artifacts: if publish {
                        vec![WorkAgentArtifactOutput {
                            title: "Loopback note".into(),
                            data: artifact::WorkArtifactDataV1::Document {
                                paragraphs: vec!["The loopback check ran.".into()],
                                formatted: None,
                            },
                            evidence: vec![],
                            general_knowledge: true,
                        }]
                    } else {
                        vec![]
                    },
                    finish: at > self.turns.len(),
                    fetch,
                    ask: None,
                    followups: vec![],
                    malformed: 0,
                },
                usage: WorkUsage::default(),
            })
        })
    }
}
struct NoSearch;
impl WorkPublicSearchProvider for NoSearch {
    fn search<'a>(
        &'a self,
        _: &'a WorkPublicSearchScope,
        _: &'a [zephium_core::work::context::WorkContextBody],
        _: WorkExecutionLimits,
    ) -> WorkPublicSearchFuture<'a> {
        Box::pin(async { Err(WorkPublicSearchError::NotDispatched(WorkError::Unavailable)) })
    }
}

fn browse(path: &str, goal: &str) -> WorkAgentFetch {
    browse_at(ACCOUNT, path, goal)
}

fn browse_at(site: &str, path: &str, goal: &str) -> WorkAgentFetch {
    WorkAgentFetch::Browse {
        start: format!("{site}{path}"),
        goal: goal.into(),
        collection: None,
    }
}

/// One page as the probe opened it.
#[derive(Clone)]
struct Opened {
    yours: bool,
    not_ready: bool,
    held_back: bool,
    status: Option<WorkStepStatus>,
    intervention: Option<WorkInterventionKindV1>,
    model_calls: Option<u16>,
    resolved_handoff: Option<WorkPageHandoffKindV1>,
}

struct Run {
    state: WorkRuntimeProjection,
    notices: Vec<String>,
    opened: Vec<Opened>,
    asked: Vec<String>,
    /// Headlines of the held steps the probe decided, in order.
    confirmed: Vec<String>,
}

struct Context<'a> {
    handle: &'a zephium_app::Handle,
    composition: &'a NativeWorkComposition,
    profile: ProfileId,
    binding: zephium_app::AgentWorkProfileBinding,
    chosen: Option<zephium_agentic::AgentProviderModelClient>,
    keys: Mutex<Vec<zephium_agentic::AgentProviderCredential>>,
    sites: Sites,
    /// Sign-in walls are passed in a tab rather than released.
    tab_sign_in: AtomicBool,
    /// Changed declines a fresh model handoff after its sole approval.
    decline_changed_followup: AtomicBool,
    human_fixture: Mutex<Option<human_fixture::Session>>,
}
impl Context<'_> {
    async fn create(&self, objective: &str) -> Result<(WorkId, WorkRevision), &'static str> {
        let created = self
            .handle
            .work_authoring_command(
                self.profile,
                WorkAuthoringCommandV1 {
                    version: 1,
                    command: WorkCommandId::generate(),
                    intent: WorkAuthoringIntent::Create {
                        objective: objective.into(),
                    },
                },
            )
            .map_err(|_| "create_admission")?
            .response(self.profile)
            .await;
        let WorkReplyV1::AuthoringApplied { receipt } = created.reply else {
            return Err("create_persistence");
        };
        Ok((receipt.work, receipt.applied_revision))
    }

    async fn projection(&self, work: WorkId) -> Option<WorkRuntimeProjection> {
        let response = self
            .handle
            .work_projection(self.profile, work)
            .ok()?
            .await
            .ok()?;
        match response.reply {
            port::WorkReply::Runtime(state) => Some(*state),
            _ => None,
        }
    }

    /// Answers every question the run puts to the person with `answer`,
    /// and releases any page waiting on them.
    async fn person(
        &self,
        work: WorkId,
        answer: &str,
        done: &AtomicBool,
        asked: &Mutex<Vec<String>>,
        decisions: &Mutex<Vec<(bool, bool)>>,
        confirmed: &Mutex<Vec<String>>,
    ) {
        let mut approved_page = None;
        while !done.load(Ordering::Relaxed) {
            tokio::time::sleep(Duration::from_millis(200)).await;
            let pages = self
                .composition
                .human_pages(self.profile, work)
                .unwrap_or_default();
            human_fixture::person(self, work, &pages).await;
            {
                for page in &pages {
                    if self.human_fixture.lock().is_ok_and(|state| state.is_some())
                        && matches!(
                            page.reason,
                            WorkHumanReasonV1::SignIn | WorkHumanReasonV1::Challenge
                        )
                    {
                        continue;
                    }
                    // The person signs in in a tab instead of the pane.
                    if self.tab_sign_in.load(Ordering::Relaxed)
                        && page.reason == WorkHumanReasonV1::SignIn
                        && page.phase == WorkHumanPhaseV1::WaitingForHuman
                    {
                        if !VAULT_OPEN.load(Ordering::SeqCst) {
                            let _ = std::net::TcpStream::connect(("127.0.0.1", self.sites.account))
                                .and_then(|mut stream| {
                                    write!(stream, "GET /vault-login HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")?;
                                    let mut sink = Vec::new();
                                    stream.read_to_end(&mut sink).map(|_| ())
                                });
                            TAB_LOADS.fetch_add(1, Ordering::SeqCst);
                        }
                        continue;
                    }
                    // A held step is decided on its Confirm step, not in the pane.
                    if page.phase == WorkHumanPhaseV1::WaitingForHuman
                        && page.reason != WorkHumanReasonV1::UserDecision
                    {
                        let _ = self
                            .composition
                            .release_human_page(self.profile, work, page.id);
                    }
                }
            }
            let Some(state) = self.projection(work).await else {
                continue;
            };
            let held = state.executions.last().and_then(|execution| {
                execution.steps.iter().find_map(|step| match &step.kind {
                    WorkStepKindV1::Confirm { confirm: held }
                        if step.status == WorkStepStatus::Running && held.decision.is_none() =>
                    {
                        Some((execution.id, step.id, held.headline.clone()))
                    }
                    _ => None,
                })
            });
            if let Some((execution, step, headline)) = held {
                let (approve, for_run) = decisions
                    .lock()
                    .ok()
                    .and_then(|mut queue| (!queue.is_empty()).then(|| queue.remove(0)))
                    .unwrap_or((false, false));
                let approval_page = if approve
                    && self.decline_changed_followup.load(Ordering::Relaxed)
                {
                    state
                        .executions
                        .last()
                        .filter(|fact| fact.id == execution)
                        .and_then(|fact| {
                            pages
                                .iter()
                                .find(|page| {
                                    page.phase == WorkHumanPhaseV1::WaitingForHuman
                                        && page.reason == WorkHumanReasonV1::UserDecision
                                        && fact
                                            .attempts
                                            .iter()
                                            .any(|attempt| attempt.id == page.id.attempt)
                                        && fact.steps.iter().any(|read| {
                                            read.id == page.id.step
                                                && read.status == WorkStepStatus::Running
                                                && matches!(read.kind, WorkStepKindV1::Read { .. })
                                        })
                                })
                                .map(|page| (execution, step, page.id))
                        })
                } else {
                    None
                };
                let applied = self
                    .handle
                    .work_command(
                        self.profile,
                        WorkCommandV1 {
                            version: 1,
                            work,
                            expected_revision: state.work.revision,
                            command: WorkCommandId::generate(),
                            intent: WorkRuntimeIntent::ApproveStep {
                                execution,
                                step,
                                approve,
                                for_run,
                            },
                        },
                    )
                    .ok();
                if let Some(applied) = applied {
                    if applied.await.is_ok() {
                        if let Some(page) = approval_page {
                            approved_page = Some(page);
                        }
                        if let Ok(mut confirmed) = confirmed.lock() {
                            confirmed.push(headline);
                        }
                    }
                }
                continue;
            }
            // A model may ask the person directly instead of proposing a new
            // Confirm. Changed declines only a successor of its approved pane;
            // closing settles the original, undispatched approval as NotSent.
            if self.decline_changed_followup.load(Ordering::Relaxed)
                && decisions.lock().is_ok_and(|queue| queue.is_empty())
            {
                if let Some((approved_execution, approved_step, original)) = approved_page {
                    if state.executions.last().is_some_and(|execution| {
                        execution.id == approved_execution
                            && execution.steps.iter().any(|step| {
                                step.id == approved_step
                                    && matches!(&step.kind, WorkStepKindV1::Confirm { confirm }
                                        if confirm.decision == Some(WorkConfirmDecisionV1::Approved))
                            })
                            && !execution.steps.iter().any(|step| {
                                step.status == WorkStepStatus::Running
                                    && match &step.kind {
                                        WorkStepKindV1::Ask { .. } => true,
                                        WorkStepKindV1::Confirm { confirm } => confirm.decision.is_none(),
                                        _ => false,
                                    }
                            })
                    }) {
                        for page in &pages {
                            if page.phase == WorkHumanPhaseV1::WaitingForHuman
                                && page.reason == WorkHumanReasonV1::UserDecision
                                && page.id.attempt == original.attempt
                                && page.id.step == original.step
                                && page.id.generation > original.generation
                            {
                                let released = self.composition.release_human_page(
                                    self.profile, work, page.id
                                ).is_ok();
                                let _ = writeln!(std::io::stdout().lock(),
                                    "loopback-site: changed_followup_declined={released} newer_handoff=true; content=synthetic");
                            }
                        }
                    }
                }
            }
            let Some((execution, step, prompt)) = state.executions.last().and_then(|execution| {
                execution.steps.iter().find_map(|step| match &step.kind {
                    WorkStepKindV1::Ask { prompt, .. }
                        if step.status == WorkStepStatus::Running =>
                    {
                        Some((execution.id, step.id, prompt.clone()))
                    }
                    _ => None,
                })
            }) else {
                continue;
            };
            let applied = self
                .handle
                .work_command(
                    self.profile,
                    WorkCommandV1 {
                        version: 1,
                        work,
                        expected_revision: state.work.revision,
                        command: WorkCommandId::generate(),
                        intent: WorkRuntimeIntent::AnswerStep {
                            execution,
                            step,
                            answer: answer.into(),
                        },
                    },
                )
                .ok();
            if let Some(applied) = applied {
                if applied.await.is_ok() {
                    if let Ok(mut asked) = asked.lock() {
                        asked.push(prompt);
                    }
                }
            }
        }
    }

    /// One run of `turns`, the probe answering questions with `answer`.
    async fn run(
        &self,
        objective: &str,
        private: bool,
        answer: &str,
        turns: Vec<Vec<WorkAgentFetch>>,
    ) -> Result<Run, &'static str> {
        self.run_deciding(objective, private, answer, turns, vec![])
            .await
    }

    /// One run whose held steps the probe decides in order, then declines.
    async fn run_deciding(
        &self,
        objective: &str,
        private: bool,
        answer: &str,
        turns: Vec<Vec<WorkAgentFetch>>,
        decisions: Vec<(bool, bool)>,
    ) -> Result<Run, &'static str> {
        let execution_timeout = self
            .human_fixture
            .lock()
            .ok()
            .and_then(|state| {
                state
                    .as_ref()
                    .map(human_fixture::Session::execution_timeout_seconds)
            })
            .unwrap_or(1200);
        let decisions = Mutex::new(decisions);
        let confirmed = Mutex::new(Vec::new());
        let work = self.create(objective).await?;
        let script = Script::new(turns);
        let opened = Mutex::new(Vec::<Opened>::new());
        let asked = Mutex::new(Vec::new());
        let done = AtomicBool::new(false);
        let callback = self.handle.callback_handle();
        let service = WorkAgentService::new(self.handle.clone()).with_diagnostic(|event| {
            let _ = writeln!(std::io::stdout().lock(), "loopback-site: loop={event:?}");
        });
        let run = async {
            let state = service
                .run(
                    self.profile,
                    WorkCommandV1 {
                        version: 1,
                        work: work.0,
                        expected_revision: work.1,
                        command: WorkCommandId::generate(),
                        intent: WorkRuntimeIntent::BeginAgent {
                            grant: WorkAgentGrantV1 {
                                no_usage_charge: false,
                                lead: None,
                                provider: WorkSearchProvider::OpenAi,
                                model: PUBLIC_SEARCH_MODEL.into(),
                                max_turns: 8,
                                max_steps: 40,
                                browse_hops: 1,
                                folders: vec![],
                                accounts: vec![],
                                private,
                                skill: None,
                                search_fallback: None,
                                draft: None,
                            },
                            limits: WorkExecutionLimits {
                                model_tokens: 1_000_000,
                                cost_micro_usd: 3_000_000,
                                operations: 512,
                                timeout_seconds: execution_timeout,
                                max_workers: 3,
                            },
                        },
                    },
                    None,
                    WorkAgentProviders {
                        turn: &script,
                        search: &NoSearch,
                    },
                    |probe, request| {
                        let key = self.keys.lock().ok().and_then(|mut keys| keys.pop());
                        let callback = &callback;
                        let opened = &opened;
                        async move {
                            let (request, yours) = self.local(request)?;
                            let _ = writeln!(std::io::stdout().lock(),
                                "loopback-site: request_scope entry={} view={} hold_typing={} own_session={} page_task={}; content=redacted",
                                request.entry, request.view, request.hold_typing, yours,
                                matches!(&request.step, WorkStepKindV1::Read { goal: Some(_), .. }));
                            let key = match key {
                                Some(key) => key,
                                None => tokio::task::spawn_blocking(
                                    zephium_agentic::load_probe_openai_credential,
                                )
                                .await
                                .map_err(|_| WorkError::Unavailable)?
                                .map_err(|_| WorkError::Unavailable)?,
                            };
                            let mut settings = browser_settings(self.binding, key);
                            if let Some(chosen) = &self.chosen {
                                settings.access =
                                    zephium_agent_controller::AgentBrowserModelAccess::Chosen(
                                        chosen.clone(),
                                    );
                            }
                            // The person's pages are never retained at the provider.
                            settings.retain_public_responses = false;
                            settings.loopback_anonymous = true;
                            let outcome = self
                                .composition
                                .run_agent_step(callback, &probe, request, settings)
                                .await;
                            if let Ok(mut opened) = opened.lock() {
                                opened.push(Opened {
                                    yours,
                                    not_ready: outcome.as_ref().is_ok_and(|o| {
                                        o.note.as_deref()
                                            == Some("The browser was not ready for this page")
                                    }),
                                    held_back: outcome.as_ref().is_ok_and(|o| o.held_back),
                                    status: outcome.as_ref().ok().map(|o| o.status),
                                    intervention: outcome
                                        .as_ref()
                                        .ok()
                                        .and_then(|o| o.intervention.as_ref())
                                        .map(|i| i.kind),
                                    model_calls: outcome
                                        .as_ref()
                                        .ok()
                                        .and_then(|o| o.measurements)
                                        .map(|m| m.planner_calls),
                                    resolved_handoff: outcome
                                        .as_ref()
                                        .ok()
                                        .and_then(|o| o.measurements)
                                        .and_then(|m| m.resolved_handoff),
                                });
                            }
                            report(&outcome);
                            outcome
                        }
                    },
                    |_| {},
                )
                .await;
            done.store(true, Ordering::Relaxed);
            state
        };
        let (state, ()) = tokio::join!(
            run,
            self.person(work.0, answer, &done, &asked, &decisions, &confirmed)
        );
        let state = state.map_err(|_| "run")?;
        Ok(Run {
            state,
            notices: script.notices.lock().map(|n| n.clone()).unwrap_or_default(),
            opened: opened.into_inner().unwrap_or_default(),
            asked: asked.into_inner().unwrap_or_default(),
            confirmed: confirmed.into_inner().unwrap_or_default(),
        })
    }

    /// One lead run whose scripted lead starts one browser part per start
    /// page; only the page agents call a model. The probe answers with `answer`.
    async fn lead_run(
        &self,
        objective: &str,
        starts: &'static str,
        answer: &str,
    ) -> Result<Run, &'static str> {
        use zephium_app::work_lead::{LeadModel, WorkLeadModels};
        let entry = zephium_app::work_models::resolve_entry(
            self.profile,
            zephium_core::work::model::WorkModelRole::Lead,
        )
        .await
        .map_err(|_| "lead_model")?
        .0;
        let scripted = LeadModel {
            entry,
            client: Arc::new(super::acceptance::Scripted { start: starts }),
        };
        let models = WorkLeadModels {
            lead: scripted.clone(),
            page: scripted.clone(),
            light: scripted,
        };
        self.lead_run_with_models(objective, answer, models, Vec::new())
            .await
    }

    async fn live_send_run(&self, objective: &str) -> Result<Run, &'static str> {
        use zephium_app::work_lead::{LeadModel, WorkLeadModels};
        use zephium_core::work::model::WorkModelRole;
        let _ = zephium_app::work_models::view(self.profile).await;
        for role in [
            WorkModelRole::Lead,
            WorkModelRole::Page,
            WorkModelRole::Light,
        ] {
            zephium_app::work_models::choose(self.profile, role, Some("openai/gpt-6-luna"))
                .map_err(|_| "fixture_model")?;
        }
        let resolve = |role| async move {
            zephium_app::work_models::resolve_entry(self.profile, role)
                .await
                .map(|(entry, client)| LeadModel { entry, client })
                .map_err(|_| "fixture_model")
        };
        let models = WorkLeadModels {
            lead: resolve(WorkModelRole::Lead).await?,
            page: resolve(WorkModelRole::Page).await?,
            light: resolve(WorkModelRole::Light).await?,
        };
        self.lead_run_with_models(objective, "Allow", models, vec![(true, false)])
            .await
    }

    async fn lead_run_with_models(
        &self,
        objective: &str,
        answer: &str,
        models: zephium_app::work_lead::WorkLeadModels,
        decisions: Vec<(bool, bool)>,
    ) -> Result<Run, &'static str> {
        use zephium_app::work_lead::WorkLeadService;
        let work = self.create(objective).await?;
        let started = std::time::Instant::now();
        let opened = Mutex::new(Vec::<Opened>::new());
        let asked = Mutex::new(Vec::new());
        let decisions = Mutex::new(decisions);
        let confirmed = Mutex::new(Vec::new());
        let done = AtomicBool::new(false);
        let callback = self.handle.callback_handle();
        let service = WorkLeadService::new(self.handle.clone()).with_diagnostic(|event| {
            let _ = writeln!(std::io::stdout().lock(), "loopback-site: lead={event:?}");
        });
        let run = async {
            let state = service
                .run(
                    self.profile,
                    WorkCommandV1 {
                        version: 1,
                        work: work.0,
                        expected_revision: work.1,
                        command: WorkCommandId::generate(),
                        intent: WorkRuntimeIntent::BeginAgent {
                            grant: WorkAgentGrantV1 {
                                no_usage_charge: false,
                                lead: None,
                                provider: WorkSearchProvider::OpenAi,
                                model: PUBLIC_SEARCH_MODEL.into(),
                                max_turns: 10,
                                max_steps: 32,
                                browse_hops: 4,
                                folders: vec![],
                                accounts: vec![],
                                private: false,
                                skill: None,
                                search_fallback: None,
                                draft: None,
                            },
                            limits: WorkExecutionLimits {
                                model_tokens: 1_000_000,
                                cost_micro_usd: 3_000_000,
                                operations: 256,
                                timeout_seconds: 1_200,
                                max_workers: 4,
                            },
                        },
                    },
                    None,
                    models,
                    &NoSearch,
                    |probe, request| {
                        let key = self.keys.lock().ok().and_then(|mut keys| keys.pop());
                        let callback = &callback;
                        let opened = &opened;
                        async move {
                            let (request, yours) = self.local(request)?;
                            let WorkStepKindV1::Read { url, goal, .. } = &request.step else {
                                return Err(WorkError::Invalid);
                            };
                            let local = [
                                self.sites.account,
                                self.sites.other,
                                self.sites.third,
                                self.sites.board,
                            ]
                            .iter()
                            .any(|port| {
                                ["127.0.0.1", "localhost"]
                                    .iter()
                                    .any(|host| url.starts_with(&format!("http://{host}:{port}/")))
                            });
                            if !local {
                                return Err(WorkError::Invalid);
                            }
                            let _ = writeln!(
                                std::io::stdout().lock(),
                                "loopback-site: synthetic_assignment={goal:?}"
                            );
                            let key = match key {
                                Some(key) => key,
                                None => tokio::task::spawn_blocking(
                                    zephium_agentic::load_probe_openai_credential,
                                )
                                .await
                                .map_err(|_| WorkError::Unavailable)?
                                .map_err(|_| WorkError::Unavailable)?,
                            };
                            let mut settings = browser_settings(self.binding, key);
                            settings.retain_public_responses = false;
                            settings.loopback_anonymous = true;
                            let outcome = self
                                .composition
                                .run_agent_step(callback, &probe, request, settings)
                                .await;
                            if let Ok(mut opened) = opened.lock() {
                                opened.push(Opened {
                                    yours,
                                    not_ready: false,
                                    held_back: outcome.as_ref().is_ok_and(|o| o.held_back),
                                    status: outcome.as_ref().ok().map(|o| o.status),
                                    intervention: None,
                                    model_calls: outcome
                                        .as_ref()
                                        .ok()
                                        .and_then(|o| o.measurements)
                                        .map(|m| m.planner_calls),
                                    resolved_handoff: outcome
                                        .as_ref()
                                        .ok()
                                        .and_then(|o| o.measurements)
                                        .and_then(|m| m.resolved_handoff),
                                });
                            }
                            report(&outcome);
                            outcome
                        }
                    },
                    |_| {},
                )
                .await;
            done.store(true, Ordering::Relaxed);
            state
        };
        let (state, ()) = tokio::join!(
            run,
            self.person(work.0, answer, &done, &asked, &decisions, &confirmed)
        );
        let state = state.map_err(|_| "lead_run")?;
        if let Some(execution) = state.executions.last() {
            super::acceptance::run_row("apps", 1, execution, started.elapsed().as_millis());
            super::acceptance::part_rows(execution);
        }
        Ok(Run {
            state,
            notices: Vec::new(),
            opened: opened.into_inner().unwrap_or_default(),
            asked: asked.into_inner().unwrap_or_default(),
            confirmed: confirmed.into_inner().unwrap_or_default(),
        })
    }

    /// Points a contract request at its loopback site.
    fn local(
        &self,
        mut request: WorkAgentBrowseRequest,
    ) -> Result<(WorkAgentBrowseRequest, bool), WorkError> {
        let WorkStepKindV1::Read { url, .. } = &mut request.step else {
            return Err(WorkError::Invalid);
        };
        *url = self.sites.local(url);
        let yours = request.session != SiteSession::Private;
        Ok((request, yours))
    }
}

fn report(outcome: &Result<WorkBrowserOutcome, WorkError>) {
    match outcome {
        Ok(outcome) => {
            let _ = writeln!(
                std::io::stdout().lock(),
                "loopback-site: page status={:?} artifacts={} held_back={} rerun={} usage={} note={:?} calls={:?} tokens={:?} wall_ms={:?}",
                outcome.status,
                outcome.artifacts.len(),
                outcome.held_back,
                outcome.rerun,
                outcome.usage.is_some(),
                outcome.note,
                outcome.measurements.map(|m| m.planner_calls),
                outcome.measurements.map(|m| m.model_tokens),
                outcome.measurements.map(|m| m.wall_millis),
            );
        }
        Err(error) => {
            let _ = writeln!(
                std::io::stdout().lock(),
                "loopback-site: page error={error:?}"
            );
        }
    }
}

/// Each page's status and model calls, in the order they settled.
fn pages_of(run: &Run) -> String {
    run.opened
        .iter()
        .map(|page| format!("{:?}:{}", page.status, page.model_calls.unwrap_or(0)))
        .collect::<Vec<_>>()
        .join(",")
}

fn says(run: &Run, text: &str) -> bool {
    run.state.executions.iter().any(|execution| {
        execution
            .artifacts
            .iter()
            .any(|artifact| artifact.data.plain_text().contains(text))
    })
}

/// Each Confirm step's decision and status, in order.
fn confirms(run: &Run) -> Vec<(Option<WorkConfirmDecisionV1>, WorkStepStatus)> {
    run.state
        .executions
        .iter()
        .flat_map(|execution| &execution.steps)
        .filter_map(|step| match &step.kind {
            WorkStepKindV1::Confirm { confirm: held } => Some((held.decision, step.status)),
            _ => None,
        })
        .collect()
}

/// Every word appears in one artifact, for facts a page task may rephrase.
fn mentions(run: &Run, words: &[&str]) -> bool {
    run.state.executions.iter().any(|execution| {
        execution.artifacts.iter().any(|artifact| {
            let text = artifact.data.plain_text().to_lowercase();
            words.iter().all(|word| text.contains(word))
        })
    })
}

pub(super) async fn workflow(
    handle: &zephium_app::Handle,
    composition: &NativeWorkComposition,
    profile: ProfileId,
    binding: zephium_app::AgentWorkProfileBinding,
    keys: Vec<zephium_agentic::AgentProviderCredential>,
) -> Result<WorkflowResult, &'static str> {
    let checks = CHECKS.get().cloned().unwrap_or_else(|| ALL.to_vec());
    let chosen = if std::env::var_os("ZEPHIUM_PROBE_CHOSEN_PAGE_MODEL").is_some()
        || (checks == [Check::Journey]
            && std::env::var_os("ZEPHIUM_PROBE_JOURNEY_SCRIPTED").is_some())
    {
        use zephium_core::work::model::WorkModelRole;
        let _ = zephium_app::work_models::view(profile).await;
        zephium_app::work_models::choose(profile, WorkModelRole::Page, Some("openai/gpt-6-luna"))
            .map_err(|_| "page_model_choice")?;
        let (entry, client) = zephium_app::work_models::resolve_entry(profile, WorkModelRole::Page)
            .await
            .map_err(|_| "page_model")?;
        let client: Arc<dyn zephium_core::work::model::WorkModelClient> =
            if checks == [Check::Journey] {
                Arc::new(journey_fixture::Client {
                    inner: client,
                    calls: AtomicUsize::new(0),
                })
            } else {
                client
            };
        Some(
            zephium_agentic::AgentProviderModelClient::try_new(&entry, client)
                .map_err(|_| "page_model")?,
        )
    } else {
        None
    };
    let _ = writeln!(
        std::io::stdout().lock(),
        "loopback-site: page_transport={}",
        if chosen.is_some() {
            "chosen_luna"
        } else {
            "exact_luna"
        }
    );
    let context = Context {
        handle,
        composition,
        profile,
        binding,
        chosen,
        keys: Mutex::new(keys),
        sites: Sites::start()?,
        tab_sign_in: AtomicBool::new(false),
        decline_changed_followup: AtomicBool::new(false),
        human_fixture: Mutex::new(None),
    };
    let line = |text: String| {
        let _ = writeln!(std::io::stdout().lock(), "loopback-site: {text}");
    };
    // This standalone public form has no account/session dependency.
    if checks == [Check::Journey] {
        return journey_fixture::run(&context).await;
    }
    // Before any session the site opens as an empty session without a
    // question. With no cookies a site opens in Work's own store, so this
    // seed's cookie stays there; checks that need the person signed in sign
    // them in through a native profile visit (`media_fixture::seed_session`).
    let seed = context
        .run(
            "Open the account site",
            false,
            "Not now",
            vec![vec![browse("/seed", "Open the welcome page and report it")]],
        )
        .await?;
    let seeded = context.sites.hits(Site::Account, "/seed").len() == 1;
    line(format!(
        "seed asked={} yours={} sites_seen={seeded}",
        seed.asked.len(),
        seed.opened.iter().all(|page| page.yours)
    ));
    if !seeded
        || seed.opened.len() != 1
        || seed.opened.iter().any(|page| {
            page.status != Some(WorkStepStatus::Succeeded) || page.not_ready || page.held_back
        })
        || context.sites.posts() != 0
    {
        return Err("seed");
    }
    // An extra setup question still fails the whole qualification, but a
    // completed read should not hide the independent action/approval checks.
    let seed_failure = (!seed.asked.is_empty()).then_some("seed_question");
    if checks.iter().any(|check| {
        matches!(
            check,
            Check::Visual
                | Check::Media(
                    media_fixture::Scenario::Mixed
                        | media_fixture::Scenario::Canvas
                        | media_fixture::Scenario::Record
                )
        )
    }) {
        media_fixture::seed_session(&context).await?;
    }
    let mut failure = seed_failure;
    let mut last = seed.state;
    let goal = "Report what the page shows";
    for check in checks {
        let passed = match check {
            Check::Visual => {
                let run = context.run(
                    "Read the inventory chart",
                    false,
                    "Allow",
                    vec![vec![browse("/visual", "Read the values painted in the inventory bar chart. Report each color and its count, and which count is largest. Use a screenshot if the text view cannot show the chart.")]],
                ).await?;
                let correct = mentions(&run, &["red", "17", "blue", "43", "green", "29"]);
                let succeeded = run.opened.len() == 1
                    && run.opened[0].yours
                    && run.asked.len() == 1
                    && run.opened[0].status == Some(WorkStepStatus::Succeeded);
                line(format!(
                    "check=visual correct={correct} succeeded={succeeded} pages=[{}]",
                    pages_of(&run)
                ));
                last = run.state;
                correct && succeeded
            }
            Check::Media(scenario) => {
                let (run, passed) = media_fixture::run(&context, scenario).await?;
                last = run.state;
                passed
            }
            Check::Human(scenario) => {
                let (run, passed) = human_fixture::run(&context, scenario).await?;
                last = run.state;
                passed
            }
            Check::Session => {
                let run = context
                    .run(
                        "Summarize my inbox",
                        false,
                        "Allow",
                        vec![vec![browse("/inbox", "Summarize my inbox")]],
                    )
                    .await?;
                let inbox = context.sites.hits(Site::Account, "/inbox");
                let cookie =
                    !inbox.is_empty() && inbox.iter().all(|(_, c)| c.contains(ACCOUNT_COOKIE));
                let content = says(&run, INBOX_FACT);
                let badge = run.state.executions[0]
                    .steps
                    .iter()
                    .any(|step| step.account.is_some());
                line(format!(
                    "check=session asked={} yours={} session_cookie={cookie} content={content} badge={badge}",
                    run.asked.len(),
                    run.opened.iter().all(|p| p.yours)
                ));
                let passed = run.asked.len() == 1 && cookie && content && badge;
                last = run.state;
                passed
            }
            Check::Always => {
                let first = context
                    .run(
                        "Check my inbox",
                        false,
                        "Always for account.probe.test",
                        vec![vec![browse("/inbox", goal)]],
                    )
                    .await?;
                let second = context
                    .run(
                        "Check my inbox again",
                        false,
                        "Not now",
                        vec![vec![browse("/inbox", goal)]],
                    )
                    .await?;
                let standing = zephium_app::work_sites::standing(handle, profile)
                    .await
                    .unwrap_or_default();
                let kept = standing
                    .iter()
                    .any(|entry| entry.site == SITE && entry.access == WorkSiteAccessV1::Always);
                line(format!(
                    "check=always first_asked={} second_asked={} kept={kept} second_yours={}",
                    first.asked.len(),
                    second.asked.len(),
                    second.opened.iter().all(|p| p.yours)
                ));
                let passed = first.asked.len() == 1
                    && second.asked.is_empty()
                    && kept
                    && second.opened.iter().all(|p| p.yours);
                last = second.state;
                passed
            }
            Check::Never => {
                zephium_app::work_sites::set_standing(
                    handle,
                    profile,
                    SITE.into(),
                    Some(WorkSiteAccessV1::Never),
                )
                .await
                .map_err(|_| "never_standing")?;
                let before = context.sites.hits(Site::Account, "/inbox").len();
                let never = context
                    .run(
                        "Inbox, never",
                        false,
                        "Allow",
                        vec![vec![browse("/inbox", goal)]],
                    )
                    .await?;
                zephium_app::work_sites::set_standing(handle, profile, SITE.into(), None)
                    .await
                    .map_err(|_| "never_standing")?;
                let private = context
                    .run(
                        "Inbox, privately",
                        true,
                        "Allow",
                        vec![vec![browse("/inbox", goal)]],
                    )
                    .await?;
                let inbox = context.sites.hits(Site::Account, "/inbox");
                let cookies = inbox[before..]
                    .iter()
                    .filter(|(_, c)| !c.is_empty())
                    .count();
                line(format!(
                    "check=never never_asked={} private_asked={} requests={} with_cookie={cookies}",
                    never.asked.len(),
                    private.asked.len(),
                    inbox.len() - before
                ));
                let passed = never.asked.is_empty()
                    && private.asked.is_empty()
                    && never.opened.iter().chain(&private.opened).all(|p| !p.yours)
                    && cookies == 0;
                last = private.state;
                passed
            }
            Check::Route => {
                let run = context
                    .run(
                        "Reports",
                        false,
                        "Allow",
                        vec![vec![browse(
                            "/app",
                            "Open the Reports view and report its total",
                        )]],
                    )
                    .await?;
                let loads = context.sites.hits(Site::Account, "/app/reports").len();
                let content = says(&run, REPORT_FACT);
                line(format!(
                    "check=route content={content} route_loads={loads} status={:?}",
                    run.opened.first().and_then(|p| p.status)
                ));
                last = run.state;
                content && loads == 0
            }
            Check::Redirect => {
                let run = context
                    .run(
                        "Landing",
                        false,
                        "Allow",
                        vec![vec![browse("/start", goal)]],
                    )
                    .await?;
                let landed = context.sites.hits(Site::Account, "/landed");
                let content = says(&run, LANDED_FACT);
                line(format!(
                    "check=redirect content={content} landed_requests={} landed_cookie={}",
                    landed.len(),
                    landed.iter().all(|(_, c)| c.contains(ACCOUNT_COOKIE))
                ));
                last = run.state;
                content && !landed.is_empty()
            }
            Check::Leave => {
                let run = context
                    .run("Leave", false, "Allow", vec![vec![browse("/leave", goal)]])
                    .await?;
                let landing = context.sites.hits(Site::Other, "/landing");
                let status = run.opened.first().and_then(|p| p.status);
                line(format!(
                    "check=leave status={status:?} landing_requests={} landing_cookie={}",
                    landing.len(),
                    landing.iter().any(|(_, c)| !c.is_empty())
                ));
                last = run.state;
                status != Some(WorkStepStatus::Succeeded)
                    && landing.iter().all(|(_, c)| c.is_empty())
            }
            Check::Autopost => {
                let before = context.sites.posts();
                let run = context
                    .run(
                        "Drafts",
                        false,
                        "Allow",
                        vec![vec![browse(
                            "/autopost",
                            "Wait a moment, then quote the visible sentence stating the drafts count verbatim",
                        )]],
                    )
                    .await?;
                let posts = context.sites.posts() - before;
                let content = says(&run, AUTOPOST_FACT);
                let drafts_term = mentions(&run, &["draft"]);
                let word_three = mentions(&run, &["three"]);
                let numeric_three = mentions(&run, &["3"]);
                line(format!(
                    "check=autopost content={content} drafts_term={drafts_term} word_three={word_three} numeric_three={numeric_three} posts={posts}"
                ));
                last = run.state;
                content && posts == 0
            }
            Check::Held => {
                let before = context.sites.posts();
                let run = context
                    .run(
                        "Send",
                        false,
                        "Allow",
                        vec![vec![browse(
                            "/compose",
                            "Write 'On my way' in the message box, then press Send",
                        )]],
                    )
                    .await?;
                let posts = context.sites.posts() - before;
                let held = run.opened.iter().any(|p| p.held_back);
                let noticed = run
                    .notices
                    .iter()
                    .any(|n| n.starts_with("A browse stopped before a step"));
                line(format!(
                    "check=held held={held} noticed={noticed} posts={posts}"
                ));
                last = run.state;
                // The page agent may stop before Send on its own; if it tried,
                // the gate held it and the lead heard. Nothing is ever sent.
                held == noticed && posts == 0
            }
            Check::Journey => {
                let result = journey_fixture::run(&context).await?;
                let passed = result.failure.is_none();
                last = result.state;
                passed
            }
            Check::Search => {
                let run = context
                    .run(
                        "Catalog",
                        false,
                        "Allow",
                        vec![vec![
                            browse(
                                "/find",
                                "Search the catalog for lantern and report the stock",
                            ),
                            browse(
                                "/filters",
                                "Show the open orders and report how many there are",
                            ),
                        ]],
                    )
                    .await?;
                let queried = context.sites.hits(Site::Account, "/results?q=");
                let filtered = context.sites.hits(Site::Account, "/results?status=open");
                let found = says(&run, SEARCH_FACT);
                let filters = says(&run, FILTER_FACT) || mentions(&run, &["open orders", "7"]);
                let held = run.opened.iter().any(|p| p.held_back);
                line(format!(
                    "check=search asked={} queried={} filtered={} found={found} filters={filters} held={held} cookie={}",
                    run.asked.len(),
                    queried.len(),
                    filtered.len(),
                    queried
                        .iter()
                        .chain(&filtered)
                        .all(|(_, c)| c.contains(ACCOUNT_COOKIE)),
                ));
                last = run.state;
                run.asked.len() <= 1 && found && filters && !held && !queried.is_empty()
            }
            Check::Approve => {
                let before = context.sites.hits(Site::Account, "/send").len();
                let run = context
                    .run_deciding(
                        "Tell #design I'm on my way",
                        false,
                        "Allow",
                        vec![vec![browse(
                            "/compose",
                            "Write 'On my way' in the message box and send it",
                        )]],
                        vec![(true, false)],
                    )
                    .await?;
                let sent = context.sites.hits(Site::Account, "/send").len() - before;
                let held = confirms(&run);
                line(format!(
                    "check=approve sent={sent} confirms={held:?} headline={:?}",
                    run.confirmed.first()
                ));
                last = run.state;
                sent == 1
                    && held.len() == 1
                    && held[0]
                        == (
                            Some(WorkConfirmDecisionV1::Approved),
                            WorkStepStatus::Succeeded,
                        )
                    && run.confirmed.first().is_some_and(|h| h.contains("#design"))
            }
            Check::SlackRequest => {
                let expected = "Zephium Work qualification: approved send and extraction test.";
                let before = context.sites.written("/edit/slack/send").len();
                let request = slack_send_request(&format!("{ACCOUNT}/edit/slack"), expected);
                let objective = request.lines().next().ok_or("fixture_request")?;
                let run = context.live_send_run(objective).await?;
                let sent = context.sites.written("/edit/slack/send");
                let sent = &sent[before.min(sent.len())..];
                let held = confirms(&run);
                let extracted = run
                    .opened
                    .iter()
                    .any(|page| page.status == Some(WorkStepStatus::Succeeded))
                    && run.state.executions.iter().any(|execution| {
                        execution
                            .artifacts
                            .iter()
                            .any(|artifact| !artifact.evidence.is_empty())
                    });
                let passed = sent.len() == 1
                    && sent[0] == expected
                    && extracted
                    && held
                        == vec![(
                            Some(WorkConfirmDecisionV1::Approved),
                            WorkStepStatus::Succeeded,
                        )];
                line(format!("check=slack_request sent={} exact={} extracted={extracted} confirms={held:?} pages=[{}] passed={passed}", sent.len(), sent.first().is_some_and(|text| text == expected), pages_of(&run)));
                last = run.state;
                passed
            }
            Check::SlackWrite | Check::SlackEnter | Check::SlackDecline => {
                let before = context.sites.written("/edit/slack/send").len();
                let goal = if check == Check::SlackEnter {
                    "Type 'On my way, ten minutes out' in the message box and press Enter to send it"
                } else {
                    "Post the message 'On my way, ten minutes out' in this channel"
                };
                let run = context
                    .run_deciding(
                        "Tell #design I'm on my way",
                        false,
                        "Allow",
                        vec![vec![browse(
                            if check == Check::SlackEnter {
                                "/edit/slack-enter"
                            } else {
                                "/edit/slack"
                            },
                            goal,
                        )]],
                        vec![(check != Check::SlackDecline, false)],
                    )
                    .await?;
                let sent = context.sites.written("/edit/slack/send");
                let sent = &sent[before.min(sent.len())..];
                let held = confirms(&run);
                let extracted = run.opened.len() == 1
                    && run.opened[0].status == Some(WorkStepStatus::Succeeded)
                    && run.state.executions.iter().any(|execution| {
                        execution
                            .artifacts
                            .iter()
                            .any(|artifact| !artifact.evidence.is_empty())
                    });
                line(format!(
                    "check=slackwrite sent={} exact={} extracted={extracted} confirms={held:?} pages=[{}]",
                    sent.len(),
                    sent.first()
                        .is_some_and(|t| t == "On my way, ten minutes out"),
                    pages_of(&run)
                ));
                last = run.state;
                if check == Check::SlackDecline {
                    sent.is_empty()
                        && held
                            == vec![(
                                Some(WorkConfirmDecisionV1::Declined),
                                WorkStepStatus::Cancelled,
                            )]
                } else {
                    sent.len() == 1
                        && sent[0] == "On my way, ten minutes out"
                        && extracted
                        && held.len() == 1
                        && held.first()
                            == Some(&(
                                Some(WorkConfirmDecisionV1::Approved),
                                WorkStepStatus::Succeeded,
                            ))
                }
            }
            Check::LinearWrite => {
                let before = context.sites.written("/edit/linear/issue").len();
                let run = context
                    .run_deciding(
                        "File the login bug in Linear",
                        false,
                        "Allow",
                        vec![vec![browse(
                            "/edit/linear",
                            "Create a new issue in the Engineering team titled 'Login fails on Safari' with the description 'Users on Safari 18 see a blank page after sign-in'",
                        )]],
                        vec![(true, false)],
                    )
                    .await?;
                let made = context.sites.written("/edit/linear/issue");
                let made = &made[before.min(made.len())..];
                let expected = "Login fails on Safari|Users on Safari 18 see a blank page after sign-in|Engineering";
                line(format!(
                    "check=linearwrite created={} exact={} parts={:?} confirms={:?} pages=[{}]",
                    made.len(),
                    made.first().is_some_and(|t| t == expected),
                    made.first()
                        .map(|t| t.split('|').map(|p| !p.is_empty()).collect::<Vec<_>>()),
                    confirms(&run),
                    pages_of(&run)
                ));
                let approved_once = confirms(&run)
                    == vec![(
                        Some(WorkConfirmDecisionV1::Approved),
                        WorkStepStatus::Succeeded,
                    )];
                last = run.state;
                made.len() == 1 && made[0] == expected && approved_once
            }
            Check::WebmailCompose
            | Check::WebmailSwap
            | Check::WebmailAutosave
            | Check::WebmailInbox
            | Check::WebmailBlocks
            | Check::WebmailThread => {
                let before = context.sites.written("/edit/webmail/send").len();
                let replies = context.sites.written("/edit/webmail/reply").len();
                let run = context
                    .run_deciding(
                        "Send Dana the status",
                        false,
                        "Allow",
                        vec![vec![browse(
                            match check {
                                Check::WebmailSwap => "/edit/webmail-swap",
                                Check::WebmailAutosave => "/edit/webmail-autosave",
                                Check::WebmailInbox => "/edit/webmail-inbox",
                                Check::WebmailBlocks => "/edit/webmail-blocks",
                                Check::WebmailThread => "/edit/webmail-thread",
                                _ => "/edit/webmail",
                            },
                            if check == Check::WebmailThread {
                                WEBMAIL_THREAD_GOAL
                            } else {
                                WEBMAIL_GOAL
                            },
                        )]],
                        vec![(true, false)],
                    )
                    .await?;
                let sent = context.sites.written("/edit/webmail/send");
                let sent = &sent[before.min(sent.len())..];
                // The blocks body also reports how often it was typed into:
                // once per line, so a single fill typed all of it.
                let expected = if check == Check::WebmailBlocks {
                    format!("{WEBMAIL_SENT}|3")
                } else {
                    WEBMAIL_SENT.to_owned()
                };
                let held = confirms(&run);
                let replied = context.sites.written("/edit/webmail/reply").len() - replies;
                if let Some(actual) = sent.first().filter(|actual| **actual != expected) {
                    let difference = expected
                        .chars()
                        .zip(actual.chars())
                        .enumerate()
                        .find(|(_, (left, right))| left != right)
                        .map(|(index, (left, right))| (index, u32::from(left), u32::from(right)));
                    line(format!(
                        "check=webmail_text expected_bytes={} actual_bytes={} expected_lines={} actual_lines={} crlf_only={} literal_linebreaks={} first_difference={difference:?}; content=synthetic_redacted",
                        expected.len(), actual.len(), expected.lines().count(), actual.lines().count(),
                        actual.replace("\r\n", "\n") == expected, actual.contains("\\n")
                    ));
                }
                line(format!(
                    "check=webmail_compose sent={} replied={replied} exact={} confirms={held:?} headline={:?} pages=[{}]",
                    sent.len(),
                    sent.first().is_some_and(|text| *text == expected),
                    run.confirmed.first(),
                    pages_of(&run)
                ));
                last = run.state;
                sent.len() == 1
                    && replied == 0
                    && sent[0] == expected
                    && held
                        == vec![(
                            Some(WorkConfirmDecisionV1::Approved),
                            WorkStepStatus::Succeeded,
                        )]
            }
            Check::WebmailRoutes => {
                let path = super::work_app_views::ROUTED_PATH;
                let before = context.sites.hits(Site::Account, path).len();
                let run = context
                    .run(
                        "My drafts",
                        false,
                        "Allow",
                        vec![vec![browse(
                            path,
                            "Click the Drafts button and note the draft. Then use the Navigate tool (not a click) to open the Inbox link, and report the first message in the inbox.",
                        )]],
                    )
                    .await?;
                let read = says(&run, ROUTES_FACT) || mentions(&run, &["quarterly"]);
                let loads = context.sites.hits(Site::Account, path).len() - before;
                line(format!(
                    "check=webmail_routes read={read} loads={loads} pages=[{}]",
                    pages_of(&run)
                ));
                last = run.state;
                read && loads == 2
            }
            Check::NotionWrite => {
                let before = context.sites.written("/edit/notion/save").len();
                let run = context
                    .run_deciding(
                        "Add the launch date to my notes page",
                        false,
                        "Allow",
                        vec![vec![browse(
                            "/edit/notion",
                            "Add a line 'Launch moves to 14 October' to this page",
                        )]],
                        vec![(true, true)],
                    )
                    .await?;
                let saved = context.sites.written("/edit/notion/save");
                let saved = &saved[before.min(saved.len())..];
                line(format!(
                    "check=notionwrite saves={} exact={} confirms={:?} pages=[{}]",
                    saved.len(),
                    saved
                        .last()
                        .is_some_and(|t| t.contains("Launch moves to 14 October")),
                    confirms(&run),
                    pages_of(&run)
                ));
                let approved_once = confirms(&run)
                    == vec![(
                        Some(WorkConfirmDecisionV1::AllowedForRun),
                        WorkStepStatus::Succeeded,
                    )];
                last = run.state;
                approved_once
                    && saved
                        .last()
                        .is_some_and(|t| t.trim() == "Launch moves to 14 October")
            }
            Check::Changed => {
                context
                    .decline_changed_followup
                    .store(true, Ordering::Relaxed);
                let before = context.sites.posts();
                let run = context
                    .run_deciding(
                        "Book the cabin",
                        false,
                        "Allow",
                        vec![vec![browse(
                            "/book/live",
                            "Request to book the cabin for these dates",
                        )]],
                        vec![(true, false)],
                    )
                    .await?;
                context
                    .decline_changed_followup
                    .store(false, Ordering::Relaxed);
                let posts = context.sites.posts() - before;
                let held = confirms(&run);
                line(format!(
                    "check=changed posts={posts} confirms={held:?} headlines={:?}",
                    run.confirmed
                ));
                last = run.state;
                // The approved total moved on before the step ran: nothing is
                // posted; a new total would be put to the person again.
                posts == 0
                    && held.first()
                        == Some(&(
                            Some(WorkConfirmDecisionV1::Approved),
                            WorkStepStatus::Failed,
                        ))
            }
            Check::Decline => {
                let before = context.sites.posts();
                let run = context
                    .run_deciding(
                        "Book the cabin",
                        false,
                        "Allow",
                        vec![vec![browse(
                            "/book",
                            "Request to book the cabin for these dates",
                        )]],
                        vec![(false, false)],
                    )
                    .await?;
                let posts = context.sites.posts() - before;
                let held = confirms(&run);
                line(format!(
                    "check=decline posts={posts} confirms={held:?} headlines={:?}",
                    run.confirmed
                ));
                last = run.state;
                posts == 0
                    && held.first()
                        == Some(&(
                            Some(WorkConfirmDecisionV1::Declined),
                            WorkStepStatus::Cancelled,
                        ))
                    && held.len() == 1
            }
            Check::Autosave => {
                let before = context.sites.hits(Site::Account, "/save").len();
                let run = context
                    .run_deciding(
                        "Meeting notes",
                        false,
                        "Allow",
                        vec![vec![browse(
                            "/notes",
                            "Type 'Agenda: launch' into the notes, then replace it with 'Agenda: launch. Owner: Dana'",
                        )]],
                        vec![(true, true)],
                    )
                    .await?;
                let saves = context.sites.hits(Site::Account, "/save").len() - before;
                let held = confirms(&run);
                line(format!("check=autosave saves={saves} confirms={held:?}"));
                last = run.state;
                saves >= 1
                    && held.len() == 1
                    && held[0]
                        == (
                            Some(WorkConfirmDecisionV1::AllowedForRun),
                            WorkStepStatus::Succeeded,
                        )
            }
            Check::SignedOut => {
                let run = context
                    .run(
                        "Offers",
                        false,
                        "Not now",
                        vec![vec![browse(
                            "/home",
                            "Report how many offers the front page lists",
                        )]],
                    )
                    .await?;
                let content = says(&run, HOME_FACT) || mentions(&run, &["five offers"]);
                line(format!(
                    "check=signedout asked={} content={content} yours={}",
                    run.asked.len(),
                    run.opened.iter().all(|p| p.yours)
                ));
                last = run.state;
                run.asked.is_empty() && content
            }
            Check::TabSignIn => {
                VAULT_OPEN.store(false, Ordering::SeqCst);
                context.tab_sign_in.store(true, Ordering::Relaxed);
                let run = context
                    .run(
                        "Vault",
                        false,
                        "Allow",
                        vec![vec![browse("/vault", "Report the vault balance")]],
                    )
                    .await;
                context.tab_sign_in.store(false, Ordering::Relaxed);
                let run = run?;
                let loads = context.sites.hits(Site::Account, "/vault").len();
                let content = says(&run, VAULT_FACT) || mentions(&run, &["318"]);
                line(format!(
                    "check=tabsignin asked={} vault_loads={loads} content={content} pages={}",
                    run.asked.len(),
                    run.opened.len()
                ));
                last = run.state;
                content && loads >= 2 && run.asked.is_empty()
            }
            Check::Back => {
                let run = context
                    .run(
                        "Shelf",
                        false,
                        "Allow",
                        vec![vec![browse(
                            "/shelf",
                            "Open the Kettle product and note its price, then press Back to return to the shelf and report the shelf total and the kettle price",
                        )]],
                    )
                    .await?;
                let read = says(&run, SHELF_FACT) || mentions(&run, &["9 products"]);
                let price = says(&run, KETTLE_FACT) || mentions(&run, &["37"]);
                line(format!(
                    "check=back read={read} price={price} shelf_loads={} pages=[{}]",
                    context.sites.hits(Site::Account, "/shelf").len()
                        - context.sites.hits(Site::Account, "/shelf/").len(),
                    pages_of(&run)
                ));
                last = run.state;
                read && price
            }
            Check::Reloaded => {
                let run = context
                    .run(
                        "Desk",
                        false,
                        "Allow",
                        vec![vec![browse(
                            "/lobby",
                            "Open the Front desk link and report what the desk page says",
                        )]],
                    )
                    .await?;
                let read = says(&run, DESK_FACT) || mentions(&run, &["bell"]);
                line(format!(
                    "check=reloaded read={read} lobby_loads={} pages=[{}]",
                    context.sites.hits(Site::Account, "/lobby").len()
                        - context.sites.hits(Site::Account, "/lobby/").len(),
                    pages_of(&run)
                ));
                last = run.state;
                read
            }
            Check::Detour => {
                let run = context
                    .run(
                        "Hours",
                        false,
                        "Allow",
                        vec![vec![browse(
                            "/hours",
                            "Open the Opening hours link and report the opening hours. If that page does not open, the Contact page lists them too",
                        )]],
                    )
                    .await?;
                let read = says(&run, HOURS_FACT) || mentions(&run, &["half past eight"]);
                let tried = context.sites.hits(Site::Account, "/hours/open").len();
                line(format!(
                    "check=detour read={read} tried={tried} pages=[{}]",
                    pages_of(&run)
                ));
                last = run.state;
                read && tried >= 1
            }
            Check::Spa => {
                let run = context
                    .run(
                        "Read the design channel",
                        false,
                        "Allow",
                        vec![vec![browse(
                            "/spa",
                            "Open the #design channel and report its newest message",
                        )]],
                    )
                    .await?;
                let read = says(&run, DESIGN_FACT) || mentions(&run, &["onboarding"]);
                line(format!("check=spa read={read} pages=[{}]", pages_of(&run)));
                last = run.state;
                read
            }
            Check::Heavy => {
                let run = context
                    .run(
                        "My day",
                        false,
                        "Allow",
                        vec![vec![browse(
                            "/agenda",
                            "Report today's events with their times",
                        )]],
                    )
                    .await?;
                let read = says(&run, AGENDA_FACT) || mentions(&run, &["14:30"]);
                line(format!(
                    "check=heavy read={read} pages=[{}]",
                    pages_of(&run)
                ));
                last = run.state;
                read
            }
            Check::Webmail => {
                let run = context
                    .run(
                        "My mail",
                        false,
                        "Allow",
                        vec![vec![browse(
                            load_fixture::START,
                            "Report the newest message in the inbox",
                        )]],
                    )
                    .await?;
                let read = says(&run, INBOX_FACT) || mentions(&run, &["Quarterly"]);
                line(format!(
                    "check=webmail read={read} pages=[{}]",
                    pages_of(&run)
                ));
                last = run.state;
                read
            }
            Check::Apps => {
                // macOS cookies are shared by host, so one native profile
                // visit to the seed page signs the person in on all three
                // apps (they share 127.0.0.1) in the profile's own store.
                #[cfg(target_os = "macos")]
                media_fixture::seed_session(&context).await?;
                // Windows Work stores isolate origins including their ports.
                // The initial Account seed was a Work view, so its cookie is
                // not in the ordinary Browse profile for cross-origin import.
                // Sign in each other app through its real HTTP response before
                // resetting decisions and testing the combined entry question.
                #[cfg(target_os = "windows")]
                for (site, fixture) in [(BOARD, Site::Board), (THIRD, Site::Third)] {
                    let before = context.sites.hits(fixture, "/seed").len();
                    let seed = context
                        .run(
                            "Open the app welcome page",
                            false,
                            "Allow",
                            vec![vec![browse_at(
                                site,
                                "/seed",
                                "Open the welcome page and report it",
                            )]],
                        )
                        .await?;
                    let seeded = context.sites.hits(fixture, "/seed").len() == before + 1
                        && seed.opened.len() == 1
                        && seed.opened.iter().all(|page| {
                            page.yours && page.status == Some(WorkStepStatus::Succeeded)
                        });
                    line(format!("apps_seed native_origin_auth_admitted={seeded}"));
                    if !seeded {
                        return Err("apps_seed");
                    }
                }
                for site in [SITE, BOARD_SITE, THIRD_SITE] {
                    zephium_app::work_sites::set_standing(handle, profile, site.into(), None)
                        .await
                        .map_err(|_| "apps_standing")?;
                }
                let before = (
                    context.sites.hits(Site::Account, "/inbox").len(),
                    context.sites.hits(Site::Board, "/board").len(),
                    context.sites.hits(Site::Third, "/day").len(),
                );
                let run = context
                    .lead_run(
                        "What do I need to do today?",
                        "https://account.probe.test/inbox https://board.probe.test/board https://third.probe.test/day",
                        "Allow",
                    )
                    .await?;
                let with = |site: Site, path: &str, skip: usize, cookie: &str| {
                    let hits = context.sites.hits(site, path);
                    hits.len() > skip && hits[skip..].iter().all(|(_, c)| c.contains(cookie))
                };
                let cookies = with(Site::Account, "/inbox", before.0, ACCOUNT_COOKIE)
                    && with(Site::Board, "/board", before.1, ACCOUNT_COOKIE)
                    && with(Site::Third, "/day", before.2, ACCOUNT_COOKIE);
                let named = run.asked.first().is_some_and(|question| {
                    [SITE, BOARD_SITE, THIRD_SITE]
                        .iter()
                        .all(|site| question.contains(site))
                });
                let read = (says(&run, INBOX_FACT) || mentions(&run, &["quarterly"]))
                    && (says(&run, BOARD_FACT) || mentions(&run, &["zp-42"]))
                    && (says(&run, DAY_FACT) || mentions(&run, &["09:15"]));
                line(format!(
                    "check=apps asked={} named={named} yours={} cookies={cookies} read={read} pages=[{}]",
                    run.asked.len(),
                    run.opened.iter().all(|p| p.yours),
                    pages_of(&run)
                ));
                last = run.state;
                run.asked.len() == 1 && named && cookies && read
            }
            Check::Views => {
                #[cfg(target_os = "macos")]
                media_fixture::seed_session(&context).await?;
                zephium_app::work_sites::set_standing(handle, profile, SITE.into(), None)
                    .await
                    .map_err(|_| "views_standing")?;
                let starts: &'static str = Box::leak(
                    super::work_app_views::VIEWS
                        .iter()
                        .map(|view| format!("{ACCOUNT}{}", view.path))
                        .collect::<Vec<_>>()
                        .join(" ")
                        .into_boxed_str(),
                );
                let run = context
                    .lead_run("What's new in my apps today?", starts, "Allow")
                    .await?;
                let read: Vec<bool> = super::work_app_views::VIEWS
                    .iter()
                    .map(|view| says(&run, view.fact))
                    .collect();
                let calls: u32 = run
                    .opened
                    .iter()
                    .filter_map(|p| p.model_calls)
                    .map(u32::from)
                    .sum();
                line(format!(
                    "check=views asked={} read={read:?} model_calls={calls} pages=[{}]",
                    run.asked.len(),
                    pages_of(&run)
                ));
                last = run.state;
                run.asked.len() == 1
                    && read.iter().all(|read| *read)
                    && run.opened.len() == super::work_app_views::VIEWS.len()
                    && run.opened.iter().all(|page| page.model_calls == Some(0))
                    && calls == 0
            }
            Check::SignedOutApp => {
                zephium_app::work_sites::set_standing(handle, profile, SITE.into(), None)
                    .await
                    .map_err(|_| "signed_out_app_standing")?;
                let run = context
                    .lead_run(
                        "List my 3 most recent pages",
                        "https://account.probe.test/app/landing",
                        "Allow",
                    )
                    .await?;
                let execution = run.state.executions.last();
                let sign_in = execution.is_some_and(|execution| {
                    execution.parts.iter().any(|part| {
                        matches!(
                            part.need,
                            Some(zephium_core::work::parts::WorkPartNeedV1::SignIn { .. })
                        )
                    })
                });
                let read = says(&run, LANDING_FACT);
                let calls: u32 = run
                    .opened
                    .iter()
                    .filter_map(|page| page.model_calls)
                    .map(u32::from)
                    .sum();
                line(format!(
                    "check=signed-out-app sign_in_need={sign_in} read_landing_page={read} model_calls={calls} pages=[{}]",
                    pages_of(&run)
                ));
                last = run.state;
                sign_in && !read && calls == 0
            }
            Check::Churn => {
                let first = context
                    .run(
                        "Churn list",
                        false,
                        "Allow",
                        vec![
                            vec![browse(
                                "/churn",
                                "Press Continue, wait for the list, then report how many items it holds",
                            )],
                            vec![browse("/inbox", goal)],
                        ],
                    )
                    .await?;
                let second = context
                    .run(
                        "Inbox after",
                        false,
                        "Allow",
                        vec![vec![browse("/inbox", goal)]],
                    )
                    .await?;
                let pages = |run: &Run| {
                    run.opened
                        .iter()
                        .map(|p| {
                            format!(
                                "{:?}{}",
                                p.status,
                                if p.not_ready { ":not_ready" } else { "" }
                            )
                        })
                        .collect::<Vec<_>>()
                        .join(",")
                };
                let churned = says(&first, CHURN_FACT) || mentions(&first, &["12"]);
                line(format!(
                    "check=churn churned={churned} first=[{}] second=[{}] inbox={}",
                    pages(&first),
                    pages(&second),
                    says(&second, INBOX_FACT)
                ));
                let passed = first
                    .opened
                    .iter()
                    .chain(&second.opened)
                    .all(|p| !p.not_ready)
                    && says(&second, INBOX_FACT);
                last = second.state;
                passed
            }
            Check::Freeze => {
                let started = std::time::Instant::now();
                let first = context
                    .run(
                        "Frozen report",
                        false,
                        "Allow",
                        vec![
                            vec![browse(
                                "/freeze",
                                "Press Load, then report what the report says",
                            )],
                            vec![browse("/inbox", goal)],
                        ],
                    )
                    .await?;
                let first_ms = started.elapsed().as_millis();
                let started = std::time::Instant::now();
                let second = context
                    .run(
                        "Inbox after freeze",
                        false,
                        "Allow",
                        vec![vec![browse("/inbox", goal)]],
                    )
                    .await?;
                let pages = |run: &Run| {
                    run.opened
                        .iter()
                        .map(|p| {
                            format!(
                                "{:?}{}",
                                p.status,
                                if p.not_ready { ":not_ready" } else { "" }
                            )
                        })
                        .collect::<Vec<_>>()
                        .join(",")
                };
                line(format!(
                    "check=freeze first=[{}] first_ms={first_ms} second=[{}] second_ms={} inbox={}",
                    pages(&first),
                    pages(&second),
                    started.elapsed().as_millis(),
                    says(&second, INBOX_FACT)
                ));
                let passed = first
                    .opened
                    .iter()
                    .chain(&second.opened)
                    .all(|p| !p.not_ready)
                    && says(&first, INBOX_FACT)
                    && says(&second, INBOX_FACT);
                last = second.state;
                passed
            }
            Check::Consent => {
                let mut passed = true;
                for path in ["/gconsent", "/jconsent"] {
                    let before = context.sites.posts();
                    let run = context
                        .run(
                            "Fares",
                            false,
                            "Allow",
                            vec![vec![browse(
                                path,
                                "Report how many flights the fares list shows",
                            )]],
                        )
                        .await?;
                    let saves = context.sites.hits(Site::Account, "/consent/").len();
                    let loads = context
                        .sites
                        .hits(Site::Account, path)
                        .iter()
                        .map(|(_, cookie)| {
                            (
                                cookie.contains(ACCOUNT_COOKIE),
                                cookie.contains("consent=no"),
                            )
                        })
                        .collect::<Vec<_>>();
                    line(format!("consent loads={loads:?}"));
                    let content = says(&run, CONSENT_FACT) || mentions(&run, &["4"]);
                    let held = run.opened.iter().any(|p| p.held_back);
                    line(format!(
                        "check=consent page={path} content={content} posts={} saves={saves} held={held} status={:?}",
                        context.sites.posts() - before,
                        run.opened.first().and_then(|p| p.status)
                    ));
                    passed &= content && !held;
                    last = run.state;
                }
                passed
            }
            Check::Memory => {
                let goal = "Report how many flights the fares list shows";
                // Never keeps the site out of the person's session: its pages
                // open in Work's memory, as a public site's would.
                zephium_app::work_sites::set_standing(
                    handle,
                    profile,
                    OTHER_SITE.into(),
                    Some(WorkSiteAccessV1::Never),
                )
                .await
                .map_err(|_| "memory_standing")?;
                let before = stores_hold(profile, "localhost").ok_or("memory_stores")?;
                let first = context
                    .run(
                        "Fares, first",
                        false,
                        "Allow",
                        vec![vec![browse_at(OTHER, "/memory", goal)]],
                    )
                    .await?;
                let between = stores_hold(profile, "localhost").ok_or("memory_stores")?;
                let loads_before = context.sites.hits(Site::Other, "/memory").len();
                let second = context
                    .run(
                        "Fares, again",
                        false,
                        "Allow",
                        vec![vec![browse_at(OTHER, "/memory", goal)]],
                    )
                    .await?;
                zephium_app::work_sites::set_standing(handle, profile, OTHER_SITE.into(), None)
                    .await
                    .map_err(|_| "memory_standing")?;
                let after = stores_hold(profile, "localhost").ok_or("memory_stores")?;
                let loads = context
                    .sites
                    .hits(Site::Other, "/memory")
                    .into_iter()
                    .filter(|(post, _)| !post)
                    .map(|(_, cookie)| cookie.contains(MEMORY_COOKIE))
                    .collect::<Vec<_>>();
                let first_fresh = loads.first() == Some(&false);
                let remembered = context
                    .sites
                    .hits(Site::Other, "/memory")
                    .get(loads_before)
                    .is_some_and(|(_, cookie)| cookie.contains(MEMORY_COOKIE));
                let saves = context.sites.hits(Site::Other, "/memory/save").len();
                let content = says(&second, CONSENT_FACT) || mentions(&second, &["4"]);
                line(format!(
                    "check=memory loads={loads:?} saves={saves} first_fresh={first_fresh} remembered_next_run={remembered} content={content} first_yours={} second_yours={} own_store(before,between,after)=({},{},{}) work_store(before,between,after)=({},{},{})",
                    first.opened.iter().any(|p| p.yours),
                    second.opened.iter().any(|p| p.yours),
                    before.0,
                    between.0,
                    after.0,
                    before.1,
                    between.1,
                    after.1,
                ));
                last = second.state;
                first_fresh
                    && saves >= 1
                    && remembered
                    && content
                    && !before.0
                    && !between.0
                    && !after.0
                    && between.1
                    && after.1
                    && !first.opened.iter().any(|p| p.yours)
                    && !second.opened.iter().any(|p| p.yours)
            }
            Check::Wall => {
                let run = context
                    .run(
                        "Private inbox",
                        true,
                        "Allow",
                        vec![vec![browse("/inbox", goal)]],
                    )
                    .await?;
                let page = run.opened.first().cloned();
                line(format!(
                    "check=wall intervention={:?} model_calls={:?} status={:?}",
                    page.as_ref().and_then(|p| p.intervention),
                    page.as_ref().and_then(|p| p.model_calls),
                    page.as_ref().and_then(|p| p.status)
                ));
                last = run.state;
                // The first view is bounded by landmarks: the page agent (or
                // Rust, on its first fuller view) stops within two calls.
                page.is_some_and(|p| {
                    p.intervention == Some(WorkInterventionKindV1::SignIn)
                        && p.model_calls.is_some_and(|calls| calls <= 2)
                })
            }
        };
        line(format!("check={check:?} passed={passed}"));
        if !passed {
            failure.get_or_insert("loopback_check");
        }
    }
    Ok(WorkflowResult {
        state: last,
        failure,
    })
}
