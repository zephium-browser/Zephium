//! Exact native image claims joined to separate host consent. This is not an
//! expansion of text sensitivity, account access, actions, or local crop rights.
use super::*;
use crate::{
    SemanticModelImageHost, SemanticModelImagePlan, SemanticScreenshot,
    SemanticScreenshotRequestId, SemanticScreenshotSource, SemanticScreenshotSourcePort,
    WorkBrowserAnonymousReadAuthority, WorkBrowserAnonymousReadLoss,
};
use std::{future::Future, pin::Pin, sync::Arc, task::Poll, time::Instant};
use zephium_core::work::model::WorkModelImageFence;
const MAX_IMAGE_CLAIMS: usize = 32;

pub(super) struct ImageOwners {
    lease: AgentPlanLeaseId,
    source: Arc<dyn SemanticScreenshotSourcePort>,
    host: Arc<dyn SemanticModelImageHost>,
    anonymous: Option<WorkBrowserAnonymousReadAuthority>,
    history: Option<Arc<ImageFence>>,
    claims: Vec<ImageClaim>,
}
struct ImageClaim {
    guard: [u8; 32],
    cohorts: Vec<AgentTaintCohort>,
}
struct ImageFence {
    host: Arc<dyn WorkModelImageFence>,
    source: Arc<dyn SemanticScreenshotSource>,
    anonymous: Option<WorkBrowserAnonymousReadLoss>,
    admitted_at: AgentPolicyInstant,
    wall_at: Instant,
}
impl ImageFence {
    fn now(&self) -> Option<AgentPolicyInstant> {
        let elapsed = u64::try_from(self.wall_at.elapsed().as_millis()).ok()?;
        self.admitted_at
            .millis()
            .checked_add(elapsed)
            .map(AgentPolicyInstant::from_millis)
    }
    fn anonymous_current(&self) -> bool {
        self.anonymous
            .as_ref()
            .is_none_or(|proof| self.now().is_some_and(|now| proof.is_current(now)))
    }
}
impl WorkModelImageFence for ImageFence {
    fn while_current(&self, operation: &mut dyn FnMut()) -> bool {
        if Instant::now() >= self.source.deadline() || !self.anonymous_current() {
            return false;
        }
        let mut native = false;
        let mut executed = false;
        let host = self.host.while_current(&mut || {
            native = self.source.while_current(&mut || {
                if self.anonymous_current() {
                    executed = true;
                    operation();
                }
            });
        });
        host && native && executed
    }
    fn revoked(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(async move {
            let mut host = self.host.revoked();
            std::future::poll_fn(|cx| {
                if !self.while_current(&mut || {})
                    || host.as_mut().poll(cx).is_ready()
                    || self.source.poll_lost(cx).is_ready()
                {
                    return Poll::Ready(());
                }
                if let Some(proof) = &self.anonymous {
                    let Some(now) = self.now() else {
                        return Poll::Ready(());
                    };
                    if proof.poll_loss(now, cx).is_ready() {
                        return Poll::Ready(());
                    }
                }
                Poll::Pending
            })
            .await
        })
    }
}
impl AgentRunPolicy {
    /// Installs the two original trusted owners once, before any model call.
    /// Neither owner alone grants disclosure; native claim and host ACK are
    /// required for every new image. No model/IPC fields can install owners.
    pub fn bind_model_image_owners(
        &mut self,
        lease: AgentPlanLeaseId,
        source: Arc<dyn SemanticScreenshotSourcePort>,
        host: Arc<dyn SemanticModelImageHost>,
        anonymous: Option<WorkBrowserAnonymousReadAuthority>,
    ) -> Result<(), AgentPolicyError> {
        if self.sealed
            || self.last_call.is_some()
            || !self.calls.is_empty()
            || self.image_owners.is_some()
            || self.lease_index(lease).is_none()
            || !host.admits_source(source.lease())
            || source.lease().run() != self.manifest.run()
        {
            return Err(AgentPolicyError::Authority);
        }
        self.image_owners = Some(ImageOwners {
            lease,
            source,
            host,
            anonymous,
            history: None,
            claims: Vec::new(),
        });
        Ok(())
    }
    /// Availability only; actual capture and disclosure need fresh joins.
    pub fn model_images_available(&self, lease: AgentPlanLeaseId) -> bool {
        !self.sealed
            && self.image_owners.as_ref().is_some_and(|owners| {
                owners.lease == lease
                    && owners.host.enabled()
                    && owners
                        .history
                        .as_ref()
                        .is_none_or(|history| history.while_current(&mut || {}))
            })
    }
    /// Queues the separate screenshot-origin question after a negative public
    /// survey. No pixels were captured or admitted by that refusal.
    pub fn model_image_consent_required(
        &self,
        context: ContextJoin,
        origin: &str,
    ) -> SemanticModelImagePlan {
        self.image_owners
            .as_ref()
            .map_or(SemanticModelImagePlan::Unavailable, |owners| {
                // A page in Work's own website data holds nothing of the
                // person's: it goes on without a picture rather than asking.
                if owners.anonymous.is_some() {
                    return SemanticModelImagePlan::Unavailable;
                }
                owners.host.require_consent(context, origin)
            })
    }
    fn public_image_candidate(
        &self,
        owners: &ImageOwners,
        context: ContextJoin,
        now: AgentPolicyInstant,
    ) -> bool {
        let Some(node) = self
            .lease_index(owners.lease)
            .and_then(|i| self.manifest.plan_node(self.leases[i].binding.node()))
        else {
            return false;
        };
        self.manifest.scope().effects().len() == 1
            && node.effects().len() == 1
            && self
                .manifest
                .scope()
                .effects()
                .contains(SemanticEffectClass::Read)
            && node.effects().contains(SemanticEffectClass::Read)
            && owners
                .anonymous
                .as_ref()
                .is_some_and(|proof| proof.matches(context.identity(), now))
            && owners.source.public_candidate(context)
    }
    /// Chooses a fixed capture mode only; a public candidate is never proof.
    pub fn model_image_plan(
        &self,
        id: SemanticScreenshotRequestId,
        context: ContextJoin,
        origin: &str,
        now: AgentPolicyInstant,
    ) -> SemanticModelImagePlan {
        let Some(owners) = self.image_owners.as_ref() else {
            return SemanticModelImagePlan::Unavailable;
        };
        if self.sealed
            || context.identity().owner() != self.manifest.run()
            || owners
                .history
                .as_ref()
                .is_some_and(|fence| !fence.while_current(&mut || {}))
        {
            return SemanticModelImagePlan::Unavailable;
        }
        owners.host.prepare(
            id,
            context,
            origin,
            self.public_image_candidate(owners, context, now),
        )
    }
    /// Consumes the exact original native receipt, then joins the host's
    /// pre-capture consent epoch and durable conservative disclosure ACK.
    pub async fn admit_model_image(
        &mut self,
        image: &SemanticScreenshot,
        observation: &SemanticObservation,
        account: AgentContextAccountBinding,
        now: AgentPolicyInstant,
    ) -> Result<(), AgentPolicyError> {
        let owners = self
            .image_owners
            .as_ref()
            .ok_or(AgentPolicyError::Authority)?;
        let guard = image.delivery_guard();
        if self.sealed
            || !self.calls.is_empty()
            || !self.effects.is_empty()
            || self.navigation.is_some()
            || owners.claims.len() >= MAX_IMAGE_CLAIMS
            || owners.claims.iter().any(|claim| claim.guard == guard)
            || account.context() != image.context()
            || image.context() != observation.request().context()
            || !image.matches_observation(observation)
        {
            return Err(AgentPolicyError::Authority);
        }
        let node = self
            .lease_index(owners.lease)
            .and_then(|i| self.manifest.plan_node(self.leases[i].binding.node()))
            .ok_or(AgentPolicyError::Lease)?;
        validate_time(&self.manifest, node.expires_at(), account, now)?;
        let public = image.requires_public_page();
        if public && !self.public_image_candidate(owners, image.context(), now) {
            return Err(AgentPolicyError::Authority);
        }
        let origin = observation
            .frames()
            .first()
            .ok_or(AgentPolicyError::Authority)?
            .frame()
            .origin();
        let cohorts = screenshot_taints(observation, account, guard)?;
        // Check every non-pixel authority without downgrading retained taint.
        let mut scope = cohorts.clone();
        for cohort in &mut scope {
            cohort.sensitivity = SemanticSensitivity::Public;
        }
        validate_context_scope(&self.manifest, node, image.context(), account, &scope)?;
        let source = owners
            .source
            .claim(image)
            .ok_or(AgentPolicyError::Authority)?;
        if !source.authenticates(image) || !source.while_current(&mut || {}) {
            return Err(AgentPolicyError::Authority);
        }
        let anonymous = if public {
            Some(
                owners
                    .anonymous
                    .as_ref()
                    .ok_or(AgentPolicyError::Authority)?
                    .watch_loss(now)
                    .ok_or(AgentPolicyError::Authority)?,
            )
        } else {
            None
        };
        let origin = origin.as_url().origin().ascii_serialization();
        let expires = account
            .observed_at()
            .millis()
            .checked_add(MAX_AGENT_ACCOUNT_ATTESTATION_AGE_MILLIS)
            .ok_or(AgentPolicyError::Authority)?
            .min(node.expires_at().millis())
            .min(self.manifest.expires_at().millis());
        let remaining = expires
            .checked_sub(now.millis())
            .filter(|remaining| *remaining > 0)
            .ok_or(AgentPolicyError::Authority)?;
        let deadline = Instant::now()
            .checked_add(std::time::Duration::from_millis(remaining))
            .ok_or(AgentPolicyError::Authority)?
            .min(source.deadline());
        let acknowledged = owners
            .host
            .before_disclosure(image.id(), image.context(), &origin, public, deadline)
            .await;
        if owners.host.private_disclosure_recorded() {
            if let Some(proof) = &owners.anonymous {
                proof.revoke_private_disclosure();
            }
        }
        let host = acknowledged.ok_or(AgentPolicyError::Authority)?;
        let owners = self
            .image_owners
            .as_mut()
            .ok_or(AgentPolicyError::Authority)?;
        if !source.authenticates(image)
            || !source.while_current(&mut || {})
            || !host.while_current(&mut || {})
        {
            return Err(AgentPolicyError::Authority);
        }
        let history = if let Some(history) = &owners.history {
            if !Arc::ptr_eq(&history.host, &host) || !history.while_current(&mut || {}) {
                return Err(AgentPolicyError::Authority);
            }
            history.clone()
        } else {
            Arc::new(ImageFence {
                host,
                source,
                anonymous,
                admitted_at: now,
                wall_at: Instant::now(),
            })
        };
        if !history.while_current(&mut || {}) {
            return Err(AgentPolicyError::Authority);
        }
        owners.history = Some(history);
        owners.claims.push(ImageClaim { guard, cohorts });
        Ok(())
    }
    // Provider/server retention can replay an earlier picture without an
    // explicit image part in the next body. Fence the whole existing history.
    #[cfg(feature = "provider-transport")]
    pub(crate) fn has_model_image_history(&self) -> bool {
        self.image_owners
            .as_ref()
            .is_some_and(|owners| owners.history.is_some())
    }
    pub(crate) fn model_image_fence(&self) -> Option<Arc<dyn WorkModelImageFence>> {
        let fence = self.image_owners.as_ref()?.history.as_ref()?;
        fence
            .while_current(&mut || {})
            .then(|| fence.clone() as Arc<dyn WorkModelImageFence>)
    }
    pub(super) fn permits_exact_image_cohort(&self, cohort: &AgentTaintCohort) -> bool {
        cohort.sensitivity == SemanticSensitivity::Sensitive
            && cohort.references.is_empty()
            && self.model_image_fence().is_some()
            && self.image_owners.as_ref().is_some_and(|owners| {
                owners
                    .claims
                    .iter()
                    .any(|claim| claim.cohorts.contains(cohort))
            })
    }
}
