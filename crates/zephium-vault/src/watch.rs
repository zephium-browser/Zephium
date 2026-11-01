//! Reports which vault folders changed, so OneDrive syncs and Explorer edits
//! appear without a refresh. Events are coalesced over a short settle window.
use std::collections::BTreeSet;
use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

use notify::{RecursiveMode, Watcher};

use crate::{hidden, Vault};

const SETTLE: Duration = Duration::from_millis(300);
const MAX_DIRS: usize = 64;

/// Keeps the watch alive; dropping it stops the reports.
pub struct VaultWatcher {
    _watcher: notify::RecommendedWatcher,
}

/// Calls `changed` with the vault-relative folders whose contents changed.
/// An empty list means "too much changed; reload everything".
pub fn watch(
    vault: Vault,
    changed: impl Fn(Vec<String>) + Send + 'static,
) -> notify::Result<VaultWatcher> {
    let (send, receive) = mpsc::channel::<notify::Event>();
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        if let Ok(event) = event {
            let _ = send.send(event);
        }
    })?;
    watcher.watch(vault.root(), RecursiveMode::Recursive)?;
    std::thread::Builder::new()
        .name("vault-watch".into())
        .spawn(move || {
            while let Ok(first) = receive.recv() {
                let mut dirs = BTreeSet::new();
                let mut overflow = false;
                let mut add = |event: notify::Event| {
                    for path in event.paths {
                        if let Some(dir) = folder_of(&vault, &path) {
                            dirs.insert(dir);
                        }
                    }
                    if dirs.len() > MAX_DIRS {
                        overflow = true;
                    }
                };
                add(first);
                while let Ok(next) = receive.recv_timeout(SETTLE) {
                    add(next);
                }
                if overflow {
                    changed(Vec::new());
                } else if !dirs.is_empty() {
                    changed(dirs.into_iter().collect());
                }
            }
        })
        .map_err(|error| notify::Error::io(error))?;
    Ok(VaultWatcher { _watcher: watcher })
}

/// The visible folder that lists `path`, or `None` when the change is inside
/// a hidden folder (`.obsidian`, `.trash`) and nothing visible changed.
fn folder_of(vault: &Vault, path: &Path) -> Option<String> {
    let relative = vault.relative(path)?;
    if relative.split('/').any(hidden) {
        return None;
    }
    Some(
        relative
            .rsplit_once('/')
            .map_or(String::new(), |(dir, _)| dir.to_owned()),
    )
}
