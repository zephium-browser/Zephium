//! Bringing bookmarks and history over from another browser. Reading happens
//! on a blocking worker from fresh discovery, never from a path chrome named;
//! writing goes through the shell into one admitted destination. Progress reaches
//! chrome as whole snapshots of the one job that may run at a time.

use super::*;
use std::sync::Mutex;
use zephium_import::{Browser, ImportError, Locations};

const EVENT_IMPORT_PROGRESS: &str = "zephium:import-progress";
const MAX_PROFILE_ID_BYTES: usize = 512;
/// An import writes up to tens of thousands of rows in one transaction.
const WRITE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(120);

#[derive(Clone, Copy, Debug, Serialize, Deserialize, specta::Type, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ImportKindView {
    /// Sites kept at the top of the other browser's sidebar.
    Essentials,
    Bookmarks,
    History,
}

/// Kinds in the order an import runs them: what is seen first lands first.
const KIND_ORDER: [ImportKindView; 3] = [
    ImportKindView::Essentials,
    ImportKindView::Bookmarks,
    ImportKindView::History,
];

#[derive(Clone, Debug, Serialize, Deserialize, specta::Type)]
pub(crate) struct ImportProfileView {
    id: String,
    name: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, specta::Type)]
pub(crate) struct ImportSourceView {
    id: String,
    browser: String,
    name: String,
    profiles: Vec<ImportProfileView>,
    kinds: Vec<ImportKindView>,
    needs_permission: bool,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, specta::Type, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ImportStateView {
    Queued,
    Running,
    Done,
    Failed,
    Skipped,
}

/// Why a kind failed, when chrome can say something useful about it.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, specta::Type, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ImportProblemView {
    /// The browser holds its files locked; quitting it lets the import run.
    Busy,
    Permission,
    Unreadable,
    Missing,
    /// Zephium could not keep what was read.
    Storage,
}

#[derive(Clone, Debug, Serialize, Deserialize, specta::Type)]
pub(crate) struct ImportKindProgressView {
    kind: ImportKindView,
    state: ImportStateView,
    done: u32,
    total: Option<u32>,
    problem: Option<ImportProblemView>,
}

/// The running import, whole, each time it moves.
#[derive(Clone, Debug, Serialize, Deserialize, specta::Type, tauri_specta::Event)]
#[tauri_specta(event_name = "zephium:import-progress")]
pub(crate) struct ImportJobView {
    source: String,
    profile: String,
    kinds: Vec<ImportKindProgressView>,
    finished: bool,
    cancelled: bool,
}

#[derive(Default)]
pub(crate) struct ImportJobs {
    running: Mutex<Option<Arc<AtomicBool>>>,
}

fn problem(error: &ImportError) -> ImportProblemView {
    match error {
        ImportError::Busy => ImportProblemView::Busy,
        ImportError::Permission => ImportProblemView::Permission,
        ImportError::Unreadable => ImportProblemView::Unreadable,
        ImportError::Missing => ImportProblemView::Missing,
    }
}

fn discover() -> Vec<zephium_import::Source> {
    Locations::from_env()
        .map(|locations| zephium_import::discover(&locations))
        .unwrap_or_default()
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn import_sources(caller: WebviewWindow) -> Vec<ImportSourceView> {
    if !authorize(&caller, CallerPolicy::Main, "import_sources") {
        return Vec::new();
    }
    tauri::async_runtime::spawn_blocking(discover)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|source| {
            let mut kinds = Vec::new();
            if source.profiles.iter().any(|profile| profile.essentials) {
                kinds.push(ImportKindView::Essentials);
            }
            if source.profiles.iter().any(|profile| profile.bookmarks) {
                kinds.push(ImportKindView::Bookmarks);
            }
            if source.profiles.iter().any(|profile| profile.history) {
                kinds.push(ImportKindView::History);
            }
            ImportSourceView {
                id: source.browser.id().to_owned(),
                browser: source.browser.id().to_owned(),
                name: source.browser.name().to_owned(),
                profiles: source
                    .profiles
                    .into_iter()
                    .map(|profile| ImportProfileView {
                        id: profile.id,
                        name: profile.name,
                    })
                    .collect(),
                kinds,
                needs_permission: source.needs_permission,
            }
        })
        .collect()
}

/// Starts importing `kinds` from one profile of one source into the focused
/// profile. Refused while another import runs.
#[tauri::command]
#[specta::specta]
pub(crate) async fn import_start(
    caller: WebviewWindow,
    app: tauri::AppHandle,
    source: String,
    profile: String,
    kinds: Vec<ImportKindView>,
) -> bool {
    if !authorize(&caller, CallerPolicy::Main, "import_start")
        || shutdown_started(&app)
        || !bounded(&profile, MAX_PROFILE_ID_BYTES)
        || kinds.is_empty()
        || kinds.len() > KIND_ORDER.len()
    {
        return false;
    }
    let Some(browser) = Browser::from_id(&source) else {
        return false;
    };
    let Some(jobs) = app.try_state::<ImportJobs>() else {
        return false;
    };
    let cancelled = Arc::new(AtomicBool::new(false));
    {
        let mut running = jobs
            .running
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if running.is_some() {
            return false;
        }
        *running = Some(cancelled.clone());
    }
    let ordered: Vec<ImportKindView> = KIND_ORDER
        .into_iter()
        .filter(|kind| kinds.contains(kind))
        .collect();
    let Some(shell) = app.try_state::<Handle>().map(|shell| shell.inner().clone()) else {
        *jobs
            .running
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        return false;
    };
    let (send, receive) = tokio::sync::oneshot::channel();
    let admitted = shell.dispatch(Command::ImportDestination {
        done: zephium_app::ImportDestinationCompletion::new(move |destination| {
            let _ = send.send(destination);
        }),
    });
    let destination = if admitted {
        tokio::time::timeout(WRITE_DEADLINE, receive)
            .await
            .ok()
            .and_then(Result::ok)
            .flatten()
    } else {
        None
    };
    let Some(destination) = destination else {
        *jobs
            .running
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        return false;
    };
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        run(&app, browser, profile, destination, ordered, cancelled).await;
        if let Some(jobs) = app.try_state::<ImportJobs>() {
            *jobs
                .running
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        }
    });
    true
}

#[tauri::command]
#[specta::specta]
pub(crate) fn import_cancel(caller: WebviewWindow, app: tauri::AppHandle) -> bool {
    if !authorize(&caller, CallerPolicy::Main, "import_cancel") {
        return false;
    }
    app.try_state::<ImportJobs>().is_some_and(|jobs| {
        jobs.running
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .inspect(|cancelled| cancelled.store(true, Ordering::Release))
            .is_some()
    })
}

/// Opens the system setting a source needs (Safari: Full Disk Access).
#[tauri::command]
#[specta::specta]
pub(crate) fn import_open_permission(caller: WebviewWindow, source: String) -> bool {
    if !authorize(&caller, CallerPolicy::Main, "import_open_permission") {
        return false;
    }
    #[cfg(target_os = "macos")]
    if source == "safari" {
        use objc2_app_kit::NSWorkspace;
        use objc2_foundation::{NSString, NSURL};
        let url = NSURL::URLWithString(&NSString::from_str(
            "x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles",
        ));
        return url.is_some_and(|url| NSWorkspace::sharedWorkspace().openURL(&url));
    }
    let _ = source;
    false
}

async fn run(
    app: &tauri::AppHandle,
    browser: Browser,
    profile: String,
    destination: zephium_app::ImportDestination,
    kinds: Vec<ImportKindView>,
    cancelled: Arc<AtomicBool>,
) {
    let mut job = ImportJobView {
        source: browser.id().to_owned(),
        profile: profile.clone(),
        kinds: kinds
            .iter()
            .map(|kind| ImportKindProgressView {
                kind: *kind,
                state: ImportStateView::Queued,
                done: 0,
                total: None,
                problem: None,
            })
            .collect(),
        finished: false,
        cancelled: false,
    };
    publish(app, &job);
    let mut sites = ImportedOrigins::default();
    for index in 0..job.kinds.len() {
        if cancelled.load(Ordering::Acquire) {
            job.cancelled = true;
            job.kinds[index].state = ImportStateView::Skipped;
            continue;
        }
        job.kinds[index].state = ImportStateView::Running;
        publish(app, &job);
        let progress = &mut job.kinds[index];
        match read(browser, &profile, progress.kind).await {
            Err(error) => {
                progress.state = ImportStateView::Failed;
                progress.problem = Some(problem(&error));
            }
            Ok((work, total)) => {
                progress.total = Some(total);
                if cancelled.load(Ordering::Acquire) {
                    progress.state = ImportStateView::Skipped;
                    job.cancelled = true;
                } else {
                    sites.note(&work);
                    match write(app, destination, work).await {
                        Some(added) => {
                            progress.done = added;
                            progress.state = ImportStateView::Done;
                        }
                        None => {
                            progress.state = ImportStateView::Failed;
                            progress.problem = Some(ImportProblemView::Storage);
                        }
                    }
                }
            }
        }
        publish(app, &job);
    }
    // Icons ride along unannounced: the source already holds them, so the
    // imported rows show real marks without asking any site.
    #[cfg(feature = "work-product")]
    if !job.cancelled && !cancelled.load(Ordering::Acquire) {
        import_icons(app, browser, &profile, destination, sites.wanted()).await;
    }
    #[cfg(not(feature = "work-product"))]
    let _ = sites;
    job.finished = true;
    publish(app, &job);
}

/// The HTTPS sites an import brought over, in the order their icons matter:
/// bookmarks and Essentials as listed, then history by how often it was seen.
#[derive(Default)]
struct ImportedOrigins {
    listed: Vec<String>,
    seen: std::collections::HashSet<String>,
    visited: std::collections::HashMap<String, u32>,
}

impl ImportedOrigins {
    fn list(&mut self, url: &str) {
        if let Some(origin) = zephium_import::https_origin(url) {
            if self.seen.insert(origin.clone()) {
                self.listed.push(origin);
            }
        }
    }

    fn note(&mut self, work: &zephium_app::ImportWork) {
        fn walk(origins: &mut ImportedOrigins, nodes: &[zephium_import::ImportNode]) {
            for node in nodes {
                match node {
                    zephium_import::ImportNode::Link { url, .. } => origins.list(url),
                    zephium_import::ImportNode::Folder { children, .. } => walk(origins, children),
                }
            }
        }
        match work {
            zephium_app::ImportWork::Bookmarks { nodes, .. } => walk(self, nodes),
            zephium_app::ImportWork::Essentials(sites) => {
                for site in sites {
                    self.list(&site.url);
                }
            }
            zephium_app::ImportWork::History(visits) => {
                for visit in visits {
                    if let Some(origin) = zephium_import::https_origin(&visit.url) {
                        *self.visited.entry(origin).or_default() += 1;
                    }
                }
            }
            zephium_app::ImportWork::Icons(_) => {}
        }
    }

    fn wanted(self) -> Vec<String> {
        let Self {
            mut listed,
            seen,
            visited,
        } = self;
        let mut visited: Vec<(String, u32)> = visited
            .into_iter()
            .filter(|(origin, _)| !seen.contains(origin))
            .collect();
        visited.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        listed.extend(visited.into_iter().map(|(origin, _)| origin));
        listed.truncate(zephium_import::MAX_ICONS);
        listed
    }
}

#[cfg(feature = "work-product")]
async fn import_icons(
    app: &tauri::AppHandle,
    browser: Browser,
    profile: &str,
    destination: zephium_app::ImportDestination,
    wanted: Vec<String>,
) {
    if wanted.is_empty() {
        return;
    }
    let profile = profile.to_owned();
    let icons = tauri::async_runtime::spawn_blocking(move || {
        let locations = Locations::from_env()?;
        let icons = zephium_import::icons(&locations, browser, &profile, &wanted).ok()?;
        Some(
            icons
                .into_iter()
                .filter_map(|icon| {
                    crate::favicon_probe::rasterize(&icon.bytes).map(|raster| (icon.origin, raster))
                })
                .collect::<Vec<_>>(),
        )
    })
    .await
    .ok()
    .flatten()
    .unwrap_or_default();
    // Sent even when empty: storing it is what refreshes the sidebar, so a
    // source with no icons of its own still has its Essentials fetched.
    let _ = write(app, destination, zephium_app::ImportWork::Icons(icons)).await;
}

async fn read(
    browser: Browser,
    profile: &str,
    kind: ImportKindView,
) -> Result<(zephium_app::ImportWork, u32), ImportError> {
    let profile = profile.to_owned();
    tauri::async_runtime::spawn_blocking(move || {
        let locations = Locations::from_env().ok_or(ImportError::Missing)?;
        // Discovery again: only a profile this source lists right now is read.
        let source = zephium_import::source(&locations, browser).ok_or(ImportError::Missing)?;
        let listed = source
            .profiles
            .iter()
            .find(|candidate| candidate.id == profile)
            .ok_or(ImportError::Missing)?;
        match kind {
            ImportKindView::Essentials => {
                let sites: Vec<zephium_app::ImportedSite> =
                    zephium_import::essentials(&locations, browser, &profile)?
                        .into_iter()
                        .map(|site| zephium_app::ImportedSite {
                            url: site.url,
                            title: site.title,
                        })
                        .collect();
                let total = u32::try_from(sites.len()).unwrap_or(u32::MAX);
                Ok((zephium_app::ImportWork::Essentials(sites), total))
            }
            ImportKindView::Bookmarks => {
                let nodes = zephium_import::bookmarks(&locations, browser, &profile)?;
                let total = links(&nodes);
                // One folder per source, and per profile when there are several,
                // so imports from two Chrome profiles stay apart.
                let folder = if source.profiles.len() > 1 {
                    format!("Imported from {} ({})", browser.name(), listed.name)
                } else {
                    format!("Imported from {}", browser.name())
                };
                Ok((zephium_app::ImportWork::Bookmarks { folder, nodes }, total))
            }
            ImportKindView::History => {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |elapsed| elapsed.as_secs() as i64);
                let visits = zephium_import::history(&locations, browser, &profile, now)?;
                let total = u32::try_from(visits.len()).unwrap_or(u32::MAX);
                Ok((zephium_app::ImportWork::History(visits), total))
            }
        }
    })
    .await
    .unwrap_or(Err(ImportError::Unreadable))
}

fn links(nodes: &[zephium_import::ImportNode]) -> u32 {
    nodes
        .iter()
        .map(|node| match node {
            zephium_import::ImportNode::Link { .. } => 1,
            zephium_import::ImportNode::Folder { children, .. } => links(children),
        })
        .fold(0u32, u32::saturating_add)
}

async fn write(
    app: &tauri::AppHandle,
    destination: zephium_app::ImportDestination,
    work: zephium_app::ImportWork,
) -> Option<u32> {
    let shell = app.try_state::<Handle>()?.inner().clone();
    let (send, receive) = tokio::sync::oneshot::channel();
    if !shell.dispatch(Command::Import {
        destination,
        work: Box::new(work),
        done: zephium_app::ImportCompletion::new(move |added| {
            let _ = send.send(added);
        }),
    }) {
        return None;
    }
    tokio::time::timeout(WRITE_DEADLINE, receive)
        .await
        .ok()?
        .ok()
        .flatten()
}

fn publish(app: &tauri::AppHandle, job: &ImportJobView) {
    emit_to_privileged(app, MAIN_LABEL, EVENT_IMPORT_PROGRESS, job);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_count_through_folders() {
        use zephium_import::ImportNode;
        let link = || ImportNode::Link {
            title: String::new(),
            url: "https://a.example/".into(),
        };
        let tree = vec![
            link(),
            ImportNode::Folder {
                title: "F".into(),
                children: vec![link(), link()],
            },
        ];
        assert_eq!(links(&tree), 3);
    }

    #[test]
    fn icons_are_wanted_for_listed_sites_first_then_the_most_visited() {
        use zephium_import::{ImportNode, ImportedVisit};
        let visit = |url: &str| ImportedVisit {
            url: url.into(),
            title: String::new(),
            visited_at: 1,
        };
        let mut origins = ImportedOrigins::default();
        origins.note(&zephium_app::ImportWork::History(vec![
            visit("https://rare.example/"),
            visit("https://often.example/a"),
            visit("https://often.example/b"),
            visit("https://x.com/home"),
            visit("http://plain.example/"),
        ]));
        origins.note(&zephium_app::ImportWork::Bookmarks {
            folder: "Imported".into(),
            nodes: vec![ImportNode::Folder {
                title: "F".into(),
                children: vec![ImportNode::Link {
                    title: String::new(),
                    url: "https://x.com/settings".into(),
                }],
            }],
        });
        assert_eq!(
            origins.wanted(),
            [
                "https://x.com",
                "https://often.example",
                "https://rare.example"
            ]
        );
    }
}
