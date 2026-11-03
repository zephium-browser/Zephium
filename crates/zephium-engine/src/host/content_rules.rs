use std::sync::Arc;

use zephium_core::blocker::{
    ContentPolicyGeneration, ContentRuleApplyFailure, ContentRules, ContentRulesPayload,
};
use zephium_core::ids::ProfileId;
use zephium_core::ports::engine::{
    ContentRuleSettlement, ContentRuleValidationOutcome, EngineEvent,
};

#[cfg(not(target_os = "windows"))]
use super::dispatch::{with_content_policy_settlement, with_content_policy_timeout};
use super::{
    AppliedContentPolicy, CompilingContentPolicy, EngineHost, ProfileContentPolicy,
    QueuedContentPolicy,
};
#[cfg(not(target_os = "windows"))]
use super::{
    ContentRuleCacheGcAttempt, ContentRuleCacheGcPhase, DeclarativeContentPolicyAttempt,
    DeclarativeContentPolicyJob, DeclarativeContentPolicyMaintenance,
};

#[cfg(not(target_os = "windows"))]
const MAX_SHARED_NATIVE_CONTENT_POLICIES: usize = zephium_core::session::MAX_SESSION_PROFILES * 2;
#[cfg(not(target_os = "windows"))]
const MAX_DECLARATIVE_CONTENT_POLICY_JOBS: usize = 2;
#[cfg(not(target_os = "windows"))]
const MAX_RESIDENT_DECLARATIVE_CONTENT_POLICY_BYTES: usize =
    zephium_core::blocker::MAX_DECLARATIVE_RULE_BYTES * 2;
#[cfg(not(target_os = "windows"))]
// The guarded host-boundary formatter cold-compiles the September release
// corpus in about 2.2 seconds on the qualification Mac. Keep a finite safety
// envelope for other hardware; the native CI performance gate is 15 seconds.
// Cancellation remains advisory and never permits a second physical compile.
const DECLARATIVE_CONTENT_POLICY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);
#[cfg(not(target_os = "windows"))]
const CONTENT_RULE_CACHE_GC_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);
#[cfg(not(target_os = "windows"))]
const MAX_CONTENT_RULE_CACHE_GC_CANDIDATES: usize = 8;
#[cfg(not(target_os = "windows"))]
const CONTENT_RULE_IDENTIFIER_PREFIX: &str = "app.zephium.rules.v1.";

impl EngineHost {
    /// Validate a source candidate through the same bounded native compiler
    /// queue as installed profiles. No dummy profile or navigation is created.
    pub(crate) fn validate_content_rules(
        &mut self,
        rules: Arc<ContentRules>,
        completion: zephium_core::ports::engine::ContentRuleValidationCompletion,
    ) {
        if matches!(rules.payload(), ContentRulesPayload::AllowAll) {
            completion.finish(ContentRuleValidationOutcome::Rejected(
                ContentRuleApplyFailure::UnsupportedArtifact,
            ));
            return;
        }
        #[cfg(not(target_os = "windows"))]
        if self.shutdown_completion.is_some() {
            return;
        }
        #[cfg(target_os = "windows")]
        {
            completion.finish(match crate::platform::imp::prepare_content_policy(&rules) {
                Ok(_) => ContentRuleValidationOutcome::Valid,
                Err(error) => ContentRuleValidationOutcome::Rejected(error),
            });
        }
        #[cfg(not(target_os = "windows"))]
        {
            let ContentRulesPayload::Declarative {
                format,
                artifact_digest,
                encoded,
            } = rules.payload()
            else {
                return;
            };
            if *format != zephium_core::blocker::DeclarativeRuleFormat::WebKitContentBlockerV1 {
                return;
            }
            self.validate_declarative_artifact(
                encoded.clone(),
                *artifact_digest.as_bytes(),
                completion,
            );
        }
    }

    #[cfg(not(target_os = "windows"))]
    fn validate_declarative_artifact(
        &mut self,
        encoded: Arc<dyn zephium_core::blocker::DeclarativeRuleText>,
        digest: [u8; 32],
        completion: zephium_core::ports::engine::ContentRuleValidationCompletion,
    ) {
        if self.content_rule_preflight.is_some() || self.shutdown_completion.is_some() {
            return;
        }
        if self
            .declarative_content_policy_cache
            .get(&digest)
            .and_then(std::rc::Weak::upgrade)
            .is_some()
        {
            self.protect_preflight_digest(digest);
            completion.finish(ContentRuleValidationOutcome::Valid);
            return;
        }
        if !self.queue_native_artifact(encoded, digest) {
            return;
        }
        self.content_rule_preflight = Some((digest, completion));
        self.start_next_declarative_content_policy_compilation();
    }

    #[cfg(not(target_os = "windows"))]
    fn protect_preflight_digest(&mut self, digest: [u8; 32]) {
        self.preflight_cache_digests.retain(|d| *d != digest);
        self.preflight_cache_digests.push_front(digest);
        self.preflight_cache_digests.truncate(2);
    }

    #[cfg(not(target_os = "windows"))]
    pub(super) fn queue_native_artifact(
        &mut self,
        encoded: Arc<dyn zephium_core::blocker::DeclarativeRuleText>,
        digest: [u8; 32],
    ) -> bool {
        if self
            .active_declarative_content_policy_maintenance
            .as_ref()
            .and_then(active_compilation)
            .is_some_and(|active| active.timed_out)
        {
            return false;
        }
        if self
            .declarative_content_policy_compilations
            .contains_key(&digest)
        {
            return true;
        }
        let encoded_bytes = encoded.len();
        let Some(total) = self
            .declarative_content_policy_bytes
            .checked_add(encoded_bytes)
            .filter(|bytes| *bytes <= MAX_RESIDENT_DECLARATIVE_CONTENT_POLICY_BYTES)
        else {
            return false;
        };
        if self.declarative_content_policy_compilations.len() >= MAX_DECLARATIVE_CONTENT_POLICY_JOBS
        {
            return false;
        }
        self.declarative_content_policy_bytes = total;
        self.declarative_content_policy_compilations
            .insert(digest, Vec::new());
        self.declarative_content_policy_queue
            .push_back(DeclarativeContentPolicyJob {
                digest,
                encoded_bytes,
                encoded,
            });
        true
    }

    #[cfg(not(target_os = "windows"))]
    fn finish_content_rule_preflight(
        &mut self,
        digest: [u8; 32],
        outcome: ContentRuleValidationOutcome,
    ) {
        if self
            .content_rule_preflight
            .as_ref()
            .is_some_and(|(expected, _)| *expected == digest)
        {
            let (_, completion) = self
                .content_rule_preflight
                .take()
                .expect("matching preflight");
            if outcome == ContentRuleValidationOutcome::Valid {
                self.protect_preflight_digest(digest);
            }
            completion.finish(outcome);
        }
    }

    pub(crate) fn install_content_rules(
        &mut self,
        profile: ProfileId,
        generation: ContentPolicyGeneration,
        rules: Arc<ContentRules>,
    ) {
        if self.erasure_tombstones.contains(&profile) {
            return;
        }
        let is_new = !self.content_policies.contains_key(&profile);
        if is_new && self.content_policies.len() >= zephium_core::session::MAX_SESSION_PROFILES {
            self.emit_content_policy_settlement(
                profile,
                generation,
                ContentRuleSettlement::Unavailable {
                    failure: ContentRuleApplyFailure::NativeInstallation,
                },
            );
            return;
        }
        let (current, applied) = content_policy_generations(self.content_policies.get(&profile));
        if current.is_some_and(|current| generation <= current) {
            self.emit_content_policy_settlement(
                profile,
                generation,
                failed_settlement(applied, ContentRuleApplyFailure::Superseded),
            );
            return;
        }
        let state = self.content_policies.entry(profile).or_default();
        let superseded_queued = state
            .queued
            .replace(QueuedContentPolicy { generation, rules });
        if let Some(superseded) = superseded_queued {
            self.emit_content_policy_settlement(
                profile,
                superseded.generation,
                failed_settlement(applied, ContentRuleApplyFailure::Superseded),
            );
        }
        let superseded_compiling = self
            .content_policies
            .get_mut(&profile)
            .and_then(|state| state.compiling.as_mut())
            .filter(|compiling| !compiling.superseded)
            .map(|compiling| {
                compiling.superseded = true;
                compiling.generation
            });
        if let Some(superseded) = superseded_compiling {
            self.emit_content_policy_settlement(
                profile,
                superseded,
                failed_settlement(applied, ContentRuleApplyFailure::Superseded),
            );
            return;
        }
        self.start_queued_content_policy(profile);
    }

    fn start_queued_content_policy(&mut self, profile: ProfileId) {
        enum Next {
            Busy,
            Idle,
            Queued(QueuedContentPolicy),
        }
        let next = {
            let Some(state) = self.content_policies.get_mut(&profile) else {
                return;
            };
            if state.compiling.is_some() {
                Next::Busy
            } else if let Some(queued) = state.queued.take() {
                state.compiling = Some(CompilingContentPolicy {
                    generation: queued.generation,
                    cosmetics: queued.rules.cosmetics().cloned(),
                    superseded: false,
                });
                Next::Queued(queued)
            } else {
                Next::Idle
            }
        };
        let queued = match next {
            Next::Busy => return,
            Next::Idle => {
                #[cfg(not(target_os = "windows"))]
                self.start_next_declarative_content_policy_compilation();
                return;
            }
            Next::Queued(queued) => queued,
        };
        let generation = queued.generation;
        let rules = queued.rules;
        match rules.payload() {
            ContentRulesPayload::AllowAll => {
                self.finish_content_policy_compilation(
                    profile,
                    generation,
                    Ok(std::rc::Rc::new(
                        crate::platform::imp::NativeContentPolicy::AllowAll,
                    )),
                );
            }
            #[cfg(target_os = "windows")]
            ContentRulesPayload::Runtime(_) => {
                let prepared =
                    crate::platform::imp::prepare_content_policy(&rules).map(std::rc::Rc::new);
                self.finish_content_policy_compilation(profile, generation, prepared);
            }
            #[cfg(not(target_os = "windows"))]
            ContentRulesPayload::Declarative {
                format,
                artifact_digest,
                encoded,
            } => {
                if *format != zephium_core::blocker::DeclarativeRuleFormat::WebKitContentBlockerV1 {
                    self.finish_content_policy_compilation(
                        profile,
                        generation,
                        Err(ContentRuleApplyFailure::UnsupportedArtifact),
                    );
                    return;
                }
                let digest = *artifact_digest.as_bytes();
                self.declarative_content_policy_cache
                    .retain(|_, cached| cached.strong_count() != 0);
                if let Some(cached) = self
                    .declarative_content_policy_cache
                    .get(&digest)
                    .and_then(std::rc::Weak::upgrade)
                {
                    self.finish_content_policy_compilation(profile, generation, Ok(cached));
                    return;
                }
                if self
                    .active_declarative_content_policy_maintenance
                    .as_ref()
                    .and_then(active_compilation)
                    .is_some_and(|active| active.timed_out)
                {
                    self.finish_content_policy_compilation(
                        profile,
                        generation,
                        Err(ContentRuleApplyFailure::NativeCompilation),
                    );
                    return;
                }
                if let Some(waiters) = self
                    .declarative_content_policy_compilations
                    .get_mut(&digest)
                {
                    let timed_out = self
                        .active_declarative_content_policy_maintenance
                        .as_ref()
                        .and_then(active_compilation)
                        .is_some_and(|active| active.digest == digest && active.timed_out);
                    if timed_out || waiters.len() >= zephium_core::session::MAX_SESSION_PROFILES {
                        self.finish_content_policy_compilation(
                            profile,
                            generation,
                            Err(ContentRuleApplyFailure::NativeCompilation),
                        );
                    } else {
                        waiters.push((profile, generation));
                    }
                    return;
                }
                let encoded_bytes = encoded.len();
                let Some(total_bytes) = self
                    .declarative_content_policy_bytes
                    .checked_add(encoded_bytes)
                else {
                    self.finish_content_policy_compilation(
                        profile,
                        generation,
                        Err(ContentRuleApplyFailure::NativeCompilation),
                    );
                    return;
                };
                if self.declarative_content_policy_compilations.len()
                    >= MAX_DECLARATIVE_CONTENT_POLICY_JOBS
                    || total_bytes > MAX_RESIDENT_DECLARATIVE_CONTENT_POLICY_BYTES
                {
                    self.finish_content_policy_compilation(
                        profile,
                        generation,
                        Err(ContentRuleApplyFailure::NativeCompilation),
                    );
                    return;
                }
                self.declarative_content_policy_compilations
                    .insert(digest, vec![(profile, generation)]);
                self.declarative_content_policy_bytes = total_bytes;
                self.declarative_content_policy_queue
                    .push_back(DeclarativeContentPolicyJob {
                        digest,
                        encoded_bytes,
                        encoded: encoded.clone(),
                    });
                self.start_next_declarative_content_policy_compilation();
            }
            _ => {
                self.finish_content_policy_compilation(
                    profile,
                    generation,
                    Err(ContentRuleApplyFailure::UnsupportedArtifact),
                );
            }
        }
    }

    #[cfg(not(target_os = "windows"))]
    pub(super) fn start_next_declarative_content_policy_compilation(&mut self) {
        if self.active_declarative_content_policy_maintenance.is_some() {
            return;
        }
        let job = loop {
            let Some(job) = self.declarative_content_policy_queue.pop_front() else {
                self.start_content_rule_cache_gc();
                return;
            };
            let has_waiters = self
                .declarative_content_policy_compilations
                .get(&job.digest)
                .is_some_and(|waiters| !waiters.is_empty());
            if has_waiters
                || self
                    .content_rule_preflight
                    .as_ref()
                    .is_some_and(|(digest, _)| *digest == job.digest)
            {
                break job;
            }
            self.declarative_content_policy_compilations
                .remove(&job.digest);
            self.release_declarative_content_policy_bytes(job.encoded_bytes);
        };

        let attempt = self.next_declarative_content_policy_maintenance_attempt;
        let Some(next_attempt) = attempt.checked_add(1) else {
            self.fail_unstarted_declarative_content_policy(job);
            self.start_next_declarative_content_policy_compilation();
            return;
        };
        self.next_declarative_content_policy_maintenance_attempt = next_attempt;
        self.active_declarative_content_policy_maintenance = Some(
            DeclarativeContentPolicyMaintenance::Compilation(DeclarativeContentPolicyAttempt {
                id: attempt,
                digest: job.digest,
                encoded_bytes: job.encoded_bytes,
                timed_out: false,
                watchdog: None,
                cancellation: None,
            }),
        );

        let digest = job.digest;
        let timeout_terminal_failure = self.native_terminal_failure.clone();
        let Some(watchdog) = crate::platform::imp::schedule_content_policy_timeout(
            DECLARATIVE_CONTENT_POLICY_TIMEOUT,
            move || {
                let admitted = with_content_policy_timeout(move |host| {
                    host.timeout_declarative_content_policy_compilation(attempt, digest);
                });
                if !admitted {
                    eprintln!("content blocker: native compilation timeout was not admitted");
                    timeout_terminal_failure(
                        "native content-policy timeout lost its exact host admission",
                    );
                }
            },
        ) else {
            self.active_declarative_content_policy_maintenance = None;
            self.fail_unstarted_declarative_content_policy(job);
            self.start_next_declarative_content_policy_compilation();
            return;
        };
        if let Some(active) = self
            .active_declarative_content_policy_maintenance
            .as_mut()
            .and_then(active_compilation_mut)
            .filter(|active| active.id == attempt && active.digest == digest)
        {
            active.watchdog = Some(watchdog);
        } else {
            watchdog.cancel();
            self.fail_unstarted_declarative_content_policy(job);
            self.start_next_declarative_content_policy_compilation();
            return;
        }

        let settlement_terminal_failure = self.native_terminal_failure.clone();
        let callback = move |result| {
            let admitted = with_content_policy_settlement(digest, move |host| {
                host.finish_shared_declarative_compilation(attempt, digest, result);
            });
            if !admitted {
                eprintln!("content blocker: native compilation settlement was not admitted");
                settlement_terminal_failure(
                    "native content-policy completion lost its exact host admission",
                );
            }
        };
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        let cancellation = crate::platform::imp::compile_content_policy(
            &self.content_rule_cache,
            job.encoded,
            digest,
            callback,
        );
        #[cfg(all(unix, not(target_os = "macos")))]
        let cancellation = crate::platform::imp::compile_content_policy(
            &self.content_rule_cache,
            job.encoded,
            digest,
            callback,
        );
        if let Some(active) = self
            .active_declarative_content_policy_maintenance
            .as_mut()
            .and_then(active_compilation_mut)
            .filter(|active| active.id == attempt && active.digest == digest)
        {
            active.cancellation = Some(cancellation);
        } else {
            cancellation.cancel();
        }
    }

    #[cfg(not(target_os = "windows"))]
    // A timeout settles logical waiters but deliberately leaves this exact
    // physical attempt installed. Its late native callback is the only event
    // allowed to release the slot and advance the global compiler queue.
    fn finish_shared_declarative_compilation(
        &mut self,
        attempt: u64,
        digest: [u8; 32],
        result: Result<crate::platform::imp::NativeContentPolicy, ContentRuleApplyFailure>,
    ) {
        let Some(active) = self
            .active_declarative_content_policy_maintenance
            .as_mut()
            .and_then(active_compilation_mut)
        else {
            return;
        };
        if active.id != attempt || active.digest != digest {
            return;
        }
        if let Some(watchdog) = active.watchdog.take() {
            watchdog.cancel();
        }
        let waiters = self
            .declarative_content_policy_compilations
            .remove(&digest)
            .unwrap_or_default();
        let result = result.map(std::rc::Rc::new);
        let preflight = if self.shutdown_completion.is_some() {
            ContentRuleValidationOutcome::Unavailable
        } else {
            match &result {
                Ok(_) => ContentRuleValidationOutcome::Valid,
                Err(error) => ContentRuleValidationOutcome::Rejected(*error),
            }
        };
        self.finish_content_rule_preflight(digest, preflight);
        if self.shutdown_completion.is_none() {
            if let Ok(native) = &result {
                self.content_rule_cache_gc_pending = true;
                self.declarative_content_policy_cache
                    .retain(|_, cached| cached.strong_count() != 0);
                if self.declarative_content_policy_cache.len() < MAX_SHARED_NATIVE_CONTENT_POLICIES
                {
                    self.declarative_content_policy_cache
                        .insert(digest, std::rc::Rc::downgrade(native));
                }
            }
        }
        for (profile, generation) in waiters {
            let result = match &result {
                Ok(native) => Ok(native.clone()),
                Err(failure) => Err(*failure),
            };
            self.finish_content_policy_compilation(profile, generation, result);
        }
        let Some(DeclarativeContentPolicyMaintenance::Compilation(active)) =
            self.active_declarative_content_policy_maintenance.take()
        else {
            return;
        };
        self.release_declarative_content_policy_bytes(active.encoded_bytes);
        if self.shutdown_completion.is_some() {
            // Drop the callback result and exact native cancellation handle
            // before acknowledging the process-lifecycle barrier.
            drop(result);
            drop(active);
            self.finish_content_policy_shutdown_if_quiescent();
        } else {
            self.start_next_declarative_content_policy_compilation();
        }
    }

    #[cfg(not(target_os = "windows"))]
    // Cancellation is advisory: settle callers and drain queued logical work,
    // but retain the physical slot until `finish_shared...` receives the exact
    // attempt callback. Starting a second native compile here could overlap a
    // WebKit operation whose cancellation has not reached a terminal state.
    fn timeout_declarative_content_policy_compilation(&mut self, attempt: u64, digest: [u8; 32]) {
        let Some(active) = self
            .active_declarative_content_policy_maintenance
            .as_mut()
            .and_then(active_compilation_mut)
        else {
            return;
        };
        if active.id != attempt || active.digest != digest || active.timed_out {
            return;
        }
        if let Some(watchdog) = active.watchdog.take() {
            watchdog.cancel();
        }
        active.timed_out = true;
        if let Some(cancellation) = &active.cancellation {
            cancellation.cancel();
        }

        // The timed-out physical compiler also drains queued candidates.
        self.content_rule_preflight.take();
        let active_waiters = self
            .declarative_content_policy_compilations
            .get_mut(&digest)
            .map(std::mem::take)
            .unwrap_or_default();
        let queued = std::mem::take(&mut self.declarative_content_policy_queue);
        let mut queued_waiters = Vec::new();
        for job in queued {
            self.release_declarative_content_policy_bytes(job.encoded_bytes);
            if let Some(waiters) = self
                .declarative_content_policy_compilations
                .remove(&job.digest)
            {
                queued_waiters.extend(waiters);
            }
        }
        for (profile, generation) in active_waiters.into_iter().chain(queued_waiters) {
            self.finish_content_policy_compilation(
                profile,
                generation,
                Err(ContentRuleApplyFailure::NativeCompilation),
            );
        }
        eprintln!(
            "content blocker: native WebKit compiler exceeded its deadline; no second physical compile will start before its callback returns"
        );
    }

    #[cfg(not(target_os = "windows"))]
    fn start_content_rule_cache_gc(&mut self) {
        if self.active_declarative_content_policy_maintenance.is_some()
            || !self.content_rule_cache_gc_pending
            || self.shutdown_completion.is_some()
        {
            return;
        }
        self.content_rule_cache_gc_pending = false;
        let attempt = self.next_declarative_content_policy_maintenance_attempt;
        let Some(next_attempt) = attempt.checked_add(1) else {
            eprintln!("content blocker: native maintenance attempt space exhausted");
            return;
        };
        self.next_declarative_content_policy_maintenance_attempt = next_attempt;
        self.active_declarative_content_policy_maintenance = Some(
            DeclarativeContentPolicyMaintenance::CacheGc(ContentRuleCacheGcAttempt {
                id: attempt,
                phase: ContentRuleCacheGcPhase::Enumerating,
                candidates: std::collections::VecDeque::new(),
                next_cursor: self.content_rule_cache_gc_cursor,
                scan_complete: false,
                removed_any: false,
                timed_out: false,
                watchdog: None,
                cancellation: None,
            }),
        );

        let timeout_terminal_failure = self.native_terminal_failure.clone();
        let Some(watchdog) = crate::platform::imp::schedule_content_policy_timeout(
            CONTENT_RULE_CACHE_GC_TIMEOUT,
            move || {
                let admitted = with_content_policy_timeout(move |host| {
                    host.timeout_content_rule_cache_gc(attempt);
                });
                if !admitted {
                    eprintln!("content blocker: native cache-GC timeout was not admitted");
                    timeout_terminal_failure(
                        "native content-policy cache-GC timeout lost exact host admission",
                    );
                }
            },
        ) else {
            self.active_declarative_content_policy_maintenance = None;
            return;
        };
        if let Some(active) = self
            .active_declarative_content_policy_maintenance
            .as_mut()
            .and_then(active_cache_gc_mut)
            .filter(|active| active.id == attempt)
        {
            active.watchdog = Some(watchdog);
        } else {
            watchdog.cancel();
            return;
        }

        let settlement_terminal_failure = self.native_terminal_failure.clone();
        let callback = move |result| {
            let admitted = with_content_policy_settlement([0; 32], move |host| {
                host.finish_content_rule_cache_enumeration(attempt, result);
            });
            if !admitted {
                eprintln!("content blocker: native cache enumeration was not admitted");
                settlement_terminal_failure(
                    "native content-policy cache enumeration lost exact host admission",
                );
            }
        };
        let cancellation = crate::platform::imp::enumerate_content_policy_cache(
            &self.content_rule_cache,
            self.content_rule_cache_gc_cursor,
            callback,
        );
        if let Some(active) = self
            .active_declarative_content_policy_maintenance
            .as_mut()
            .and_then(active_cache_gc_mut)
            .filter(|active| {
                active.id == attempt && active.phase == ContentRuleCacheGcPhase::Enumerating
            })
        {
            active.cancellation = Some(cancellation);
        } else {
            cancellation.cancel();
        }
    }

    #[cfg(not(target_os = "windows"))]
    fn finish_content_rule_cache_enumeration(
        &mut self,
        attempt: u64,
        result: Result<crate::platform::imp::ContentPolicyCachePage, ()>,
    ) {
        let exact = self
            .active_declarative_content_policy_maintenance
            .as_ref()
            .and_then(active_cache_gc)
            .is_some_and(|active| {
                active.id == attempt && active.phase == ContentRuleCacheGcPhase::Enumerating
            });
        if !exact {
            return;
        }
        let timed_out = self
            .active_declarative_content_policy_maintenance
            .as_mut()
            .and_then(active_cache_gc_mut)
            .is_some_and(|active| {
                drop(active.cancellation.take());
                active.timed_out
            });
        if timed_out {
            self.finish_content_rule_cache_gc_attempt(attempt, false);
            return;
        }
        let Ok(page) = result else {
            self.finish_content_rule_cache_gc_attempt(attempt, false);
            return;
        };
        let candidates = select_content_rule_cache_gc_candidates(
            page.identifiers.iter().map(String::as_str),
            MAX_CONTENT_RULE_CACHE_GC_CANDIDATES,
            |digest| self.is_content_rule_cache_digest_protected(digest),
        );
        if page.over_budget {
            eprintln!(
                "content blocker: native content-rule identifier inventory exceeded its bounded scan budget"
            );
        }
        let Some(active) = self
            .active_declarative_content_policy_maintenance
            .as_mut()
            .and_then(active_cache_gc_mut)
            .filter(|active| active.id == attempt)
        else {
            return;
        };
        active.candidates = candidates;
        active.next_cursor = page.next_cursor;
        active.scan_complete = page.scan_complete;
        self.start_next_content_rule_cache_removal(attempt);
    }

    #[cfg(not(target_os = "windows"))]
    fn start_next_content_rule_cache_removal(&mut self, attempt: u64) {
        loop {
            let candidate = self
                .active_declarative_content_policy_maintenance
                .as_mut()
                .and_then(active_cache_gc_mut)
                .filter(|active| active.id == attempt && !active.timed_out)
                .and_then(|active| active.candidates.pop_front());
            let Some(digest) = candidate else {
                self.finish_content_rule_cache_gc_attempt(attempt, true);
                return;
            };
            // Revalidate immediately before the native deletion linearization
            // point. A policy may have been queued while enumeration was in
            // flight; such a digest is skipped without touching WebKit.
            if self.is_content_rule_cache_digest_protected(&digest) {
                continue;
            }
            let Some(active) = self
                .active_declarative_content_policy_maintenance
                .as_mut()
                .and_then(active_cache_gc_mut)
                .filter(|active| active.id == attempt && !active.timed_out)
            else {
                return;
            };
            let operation = match active.phase {
                ContentRuleCacheGcPhase::Enumerating => 1,
                ContentRuleCacheGcPhase::Removing { operation, .. } => {
                    let Some(operation) = operation.checked_add(1) else {
                        self.finish_content_rule_cache_gc_attempt(attempt, false);
                        return;
                    };
                    operation
                }
            };
            active.phase = ContentRuleCacheGcPhase::Removing { operation, digest };

            let settlement_terminal_failure = self.native_terminal_failure.clone();
            let callback = move |result| {
                let admitted = with_content_policy_settlement(digest, move |host| {
                    host.finish_content_rule_cache_removal(attempt, operation, digest, result);
                });
                if !admitted {
                    eprintln!("content blocker: native cache removal was not admitted");
                    settlement_terminal_failure(
                        "native content-policy cache removal lost exact host admission",
                    );
                }
            };
            let cancellation = crate::platform::imp::remove_content_policy_cache_identifier(
                &self.content_rule_cache,
                content_rule_identifier(digest),
                callback,
            );
            if let Some(active) = self
                .active_declarative_content_policy_maintenance
                .as_mut()
                .and_then(active_cache_gc_mut)
                .filter(|active| {
                    active.id == attempt
                        && active.phase == ContentRuleCacheGcPhase::Removing { operation, digest }
                })
            {
                active.cancellation = Some(cancellation);
            } else {
                cancellation.cancel();
            }
            return;
        }
    }

    #[cfg(not(target_os = "windows"))]
    fn finish_content_rule_cache_removal(
        &mut self,
        attempt: u64,
        operation: u16,
        digest: [u8; 32],
        result: Result<(), ()>,
    ) {
        let Some(active) = self
            .active_declarative_content_policy_maintenance
            .as_mut()
            .and_then(active_cache_gc_mut)
            .filter(|active| {
                active.id == attempt
                    && active.phase == ContentRuleCacheGcPhase::Removing { operation, digest }
            })
        else {
            return;
        };
        drop(active.cancellation.take());
        if active.timed_out || result.is_err() {
            self.finish_content_rule_cache_gc_attempt(attempt, false);
            return;
        }
        active.removed_any = true;
        self.start_next_content_rule_cache_removal(attempt);
    }

    #[cfg(not(target_os = "windows"))]
    fn timeout_content_rule_cache_gc(&mut self, attempt: u64) {
        let Some(active) = self
            .active_declarative_content_policy_maintenance
            .as_mut()
            .and_then(active_cache_gc_mut)
            .filter(|active| active.id == attempt && !active.timed_out)
        else {
            return;
        };
        if let Some(watchdog) = active.watchdog.take() {
            watchdog.cancel();
        }
        active.timed_out = true;
        if let Some(cancellation) = &active.cancellation {
            cancellation.cancel();
        }
        eprintln!(
            "content blocker: native WebKit cache maintenance exceeded its deadline; the physical slot remains retained until its exact callback"
        );
    }

    #[cfg(not(target_os = "windows"))]
    fn finish_content_rule_cache_gc_attempt(&mut self, attempt: u64, allow_followup: bool) {
        let exact = self
            .active_declarative_content_policy_maintenance
            .as_ref()
            .and_then(active_cache_gc)
            .is_some_and(|active| active.id == attempt);
        if !exact {
            return;
        }
        let Some(DeclarativeContentPolicyMaintenance::CacheGc(mut active)) =
            self.active_declarative_content_policy_maintenance.take()
        else {
            return;
        };
        if let Some(watchdog) = active.watchdog.take() {
            watchdog.cancel();
        }
        drop(active.cancellation.take());
        let progress = advance_content_rule_cache_gc(
            ContentRuleCacheGcProgress {
                cursor: self.content_rule_cache_gc_cursor,
                removed_in_cycle: self.content_rule_cache_gc_removed_in_cycle,
                pending: false,
            },
            active.next_cursor,
            active.scan_complete,
            active.removed_any,
            allow_followup && !active.timed_out,
        );
        self.content_rule_cache_gc_cursor = progress.cursor;
        self.content_rule_cache_gc_removed_in_cycle = progress.removed_in_cycle;
        self.content_rule_cache_gc_pending = progress.pending;
        drop(active);
        if self.shutdown_completion.is_some() {
            self.finish_content_policy_shutdown_if_quiescent();
        } else {
            self.start_next_declarative_content_policy_compilation();
        }
    }

    #[cfg(not(target_os = "windows"))]
    pub(crate) fn retain_content_rule_cache_digests(&mut self, mut digests: Vec<[u8; 32]>) {
        digests.truncate(4);
        self.retained_cache_digests = digests;
    }

    #[cfg(not(target_os = "windows"))]
    fn is_content_rule_cache_digest_protected(&self, digest: &[u8; 32]) -> bool {
        if self.preflight_cache_digests.contains(digest)
            || self.retained_cache_digests.contains(digest)
        {
            return true;
        }
        if self.content_policies.values().any(|state| {
            state
                .applied
                .as_ref()
                .and_then(|applied| applied.digest)
                .as_ref()
                == Some(digest)
                || state.previous_known_good_digest.as_ref() == Some(digest)
                || state
                    .queued
                    .as_ref()
                    .and_then(|queued| declarative_rule_digest(&queued.rules))
                    .as_ref()
                    == Some(digest)
        }) {
            return true;
        }
        if self
            .declarative_content_policy_compilations
            .contains_key(digest)
            || self
                .declarative_content_policy_queue
                .iter()
                .any(|job| &job.digest == digest)
        {
            return true;
        }
        if self
            .active_declarative_content_policy_maintenance
            .as_ref()
            .and_then(active_compilation)
            .is_some_and(|active| &active.digest == digest)
        {
            return true;
        }
        self.declarative_content_policy_cache
            .get(digest)
            .is_some_and(|native| native.strong_count() != 0)
    }

    #[cfg(not(target_os = "windows"))]
    fn fail_unstarted_declarative_content_policy(&mut self, job: DeclarativeContentPolicyJob) {
        self.finish_content_rule_preflight(job.digest, ContentRuleValidationOutcome::Unavailable);
        self.release_declarative_content_policy_bytes(job.encoded_bytes);
        let waiters = self
            .declarative_content_policy_compilations
            .remove(&job.digest)
            .unwrap_or_default();
        for (profile, generation) in waiters {
            self.finish_content_policy_compilation(
                profile,
                generation,
                Err(ContentRuleApplyFailure::NativeCompilation),
            );
        }
    }

    #[cfg(not(target_os = "windows"))]
    fn release_declarative_content_policy_bytes(&mut self, released: usize) {
        let Some(remaining) = self.declarative_content_policy_bytes.checked_sub(released) else {
            self.declarative_content_policy_bytes = 0;
            self.native_resource_accounting_failed = true;
            eprintln!("content blocker: declarative compilation byte accounting underflowed");
            return;
        };
        self.declarative_content_policy_bytes = remaining;
    }

    pub(super) fn finish_content_policy_compilation(
        &mut self,
        profile: ProfileId,
        generation: ContentPolicyGeneration,
        result: Result<
            std::rc::Rc<crate::platform::imp::NativeContentPolicy>,
            ContentRuleApplyFailure,
        >,
    ) {
        let Some(state) = self.content_policies.get_mut(&profile) else {
            return;
        };
        let Some(compiling) = state.compiling.take() else {
            return;
        };
        if compiling.generation != generation || self.erasure_tombstones.contains(&profile) {
            state.compiling = Some(compiling);
            return;
        }
        if compiling.superseded {
            self.start_queued_content_policy(profile);
            return;
        }
        let applied = state.applied.as_ref().map(|policy| policy.generation);
        let native = match result {
            Ok(native) => native,
            Err(failure) => {
                self.emit_content_policy_settlement(
                    profile,
                    generation,
                    failed_settlement(applied, failure),
                );
                self.start_queued_content_policy(profile);
                return;
            }
        };
        #[cfg(not(target_os = "windows"))]
        let native_digest = crate::platform::imp::content_policy_digest(&native);

        // A generation change does not imply an artifact change. Reinstalling
        // a digest-identical WebKit list under the same native identifier and
        // then retiring the old registration can remove the newly added list,
        // because WebKit keys registrations by identifier. Promote the exact
        // generation in place instead; this also avoids needless per-view COM
        // and WebKit work for unchanged maintained-list refreshes.
        let retained_native = retained_equivalent_native_policy(
            self.content_policies
                .get(&profile)
                .and_then(|state| state.applied.as_ref()),
            &native,
        );
        if let Some(retained_native) = retained_native {
            let Some(state) = self.content_policies.get_mut(&profile) else {
                return;
            };
            state.applied = Some(AppliedContentPolicy {
                generation,
                cosmetics: compiling.cosmetics,
                // Existing WebView2 handlers capture this object. Retaining it
                // also makes future views share the exact same runtime
                // diagnostics counters instead of splitting one digest across
                // old and newly constructed policy objects.
                native: retained_native,
                #[cfg(not(target_os = "windows"))]
                digest: native_digest,
            });
            self.refresh_profile_document_styles(profile);
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            if self.wait_for_scriptlets(profile, generation) {
                return;
            }
            self.settle_applied_content_policy(profile, generation);
            return;
        }

        let mut registrations = Vec::new();
        let mut ids: Vec<_> = self
            .partitions
            .iter()
            .filter_map(|(id, partition)| {
                (partition.profile() == profile && self.views.contains_key(id)).then_some(*id)
            })
            .collect();
        ids.sort();
        for id in ids {
            let Some(view) = self.views.get(&id) else {
                continue;
            };
            match crate::platform::imp::install_scoped_content_policy_on_view(
                view,
                &native,
                &view.site_scope.pause,
                &view.site_scope.protection,
            ) {
                Ok(registration) => registrations.push((id, registration)),
                Err(failure) => {
                    if !self.rollback_content_policy_cohort(registrations, None) {
                        return;
                    }
                    if failure == ContentRuleApplyFailure::NativeCleanup {
                        self.fail_content_policy_retirement();
                        return;
                    }
                    self.emit_content_policy_settlement(
                        profile,
                        generation,
                        failed_settlement(applied, failure),
                    );
                    self.start_queued_content_policy(profile);
                    return;
                }
            }
        }
        let spare_registration = self
            .spare
            .as_ref()
            .filter(|spare| spare.partition.profile() == profile)
            .map(|spare| {
                crate::platform::imp::install_scoped_content_policy_on_view(
                    &spare.view,
                    &native,
                    &spare.view.site_scope.pause,
                    &spare.view.site_scope.protection,
                )
            })
            .transpose();
        let spare_registration = match spare_registration {
            Ok(registration) => registration,
            Err(failure) => {
                if !self.rollback_content_policy_cohort(registrations, None) {
                    return;
                }
                if failure == ContentRuleApplyFailure::NativeCleanup {
                    self.fail_content_policy_retirement();
                    return;
                }
                self.emit_content_policy_settlement(
                    profile,
                    generation,
                    failed_settlement(applied, failure),
                );
                self.start_queued_content_policy(profile);
                return;
            }
        };
        #[cfg(all(
            feature = "agentic-browser",
            any(target_os = "macos", target_os = "windows")
        ))]
        let agent_registrations = {
            let mut context_ids: Vec<_> = self
                .agent_contexts
                .iter()
                .filter_map(|(id, context)| (context.profile() == profile).then_some(*id))
                .collect();
            context_ids.sort();
            let mut agent_registrations = Vec::with_capacity(context_ids.len());
            let mut agent_failure = None;
            for id in context_ids {
                let Some(context) = self.agent_contexts.get(&id) else {
                    agent_failure = Some(ContentRuleApplyFailure::NativeInstallation);
                    break;
                };
                #[cfg(target_os = "windows")]
                if !context.permits_content_policy_install() {
                    agent_failure = Some(ContentRuleApplyFailure::NativeInstallation);
                    break;
                }
                match crate::platform::imp::install_content_policy_on_view(context.view(), &native)
                {
                    Ok(registration) => agent_registrations.push((id, registration)),
                    Err(failure) => {
                        agent_failure = Some(failure);
                        break;
                    }
                }
            }
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            if agent_failure.is_none() {
                let mut resource_ids: Vec<_> = self
                    .work_resources
                    .iter()
                    .filter_map(|(id, resource)| (resource.profile() == profile).then_some(*id))
                    .collect();
                resource_ids.sort();
                for id in resource_ids {
                    let Some(view) = self
                        .work_resources
                        .get(&id)
                        .and_then(|resource| resource.view())
                    else {
                        agent_failure = Some(ContentRuleApplyFailure::NativeInstallation);
                        break;
                    };
                    match crate::platform::imp::install_content_policy_on_view(view, &native) {
                        Ok(registration) => agent_registrations.push((id, registration)),
                        Err(failure) => {
                            agent_failure = Some(failure);
                            break;
                        }
                    }
                }
            }
            if let Some(failure) = agent_failure {
                let agent_clean = self.rollback_agent_content_policy_cohort(agent_registrations);
                let ordinary_clean =
                    self.rollback_content_policy_cohort(registrations, spare_registration);
                if !agent_clean || !ordinary_clean {
                    return;
                }
                if failure == ContentRuleApplyFailure::NativeCleanup {
                    self.fail_content_policy_retirement();
                    return;
                }
                self.emit_content_policy_settlement(
                    profile,
                    generation,
                    failed_settlement(applied, failure),
                );
                self.start_queued_content_policy(profile);
                return;
            }
            agent_registrations
        };

        // Construct the complete replacement cohort before swapping any
        // registration. A fallible view therefore leaves every prior exact
        // policy intact.
        for (id, registration) in registrations {
            let replaced = self
                .views
                .get_mut(&id)
                .and_then(|view| view.content_policy_registration.replace(registration));
            let Some(replaced) = replaced else {
                self.fail_content_policy_retirement();
                return;
            };
            if replaced.retire().is_err() {
                self.fail_content_policy_retirement();
                return;
            }
        }
        if let (Some(spare), Some(registration)) = (&mut self.spare, spare_registration) {
            let Some(replaced) = spare.view.content_policy_registration.replace(registration)
            else {
                self.fail_content_policy_retirement();
                return;
            };
            if replaced.retire().is_err() {
                self.fail_content_policy_retirement();
                return;
            }
        }
        #[cfg(all(
            feature = "agentic-browser",
            any(target_os = "macos", target_os = "windows")
        ))]
        for (id, registration) in agent_registrations {
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            if let Some(resource) = self.work_resources.get_mut(&id) {
                let Some(replaced) = resource.replace_content_policy_registration(registration)
                else {
                    self.fail_content_policy_retirement();
                    return;
                };
                if replaced.retire().is_err() {
                    self.fail_content_policy_retirement();
                    return;
                }
                continue;
            }
            let replaced = self
                .agent_contexts
                .get_mut(&id)
                .and_then(|context| context.replace_content_policy_registration(registration));
            let Some(replaced) = replaced else {
                self.fail_content_policy_retirement();
                return;
            };
            if replaced.retire().is_err() {
                self.fail_content_policy_retirement();
                return;
            }
        }
        let Some(state) = self.content_policies.get_mut(&profile) else {
            return;
        };
        #[cfg(not(target_os = "windows"))]
        {
            state.previous_known_good_digest = advance_previous_known_good_digest(
                state.previous_known_good_digest,
                state.applied.as_ref().and_then(|applied| applied.digest),
                native_digest,
            );
        }
        state.applied = Some(AppliedContentPolicy {
            generation,
            cosmetics: compiling.cosmetics,
            native,
            #[cfg(not(target_os = "windows"))]
            digest: native_digest,
        });
        self.refresh_profile_document_styles(profile);
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        if self.wait_for_scriptlets(profile, generation) {
            return;
        }
        self.settle_applied_content_policy(profile, generation);
    }

    /// The applied generation's terminal settlement. On macOS it follows the
    /// profile's scriptlet context, so views held for startup protection
    /// wait for both.
    pub(super) fn settle_applied_content_policy(
        &mut self,
        profile: ProfileId,
        generation: ContentPolicyGeneration,
    ) {
        self.emit_content_policy_settlement(
            profile,
            generation,
            ContentRuleSettlement::Applied { generation },
        );
        self.start_queued_content_policy(profile);
    }

    fn rollback_content_policy_cohort(
        &mut self,
        registrations: Vec<(
            zephium_core::ids::ItemId,
            crate::platform::imp::ContentPolicyRegistration,
        )>,
        spare: Option<crate::platform::imp::ContentPolicyRegistration>,
    ) -> bool {
        let mut clean = true;
        // A replacement published its state on its view; the cohort still
        // installed there takes the view back.
        for (id, registration) in registrations {
            clean &= registration.retire().is_ok();
            #[cfg(target_os = "windows")]
            if let Some(live) = self
                .views
                .get(&id)
                .and_then(|view| view.content_policy_registration.as_ref())
            {
                live.republish();
            }
            #[cfg(not(target_os = "windows"))]
            let _ = id;
        }
        if let Some(registration) = spare {
            clean &= registration.retire().is_ok();
            #[cfg(target_os = "windows")]
            if let Some(live) = self
                .spare
                .as_ref()
                .and_then(|spare| spare.view.content_policy_registration.as_ref())
            {
                live.republish();
            }
        }
        if !clean {
            self.fail_content_policy_retirement();
        }
        clean
    }

    #[cfg(all(
        feature = "agentic-browser",
        any(target_os = "macos", target_os = "windows")
    ))]
    fn rollback_agent_content_policy_cohort(
        &mut self,
        registrations: Vec<(
            zephium_agentic::ContextId,
            crate::platform::imp::ContentPolicyRegistration,
        )>,
    ) -> bool {
        let clean = registrations
            .into_iter()
            .fold(true, |clean, (_, registration)| {
                registration.retire().is_ok() && clean
            });
        if !clean {
            self.fail_content_policy_retirement();
        }
        clean
    }

    pub(super) fn applied_content_policy(
        &self,
        profile: ProfileId,
    ) -> Option<std::rc::Rc<crate::platform::imp::NativeContentPolicy>> {
        self.content_policies
            .get(&profile)
            .and_then(|state| state.applied.as_ref())
            .map(|policy| policy.native.clone())
    }

    pub(super) fn has_applied_content_policy(&self, profile: ProfileId) -> bool {
        self.content_policies
            .get(&profile)
            .is_some_and(|state| state.applied.is_some())
    }

    pub(super) fn retire_content_policy(&mut self, profile: ProfileId) {
        self.content_policies.remove(&profile);
        self.blocker_sites.remove(&profile);
        self.page_styles.remove(&profile);
        self.blocker_statistics.remove(&profile);
        for waiters in self.declarative_content_policy_compilations.values_mut() {
            waiters.retain(|(waiting_profile, _)| *waiting_profile != profile);
        }
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        {
            self.refresh_profile_scriptlets(profile);
            self.scriptlets.forget(profile);
        }
    }

    pub(super) fn begin_content_policy_shutdown(&mut self) {
        self.style_worker.take();
        #[cfg(not(target_os = "windows"))]
        {
            self.content_rule_preflight.take();
            self.preflight_cache_digests.clear();
        }
        self.content_policies.clear();
        self.declarative_content_policy_cache.clear();
        self.declarative_content_policy_compilations.clear();
        #[cfg(not(target_os = "windows"))]
        {
            self.content_rule_cache_gc_pending = false;
            // GIO cancellation may emit signals synchronously; macOS only has
            // advisory cancellation. In both cases retain the typed physical
            // slot until its exact callback proves native ownership ended.
            if let Some(active) = self.active_declarative_content_policy_maintenance.as_mut() {
                match active {
                    DeclarativeContentPolicyMaintenance::Compilation(active) => {
                        if let Some(watchdog) = active.watchdog.take() {
                            watchdog.cancel();
                        }
                        if let Some(cancellation) = active.cancellation.as_ref() {
                            cancellation.cancel();
                        }
                    }
                    DeclarativeContentPolicyMaintenance::CacheGc(active) => {
                        if let Some(watchdog) = active.watchdog.take() {
                            watchdog.cancel();
                        }
                        active.timed_out = true;
                        active.candidates.clear();
                        if let Some(cancellation) = active.cancellation.as_ref() {
                            cancellation.cancel();
                        }
                    }
                }
            }
            let queued = std::mem::take(&mut self.declarative_content_policy_queue);
            for job in queued {
                self.release_declarative_content_policy_bytes(job.encoded_bytes);
            }
        }
    }

    #[cfg(not(target_os = "windows"))]
    pub(super) fn finish_content_policy_shutdown_if_quiescent(&mut self) {
        if self.active_declarative_content_policy_maintenance.is_some() {
            return;
        }
        let Some(done) = self.shutdown_completion.take() else {
            return;
        };
        let clean = !self.native_resource_accounting_failed
            && self.native_resources.is_quiescent()
            && self.declarative_content_policy_bytes == 0
            && self.declarative_content_policy_queue.is_empty();
        done(clean);
    }

    fn emit_content_policy_settlement(
        &self,
        profile: ProfileId,
        requested: ContentPolicyGeneration,
        settlement: ContentRuleSettlement,
    ) {
        self.sink.emit(EngineEvent::ContentRulesSettled {
            profile,
            requested,
            settlement,
        });
    }

    pub(super) fn fail_content_policy_retirement(&mut self) {
        self.native_resource_accounting_failed = true;
        eprintln!("content blocker: exact native registration retirement failed");
        (self.native_terminal_failure)(
            "content-policy replacement could not retire its exact previous native registration",
        );
    }
}

#[cfg(not(target_os = "windows"))]
fn active_compilation(
    maintenance: &DeclarativeContentPolicyMaintenance,
) -> Option<&DeclarativeContentPolicyAttempt> {
    match maintenance {
        DeclarativeContentPolicyMaintenance::Compilation(active) => Some(active),
        DeclarativeContentPolicyMaintenance::CacheGc(_) => None,
    }
}

#[cfg(not(target_os = "windows"))]
fn active_compilation_mut(
    maintenance: &mut DeclarativeContentPolicyMaintenance,
) -> Option<&mut DeclarativeContentPolicyAttempt> {
    match maintenance {
        DeclarativeContentPolicyMaintenance::Compilation(active) => Some(active),
        DeclarativeContentPolicyMaintenance::CacheGc(_) => None,
    }
}

#[cfg(not(target_os = "windows"))]
fn active_cache_gc(
    maintenance: &DeclarativeContentPolicyMaintenance,
) -> Option<&ContentRuleCacheGcAttempt> {
    match maintenance {
        DeclarativeContentPolicyMaintenance::Compilation(_) => None,
        DeclarativeContentPolicyMaintenance::CacheGc(active) => Some(active),
    }
}

#[cfg(not(target_os = "windows"))]
fn active_cache_gc_mut(
    maintenance: &mut DeclarativeContentPolicyMaintenance,
) -> Option<&mut ContentRuleCacheGcAttempt> {
    match maintenance {
        DeclarativeContentPolicyMaintenance::Compilation(_) => None,
        DeclarativeContentPolicyMaintenance::CacheGc(active) => Some(active),
    }
}

#[cfg(not(target_os = "windows"))]
fn declarative_rule_digest(rules: &ContentRules) -> Option<[u8; 32]> {
    let ContentRulesPayload::Declarative {
        artifact_digest, ..
    } = rules.payload()
    else {
        return None;
    };
    Some(*artifact_digest.as_bytes())
}

#[cfg(not(target_os = "windows"))]
fn content_rule_identifier(digest: [u8; 32]) -> String {
    use std::fmt::Write as _;

    let mut identifier = String::with_capacity(CONTENT_RULE_IDENTIFIER_PREFIX.len() + 64);
    identifier.push_str(CONTENT_RULE_IDENTIFIER_PREFIX);
    for byte in digest {
        let _ = write!(identifier, "{byte:02x}");
    }
    identifier
}

#[cfg(not(target_os = "windows"))]
fn parse_content_rule_identifier(identifier: &str) -> Option<[u8; 32]> {
    let encoded = identifier.strip_prefix(CONTENT_RULE_IDENTIFIER_PREFIX)?;
    if encoded.len() != 64
        || !encoded
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return None;
    }
    let mut digest = [0u8; 32];
    for (index, pair) in encoded.as_bytes().chunks_exact(2).enumerate() {
        let high = hex_nibble(pair[0])?;
        let low = hex_nibble(pair[1])?;
        digest[index] = (high << 4) | low;
    }
    Some(digest)
}

#[cfg(not(target_os = "windows"))]
fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

#[cfg(not(target_os = "windows"))]
fn select_content_rule_cache_gc_candidates<'a>(
    identifiers: impl IntoIterator<Item = &'a str>,
    limit: usize,
    mut protected: impl FnMut(&[u8; 32]) -> bool,
) -> std::collections::VecDeque<[u8; 32]> {
    let mut selected = std::collections::VecDeque::with_capacity(limit);
    for identifier in identifiers {
        let Some(digest) = parse_content_rule_identifier(identifier) else {
            continue;
        };
        if protected(&digest) || selected.contains(&digest) {
            continue;
        }
        if selected.len() >= limit {
            break;
        }
        selected.push_back(digest);
    }
    selected
}

#[cfg(not(target_os = "windows"))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ContentRuleCacheGcProgress {
    cursor: usize,
    removed_in_cycle: bool,
    pending: bool,
}

#[cfg(not(target_os = "windows"))]
fn advance_content_rule_cache_gc(
    current: ContentRuleCacheGcProgress,
    next_cursor: usize,
    scan_complete: bool,
    removed_in_pass: bool,
    allow_followup: bool,
) -> ContentRuleCacheGcProgress {
    let removed_in_cycle = current.removed_in_cycle || removed_in_pass;
    if !allow_followup {
        return ContentRuleCacheGcProgress {
            cursor: next_cursor,
            removed_in_cycle,
            pending: false,
        };
    }
    if !scan_complete {
        return ContentRuleCacheGcProgress {
            cursor: next_cursor,
            removed_in_cycle,
            pending: true,
        };
    }
    ContentRuleCacheGcProgress {
        cursor: 0,
        // A cycle that deleted anything gets exactly one verification cycle.
        // A fully clean cycle terminates instead of polling the native store.
        removed_in_cycle: false,
        pending: removed_in_cycle,
    }
}

#[cfg(not(target_os = "windows"))]
fn advance_previous_known_good_digest(
    previous: Option<[u8; 32]>,
    current: Option<[u8; 32]>,
    next: Option<[u8; 32]>,
) -> Option<[u8; 32]> {
    match current {
        Some(current) if Some(current) != next => Some(current),
        _ => previous,
    }
}

fn failed_settlement(
    applied: Option<ContentPolicyGeneration>,
    failure: ContentRuleApplyFailure,
) -> ContentRuleSettlement {
    match applied {
        Some(generation) => ContentRuleSettlement::Retained {
            generation,
            failure,
        },
        None => ContentRuleSettlement::Unavailable { failure },
    }
}

fn retained_equivalent_native_policy(
    applied: Option<&AppliedContentPolicy>,
    candidate: &std::rc::Rc<crate::platform::imp::NativeContentPolicy>,
) -> Option<std::rc::Rc<crate::platform::imp::NativeContentPolicy>> {
    applied.and_then(|applied| {
        crate::platform::imp::same_content_policy(&applied.native, candidate)
            .then(|| applied.native.clone())
    })
}

fn content_policy_generations(
    state: Option<&ProfileContentPolicy>,
) -> (
    Option<ContentPolicyGeneration>,
    Option<ContentPolicyGeneration>,
) {
    state.map_or((None, None), |state| {
        let applied = state.applied.as_ref().map(|policy| policy.generation);
        (
            state
                .queued
                .as_ref()
                .map(|queued| queued.generation)
                .or_else(|| {
                    state
                        .compiling
                        .as_ref()
                        .map(|compiling| compiling.generation)
                })
                .or(applied),
            applied,
        )
    })
}

#[cfg(test)]
pub(super) fn policy_generation_is_applied(state: Option<&ProfileContentPolicy>) -> bool {
    state.is_some_and(|state| state.applied.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_and_pending_policy_never_admit_a_first_view() {
        assert!(!policy_generation_is_applied(None));
        assert!(!policy_generation_is_applied(Some(&ProfileContentPolicy {
            applied: None,
            compiling: Some(CompilingContentPolicy {
                generation: ContentPolicyGeneration::new(1).unwrap(),
                cosmetics: None,
                superseded: false,
            }),
            queued: None,
            #[cfg(not(target_os = "windows"))]
            previous_known_good_digest: None,
        })));
    }

    #[test]
    fn failed_settlement_cannot_claim_both_success_and_failure() {
        let prior = ContentPolicyGeneration::new(7).unwrap();
        assert_eq!(
            failed_settlement(Some(prior), ContentRuleApplyFailure::NativeInstallation,),
            ContentRuleSettlement::Retained {
                generation: prior,
                failure: ContentRuleApplyFailure::NativeInstallation,
            }
        );
        assert_eq!(
            failed_settlement(None, ContentRuleApplyFailure::NativeCompilation),
            ContentRuleSettlement::Unavailable {
                failure: ContentRuleApplyFailure::NativeCompilation,
            }
        );
    }

    #[test]
    fn admission_orders_queued_then_compiling_then_applied_generation() {
        let applied = ContentPolicyGeneration::new(3).unwrap();
        let compiling = ContentPolicyGeneration::new(4).unwrap();
        let queued = ContentPolicyGeneration::new(5).unwrap();
        let allow_all = std::rc::Rc::new(crate::platform::imp::NativeContentPolicy::AllowAll);
        let state = ProfileContentPolicy {
            applied: Some(AppliedContentPolicy {
                generation: applied,
                cosmetics: None,
                native: allow_all,
                #[cfg(not(target_os = "windows"))]
                digest: None,
            }),
            compiling: Some(CompilingContentPolicy {
                generation: compiling,
                cosmetics: None,
                superseded: true,
            }),
            queued: Some(QueuedContentPolicy {
                generation: queued,
                rules: ContentRules::allow_all(
                    zephium_core::blocker::ContentRuleDigest::from_bytes([0; 32]),
                ),
            }),
            #[cfg(not(target_os = "windows"))]
            previous_known_good_digest: None,
        };
        assert_eq!(
            content_policy_generations(Some(&state)),
            (Some(queued), Some(applied))
        );

        let state = ProfileContentPolicy {
            queued: None,
            ..state
        };
        assert_eq!(
            content_policy_generations(Some(&state)),
            (Some(compiling), Some(applied))
        );

        let state = ProfileContentPolicy {
            compiling: None,
            ..state
        };
        assert_eq!(
            content_policy_generations(Some(&state)),
            (Some(applied), Some(applied))
        );
        assert_eq!(content_policy_generations(None), (None, None));
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn native_cache_gc_accepts_only_exact_owned_lowercase_digest_identifiers() {
        let digest = [0xab; 32];
        let identifier = content_rule_identifier(digest);
        assert_eq!(parse_content_rule_identifier(&identifier), Some(digest));

        let mut uppercase = identifier.clone();
        uppercase.replace_range(identifier.len() - 2.., "AB");
        assert_eq!(parse_content_rule_identifier(&uppercase), None);
        assert_eq!(
            parse_content_rule_identifier(
                "org.example.rules.v1.abababababababababababababababababababababababababababababababab"
            ),
            None
        );
        assert_eq!(
            parse_content_rule_identifier("app.zephium.rules.v1.not-a-digest"),
            None
        );
        assert_eq!(
            parse_content_rule_identifier(&format!("{identifier}00")),
            None
        );
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn native_cache_gc_selection_is_bounded_deduplicated_and_protection_exact() {
        let protected = [1; 32];
        let first = [2; 32];
        let second = [3; 32];
        let third = [4; 32];
        let identifiers = [
            "foreign-owner".to_owned(),
            content_rule_identifier(protected),
            content_rule_identifier(first),
            content_rule_identifier(first),
            content_rule_identifier(second),
            content_rule_identifier(third),
        ];
        let selected = select_content_rule_cache_gc_candidates(
            identifiers.iter().map(String::as_str),
            2,
            |digest| digest == &protected,
        );
        assert_eq!(
            selected.into_iter().collect::<Vec<_>>(),
            vec![first, second]
        );
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn foreign_identifier_pages_advance_without_becoming_removal_work() {
        let foreign = "x".repeat(CONTENT_RULE_IDENTIFIER_PREFIX.len() + 64);
        let identifiers = vec![foreign; 128];
        let selected = select_content_rule_cache_gc_candidates(
            identifiers.iter().map(String::as_str),
            MAX_CONTENT_RULE_CACHE_GC_CANDIDATES,
            |_| false,
        );
        assert!(selected.is_empty());
        assert_eq!(
            advance_content_rule_cache_gc(
                ContentRuleCacheGcProgress {
                    cursor: 0,
                    removed_in_cycle: false,
                    pending: false,
                },
                128,
                false,
                false,
                true,
            ),
            ContentRuleCacheGcProgress {
                cursor: 128,
                removed_in_cycle: false,
                pending: true,
            }
        );
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn native_cache_gc_progress_requires_a_clean_verification_cycle() {
        let initial = ContentRuleCacheGcProgress {
            cursor: 0,
            removed_in_cycle: false,
            pending: false,
        };
        let middle = advance_content_rule_cache_gc(initial, 128, false, true, true);
        assert_eq!(
            middle,
            ContentRuleCacheGcProgress {
                cursor: 128,
                removed_in_cycle: true,
                pending: true,
            }
        );
        let wrap = advance_content_rule_cache_gc(middle, 0, true, false, true);
        assert_eq!(
            wrap,
            ContentRuleCacheGcProgress {
                cursor: 0,
                removed_in_cycle: false,
                pending: true,
            }
        );
        let clean = advance_content_rule_cache_gc(wrap, 0, true, false, true);
        assert_eq!(
            clean,
            ContentRuleCacheGcProgress {
                cursor: 0,
                removed_in_cycle: false,
                pending: false,
            }
        );
        let failed = advance_content_rule_cache_gc(middle, 256, false, false, false);
        assert_eq!(failed.cursor, 256);
        assert!(failed.removed_in_cycle);
        assert!(!failed.pending);
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn native_cache_gc_retains_exactly_the_previous_known_good_digest() {
        let older = [1; 32];
        let current = [2; 32];
        let next = [3; 32];
        assert_eq!(
            advance_previous_known_good_digest(Some(older), Some(current), Some(next)),
            Some(current)
        );
        assert_eq!(
            advance_previous_known_good_digest(Some(older), Some(current), Some(current)),
            Some(older)
        );
        assert_eq!(
            advance_previous_known_good_digest(Some(older), None, Some(next)),
            Some(older)
        );
        assert_eq!(
            advance_previous_known_good_digest(None, Some(current), None),
            Some(current)
        );
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn native_declarative_compilation_is_serialized_bounded_and_attempt_exact() {
        assert_eq!(MAX_DECLARATIVE_CONTENT_POLICY_JOBS, 2);
        assert_eq!(
            DECLARATIVE_CONTENT_POLICY_TIMEOUT,
            std::time::Duration::from_secs(60)
        );
        assert_eq!(
            MAX_RESIDENT_DECLARATIVE_CONTENT_POLICY_BYTES,
            zephium_core::blocker::MAX_DECLARATIVE_RULE_BYTES * 2
        );
        let source = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/host/content_rules.rs"
        ));
        let start = source
            .find("fn start_next_declarative_content_policy_compilation")
            .expect("native compiler start state machine disappeared");
        let finish = source
            .find("fn finish_shared_declarative_compilation")
            .expect("native compiler terminal callback disappeared");
        let timeout = source
            .find("fn timeout_declarative_content_policy_compilation")
            .expect("native compiler timeout state machine disappeared");
        let gc = source
            .find("fn start_content_rule_cache_gc")
            .expect("shared native maintenance phase disappeared");
        let fail_unstarted = source
            .find("fn fail_unstarted_declarative_content_policy")
            .expect("native compiler admission failure path disappeared");
        assert!(start < finish && finish < timeout && timeout < gc && gc < fail_unstarted);
        let start_source = &source[start..finish];
        let finish_source = &source[finish..timeout];
        let timeout_source = &source[timeout..gc];

        let active_gate = start_source
            .find("active_declarative_content_policy_maintenance")
            .expect("global native compiler active gate disappeared");
        let dequeue = start_source
            .find("declarative_content_policy_queue.pop_front()")
            .expect("bounded native compiler queue disappeared");
        let watchdog = start_source
            .find("schedule_content_policy_timeout(")
            .expect("native compiler watchdog disappeared");
        let native_start = start_source
            .find("crate::platform::imp::compile_content_policy(")
            .expect("native compiler invocation disappeared");
        assert!(active_gate < dequeue);
        assert!(watchdog < native_start);
        let retain_watchdog = start_source
            .find("active.watchdog = Some(watchdog);")
            .expect("physical compiler watchdog handle is not retained");
        let cancel_unretained_watchdog = start_source[retain_watchdog..]
            .find("watchdog.cancel();")
            .expect("a watchdog which cannot be retained is not canceled")
            + retain_watchdog;
        assert!(watchdog < retain_watchdog);
        assert!(retain_watchdog < cancel_unretained_watchdog);
        let retain_cancellation = start_source
            .find("active.cancellation = Some(cancellation);")
            .expect("physical compiler cancellation handle is not retained");
        let cancel_after_sync_completion = start_source[retain_cancellation..]
            .find("cancellation.cancel();")
            .expect("synchronously completed native work does not cancel its returned handle")
            + retain_cancellation;
        assert!(native_start < retain_cancellation);
        assert!(retain_cancellation < cancel_after_sync_completion);
        assert_eq!(
            start_source
                .matches("self.fail_unstarted_declarative_content_policy(job);")
                .count(),
            3,
            "attempt exhaustion, watchdog admission, and watchdog retention failure must fail their exact job"
        );
        assert_eq!(
            start_source
                .matches("self.start_next_declarative_content_policy_compilation();")
                .count(),
            3,
            "an unstarted failure must never strand an already-admitted queued job"
        );

        let exact_callback = finish_source
            .find("active.id != attempt || active.digest != digest")
            .expect("late callback lost its exact attempt identity gate");
        let cancel_watchdog = finish_source
            .find("watchdog.cancel();")
            .expect("terminal callback no longer cancels its exact watchdog");
        let terminal_waiters = finish_source
            .find("declarative_content_policy_compilations")
            .expect("terminal callback no longer owns waiter settlement");
        let release_slot = finish_source
            .find("active_declarative_content_policy_maintenance.take()")
            .expect("terminal callback no longer releases the physical compiler slot");
        let release_bytes = finish_source
            .find("self.release_declarative_content_policy_bytes(active.encoded_bytes)")
            .expect("terminal callback no longer releases its exact byte debt");
        let drop_result = finish_source
            .find("drop(result);")
            .expect("shutdown can retain the native compiler result through completion");
        let drop_active = finish_source
            .find("drop(active);")
            .expect("shutdown can retain the native cancellation handle through completion");
        let finish_shutdown = finish_source
            .find("self.finish_content_policy_shutdown_if_quiescent();")
            .expect("terminal callback no longer completes deferred shutdown");
        assert_eq!(
            finish_source
                .matches("self.finish_content_policy_shutdown_if_quiescent();")
                .count(),
            1,
            "one native completion may settle the deferred shutdown barrier only once"
        );
        let start_next = finish_source
            .find("start_next_declarative_content_policy_compilation();")
            .expect("terminal callback no longer advances the physical compiler queue");
        assert!(
            exact_callback < cancel_watchdog
                && cancel_watchdog < terminal_waiters
                && terminal_waiters < release_slot
                && release_slot < start_next
        );
        assert!(
            release_slot < release_bytes
                && release_bytes < drop_result
                && drop_result < drop_active
                && drop_active < finish_shutdown,
            "shutdown may complete only after exact slot, byte, result, and cancellation retirement"
        );
        assert!(
            !finish_source.contains("active.timed_out"),
            "the exact late callback must release a timed-out physical slot"
        );

        let timeout_identity = timeout_source
            .find("active.id != attempt || active.digest != digest || active.timed_out")
            .expect("watchdog lost its exact attempt identity gate");
        let cancel_watchdog = timeout_source
            .find("watchdog.cancel();")
            .expect("fired watchdog handle is not retired");
        let mark_timed_out = timeout_source
            .find("active.timed_out = true;")
            .expect("watchdog no longer marks the physical attempt timed out");
        let cancel = timeout_source
            .find("cancellation.cancel();")
            .expect("watchdog no longer cancels native WebKit work");
        let settle_waiters = timeout_source
            .find("let active_waiters")
            .expect("watchdog no longer settles the active waiters");
        assert!(timeout_identity < cancel_watchdog && cancel_watchdog < mark_timed_out);
        assert!(mark_timed_out < cancel && cancel < settle_waiters);
        assert!(
            !timeout_source.contains("active_declarative_content_policy_maintenance.take()")
                && !timeout_source.contains("start_next_declarative_content_policy_compilation();"),
            "timeout may settle callers, but only a native terminal callback may release or advance the physical slot"
        );

        let clear = source
            .find("pub(super) fn begin_content_policy_shutdown")
            .expect("content-policy shutdown cleanup disappeared");
        let emit = source
            .find("fn emit_content_policy_settlement")
            .expect("content-policy shutdown cleanup boundary disappeared");
        let clear_source = &source[clear..emit];
        let retain_active = clear_source
            .find("active_declarative_content_policy_maintenance")
            .expect("shutdown no longer retains active native work");
        let cancel_watchdog = clear_source
            .find("watchdog.cancel();")
            .expect("shutdown no longer cancels the active compiler watchdog");
        let cancel_active = clear_source
            .find("cancellation.cancel();")
            .expect("shutdown no longer cancels active native work");
        let drain_queue = clear_source
            .find("std::mem::take(&mut self.declarative_content_policy_queue)")
            .expect("shutdown no longer takes ownership of queued native work");
        let await_active = clear_source
            .find("active_declarative_content_policy_maintenance")
            .and_then(|first| {
                clear_source[first + 1..]
                    .find("active_declarative_content_policy_maintenance")
                    .map(|next| first + 1 + next)
            })
            .expect("shutdown completion no longer rechecks native compiler quiescence");
        let finish = clear_source
            .find("let Some(done) = self.shutdown_completion.take()")
            .expect("shutdown lost its exact deferred completion");
        assert!(
            retain_active < cancel_watchdog
                && cancel_watchdog < cancel_active
                && cancel_active < drain_queue
                && drain_queue < await_active
                && await_active < finish,
            "shutdown must retain and cancel active work, drain only queued work, and acknowledge only after exact native quiescence"
        );
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn native_cache_gc_requires_a_successful_declarative_policy_trigger() {
        let dispatch = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/host/dispatch.rs"));
        assert!(
            dispatch.contains("content_rule_cache_gc_pending: false"),
            "startup must not enumerate cache entries before policy submissions establish protected digests"
        );
        assert!(!dispatch.contains("content_rule_cache_gc_pending: true"));

        let source = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/host/content_rules.rs"
        ));
        let allow_all = source
            .split("ContentRulesPayload::AllowAll =>")
            .nth(1)
            .and_then(|branch| branch.split("ContentRulesPayload::Runtime").next())
            .expect("explicit allow-all policy branch disappeared");
        assert!(
            !allow_all.contains("content_rule_cache_gc_pending = true"),
            "startup allow-all submissions must not enumerate or purge the warm native cache"
        );
        let successful_compile = source
            .split("fn finish_shared_declarative_compilation")
            .nth(1)
            .and_then(|body| body.split("fn ").next())
            .expect("shared declarative completion body disappeared");
        assert!(
            successful_compile.contains("if let Ok(native) = &result")
                && successful_compile.contains("self.content_rule_cache_gc_pending = true;"),
            "only an exact successful declarative compile may trigger bounded native cache maintenance"
        );
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn native_cache_gc_shares_the_exact_compiler_slot_and_shutdown_barrier() {
        assert_eq!(MAX_CONTENT_RULE_CACHE_GC_CANDIDATES, 8);
        let source = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/host/content_rules.rs"
        ));
        let host = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/host/mod.rs"));
        assert!(host.contains("enum DeclarativeContentPolicyMaintenance"));
        assert!(host.contains("Compilation(DeclarativeContentPolicyAttempt)"));
        assert!(host.contains("CacheGc(ContentRuleCacheGcAttempt)"));

        let start = source
            .find("fn start_content_rule_cache_gc")
            .expect("native cache-GC start disappeared");
        let enumeration = source
            .find("fn finish_content_rule_cache_enumeration")
            .expect("native cache enumeration terminal disappeared");
        let removal_start = source
            .find("fn start_next_content_rule_cache_removal")
            .expect("native cache removal start disappeared");
        let removal_finish = source
            .find("fn finish_content_rule_cache_removal")
            .expect("native cache removal terminal disappeared");
        let timeout = source
            .find("fn timeout_content_rule_cache_gc")
            .expect("native cache-GC watchdog disappeared");
        let finish = source
            .find("fn finish_content_rule_cache_gc_attempt")
            .expect("native cache-GC retirement disappeared");
        assert!(
            start < enumeration
                && enumeration < removal_start
                && removal_start < removal_finish
                && removal_finish < timeout
                && timeout < finish
        );

        let start_source = &source[start..enumeration];
        let active_gate = start_source
            .find("active_declarative_content_policy_maintenance")
            .expect("cache GC lost the shared maintenance gate");
        let watchdog = start_source
            .find("schedule_content_policy_timeout(")
            .expect("cache GC lost its bounded watchdog");
        let enumerate = start_source
            .find("enumerate_content_policy_cache(")
            .expect("native cache enumeration disappeared");
        let retain_cancellation = start_source
            .find("active.cancellation = Some(cancellation);")
            .expect("enumeration cancellation ownership disappeared");
        assert!(active_gate < watchdog && watchdog < enumerate);
        assert!(enumerate < retain_cancellation);

        let enumeration_source = &source[enumeration..removal_start];
        let exact_phase = enumeration_source
            .find("active.phase == ContentRuleCacheGcPhase::Enumerating")
            .expect("enumeration lost exact attempt/phase attribution");
        let retire_native = enumeration_source
            .find("drop(active.cancellation.take())")
            .expect("enumeration native ownership is not retired");
        let selection = enumeration_source
            .find("select_content_rule_cache_gc_candidates")
            .expect("bounded owned-namespace selection disappeared");
        assert!(exact_phase < retire_native && retire_native < selection);

        let removal_source = &source[removal_start..removal_finish];
        let revalidate = removal_source
            .find("is_content_rule_cache_digest_protected")
            .expect("removal lost its just-in-time protection gate");
        let native_remove = removal_source
            .find("remove_content_policy_cache_identifier(")
            .expect("exact native identifier removal disappeared");
        assert!(revalidate < native_remove);

        let removal_terminal = &source[removal_finish..timeout];
        let exact_removal = removal_terminal
            .find("ContentRuleCacheGcPhase::Removing { operation, digest }")
            .expect("removal callback lost exact operation identity");
        let release = removal_terminal
            .find("drop(active.cancellation.take())")
            .expect("removal callback does not retire native ownership");
        let advance = removal_terminal
            .find("start_next_content_rule_cache_removal(attempt)")
            .expect("successful removal no longer advances the bounded pass");
        assert!(exact_removal < release && release < advance);

        let timeout_source = &source[timeout..finish];
        assert!(timeout_source.contains("active.timed_out = true;"));
        assert!(timeout_source.contains("cancellation.cancel();"));
        assert!(
            !timeout_source.contains("active_declarative_content_policy_maintenance.take()"),
            "a watchdog may cancel, but only the exact native callback may release the slot"
        );

        let protection_start = source
            .find("fn is_content_rule_cache_digest_protected")
            .expect("cache protection boundary disappeared");
        let finish_source = &source[finish..protection_start];
        let release_slot = finish_source
            .find("active_declarative_content_policy_maintenance.take()")
            .expect("cache GC terminal does not release its exact typed slot");
        let drop_active = finish_source
            .find("drop(active);")
            .expect("cache GC attempt resources are not explicitly retired");
        let shutdown = finish_source
            .find("finish_content_policy_shutdown_if_quiescent")
            .expect("cache GC no longer participates in shutdown quiescence");
        assert!(release_slot < drop_active && drop_active < shutdown);

        let protection = &source[protection_start..];
        for invariant in [
            "applied.digest",
            "previous_known_good_digest",
            "queued",
            "declarative_content_policy_compilations",
            "declarative_content_policy_queue",
            "active_compilation",
            "declarative_content_policy_cache",
            "strong_count() != 0",
        ] {
            assert!(
                protection.contains(invariant),
                "cache protection set lost {invariant}"
            );
        }
    }

    #[test]
    fn replacement_cohort_precedes_every_live_policy_swap_and_settlement() {
        let source = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/host/content_rules.rs"
        ));
        let cohort = source
            .find("let mut registrations = Vec::new();")
            .expect("replacement lost its fallible registration cohort");
        let swap = cohort
            + source[cohort..]
                .find("content_policy_registration.replace(registration)")
                .expect("replacement lost its exact per-view swap");
        let applied = swap
            + source[swap..]
                .find("state.applied = Some(AppliedContentPolicy")
                .expect("replacement lost its authoritative applied state");
        let settlement = applied
            + source[applied..]
                .find("self.settle_applied_content_policy(profile, generation);")
                .expect("replacement lost its terminal applied settlement");
        assert!(
            cohort < swap && swap < applied && applied < settlement,
            "fallible registrations must complete before swapping, state, and settlement"
        );
    }

    #[test]
    fn identical_native_artifact_is_promoted_without_reinstallation() {
        let source = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/host/content_rules.rs"
        ));
        let equivalence = source
            .find("let retained_native = retained_equivalent_native_policy")
            .expect("native artifact equivalence gate disappeared");
        let cohort = source
            .find("let mut registrations = Vec::new();")
            .expect("replacement cohort disappeared");
        assert!(
            equivalence < cohort,
            "identical named WebKit artifacts must bypass add-then-remove replacement"
        );
    }

    #[test]
    fn identical_promotion_retains_the_installed_native_policy_object() {
        let installed = std::rc::Rc::new(crate::platform::imp::NativeContentPolicy::AllowAll);
        let candidate = std::rc::Rc::new(crate::platform::imp::NativeContentPolicy::AllowAll);
        let applied = AppliedContentPolicy {
            generation: ContentPolicyGeneration::new(1).unwrap(),
            cosmetics: None,
            native: installed.clone(),
            #[cfg(not(target_os = "windows"))]
            digest: None,
        };

        let retained = retained_equivalent_native_policy(Some(&applied), &candidate)
            .expect("digest-equivalent promotion must retain the installed policy");
        assert!(std::rc::Rc::ptr_eq(&retained, &installed));
        assert!(!std::rc::Rc::ptr_eq(&retained, &candidate));
    }

    #[test]
    fn native_cache_identity_never_rehashes_encoded_json_on_the_host_thread() {
        let source = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/host/content_rules.rs"
        ));
        let production = source
            .split_once("#[cfg(test)]")
            .expect("content-rules production/test boundary disappeared")
            .0;
        assert!(!production.contains(&["Sha256::digest", "(encoded"].concat()));
        assert!(production.contains("let digest = *artifact_digest.as_bytes();"));
    }
}
