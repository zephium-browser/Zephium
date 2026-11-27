use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RetainedHumanPhase {
    WaitingForHuman,
    Presenting,
    Presented,
    Continuing,
    ReadyToResume,
    Released,
}

#[derive(Clone, Copy, Debug)]
pub struct RetainedHumanSnapshot {
    pub generation: u32,
    pub phase: RetainedHumanPhase,
    pub reason: AgentBrowserHumanReason,
    pub deadline: Instant,
    pub document_revision: u64,
    pub can_continue: bool,
    /// Native observations of the same live document verified its wall is gone.
    pub verified_wall_clear: bool,
}

/// Native-finalized successor operands; this object grants no provider scope.
#[derive(Clone, Debug)]
pub struct RetainedHumanResume {
    pub generation: u32,
    pub context: ContextId,
    pub document: ContextNavigationTarget,
    pub usage: zephium_core::work::runtime::WorkUsage,
    pub model_calls: u32,
    pub actions: u32,
    pub reason: AgentBrowserHumanReason,
    /// Manual Continue alone never attests that a sign-in/check was resolved.
    pub verified_wall_clear: bool,
}

/// One native human-handoff operation on a retained page.
pub(in crate::work_resources) enum HumanStep {
    Present(WorkBrowserHumanRegion, bool),
    /// Ends the actor lease without showing the page, for a decision the
    /// person makes elsewhere.
    HandOver,
    Continue,
}

pub(super) struct HumanHandoff {
    pub(super) phase: RetainedHumanPhase,
    generation: u32,
    reason: AgentBrowserHumanReason,
    deadline: Instant,
    expires: AgentPolicyInstant,
    pending: Option<PendingLifecycle>,
    progress: Option<WorkBrowserHumanProgress>,
    /// Captured at admitted Continue and kept only if native handback succeeds.
    continued_wall_clear: bool,
}

fn requires_wall_proof(reason: AgentBrowserHumanReason) -> bool {
    matches!(
        reason,
        AgentBrowserHumanReason::SignIn | AgentBrowserHumanReason::HumanChallenge
    )
}

impl RetainedWork {
    pub(in crate::work_resources) fn start_human_wait(
        &mut self,
        original_deadline: Instant,
        now: AgentPolicyInstant,
    ) {
        if self.human.is_some()
            || !self.ready()
            || self
                .record
                .is_none_or(|record| record.disposition() != AgentWorkDisposition::WaitingForHuman)
        {
            return;
        }
        let Some(AgentWorkRetainedOutcome::WaitingForHuman(waiting)) = self
            .active
            .as_ref()
            .and_then(|active| active.outcome.as_ref())
        else {
            return;
        };
        let Some(generation) = self.human_generation.checked_add(1) else {
            self.fail(crate::contract_failure!());
            return;
        };
        let sampled = Instant::now();
        let deadline =
            original_deadline.min(sampled + Duration::from_millis(MAX_WORK_HUMAN_WAIT_MILLIS));
        let remaining =
            u64::try_from(deadline.saturating_duration_since(sampled).as_millis()).unwrap_or(0);
        let Some(expires) = now
            .millis()
            .checked_add(remaining)
            .map(AgentPolicyInstant::from_millis)
        else {
            self.fail(AgentWorkFailure::Deadline);
            return;
        };
        self.human_generation = generation;
        self.human = Some(HumanHandoff {
            generation,
            reason: waiting.request().reason(),
            phase: RetainedHumanPhase::WaitingForHuman,
            deadline,
            expires,
            pending: None,
            progress: None,
            continued_wall_clear: false,
        });
    }
    pub(in crate::work_resources) fn human_snapshot(&self) -> Option<RetainedHumanSnapshot> {
        let human = self.human.as_ref()?;
        Some(RetainedHumanSnapshot {
            generation: human.generation,
            phase: human.phase,
            reason: human.reason,
            deadline: human.deadline,
            document_revision: human
                .progress
                .as_ref()
                .map_or(0, WorkBrowserHumanProgress::revision),
            can_continue: human.phase == RetainedHumanPhase::Presented
                && human.progress.as_ref().is_some_and(|progress| {
                    progress.ready()
                        && (!requires_wall_proof(human.reason) || progress.verified_wall_clear())
                }),
            verified_wall_clear: human.phase == RetainedHumanPhase::Presented
                && human
                    .progress
                    .as_ref()
                    .is_some_and(WorkBrowserHumanProgress::verified_wall_clear),
        })
    }
    pub(in crate::work_resources) fn human_check_progress(
        &self,
    ) -> Option<WorkBrowserHumanProgress> {
        let human = self.human.as_ref()?;
        (human.phase == RetainedHumanPhase::Presented
            && requires_wall_proof(human.reason)
            && !self.stopping
            && Instant::now() < human.deadline)
            .then(|| human.progress.clone())
            .flatten()
    }
    pub(in crate::work_resources) fn human_deadline(&self) -> Option<Instant> {
        self.human
            .as_ref()
            .filter(|human| human.phase != RetainedHumanPhase::Released)
            .map(|human| human.deadline)
    }
    pub(in crate::work_resources) fn present_human(
        &mut self,
        generation: u32,
        region: Option<WorkBrowserHumanRegion>,
        now: AgentPolicyInstant,
    ) -> bool {
        let Some(human) = self.human.as_mut().filter(|human| {
            human.generation == generation
                && human.phase == RetainedHumanPhase::WaitingForHuman
                && human.pending.is_none()
                && now < human.expires
                && Instant::now() < human.deadline
                && !self.stopping
        }) else {
            return false;
        };
        let step = match region {
            Some(region) => {
                HumanStep::Present(region, human.reason == AgentBrowserHumanReason::SignIn)
            }
            None => HumanStep::HandOver,
        };
        let pending = match self
            .owner
            .human_lifecycle(&self.resource, step, now, human.expires)
        {
            Ok(pending) => pending,
            Err(_) => return false,
        };
        human.progress = self
            .owner
            .shared
            .lock_rows()
            .ok()
            .and_then(|rows| rows.human_progress(&self.resource));
        human.pending = Some(pending);
        human.phase = RetainedHumanPhase::Presenting;
        true
    }
    pub(in crate::work_resources) fn continue_human(
        &mut self,
        generation: u32,
        now: AgentPolicyInstant,
    ) -> bool {
        let Some(human) = self.human.as_mut().filter(|human| {
            human.generation == generation
                && human.phase == RetainedHumanPhase::Presented
                && human.pending.is_none()
                && now < human.expires
                && Instant::now() < human.deadline
                && !self.stopping
        }) else {
            return false;
        };
        if requires_wall_proof(human.reason) {
            if !human
                .progress
                .as_ref()
                .is_some_and(WorkBrowserHumanProgress::capture_wall_continuation)
            {
                return false;
            }
            human.continued_wall_clear = true;
        }
        let pending = match self.owner.human_lifecycle(
            &self.resource,
            HumanStep::Continue,
            now,
            human.expires,
        ) {
            Ok(pending) => pending,
            Err(_) => return false,
        };
        human.pending = Some(pending);
        human.phase = RetainedHumanPhase::Continuing;
        true
    }
    pub(in crate::work_resources) fn human_resume(&self) -> Option<RetainedHumanResume> {
        let human = self
            .human
            .as_ref()
            .filter(|human| human.phase == RetainedHumanPhase::ReadyToResume)?;
        let AgentWorkRetainedOutcome::WaitingForHuman(waiting) =
            self.active.as_ref()?.outcome.as_ref()?
        else {
            return None;
        };
        Some(RetainedHumanResume {
            generation: human.generation,
            context: self.resource.identity().context(),
            document: self
                .owner
                .shared
                .lock_rows()
                .ok()?
                .retained_document(&self.resource)?
                .clone(),
            usage: self.usage()?,
            model_calls: waiting.closure().model_calls(),
            actions: waiting.closure().actions(),
            reason: human.reason,
            verified_wall_clear: human.continued_wall_clear,
        })
    }
    pub(in crate::work_resources) fn poll_human(&mut self, now: AgentPolicyInstant) {
        let Some(human) = self.human.as_mut() else {
            return;
        };
        if self.stopping || now >= human.expires || Instant::now() >= human.deadline {
            human.phase = RetainedHumanPhase::Released;
        }
        let Some(mut pending) = human.pending.take() else {
            return;
        };
        let result = pending.poll(now);
        match result {
            Ok(None) => human.pending = Some(pending),
            Ok(Some(LifecycleResult::Event(
                WorkBrowserResourceEvent::HumanPresented(resource)
                | WorkBrowserResourceEvent::HumanContinued(resource),
            ))) if resource == self.resource && human.phase == RetainedHumanPhase::Released => {}
            Ok(Some(LifecycleResult::Event(WorkBrowserResourceEvent::HumanPresented(
                resource,
            )))) if resource == self.resource && human.phase == RetainedHumanPhase::Presenting => {
                human.phase = RetainedHumanPhase::Presented
            }
            Ok(Some(LifecycleResult::Event(WorkBrowserResourceEvent::HumanContinued(
                resource,
            )))) if resource == self.resource && human.phase == RetainedHumanPhase::Continuing => {
                human.phase = RetainedHumanPhase::ReadyToResume;
                if let Ok(resource) = self.owner.shared.resource(&self.resource) {
                    resource.reusable.store(true, Ordering::Release);
                }
            }
            Ok(Some(LifecycleResult::Event(WorkBrowserResourceEvent::DebtSettled(resource))))
                if resource == self.resource && human.phase == RetainedHumanPhase::Released => {}
            _ => {
                human.phase = RetainedHumanPhase::Released;
                self.fail(AgentWorkFailure::ContextLost);
            }
        }
    }
}
