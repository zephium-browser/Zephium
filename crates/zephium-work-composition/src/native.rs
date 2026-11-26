//! The only production adapter from actual desktop owners to Work admission.

use std::sync::{Arc, Mutex};
use zephium_agent_controller::{
    AgentBrowserModelAccess, AgentWorkFailure, AgentWorkRunInput, AgentWorkTask,
};
use zephium_app::{
    AgentWorkApplicationConfig, AgentWorkApplicationHandle, AgentWorkApplicationPorts,
    CallbackHandle, PreparedAgentWork,
};
use zephium_engine::WebviewEngine;
use zephium_store::SqliteStore;

/// Product-owned input. No objective parser or model text may mint the task,
/// effect assessment, account attestation, manifest or profile assignment.
/// This layer intentionally has no default task or implicit effect permission.
pub struct TrustedWorkRequest {
    pub(crate) construction_attempt: zephium_agentic::WorkBrowserConstructionAttempt,
    page: Option<zephium_app::RetainedPageAdmission>,
    anonymous_session: Option<zephium_agentic::WorkBrowserSession>,
    pub(crate) redirect: Option<zephium_agentic::WorkBrowserRedirect>,
    work: Option<zephium_agentic::WorkId>,
    pub(crate) input: AgentWorkRunInput,
    pub(crate) config: AgentWorkApplicationConfig,
    pub(crate) access: AgentBrowserModelAccess,
    pub(crate) task: Box<dyn AgentWorkTask>,
    browser_profile: Option<zephium_app::AgentWorkProfileBinding>,
    #[cfg(feature = "public-qualification")]
    public_qualification: bool,
}

impl TrustedWorkRequest {
    /// Transfers the approved functional-core input and trusted task authority.
    pub fn new(
        input: AgentWorkRunInput,
        config: AgentWorkApplicationConfig,
        access: impl Into<AgentBrowserModelAccess>,
        task: Box<dyn AgentWorkTask>,
    ) -> Self {
        Self {
            construction_attempt: Default::default(),
            page: None,
            anonymous_session: None,
            redirect: None,
            work: None,
            input,
            config,
            access: access.into(),
            task,
            browser_profile: None,
            #[cfg(feature = "public-qualification")]
            public_qualification: false,
        }
    }
    #[cfg(feature = "durable-runtime")]
    pub(crate) fn with_construction_attempt(
        mut self,
        attempt: zephium_agentic::WorkBrowserConstructionAttempt,
    ) -> Self {
        self.construction_attempt = attempt;
        self
    }
    #[cfg(feature = "durable-runtime")]
    pub(crate) fn with_page_admission(mut self, page: zephium_app::RetainedPageAdmission) -> Self {
        self.page = Some(page);
        self
    }
    #[cfg(feature = "durable-runtime")]
    pub(crate) fn model_images_for(
        &mut self,
        probe: &zephium_app::work_runtime::WorkAttemptProbe,
        blocked: bool,
        entry: Option<zephium_app::work_agent::WorkConfirmPort>,
    ) -> Result<
        Option<Arc<zephium_app::work_screenshots::WorkScreenshotAccess>>,
        zephium_core::work::WorkError,
    > {
        let Some(page) = self.page.as_ref() else {
            return Ok(None);
        };
        let identity = self
            .input
            .retained_resource_spec()
            .map_err(|_| zephium_core::work::WorkError::Invalid)?
            .identity;
        let host = zephium_app::work_screenshots::for_page(probe, page, identity, blocked, entry)
            .ok_or(zephium_core::work::WorkError::Invalid)?;
        self.input
            .bind_model_image_host(host.clone())
            .map_err(|_| zephium_core::work::WorkError::Invalid)?;
        Ok(Some(host))
    }
    pub(crate) fn with_anonymous_session(
        mut self,
        session: zephium_agentic::WorkBrowserSession,
    ) -> Self {
        self.anonymous_session = Some(session);
        self
    }
    /// Where the native gate keeps a public first load's refused redirect.
    #[cfg(feature = "durable-runtime")]
    pub(crate) fn with_redirect(mut self, redirect: zephium_agentic::WorkBrowserRedirect) -> Self {
        self.redirect = Some(redirect);
        self
    }
    /// Bind the aggregate owning this fresh dispatch; no authority is added.
    pub fn with_work_identity(mut self, work: zephium_agentic::WorkId) -> Self {
        self.work = Some(work);
        self
    }
    /// Requests the exact actor-selected browser session, revalidated by Shell.
    pub fn with_browser_profile(mut self, binding: zephium_app::AgentWorkProfileBinding) -> Self {
        self.browser_profile = Some(binding);
        self
    }

    /// Explicit public-only diagnostic retention. Absent from release graphs;
    /// it does not change the ordinary profile-bound application admission.
    #[cfg(feature = "public-qualification")]
    pub fn with_public_qualification_retention(mut self) -> Self {
        self.public_qualification = true;
        self
    }
}

impl std::fmt::Debug for TrustedWorkRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TrustedWorkRequest([owned, redacted])")
    }
}

/// Dormant, move-only desktop owners. Construction neither claims persistence
/// nor takes native authority. The shell checks both exact Arc identities.
pub struct NativeWorkComposition {
    #[cfg(feature = "durable-runtime")]
    pub(crate) human_pages: crate::human::HumanPages,
    /// Public record pictures are refused until the composition root sets it.
    #[cfg(feature = "durable-runtime")]
    pub(crate) public_routes: std::sync::OnceLock<zephium_agentic::public_asset::RouteSource>,
    engine: Arc<WebviewEngine>,
    store: Arc<SqliteStore>,
    native: Arc<Mutex<NativeLifetimeOwner>>,
}

enum NativeLifetimeOwner {
    Dormant,
    Factory(zephium_engine::AgentBrowserLifetimeFactory),
    Unavailable,
}

impl NativeWorkComposition {
    #[cfg(feature = "retained-lifetime-diagnostic")]
    pub fn retained_resource_failure_cause(
        &self,
        view: &zephium_app::RetainedWorkHandle,
    ) -> Option<zephium_engine::WorkResourceFailureCause> {
        self.engine
            .work_resource_failure_cause(&view.construction_resource_for_qualification()?)
    }
    /// Launches a stateless, single-page retained Work request through the
    /// original Shell. No qualification owner, rendering or navigation lease
    /// is substituted. Queue acceptance is not profile/durable admission;
    /// observe the returned bounded handle for the actual outcome.
    pub fn launch_retained(
        &self,
        shell: &CallbackHandle,
        request: TrustedWorkRequest,
    ) -> Result<Option<zephium_app::RetainedWorkHandle>, AgentWorkFailure> {
        let binding = request.browser_profile.ok_or(AgentWorkFailure::Contract)?;
        let ports = zephium_app::RetainedWorkPorts::new(
            self.engine.clone(),
            self.store.clone(),
            self.store.clone(),
            self.native_factory_for_group(request.page.as_ref().map(|page| page.native_group())),
        );
        let prepare = zephium_app::PreparedRetainedWork::try_new;
        #[cfg(feature = "public-qualification")]
        let prepare = if request.public_qualification {
            zephium_app::PreparedRetainedWork::try_new_for_public_qualification
        } else {
            prepare
        };
        let prepared = prepare(
            request.input,
            binding,
            request.config,
            request.access,
            request.task,
            ports,
        )?;
        let prepared = prepared.with_construction_attempt(request.construction_attempt)?;
        let prepared = match request.work {
            Some(work) => prepared.with_work_identity(work),
            None => prepared,
        };
        let prepared = match request.anonymous_session {
            Some(session) => prepared.with_anonymous_session(session)?,
            None => prepared,
        };
        let prepared = match request.redirect {
            Some(redirect) => prepared.with_redirect(redirect)?,
            None => prepared,
        };
        let prepared = match request.page {
            Some(page) => prepared.with_page_admission(page)?,
            None => prepared,
        };
        Ok(shell.attach_retained_work(prepared))
    }
    /// Uses the same owners passed to the normal application shell.
    pub fn new(engine: Arc<WebviewEngine>, store: Arc<SqliteStore>) -> Self {
        Self {
            engine,
            store,
            native: Arc::new(Mutex::new(NativeLifetimeOwner::Dormant)),
            #[cfg(feature = "durable-runtime")]
            human_pages: crate::human::HumanPages::default(),
            #[cfg(feature = "durable-runtime")]
            public_routes: std::sync::OnceLock::new(),
        }
    }

    /// The route anonymous public fetches take; the first one set is kept.
    #[cfg(feature = "durable-runtime")]
    pub fn set_public_routes(&self, routes: zephium_agentic::public_asset::RouteSource) {
        let _ = self.public_routes.set(routes);
    }

    /// Claims content-free recovery inventory only. No runtime/native page starts.
    pub fn attach(&self, shell: &CallbackHandle) -> Option<AgentWorkApplicationHandle> {
        shell.attach_work(self.store.clone(), self.engine.clone())
    }

    /// Explicit successor to this composition's exact completed application
    /// handle. The Shell independently proves old ownership is fully drained.
    pub fn attach_successor(
        &self,
        shell: &CallbackHandle,
        predecessor: &AgentWorkApplicationHandle,
    ) -> Option<AgentWorkApplicationHandle> {
        shell.attach_successor_work(self.store.clone(), self.engine.clone(), predecessor)
    }

    /// Validates without consuming this composition or attaching the journal.
    /// The deferred factory acquires a fresh, independently sealable lifetime
    /// only after both durable admission acknowledgements. Captured Arc owners
    /// retain exact identity; no prior port is reopened or reassigned.
    pub fn prepare(
        &self,
        request: TrustedWorkRequest,
    ) -> Result<PreparedAgentWork, AgentWorkFailure> {
        if request.anonymous_session.is_some() || request.page.is_some() {
            return Err(AgentWorkFailure::Contract);
        }
        let binding = request.browser_profile.ok_or(AgentWorkFailure::Contract)?;
        #[cfg(feature = "public-qualification")]
        if request.public_qualification {
            return PreparedAgentWork::try_new_for_public_probe(
                request.input,
                request.config,
                request.access,
                request.task,
                self.ports(),
            )?
            .with_browser_profile(binding);
        }
        let prepared = PreparedAgentWork::try_new(
            request.input,
            request.config,
            request.access,
            request.task,
            self.ports(),
        )?;
        prepared.with_browser_profile(binding)
    }

    pub(crate) fn ports(&self) -> AgentWorkApplicationPorts {
        let engine = self.engine.clone();
        let factory = self.native_factory();
        AgentWorkApplicationPorts::new(
            engine.clone(),
            self.store.clone(),
            Box::new(move |sink| {
                factory(Arc::new(move |event| {
                    let _ = sink.publish(event);
                }))
            }),
        )
    }

    // Both product lifetimes share the original one-shot factory acquisition;
    // neither composition can reopen a port or fork native lifetime ownership.
    fn native_factory(&self) -> zephium_app::RetainedWorkNativeFactory {
        self.native_factory_for_group(None)
    }

    fn native_factory_for_group(
        &self,
        group: Option<(zephium_agentic::WorkId, u8)>,
    ) -> zephium_app::RetainedWorkNativeFactory {
        let engine = self.engine.clone();
        let native = self.native.clone();
        Box::new(move |sink| {
            let mut native = native.lock().ok()?;
            if matches!(*native, NativeLifetimeOwner::Dormant) {
                *native = engine.take_agent_browser_lifetime_factory().map_or(
                    NativeLifetimeOwner::Unavailable,
                    NativeLifetimeOwner::Factory,
                );
            }
            let NativeLifetimeOwner::Factory(factory) = &mut *native else {
                return None;
            };
            match group {
                Some((work, capacity)) => factory
                    .begin_work_page(work, capacity, move |event| sink(event))
                    .ok(),
                None => factory.begin(move |event| sink(event)).ok(),
            }
        })
    }
}
