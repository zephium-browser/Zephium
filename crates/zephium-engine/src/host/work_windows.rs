//! Windows retained Work construction on the original profile environment.
use super::{EngineHost, ParentHandle};
use crate::agent_context_port::WorkResourceGuard;
use crate::platform::windows::{AgentOwnedProfile, AgentOwnedView, AgentOwnedViewCallbacks};
use std::sync::Arc;
use std::time::Instant;
use zephium_agentic::{ContextOwnedViewport, ContextPortFailure, ContextProfileStorageClass};

pub(super) struct AnonymousWorkEnvironment {
    pub(super) profile: zephium_core::ids::ProfileId,
    pub(super) native_profile: zephium_core::ids::ProfileId,
    pub(super) session: Option<zephium_agentic::WeakWorkBrowserSession>,
    pub(super) context: Option<zephium_agentic::ContextId>,
    retirement: Option<Arc<crate::erasure::Completion>>,
}
impl EngineHost {
    pub(super) fn retire_windows_anonymous_work_sessions(&mut self) {
        // Keep unresolved or failed native erasure obligations bounded and visible
        // to profile erasure, even after the public watchdog reported a timeout.
        self.anonymous_work_environments.retain(|_, entry| {
            entry
                .retirement
                .as_ref()
                .and_then(|completion| completion.terminal_outcome())
                != Some(crate::ProfileDataErasureOutcome::Verified)
        });
        let expired: Vec<_> = self
            .anonymous_work_environments
            .iter()
            .filter_map(|(id, entry)| {
                let current = entry
                    .session
                    .as_ref()
                    .is_some_and(|session| session.is_current());
                let resident = self.work_resources.values().any(|resource| {
                    resource.resident()
                        && (entry.context == Some(resource.guard.resource().identity().context())
                            || resource
                                .guard
                                .anonymous_session()
                                .is_some_and(|session| session.id() == *id))
                });
                (!current
                    && !resident
                    && entry.retirement.is_none()
                    && !self.erasure_tombstones.contains(&entry.profile))
                .then_some(*id)
            })
            .collect();
        for id in expired {
            let completion = crate::erasure::Completion::start(
                Box::new(|_| {}),
                Arc::new(std::sync::atomic::AtomicBool::new(true)),
            );
            let native_profile = if let Some(entry) = self.anonymous_work_environments.get_mut(&id)
            {
                entry.retirement = Some(completion.clone());
                entry.native_profile
            } else {
                continue;
            };
            self.erase_windows_native_environment(
                native_profile,
                self.private_runtime.root().to_owned(),
                completion,
            );
        }
    }
    pub(super) fn prepare_windows_anonymous_erasure(
        &mut self,
        profile: zephium_core::ids::ProfileId,
        completion: Arc<crate::erasure::Completion>,
    ) -> Arc<crate::erasure::Completion> {
        let ids: Vec<_> = self
            .anonymous_work_environments
            .iter()
            .filter_map(|(id, entry)| (entry.profile == profile).then_some(*id))
            .collect();
        if ids.is_empty() {
            return completion;
        }
        let mut completions = crate::erasure::Completion::split(completion, ids.len() + 1);
        let selected = completions.remove(0);
        for (id, completion) in ids.into_iter().zip(completions) {
            let Some(entry) = self.anonymous_work_environments.get_mut(&id) else {
                completion.finish(crate::ProfileDataErasureOutcome::Failed);
                continue;
            };
            if let Some(retirement) = &entry.retirement {
                retirement.forward_terminal(completion);
            } else {
                let native_profile = entry.native_profile;
                entry.retirement = Some(completion.clone());
                self.erase_windows_native_environment(
                    native_profile,
                    self.private_runtime.root().to_owned(),
                    completion,
                );
            }
        }
        selected
    }
    /// Profile erasure also removes Work's own store folder, whether or not
    /// this process opened it.
    pub(super) fn prepare_windows_work_store_erasure(
        &mut self,
        profile: zephium_core::ids::ProfileId,
        completion: Arc<crate::erasure::Completion>,
    ) -> Arc<crate::erasure::Completion> {
        let mut completions = crate::erasure::Completion::split(completion, 2);
        let selected = completions.remove(0);
        let work = completions.remove(0);
        self.erase_windows_native_environment(
            crate::work_website_data::native_profile(profile),
            self.profiles_root.clone(),
            work,
        );
        selected
    }
    /// Empties the profile's Work store. A live Work page clears it through
    /// WebView2; with no environment running the folder is removed. While
    /// the environment runs without a page the answer is false.
    pub(crate) fn clear_work_website_data(
        &mut self,
        profile: zephium_core::ids::ProfileId,
        reply: std::sync::mpsc::Sender<bool>,
    ) {
        if self.erasure_tombstones.contains(&profile) {
            let _ = reply.send(false);
            return;
        }
        let expected = AgentOwnedProfile::work(crate::work_website_data::identifier(profile));
        let live = self
            .work_resources
            .values()
            .filter(|resource| resource.profile() == profile)
            .find_map(|resource| {
                resource
                    .view
                    .as_ref()
                    .filter(|view| view.work_profile_matches(&expected))
            });
        if let Some(view) = live {
            if !view.clear_profile_browsing_data(reply.clone()) {
                let _ = reply.send(false);
            }
            return;
        }
        let native = crate::work_website_data::native_profile(profile);
        if self.environments.contains_key(&native) || self.erasure_tombstones.contains(&native) {
            let _ = reply.send(false);
            return;
        }
        let root = self.profiles_root.clone();
        let spawned = std::thread::Builder::new()
            .name("work-website-data-clear".into())
            .spawn(move || {
                let _ = reply.send(crate::erasure::remove_profile_directories_verified(
                    &[root],
                    native,
                ));
            });
        drop(spawned);
    }
    fn erase_windows_native_environment(
        &mut self,
        native_profile: zephium_core::ids::ProfileId,
        root: std::path::PathBuf,
        completion: Arc<crate::erasure::Completion>,
    ) {
        self.erasure_tombstones.insert(native_profile);
        self.browser_version_observers.remove(&native_profile);
        self.retry_windows_cleanup_debts(3);
        let clean = !self.windows_cleanup_debts.contains_key(&native_profile)
            && !self
                .unverifiable_browser_processes
                .contains(&native_profile)
            && !self.windows_cleanup_invariant_failed;
        let proof = self
            .browser_process_exit_observers
            .get(&native_profile)
            .map(crate::platform::imp::BrowserProcessExitObserver::proof);
        // Retain the original process/proof pair for concurrent app shutdown.
        // Erasure shares its opened HANDLE; the exact exit callback retires
        // both host entries together, after the kernel object is signalled.
        let process = self.browser_processes.get(&native_profile).cloned();
        self.web_contexts.remove(&native_profile);
        self.environments.remove(&native_profile);
        crate::platform::imp::erase_profile_data(
            None,
            process,
            proof,
            clean,
            vec![root],
            native_profile,
            completion,
        );
    }
    pub(super) fn work_store_cookie_presence(
        &mut self,
        profile: zephium_core::ids::ProfileId,
        target: &zephium_agentic::ContextNavigationTarget,
        deadline: Instant,
        remaining_constructions: &mut usize,
    ) -> Result<bool, ()> {
        let owned = AgentOwnedProfile::work_site(target)?;
        for resource in self.work_resources.values() {
            if resource.profile() == profile {
                if let Some(view) = &resource.view {
                    if view.work_profile_matches(&owned) {
                        #[cfg(feature = "native-agentic-work-lifetime-diagnostic")]
                        diagnose_cookie_profile(view, "work_resident");
                        return query_cookie_presence(
                            view.work_presence_cookie_manager()?,
                            target.as_url(),
                            deadline,
                        );
                    }
                }
            }
        }
        self.temporary_cookie_presence(
            profile,
            owned,
            target.as_url(),
            deadline,
            remaining_constructions,
        )
    }

    pub(super) fn selected_work_cookie_presence(
        &mut self,
        profile: zephium_core::ids::ProfileId,
        url: &url::Url,
        deadline: Instant,
        remaining_constructions: &mut usize,
    ) -> Result<bool, ()> {
        if let Some(environment) = self.environments.get(&profile) {
            if let Ok(manager) = self.selected_profile_cookie_source(profile, environment) {
                #[cfg(feature = "native-agentic-work-lifetime-diagnostic")]
                {
                    use std::io::Write as _;
                    let _ = writeln!(
                        std::io::stderr().lock(),
                        "windows-work-cookies: stage=selected_resident; content=redacted"
                    );
                }
                return query_cookie_presence(manager, url, deadline);
            }
        }
        self.temporary_cookie_presence(
            profile,
            AgentOwnedProfile::Selected,
            url,
            deadline,
            remaining_constructions,
        )
    }

    fn temporary_cookie_presence(
        &mut self,
        profile: zephium_core::ids::ProfileId,
        owned: AgentOwnedProfile,
        url: &url::Url,
        deadline: Instant,
        remaining_constructions: &mut usize,
    ) -> Result<bool, ()> {
        if self.erasure_tombstones.contains(&profile)
            || self.windows_view_admission_blocked(profile)
            || *remaining_constructions == 0
            || Instant::now() >= deadline
        {
            return Err(());
        }
        *remaining_constructions -= 1;
        let path = crate::erasure::prepare_profile_directory(&self.profiles_root, profile)
            .map_err(|_| ())?;
        let bootstrap = self
            .begin_windows_work_profile_environment(profile, deadline, path.clone(), true)
            .map_err(|_| ())?;
        let environment = self.environments.get(&profile).cloned().ok_or(())?;
        let mut resource = Some(
            self.native_resources
                .try_acquire(super::resources::NativeResourceClass::TransientConstruction)
                .map_err(|_| ())?,
        );
        // An empty selected source may refuse installed extensions; that yields
        // unknown presence, never authority to read an existing session without asking.
        let extensions_enabled = matches!(owned, AgentOwnedProfile::Automation { .. });
        let built = crate::platform::windows::build_owned_agent_view(
            &ParentHandle(self.parent.0),
            ContextOwnedViewport::STANDARD,
            &environment,
            owned,
            ContextProfileStorageClass::Durable,
            &path,
            deadline,
            extensions_enabled,
            AgentOwnedViewCallbacks::new(|_| {}, || {}, || {}, || {}, || {}, || {}),
        );
        for debt in wry::pending_webview2_cleanup_debts() {
            let lease = resource.take().or_else(|| {
                self.native_resources
                    .try_acquire(super::resources::NativeResourceClass::TeardownDebt)
                    .ok()
            });
            let debt = super::OwnedWindowsCleanupDebt::new(debt, lease);
            if !debt.accounted_as_debt() {
                self.native_resource_accounting_failed = true;
            }
            self.retain_windows_cleanup_debt(profile, debt);
        }
        if wry::webview2_cleanup_overflowed() {
            self.fail_windows_cleanup_invariant();
        }
        let (mut view, _) = built.map_err(|_| ())?;
        #[cfg(feature = "native-agentic-work-lifetime-diagnostic")]
        diagnose_cookie_profile(&view, "presence_temporary");
        let result = self
            .capture_windows_work_environment(profile, environment, deadline)
            .map_err(|_| ())
            .and_then(|_| view.work_presence_cookie_manager())
            .and_then(|manager| query_cookie_presence(manager, url, deadline));
        // The controller and its counted resource survive every native cookie query.
        // A timed-out result cell is closed before teardown, so late callbacks cannot publish.
        let clean = view.retire_semantic_runtime();
        if let Err(debt) = view.close() {
            self.retain_windows_cleanup_debt(
                profile,
                super::OwnedWindowsCleanupDebt::new(debt, resource.take()),
            );
            return Err(());
        }
        drop(bootstrap);
        self.collect_pending_windows_cleanup_debts();
        if !clean {
            self.fail_windows_cleanup_invariant();
            return Err(());
        }
        if Instant::now() >= deadline || self.windows_view_admission_blocked(profile) {
            return Err(());
        }
        result
    }
    pub(super) fn build_windows_work_view(
        &mut self,
        guard: &Arc<WorkResourceGuard>,
        native_resource: &mut Option<super::NativeResourceLease>,
    ) -> Result<AgentOwnedView, ContextPortFailure> {
        let result = self.build_windows_work_view_inner(guard, native_resource);
        // Bootstrap Drop may publish an exact close debt even on an early error.
        self.collect_pending_windows_cleanup_debts();
        result.and_then(|view| {
            if view
                .work_native_profile()
                .is_none_or(|profile| self.windows_view_admission_blocked(profile))
            {
                Err(ContextPortFailure::NativeRefused)
            } else {
                Ok(view)
            }
        })
    }
    fn build_windows_work_view_inner(
        &mut self,
        guard: &Arc<WorkResourceGuard>,
        native_resource: &mut Option<super::NativeResourceLease>,
    ) -> Result<AgentOwnedView, ContextPortFailure> {
        let profile = guard.resource().identity().profile();
        let work_data = guard.work_website_data();
        let storage = if work_data {
            ContextProfileStorageClass::Durable
        } else if guard.isolated_public() {
            ContextProfileStorageClass::Ephemeral
        } else {
            guard.storage()
        };
        if let Some(session) = guard.anonymous_session() {
            if !session.admits(profile, guard.resource().identity().work()) {
                return Err(ContextPortFailure::Stale);
            }
        }
        self.collect_pending_windows_cleanup_debts();
        self.retire_windows_anonymous_work_sessions();
        self.work_site_stores.retain(|_, store| {
            std::rc::Rc::strong_count(store) > 1 || !store.quiescent() || store.return_pending()
        });
        self.prune_work_profile_cookie_sources();
        if self.windows_view_admission_blocked(profile)
            || !self.windows_profile_process_group_capacity_allows(profile)
            || self.agent_cookie_quarantined_profiles.contains(&profile)
        {
            return Err(ContextPortFailure::ResourceExhausted);
        }
        let native_profile = if work_data {
            if self.profile_persistence_classes.get(&profile)
                != Some(&super::profiles::ProfilePersistenceClass::Durable)
            {
                return Err(ContextPortFailure::ProfileUnavailable);
            }
            crate::work_website_data::native_profile(profile)
        } else if storage == ContextProfileStorageClass::Ephemeral && guard.isolated_public() {
            if let Some(session) = guard.anonymous_session() {
                if let Some(entry) = self.anonymous_work_environments.get(&session.id()) {
                    if entry.retirement.is_some() {
                        return Err(ContextPortFailure::Stale);
                    }
                    entry.native_profile
                } else {
                    if self.anonymous_work_environments.len() >= zephium_agentic::MAX_LIVE_CONTEXTS
                    {
                        return Err(ContextPortFailure::ResourceExhausted);
                    }
                    let native_profile = zephium_core::ids::ProfileId::generate();
                    let retirement = guard
                        .retirement_notification(|| {
                            super::best_effort_with(|host| {
                                host.retire_windows_anonymous_work_sessions()
                            });
                        })
                        .ok_or(ContextPortFailure::NativeRefused)?;
                    if !session.register_retirement(retirement) {
                        return Err(ContextPortFailure::Stale);
                    }
                    self.anonymous_work_environments.insert(
                        session.id(),
                        AnonymousWorkEnvironment {
                            profile,
                            native_profile,
                            session: Some(session.downgrade()),
                            context: None,
                            retirement: None,
                        },
                    );
                    native_profile
                }
            } else {
                if self.anonymous_work_environments.len() >= zephium_agentic::MAX_LIVE_CONTEXTS {
                    return Err(ContextPortFailure::ResourceExhausted);
                }
                let native_profile = zephium_core::ids::ProfileId::generate();
                self.anonymous_work_environments.insert(
                    zephium_agentic::ContextRunId::generate(),
                    AnonymousWorkEnvironment {
                        profile,
                        native_profile,
                        session: None,
                        context: Some(guard.resource().identity().context()),
                        retirement: None,
                    },
                );
                native_profile
            }
        } else {
            profile
        };
        #[cfg(feature = "native-agentic-work-lifetime-diagnostic")]
        {
            use std::io::Write as _;
            let _ = writeln!(std::io::stderr().lock(), "windows-work-construction: stage=storage storage={storage:?} isolated={} anonymous={} native_matches_logical={}; content=redacted", guard.isolated_public(), guard.anonymous_session().is_some(), native_profile == profile);
        }
        let storage_root = if storage == ContextProfileStorageClass::Durable {
            &self.profiles_root
        } else {
            self.private_runtime.root()
        };
        let path = crate::erasure::prepare_profile_directory(storage_root, native_profile)
            .map_err(|_| ContextPortFailure::ProfileUnavailable)?;
        let deadline = guard
            .construction_deadline(Instant::now())
            .ok_or(ContextPortFailure::TimedOut)?;
        let mut bootstrap = self
            .begin_windows_work_profile_environment(
                native_profile,
                deadline,
                path.clone(),
                storage == ContextProfileStorageClass::Durable && !work_data,
            )
            .map_err(|_| ContextPortFailure::ProfileUnavailable)?;
        let environment = self
            .environments
            .get(&native_profile)
            .cloned()
            .ok_or(ContextPortFailure::ProfileUnavailable)?;
        let (process_id, process_generation) = self
            .capture_windows_work_environment(native_profile, environment.clone(), deadline)
            .map_err(|_| ContextPortFailure::ProfileUnavailable)?;
        let legacy = guard.clone();
        let location = guard.clone();
        let renderer = guard.clone();
        let browser = guard.clone();
        let invariant = guard.clone();
        let panic = guard.clone();
        let redirected = guard.clone();
        let unreachable = guard.clone();
        let owned_profile = if work_data {
            AgentOwnedProfile::work(crate::work_website_data::identifier(profile))
        } else if storage == ContextProfileStorageClass::Durable {
            AgentOwnedProfile::work_site(guard.document().ok_or(ContextPortFailure::NativeRefused)?)
                .map_err(|_| ContextPortFailure::NativeRefused)?
        } else if let Some(session) = guard.anonymous_session() {
            AgentOwnedProfile::work(session.id().bytes())
        } else {
            AgentOwnedProfile::work(guard.resource().identity().context().bytes())
        };
        #[cfg(feature = "native-agentic-work-lifetime-diagnostic")]
        {
            use sha2::{Digest, Sha256};
            use std::io::Write as _;
            if let AgentOwnedProfile::Automation { name } = &owned_profile {
                let hash = format!("{:x}", Sha256::digest(name.as_bytes()));
                let reused = self.work_site_stores.contains_key(&(profile, name.clone()));
                let _ = writeln!(std::io::stderr().lock(), "windows-work-cookies: stage=construct store_reused={reused} expected_native_name_hash={hash}; content=redacted");
            }
        }
        let store = if storage == ContextProfileStorageClass::Durable && !work_data {
            let AgentOwnedProfile::Automation { name } = &owned_profile else {
                return Err(ContextPortFailure::NativeRefused);
            };
            if self.work_site_stores.len() >= 256
                && !self.work_site_stores.contains_key(&(profile, name.clone()))
            {
                return Err(ContextPortFailure::ResourceExhausted);
            }
            let metadata = std::rc::Rc::new(
                crate::platform::windows::work_seed_metadata::WorkSeedMetadata::open(&path)
                    .map_err(|_| ContextPortFailure::ProfileUnavailable)?,
            );
            let target = guard.document().ok_or(ContextPortFailure::NativeRefused)?;
            let site = zephium_agentic::registrable_site(target)
                .ok_or(ContextPortFailure::NativeRefused)?;
            metadata
                .record_store(
                    &site,
                    target.as_url().scheme() == "https",
                    target
                        .as_url()
                        .port_or_known_default()
                        .ok_or(ContextPortFailure::NativeRefused)?,
                )
                .map_err(|_| ContextPortFailure::ProfileUnavailable)?;
            Some(
                self.work_site_stores
                    .entry((profile, name.clone()))
                    .or_insert_with(|| {
                        std::rc::Rc::new(crate::platform::windows::WorkStoreSeed::new(
                            metadata,
                            name.clone(),
                        ))
                    })
                    .clone(),
            )
        } else {
            None
        };
        let credential_denied = guard.clone();
        let built = crate::platform::windows::build_owned_work_view(
            &ParentHandle(self.parent.0),
            ContextOwnedViewport::STANDARD,
            &environment,
            owned_profile,
            storage,
            &path,
            deadline,
            storage == ContextProfileStorageClass::Durable && !work_data,
            AgentOwnedViewCallbacks::new(
                move |_| {
                    legacy.fail();
                    super::notify_work_resource(legacy.clone());
                },
                move || super::notify_work_resource(location.clone()),
                move || {
                    renderer.fail();
                    super::notify_work_resource(renderer.clone());
                },
                move || {
                    browser.fail();
                    super::notify_work_resource(browser.clone());
                    super::best_effort_with(move |host| {
                        host.on_profile_process_exit(native_profile, process_id, process_generation)
                    });
                },
                move || {
                    invariant.fail();
                    super::notify_work_resource(invariant.clone());
                },
                move || {
                    panic.fail();
                    super::notify_work_resource(panic.clone());
                },
            )
            .with_credential_denied(move || credential_denied.revoke_public_visual_lineage())
            .with_redirect(move |destination| {
                #[cfg(feature = "native-agentic-work-lifetime-diagnostic")]
                redirected.record_failure_cause(
                    crate::agent_context_port::work_resource_failure_diagnostic::WorkResourceFailureCause::Redirected,
                );
                redirected.record_redirect(destination);
                redirected.fail();
                super::notify_work_resource(redirected.clone());
            })
            .with_unreachable(move || {
                #[cfg(feature = "native-agentic-work-lifetime-diagnostic")]
                unreachable.record_failure_cause(
                    crate::agent_context_port::work_resource_failure_diagnostic::WorkResourceFailureCause::Unreachable,
                );
                unreachable.record_unreachable();
            }),
        );
        for debt in wry::pending_webview2_cleanup_debts() {
            let resource = native_resource.take().or_else(|| {
                self.native_resources
                    .try_acquire(super::resources::NativeResourceClass::TeardownDebt)
                    .ok()
            });
            let debt = super::OwnedWindowsCleanupDebt::new(debt, resource);
            if !debt.accounted_as_debt() {
                self.native_resource_accounting_failed = true;
            }
            self.retain_windows_cleanup_debt(native_profile, debt);
        }
        if wry::webview2_cleanup_overflowed() {
            self.fail_windows_cleanup_invariant();
        }
        let (mut view, _) = built.map_err(|_| ContextPortFailure::NativeRefused)?;
        view.set_work_native_profile(native_profile);
        if let Some(store) = store {
            let document = guard.document().ok_or(ContextPortFailure::NativeRefused)?;
            // With no tab of the profile open, this construction's own
            // page-inert controller of the profile stands in for one.
            let tab = self
                .selected_profile_cookie_source(profile, &environment)
                .ok();
            let borrowed = bootstrap
                .as_ref()
                .filter(|_| tab.is_none())
                .and_then(|bootstrap| bootstrap.profile_cookie_manager(&environment).ok());
            let borrows_bootstrap = borrowed.is_some();
            let profile_source = tab.or(borrowed);
            let origin = document.as_url().origin().ascii_serialization();
            let scope = crate::work_site_session::session_scope(document, None)
                .map_err(|_| ContextPortFailure::NativeRefused)?;
            let context = guard.resource().identity().context();
            let done = guard.clone();
            let panicked = guard.clone();
            view.seed_work_session(
                profile_source,
                scope,
                origin,
                store,
                deadline,
                move |success, unproven| {
                    done.dispatch_notification(move || {
                        super::best_effort_with(move |host| {
                            host.release_work_profile_cookie_source(context);
                            if unproven {
                                host.agent_cookie_quarantined_profiles.insert(profile);
                            }
                        });
                    });
                    if !success {
                        #[cfg(feature = "native-agentic-work-lifetime-diagnostic")]
                        {
                            done.record_failure_cause(crate::agent_context_port::work_resource_failure_diagnostic::WorkResourceFailureCause::NativeAdmission(ContextPortFailure::NativeRefused));
                        }
                        done.fail();
                    }
                    super::notify_work_resource(done.clone());
                },
                move || {
                    panicked.dispatch_notification(move || {
                        super::best_effort_with(move |host| {
                            host.release_work_profile_cookie_source(context);
                            host.agent_cookie_quarantined_profiles.insert(profile);
                        });
                    });
                    panicked.fail();
                    super::notify_work_resource(panicked.clone());
                },
            );
            if borrows_bootstrap {
                if let Some(bootstrap) = bootstrap.take() {
                    self.work_profile_cookie_sources.insert(context, bootstrap);
                }
            }
        }
        drop(bootstrap);
        Ok(view)
    }
}

/// A return's whole native window: a few seconds to copy above the transfer's
/// cleanup reserve, while the continued page waits on it.
const WORK_SESSION_RETURN_WINDOW: std::time::Duration = std::time::Duration::from_secs(15);

impl EngineHost {
    /// After a sign-in handoff on a Work page in the person's account, adds
    /// that page's site cookies to the person's profile, so their tabs are
    /// signed in too, then lets the page continue. A return that cannot start
    /// stays pending for the site's next page; the page continues either way.
    pub(super) fn return_work_site_session(&mut self, guard: &Arc<WorkResourceGuard>) {
        if !self.start_work_site_return(guard) {
            self.continue_after_work_site_return(guard);
        }
    }

    /// The continued page goes idle and resumes only now, so its suspension
    /// never overlaps the copy that reads its store.
    fn continue_after_work_site_return(&mut self, guard: &Arc<WorkResourceGuard>) {
        let context = guard.resource().identity().context();
        let Some(resource) = self
            .work_resources
            .get_mut(&context)
            .filter(|resource| Arc::ptr_eq(&resource.guard, guard))
        else {
            return;
        };
        if let Some((task, document)) = resource.human_return.take() {
            resource.finish_human_continue(task, document);
        }
    }

    fn start_work_site_return(&mut self, guard: &Arc<WorkResourceGuard>) -> bool {
        let context = guard.resource().identity().context();
        let profile = guard.resource().identity().profile();
        let pending = self
            .work_resources
            .get(&context)
            .filter(|resource| Arc::ptr_eq(&resource.guard, guard))
            .and_then(|resource| resource.view.as_ref())
            .is_some_and(|view| view.work_session_return_pending());
        if !pending
            || self.erasure_tombstones.contains(&profile)
            || self.agent_cookie_quarantined_profiles.contains(&profile)
        {
            return false;
        }
        let (Some(document), Some(deadline), Some(environment)) = (
            guard.document().cloned(),
            Instant::now().checked_add(WORK_SESSION_RETURN_WINDOW),
            self.environments.get(&profile).cloned(),
        ) else {
            return false;
        };
        self.prune_work_profile_cookie_sources();
        let (manager, bootstrap) = match self.selected_profile_cookie_source(profile, &environment)
        {
            Ok(manager) => (manager, None),
            Err(_) => {
                let bootstrap =
                    crate::erasure::prepare_profile_directory(&self.profiles_root, profile)
                        .ok()
                        .and_then(|path| {
                            self.begin_windows_work_profile_environment(
                                profile, deadline, path, true,
                            )
                            .ok()
                            .flatten()
                        });
                let manager = bootstrap
                    .as_ref()
                    .and_then(|bootstrap| bootstrap.profile_cookie_manager(&environment).ok());
                match (manager, bootstrap) {
                    (Some(manager), Some(bootstrap)) => (manager, Some(bootstrap)),
                    (_, bootstrap) => {
                        drop(bootstrap);
                        self.collect_pending_windows_cleanup_debts();
                        return false;
                    }
                }
            }
        };
        let started = self
            .work_resources
            .get(&context)
            .filter(|resource| Arc::ptr_eq(&resource.guard, guard))
            .and_then(|resource| resource.view.as_ref())
            .is_some_and(|view| {
                let current = crate::platform::imp::current_url(view.view())
                    .and_then(|current| url::Url::parse(&current).ok());
                let Ok(scope) =
                    crate::work_site_session::session_scope(&document, current.as_ref())
                else {
                    return false;
                };
                let done = guard.clone();
                let panicked = guard.clone();
                view.return_work_session(
                    manager,
                    scope,
                    deadline,
                    move |result| {
                        let continuing = done.clone();
                        done.dispatch_notification(move || {
                            super::best_effort_with(move |host| {
                                host.release_work_profile_cookie_source(context);
                                if result == crate::work_site_session::SyncResult::Unproven {
                                    host.agent_cookie_quarantined_profiles.insert(profile);
                                }
                                host.continue_after_work_site_return(&continuing);
                            });
                        });
                    },
                    move || {
                        let continuing = panicked.clone();
                        panicked.dispatch_notification(move || {
                            super::best_effort_with(move |host| {
                                host.release_work_profile_cookie_source(context);
                                host.agent_cookie_quarantined_profiles.insert(profile);
                                host.continue_after_work_site_return(&continuing);
                            });
                        });
                    },
                )
            });
        match bootstrap {
            Some(bootstrap) if started => {
                self.work_profile_cookie_sources.insert(context, bootstrap);
            }
            bootstrap => {
                drop(bootstrap);
                self.collect_pending_windows_cleanup_debts();
            }
        }
        started
    }

    pub(super) fn release_work_profile_cookie_source(
        &mut self,
        context: zephium_agentic::ContextId,
    ) {
        if self.work_profile_cookie_sources.remove(&context).is_some() {
            self.collect_pending_windows_cleanup_debts();
        }
    }

    /// Releases every borrowed profile controller, or only one profile's.
    pub(super) fn release_work_profile_cookie_sources(
        &mut self,
        profile: Option<zephium_core::ids::ProfileId>,
    ) {
        let before = self.work_profile_cookie_sources.len();
        self.work_profile_cookie_sources
            .retain(|_, bootstrap| profile.is_some_and(|profile| bootstrap.profile() != profile));
        if self.work_profile_cookie_sources.len() != before {
            self.collect_pending_windows_cleanup_debts();
        }
    }

    /// A page that ended before its sync reported back no longer needs it.
    fn prune_work_profile_cookie_sources(&mut self) {
        let before = self.work_profile_cookie_sources.len();
        let resident: std::collections::HashSet<_> = self
            .work_resources
            .iter()
            .filter(|(_, resource)| resource.view.is_some())
            .map(|(context, _)| *context)
            .collect();
        self.work_profile_cookie_sources
            .retain(|context, _| resident.contains(context));
        if self.work_profile_cookie_sources.len() != before {
            self.collect_pending_windows_cleanup_debts();
        }
    }
}

#[derive(Default)]
struct CookiePresenceResult {
    closed: std::cell::Cell<bool>,
    value: std::cell::RefCell<Option<Result<bool, ()>>>,
}
impl CookiePresenceResult {
    fn settle(&self, observed: Result<bool, ()>) {
        if self.closed.replace(true) {
            return;
        }
        if let Ok(mut value) = self.value.try_borrow_mut() {
            *value = Some(observed);
        }
    }
    fn close(&self) {
        self.closed.set(true);
    }
}

fn query_cookie_presence(
    manager: webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2CookieManager,
    url: &url::Url,
    deadline: Instant,
) -> Result<bool, ()> {
    use std::rc::Rc;
    let result = Rc::new(CookiePresenceResult::default());
    let callback_result = result.clone();
    let handler = webview2_com::GetCookiesCompletedHandler::create(Box::new(
        move |status, cookies| {
            if callback_result.closed.get() {
                return Ok(());
            }
            let observed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                status.map_err(|_| ())?;
                let cookies = cookies.ok_or(())?;
                let mut count = 0;
                // SAFETY: this callback owns the native list for the STA call; output is initialized and bounded.
                unsafe { cookies.Count(&mut count) }.map_err(|_| ())?;
                if !(0..=8192).contains(&count) {
                    return Err(());
                }
                #[cfg(feature = "native-agentic-work-lifetime-diagnostic")]
                {
                    use std::io::Write as _;
                    let mut session = 0; let mut persistent = 0; let mut future = 0; let mut invalid = 0;
                    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).ok().map(|time| time.as_secs_f64());
                    let sampled = count.min(8);
                    for index in 0..sampled {
                        // SAFETY: index is below this callback-owned native list's bounded Count.
                        let cookie = match unsafe { cookies.GetValueAtIndex(index) } {
                            Ok(cookie) => cookie,
                            Err(_) => { invalid += 1; continue; }
                        };
                        let mut is_session = windows_core::BOOL::default();
                        let mut expires = 0.0;
                        // SAFETY: this callback retains the exact cookie COM owner; outputs are initialized, and no names or values are read.
                        if unsafe { cookie.IsSession(&mut is_session).and_then(|_| cookie.Expires(&mut expires)) }.is_err() { invalid += 1; continue; }
                        if is_session.as_bool() { session += 1; } else {
                            persistent += 1;
                            if expires.is_finite() && now.is_some_and(|now| expires > now) { future += 1; }
                        }
                    }
                    let _ = writeln!(std::io::stderr().lock(), "windows-work-cookies: stage=native_query count={count} sampled={sampled} session={session} persistent={persistent} expires_future={future} invalid={invalid}; content=redacted");
                }
                Ok(count > 0)
            }))
            .unwrap_or(Err(()));
            callback_result.settle(observed);
            Ok(())
        },
    ));
    let value = windows_core::HSTRING::from(url.as_str());
    // SAFETY: the owning controller remains live through this bounded STA query; URL/handler are retained.
    if unsafe { manager.GetCookies(windows_core::PCWSTR(value.as_ptr()), &handler) }.is_err() {
        result.close();
        return Err(());
    }
    loop {
        if Instant::now() >= deadline {
            result.close();
            return Err(());
        }
        if let Some(observed) = result.value.borrow_mut().take() {
            return observed;
        }
        if !crate::platform::windows::pump_browser_exit_callbacks(deadline) {
            result.close();
            return Err(());
        }
    }
}

#[cfg(feature = "native-agentic-work-lifetime-diagnostic")]
fn diagnose_cookie_profile(view: &AgentOwnedView, stage: &'static str) {
    use std::io::Write as _;
    let hash = view.work_cookie_profile_hash();
    view.diagnose_work_cookie_disk();
    let _ = writeln!(
        std::io::stderr().lock(),
        "windows-work-cookies: stage={stage} native_name_hash={hash:?}; content=redacted"
    );
}

#[cfg(feature = "native-agentic-work-lifetime-diagnostic")]
pub(super) fn diagnose_work_cookie_before_close(
    view: &AgentOwnedView,
    target: &zephium_agentic::ContextNavigationTarget,
) {
    use std::io::Write as _;
    diagnose_cookie_profile(view, "before_close");
    let deadline = Instant::now() + std::time::Duration::from_millis(500);
    let observed = view
        .work_presence_cookie_manager()
        .and_then(|manager| query_cookie_presence(manager, target.as_url(), deadline));
    let _ = writeln!(
        std::io::stderr().lock(),
        "windows-work-cookies: stage=before_close_terminal presence={observed:?}; content=redacted"
    );
}

#[cfg(test)]
mod cookie_presence_tests {
    use super::CookiePresenceResult;
    #[test]
    fn timed_out_native_reply_cannot_publish_late_or_certify_absence() {
        let result = CookiePresenceResult::default();
        result.close();
        result.settle(Ok(false));
        assert!(result.value.borrow().is_none());
    }
    #[test]
    fn first_native_error_stays_unknown_and_cannot_be_replaced_by_empty() {
        let result = CookiePresenceResult::default();
        result.settle(Err(()));
        result.settle(Ok(false));
        assert_eq!(*result.value.borrow(), Some(Err(())));
        let empty = CookiePresenceResult::default();
        empty.settle(Ok(false));
        assert_eq!(*empty.value.borrow(), Some(Ok(false)));
    }
}
