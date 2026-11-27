//! QA builds only: each daily app view a run observes is kept as its public
//! semantic wire under `captures/`, the newest 20, for reader fixtures.
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

const KEEP: usize = 20;
const RECENT: usize = 8;
static DIRECTORY: OnceLock<PathBuf> = OnceLock::new();
static RECENT_VIEWS: Mutex<Vec<u64>> = Mutex::new(Vec::new());

pub(crate) fn install(app: &tauri::AppHandle) {
    let Ok(directory) = super::data_root::root(app) else {
        return;
    };
    if DIRECTORY.set(directory.join("captures")).is_ok() {
        zephium_agentic::capture_app_views(capture);
    }
}

fn capture(app: zephium_agentic::DailyApp, page: &str, snapshot: serde_json::Value) {
    let Some(directory) = DIRECTORY.get() else {
        return;
    };
    let body = snapshot.to_string();
    let digest = {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        (page, &body).hash(&mut hasher);
        hasher.finish()
    };
    {
        let Ok(mut recent) = RECENT_VIEWS.lock() else {
            return;
        };
        if recent.contains(&digest) {
            return;
        }
        if recent.len() == RECENT {
            recent.remove(0);
        }
        recent.push(digest);
    }
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |time| time.as_millis());
    let view = view_of(app, page);
    let record = serde_json::json!({
        "app": app.name(),
        "view": view,
        "url": page,
        "captured_at": millis as u64,
        "snapshot": snapshot,
    });
    let path = directory.join(format!("{}-{view}-{millis}.json", app.name()));
    let directory = directory.clone();
    std::thread::spawn(move || {
        if write(&directory, &path, &record).is_ok() {
            super::work_diagnostics::record(format_args!(
                "work: phase=capture app={} view={view} nodes={}",
                record["app"].as_str().unwrap_or_default(),
                record["snapshot"]["n"].as_array().map_or(0, Vec::len)
            ));
        }
    });
}

fn write(directory: &Path, path: &Path, record: &serde_json::Value) -> std::io::Result<()> {
    std::fs::create_dir_all(directory)?;
    let mut options = std::fs::OpenOptions::new();
    options.create(true).write(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    serde_json::to_writer_pretty(options.open(path)?, record)?;
    prune(directory)
}

/// Keeps the newest captures by the time in their names.
fn prune(directory: &Path) -> std::io::Result<()> {
    let mut kept: Vec<(u128, PathBuf)> = std::fs::read_dir(directory)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter_map(|path| {
            let stem = path.file_stem()?.to_str()?;
            let millis = stem.rsplit('-').next()?.parse().ok()?;
            (path.extension()? == "json").then_some((millis, path))
        })
        .collect();
    kept.sort_unstable_by_key(|(millis, _)| std::cmp::Reverse(*millis));
    for (_, path) in kept.into_iter().skip(KEEP) {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

/// The view's own word: Slack's channel, DM or rail view, Linear's inbox or
/// my issues, Gmail's label; "home" when the address names none.
fn view_of(app: zephium_agentic::DailyApp, page: &str) -> String {
    use zephium_agentic::DailyApp;
    let word = |segment: &str| {
        (!segment.is_empty()
            && segment.len() <= 24
            && segment
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte == b'-' || byte == b'_'))
        .then(|| segment.replace('_', "-"))
    };
    let (address, fragment) = page.split_once('#').unwrap_or((page, ""));
    let address = address.split('?').next().unwrap_or_default();
    let path = address
        .split_once("://")
        .map_or(address, |(_, rest)| rest)
        .split_once('/')
        .map_or("", |(_, path)| path);
    let segments: Vec<&str> = path
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect();
    let view = match app {
        DailyApp::Slack => segments
            .iter()
            .skip_while(|segment| **segment != "client")
            .nth(2)
            .and_then(|segment| {
                word(segment).or(match segment.as_bytes().first() {
                    Some(b'C') => Some("channel".to_owned()),
                    Some(b'D') => Some("dm".to_owned()),
                    Some(b'G') => Some("group".to_owned()),
                    _ => None,
                })
            }),
        DailyApp::Gmail => fragment.split('/').next().and_then(word),
        DailyApp::Linear => segments.iter().skip(1).find_map(|segment| word(segment)),
        _ => segments.iter().rev().find_map(|segment| word(segment)),
    };
    view.unwrap_or_else(|| "home".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use zephium_agentic::DailyApp;

    #[test]
    fn views_are_named_by_the_apps_own_words() {
        let view = view_of;
        assert_eq!(
            view(DailyApp::Slack, "https://app.slack.com/client/T01AB/C02CD"),
            "channel"
        );
        assert_eq!(
            view(DailyApp::Slack, "https://app.slack.com/client/T01AB/D02CD"),
            "dm"
        );
        assert_eq!(
            view(
                DailyApp::Slack,
                "https://app.slack.com/client/T01AB/unreads"
            ),
            "unreads"
        );
        assert_eq!(
            view(DailyApp::Slack, "https://app.slack.com/client/T01AB"),
            "home"
        );
        assert_eq!(
            view(
                DailyApp::Linear,
                "https://linear.app/acme/my-issues/assigned"
            ),
            "my-issues"
        );
        assert_eq!(
            view(DailyApp::Linear, "https://linear.app/acme/inbox"),
            "inbox"
        );
        assert_eq!(
            view(DailyApp::Gmail, "https://mail.google.com/mail/u/0/#inbox"),
            "inbox"
        );
    }

    #[test]
    fn only_the_newest_captures_stay() {
        let directory = tempfile::tempdir().unwrap();
        for at in 0..25_u32 {
            std::fs::write(
                directory.path().join(format!("slack-channel-{at}.json")),
                "{}",
            )
            .unwrap();
        }
        prune(directory.path()).unwrap();
        let mut left: Vec<String> = std::fs::read_dir(directory.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        left.sort();
        assert_eq!(left.len(), KEEP);
        assert!(!left.contains(&"slack-channel-4.json".to_owned()));
        assert!(left.contains(&"slack-channel-5.json".to_owned()));
    }
}
