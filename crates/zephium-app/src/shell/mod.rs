alse;
            }
            Err(_) => {
                crate::diagnostic!("shutdown: storage durability preflight panicked");
                terminal_clean = false;
            }
        }

        #[cfg(feature = "agentic-browser")]
        let agent_lifecycle_clean = self.shutdown_agent_lifecycle_until(storage_deadline);
        #[cfg(not(feature = "agentic-browser"))]
        let agent_lifecycle_clean = true;
        // Fold every result already published before Store's terminal
        // barrier while ordinary Store/native admission is still valid. Any
        // follow-up reconciliation is then ordered ahead of Store shutdown.
        // A projection or adapter panic is terminal, but cannot skip the
        // independent Store/native/blocker barriers below.
        let pre_store_coordination_clean = if std::time::Instant::now() >= storage_deadline {
            crate::diagnostic!("shutdown: pre-Store blocker result folding deadline passed");
            false
        } else if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.drain_blocker_inbox();
        }))
        .is_err()
        {
            crate::diagnostic!("shutdown: pre-Store blocker result folding panicked");
            false
        } else {
            true
        };
        if !pre_store_coordination_clean {
            crate::diagnostic!("shutdown: pre-Store blocker result folding panicked");
        }
        terminal_clean &= self.flush_blocker_statistics_until(storage_deadline);
        let storage_clean = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.store.shutdown_until(storage_deadline)
        })) {
            Ok(StoreShutdownOutcome::Clean) => true,
            Ok(StoreShutdownOutcome::RetryableFailure) => {
                crate::diagnostic!("shutdown: storage rejected terminal teardown");
                false
            }
            Ok(StoreShutdownOutcome::Unclean) => {
                crate::diagnostic!(
                    "shutdown: storage actor termination was not proven before the deadline"
                );
                false
            }
            Err(_) => {
                crate::diagnostic!("shutdown: storage terminal barrier panicked");
                false
            }
        };

        // Projection callbacks are composition ports too. Contain this phase
        // independently so an unhealthy UI cannot skip native or blocker
        // teardown after terminal ownership transfer has begun.
        let post_store_coordination_clean =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                self.discard_blocker_inbox_for_shutdown();
                self.finish_pending_blocker_operations_for_shutdown();
                self.fail_pending_history_calls();
                self.fail_pending_history_lists();
                self.fail_pending_bookmark_calls();
                self.fail_pending_extension_data();
            }))
            .is_ok();
        if !post_store_coordination_clean {
            crate::diagnostic!("shutdown: pending operation finalization panicked");
        }
        let coordination_clean = pre_store_coordination_clean && post_store_coordination_clean;
        let reads_stopped = self.store_reads.as_ref().is_none_or(|reads| {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| reads.stop())).is_ok()
        });
        if !reads_stopped {
            crate::diagnostic!("shutdown: storage-reader stop panicked");
        }

        let (native_clean, blocker_clean) = self.shutdown_native_and_blocker_until(deadline);

        let clean = terminal_clean
            && agent_lifecycle_clean
            && storage_clean
            && coordination_clean
            && reads_stopped
            && native_clean
            && blocker_clean;
        if !clean {
            crate::diagnostic!(
                "shutdown: clean proof incomplete terminal={terminal_clean} agent_lifecycle={agent_lifecycle_clean} storage={storage_clean} coordination={coordination_clean} readers={reads_stopped} native={native_clean} blocker={blocker_clean}"
            );
        }
        let outcome = if clean {
            ShutdownOutcome::Clean
        } else {
            ShutdownOutcome::Unclean
        };
        self.recent_tabs = Default::default();
        self.tab_previews = Default::default();
        self.shutdown_result = Some(outcome);
        let _ = ack.send(outcome);
    }

    fn shutdown_native_and_blocker_until(&self, deadline: std::time::Instant) -> (bool, bool) {
        let (native_done, native_wait) = sync_channel(1);
        let native_admitted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.engine.shutdown(Box::new(move |clean| {
                let _ = native_done.send(clean);
            }));
        }))
        .is_ok();
        if !native_admitted {
            crate::diagnostic!("shutdown: native cleanup admission panicked");
        }

        // Native teardown and blocker joins are independent and share the
        // same caller-owned absolute deadline.
        let blocker_clean = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.blocker.shutdown_until(deadline)
        })) {
            Ok(BlockerShutdownOutcome::Clean) => true,
            Ok(BlockerShutdownOutcome::Unclean) => {
                crate::diagnostic!(
                    "shutdown: content-policy compiler termination was not proven before the deadline"
                );
                false
            }
            Err(_) => {
                crate::diagnostic!("shutdown: content-policy compiler shutdown panicked");
                false
            }
        };
        let native_budget = deadline.saturating_duration_since(std::time::Instant::now());
        // Observe the callback independently even when admission panicked: a
        // faulty adapter may have retained callback ownership before unwind.
        let native_ack_clean = native_wait.recv_timeout(native_budget).unwrap_or(false);
        let native_clean = native_admitted && native_ack_clean;
        if !native_clean {
            crate::diagnostic!(
                "shutdown: native cleanup did not acknowledge cleanly; forcing process exit"
            );
        }
        (native_clean, blocker_clean)
    }

    /// Best-effort terminal cleanup for actor unwind or loss of the last
    /// public handle. No mutable state is persisted and no refusal is
    /// retryable because the sole authoritative actor is already exiting.
    pub(super) fn cleanup_after_unexpected_exit_until(
        &mut self,
        deadline: std::time::Instant,
    ) -> bool {
        let reads_quiesced = self.store_reads.as_ref().is_none_or(|reads| {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                reads.quiesce_until(deadline)
            }))
            .unwrap_or(false)
        });
        let reads_stopped = self.store_reads.as_ref().is_none_or(|reads| {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| reads.stop())).is_ok()
        });
        #[cfg(feature = "agentic-browser")]
        let agent_lifecycle_clean = self.shutdown_agent_lifecycle_until(deadline);
        #[cfg(not(feature = "agentic-browser"))]
        let agent_lifecycle_clean = true;
        let storage_clean = matches!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                self.store.shutdown_until(deadline)
            })),
            Ok(StoreShutdownOutcome::Clean)
        );
        if !storage_clean {
            crate::diagnostic!(
                "shutdown: storage cleanup was not proven during unexpected shell exit"
            );
        }
        let (native_clean, blocker_clean) = self.shutdown_native_and_blocker_until(deadline);
        reads_quiesced
            && reads_stopped
            && agent_lifecycle_clean
            && storage_clean
            && native_clean
            && blocker_clean
    }

    pub(super) fn report_terminal_failure(&mut self, failure: ShellTerminalFailure) {
        let Some(callback) = self.terminal_failure.take() else {
            return;
        };
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| callback(failure))).is_err() {
            crate::diagnostic!("shutdown: terminal shell failure handoff panicked");
            self.terminal_failure_handoff_panicked = true;
        }
    }

    /// Consumes the optional complete agent runtime before terminal Store and
    /// engine teardown. A clean result cannot exist without its native zero
    /// proof; the proof is deliberately consumed inside the actor barrier.
    #[cfg(feature = "agentic-browser")]
    fn shutdown_agent_lifecycle_until(&mut self, deadline: std::time::Instant) -> bool {
        #[cfg(feature = "work-execution")]
        let mut buried = true;
        #[cfg(feature = "work-execution")]
        if self.retained_page_runtime.is_some() {
            for page in &mut self.retained_pages {
                page.begin_shutdown();
            }
            let open = |shell: &Self| {
                shell.retained_pages.iter().any(|page| !page.is_closed())
                    || shell
                        .retained_graveyard
                        .iter()
                        .any(|work| work.awaits_native_close())
            };
            while std::time::Instant::now() < deadline && open(self) {
                self.grant_native_audit();
                for work in self
                    .retained_pages
                    .iter_mut()
                    .chain(&mut self.retained_graveyard)
                {
                    work.poll();
                }
                if open(self) {
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
            }
            buried &= self.retained_pages.iter().all(|page| page.is_closed());
        }
        #[cfg(feature = "work-execution")]
        let retained_clean = self
            .retained_work
            .as_mut()
            .map(|work| work.shutdown_until(deadline));
        // What the group could not close audits last, one at a time.
        #[cfg(feature = "work-execution")]
        for work in &mut self.retained_graveyard {
            work.allow_group_shutdown();
            buried &= work.shutdown_until(deadline);
        }
        #[cfg(feature = "work-execution")]
        if let Some(clean) = retained_clean {
            return clean && buried;
        }
        #[cfg(feature = "work-execution")]
        if !buried {
            return false;
        }
        #[cfg(feature = "work-execution")]
        if let Some(work) = &mut self.work {
            return work.shutdown_until(deadline);
        }
        let owner = std::mem::replace(&mut self.agent_lifecycle, AgentLifecycleOwner::Consumed);
        let lifecycle = match owner {
            AgentLifecycleOwner::Absent => return true,
            AgentLifecycleOwner::Owned(lifecycle) => lifecycle,
            AgentLifecycleOwner::Consumed => {
                crate::diagnostic!("shutdown: agent browser lifecycle owner is missing");
                return false;
            }
        };
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            lifecycle.shutdown_until(deadline)
        })) {
            Ok(AgentBrowserShutdownOutcome::Clean(native_zero_proof)) => {
                drop(native_zero_proof);
                true
            }
            Ok(AgentBrowserShutdownOutcome::Unclean) => {
                crate::diagnostic!(
                    "shutdown: agent browser lifecycle did not prove complete cleanup"
                );
                false
            }
            Err(_) => {
                crate::diagnostic!("shutdown: agent browser lifecycle shutdown panicked");
                false
            }
        }
    }

    fn clear_pending_store_reads(&mut self) {
        self.clear_pending_favicon_probe_lookups();
        self.search.pending = None;
        self.favicons.pending_batch = None;
        self.favicons.store_reads.clear();
    }

    fn on_store_read(&mut self, result: StoreReadResult) {
        match result {
            StoreReadResult::History {
                generation,
                profile,
                query,
                hits,
                bookmarks,
            } => self.on_history_read(generation, profile, query, hits, bookmarks),
            StoreReadResult::Favicon {
                generation,
                id,
                profile,
                origin,
                icon,
                page,
            } => self.on_favicon_read(generation, id, profile, origin, icon, page),
            StoreReadResult::FaviconBatch {
                generation,
                profile,
                space,
                origins,
                rasters,
            } => self.on_favicon_batch_read(generation, profile, space, origins, rasters),
            StoreReadResult::FaviconProbe {
                generation,
                profile,
                origins,
                rasters,
            } => self.on_favicon_probe_read(generation, profile, origins, rasters),
            StoreReadResult::HistorySurface {
                token,
                profile,
                visits,
                next,
                removed,
            } => self.on_history_surface_read(token, profile, visits, next, removed),
            StoreReadResult::HistorySurfaceFailed { token, profile } => {
                self.on_history_surface_failed(token, profile)
            }
            StoreReadResult::Bookmarks {
                token,
                profile,
                reply,
            } => self.on_bookmarks_read(token, profile, reply),
            StoreReadResult::Imported { token, added } => self.on_import_read(token, added),
            StoreReadResult::Exported { token, text } => self.on_export_read(token, text),
            StoreReadResult::ExtensionData { token, reply } => {
                self.on_extension_data_read(token, reply)
            }
            StoreReadResult::ExtensionDataChanged { profile, change } => {
                self.on_extension_data_changed(profile, *change)
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests;

#[cfg(all(test, feature = "work-runtime"))]
mod work_address_tests;
#[cfg(all(test, feature = "work-planning"))]
mod work_context_tests;
#[cfg(all(test, feature = "work-runtime"))]
mod work_coordination_tests;
#[cfg(all(test, feature = "work-runtime"))]
mod work_lead_tests;
#[cfg(all(test, feature = "work-runtime"))]
mod work_personal_tests;
#[cfg(all(test, feature = "work-planning"))]
mod work_planning_tests;
#[cfg(all(test, feature = "work-runtime"))]
mod work_product_tests;
#[cfg(all(test, feature = "work-runtime"))]
mod work_runtime_tests;
#[cfg(all(test, feature = "work-runtime"))]
mod work_stop_tests;
