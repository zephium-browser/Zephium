//! Explicit public-only diagnostic preparation through the original product join.
use super::*;

impl RetainedWorkHandle {
    /// Excluded diagnostic selector only. Not exposed in shipping handles or
    /// IPC and not a lease, read capability, or replacement native owner.
    #[doc(hidden)]
    pub fn construction_resource_for_qualification(&self) -> Option<WorkBrowserResourceJoin> {
        self.signal
            .projection
            .lock()
            .ok()?
            .construction_resource
            .clone()
    }
}

impl PreparedRetainedWork {
    #[doc(hidden)]
    pub fn try_new_for_public_qualification(
        input: AgentWorkRunInput,
        profile: AgentWorkProfileBinding,
        config: crate::AgentWorkApplicationConfig,
        access: AgentBrowserModelAccess,
        task: Box<dyn AgentWorkTask>,
        ports: RetainedWorkPorts,
    ) -> Result<Self, AgentWorkFailure> {
        let (runtime, provider) = config.into_parts();
        let spec = input.retained_resource_spec()?;
        let actor = ActorRequest {
            run: spec.identity.owner(),
            deadline: spec.expires_at,
            prepare: Box::new(move |browser, audit, waiting| {
                if waiting.is_some() {
                    return Err(crate::contract_failure!());
                }
                StagedActor::for_public_qualification(
                    input, browser, runtime, provider, access, audit, task,
                )
            }),
        };
        Self::from_actor(
            spec,
            actor,
            profile,
            ports.engine,
            ports.journal,
            ports.audit,
            ports.native,
        )
    }
}
