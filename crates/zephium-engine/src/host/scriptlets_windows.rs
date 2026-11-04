//! Each profile's scriptlet program on Windows: a script WebView2 runs as
//! each document of a view is created, kept in memory only. A view gets it
//! before its first navigation; a change of program or paused sites replaces
//! it on every view of the profile.

use std::cell::RefCell;
use std::collections::HashMap;
use std::time::Duration;

use sha2::{Digest, Sha256};
use webview2_com::AddScriptToExecuteOnDocumentCreatedCompletedHandler;
use windows_core::HSTRING;
use wry::WebViewExtWindows;
use zephium_core::blocker::ContentPolicyGeneration;
use zephium_core::ids::{ItemId, ProfileId};
use zephium_core::ports::engine::BlockerSiteCompletion;

use super::dispatch::with_scriptlets;
use super::EngineHost;

/// How long a site change waits for its views to take the new program.
const READY_WITHIN: Duration = Duration::from_secs(2);

#[derive(Default)]
pub(super) struct Scriptlets {
    adds: u64,
    waits: HashMap<ProfileId, Wait>,
}

/// Site changes waiting for the profile's views to take the program that
/// knows them, so the page reload that follows runs it.
#[derive(Default)]
struct Wait {
    adds: Vec<u64>,
    sites: Vec<BlockerSiteCompletion>,
    timeout: Option<crate::platform::imp::ContentPolicyTimeout>,
}

/// The script one view runs.
#[derive(Default)]
pub(super) struct ViewScriptlet(RefCell<Option<Installed>>);

struct Installed {
    digest: [u8; 32],
    add: u64,
    /// None until WebView2 confirms the add.
    id: Option<String>,
}

impl Scriptlets {
    pub(super) fn forget(&mut self, profile: ProfileId) {
        self.waits.remove(&profile);
    }
}

fn digest(source: &str) -> [u8; 32] {
    Sha256::digest(source.as_bytes()).into()
}

/// Records a finished add in its view's slot and returns a script to remove:
/// one a newer program replaced while it was being added. A failed add of the
/// current program empties the slot, so the next ensure or refresh adds it
/// again.
fn settle_add(slot: &mut Option<Installed>, add: u64, id: Option<String>) -> Option<String> {
    if slot.as_ref().is_none_or(|installed| installed.add != add) {
        return id;
    }
    match id {
        Some(id) => {
            if let Some(installed) = slot.as_mut() {
                installed.id = Some(id);
            }
        }
        None => *slot = None,
    }
    None
}

fn remove(view: &wry::WebView, id: &str) {
    // SAFETY: the view's core stays on its creating STA, where this runs.
    let _ = unsafe {
        view.webview()
            .RemoveScriptToExecuteOnDocumentCreated(&HSTRING::from(id))
    };
}

impl EngineHost {
    /// Puts the profile's program on a view about to navigate, waiting for
    /// WebView2 to take it so the first document runs it.
    pub(super) fn ensure_view_scriptlets(
        &mut self,
        profile: ProfileId,
        view: &wry::WebView,
        slot: &ViewScriptlet,
    ) {
        let source = self.scriptlet_source(profile);
        let wanted = source.as_deref().map(digest);
        let current = slot.0.borrow().as_ref().map(|installed| installed.digest);
        if wanted == current {
            return;
        }
        if let Some(id) = slot.0.take().and_then(|installed| installed.id) {
            remove(view, &id);
        }
        let (Some(source), Some(digest)) = (source, wanted) else {
            return;
        };
        self.scriptlets.adds += 1;
        let add = self.scriptlets.adds;
        let core = view.webview();
        let id = std::rc::Rc::new(RefCell::new(None));
        let result = id.clone();
        let added = AddScriptToExecuteOnDocumentCreatedCompletedHandler::wait_for_async_operation(
            Box::new(move |handler| {
                // SAFETY: the view's core stays on its creating STA, where this runs.
                unsafe {
                    core.AddScriptToExecuteOnDocumentCreated(&HSTRING::from(source), &handler)
                }
                .map_err(Into::into)
            }),
            Box::new(move |error, id| {
                error?;
                result.replace(Some(id));
                Ok(())
            }),
        );
        match (added, id.take()) {
            (Ok(()), Some(id)) => {
                slot.0.replace(Some(Installed {
                    digest,
                    add,
                    id: Some(id),
                }));
            }
            (added, _) => eprintln!(
                "content blocker: the scriptlet program was not added to a view: {:?}",
                added.err()
            ),
        }
    }

    /// Replaces the program on every view of the profile whose program
    /// changed; true while WebView2 is still taking one.
    pub(super) fn refresh_profile_scriptlets(&mut self, profile: ProfileId) -> bool {
        let source = self.scriptlet_source(profile);
        let wanted = source.as_deref().map(digest);
        let ids: Vec<ItemId> = self
            .partitions
            .iter()
            .filter_map(|(id, partition)| (partition.profile() == profile).then_some(*id))
            .collect();
        let mut pending = Vec::new();
        for id in ids {
            let Some(observed) = self.views.get(&id) else {
                continue;
            };
            let slot = &observed.scriptlet;
            let current = slot.0.borrow().as_ref().map(|installed| installed.digest);
            if current == wanted {
                continue;
            }
            if let Some(old) = slot.0.take().and_then(|installed| installed.id) {
                remove(&observed.view, &old);
            }
            let (Some(source), Some(digest)) = (source.as_ref(), wanted) else {
                continue;
            };
            self.scriptlets.adds += 1;
            let add = self.scriptlets.adds;
            let handler = AddScriptToExecuteOnDocumentCreatedCompletedHandler::create(Box::new(
                move |error, script| {
                    let script = error.ok().map(|()| script);
                    with_scriptlets(move |host| host.scriptlet_added(profile, id, add, script));
                    Ok(())
                },
            ));
            // SAFETY: the view's core stays on its creating STA, where this runs.
            let started = unsafe {
                observed
                    .view
                    .webview()
                    .AddScriptToExecuteOnDocumentCreated(&HSTRING::from(source.as_str()), &handler)
            };
            if started.is_ok() {
                slot.0.replace(Some(Installed {
                    digest,
                    add,
                    id: None,
                }));
                pending.push(add);
            } else {
                eprintln!("content blocker: the scriptlet program could not be replaced on a view");
            }
        }
        if pending.is_empty() {
            return false;
        }
        let wait = self.scriptlets.waits.entry(profile).or_default();
        wait.adds.extend(pending);
        if wait.timeout.is_none() {
            wait.timeout =
                crate::platform::imp::schedule_browser_timeout(READY_WITHIN, move || {
                    with_scriptlets(move |host| host.release_scriptlet_wait(profile));
                });
        }
        true
    }

    fn scriptlet_added(&mut self, profile: ProfileId, view: ItemId, add: u64, id: Option<String>) {
        if let Some(observed) = self.views.get(&view) {
            if let Some(stale) = settle_add(&mut observed.scriptlet.0.borrow_mut(), add, id) {
                remove(&observed.view, &stale);
            }
        }
        let done = self.scriptlets.waits.get_mut(&profile).is_some_and(|wait| {
            wait.adds.retain(|pending| *pending != add);
            wait.adds.is_empty()
        });
        if done {
            self.release_scriptlet_wait(profile);
        }
    }

    fn release_scriptlet_wait(&mut self, profile: ProfileId) {
        if let Some(wait) = self.scriptlets.waits.remove(&profile) {
            for site in wait.sites {
                site.finish(true);
            }
        }
    }

    /// Finishes a site change now, or once its views took the program that
    /// knows it.
    pub(crate) fn finish_site_change(
        &mut self,
        profile: ProfileId,
        completion: BlockerSiteCompletion,
    ) {
        match self.scriptlets.waits.get_mut(&profile) {
            Some(wait) => wait.sites.push(completion),
            None => completion.finish(true),
        }
    }

    /// New views take the program before they navigate, so a policy's
    /// settlement waits for nothing here.
    pub(super) fn wait_for_scriptlets(
        &mut self,
        profile: ProfileId,
        _generation: ContentPolicyGeneration,
    ) -> bool {
        self.refresh_profile_scriptlets(profile);
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn installed(add: u64) -> Option<Installed> {
        Some(Installed {
            digest: [1; 32],
            add,
            id: None,
        })
    }

    #[test]
    fn a_failed_add_of_the_current_program_is_added_again() {
        let mut slot = installed(4);
        assert_eq!(settle_add(&mut slot, 4, None), None);
        assert!(slot.is_none());
    }

    #[test]
    fn a_finished_add_is_kept_and_a_replaced_one_removed() {
        let mut slot = installed(4);
        assert_eq!(settle_add(&mut slot, 4, Some("a".into())), None);
        assert_eq!(
            slot.as_ref().and_then(|installed| installed.id.as_deref()),
            Some("a")
        );
        assert_eq!(
            settle_add(&mut slot, 3, Some("old".into())).as_deref(),
            Some("old")
        );
        assert_eq!(settle_add(&mut slot, 3, None), None);
        assert!(slot.is_some());
    }
}
