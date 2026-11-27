//! One light step on the person's own tab: what it would do, judged from the
//! observed control and its page with the same rules as Work
//! (`open_objective::site_work::classify`), and the closed action proposal
//! the engine runs. Protected fields, other sites and controls a verb cannot
//! use are refused here, before anything reaches the page.

use zephium_agentic::{
    ContextNavigationTarget, SemanticActionBatch, SemanticActionBatchId, SemanticActionIntent,
    SemanticActionKind, SemanticActionProposal, SemanticActionText, SemanticEffectClass,
    SemanticFrameJoin, SemanticNode, SemanticObservation, SemanticPressKey, SemanticReferenceId,
    SemanticRole, SemanticScrollAmount, SemanticScrollDirection, SemanticSensitivity,
    SemanticSettleBudget, SemanticSnapshot, SemanticState, SemanticVerification,
    SemanticWaitCondition,
};
use zephium_ipc::page_assist::WorkConfirmCategoryV1;
use zephium_page_assist::actions::{Direction, Verb};

use super::port::{ResolvedStep, StepEffect, StepFailure, StepRequest};
use crate::open_objective::site_work::{classify, Consequence, SiteEffect};

const SETTLE_MILLIS: u32 = 2_000;
const MAX_LABEL_CHARS: usize = 60;

/// A judged step and the proposal that runs it.
pub struct PlannedStep {
    /// What the person sees, and what it would do.
    pub resolved: ResolvedStep,
    /// The closed action the engine prepares against the page as it is.
    pub proposal: SemanticActionProposal,
}

fn category(consequence: Consequence) -> WorkConfirmCategoryV1 {
    match consequence {
        Consequence::Communication => WorkConfirmCategoryV1::Communication,
        Consequence::Purchase => WorkConfirmCategoryV1::Purchase,
        Consequence::Destructive => WorkConfirmCategoryV1::Destructive,
        Consequence::Save => WorkConfirmCategoryV1::Save,
        Consequence::Edit => WorkConfirmCategoryV1::Edit,
        Consequence::Type => WorkConfirmCategoryV1::Type,
    }
}

fn name(node: &SemanticNode) -> String {
    let raw = node
        .name()
        .or_else(|| node.text())
        .map(|text| text.as_str())
        .unwrap_or_default();
    let mut name: String = raw
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_LABEL_CHARS)
        .collect();
    name = name.replace('"', "'");
    if raw.chars().count() > MAX_LABEL_CHARS {
        name.push('…');
    }
    name
}

fn page_site(frame: &SemanticFrameJoin) -> Option<String> {
    ContextNavigationTarget::parse(frame.origin().as_url().as_str())
        .ok()
        .and_then(|target| zephium_agentic::registrable_site(&target))
}

/// The option of `select` named `value`, among its descendants.
fn option(
    snapshot: &SemanticSnapshot,
    select: &SemanticNode,
    value: &str,
) -> Option<SemanticReferenceId> {
    let nodes = snapshot.nodes();
    let index = nodes
        .iter()
        .position(|node| node.reference() == select.reference())?;
    let wanted = value.trim().to_lowercase();
    let mut inside = vec![index];
    for (at, node) in nodes.iter().enumerate().skip(index + 1) {
        let Some(parent) = node.parent().map(usize::from) else {
            continue;
        };
        if !inside.contains(&parent) {
            continue;
        }
        inside.push(at);
        if node.role() == SemanticRole::Option
            && node
                .name()
                .is_some_and(|name| name.as_str().trim().to_lowercase() == wanted)
        {
            return Some(node.reference());
        }
    }
    None
}

/// Judges `step` against the read the person's question was answered from.
/// `target` is the observed node behind the step's passage.
pub fn plan(
    observation: &SemanticObservation,
    frame: &SemanticFrameJoin,
    target: Option<SemanticReferenceId>,
    step: &StepRequest,
) -> Result<PlannedStep, StepFailure> {
    let snapshot = observation
        .frames()
        .first()
        .ok_or(StepFailure::Unsuitable)?;
    let settle = SemanticSettleBudget::try_new(SETTLE_MILLIS).map_err(|_| StepFailure::Failed)?;
    let propose = |intent, effect, verification| {
        let proposal = SemanticActionProposal::try_new(
            intent,
            effect,
            SemanticWaitCondition::Immediate,
            verification,
            settle,
        )
        .map_err(|_| StepFailure::Unsuitable)?;
        // Binding is the same check the engine repeats on the live page.
        SemanticActionBatch::bind(
            SemanticActionBatchId::new(1).ok_or(StepFailure::Failed)?,
            observation,
            std::slice::from_ref(frame),
            vec![proposal.clone()],
        )
        .map_err(|_| StepFailure::Unsuitable)?;
        Ok::<_, StepFailure>(proposal)
    };

    if step.verb == Verb::Scroll && target.is_none() {
        let page = snapshot.nodes().first().ok_or(StepFailure::Unsuitable)?;
        let (direction, label) = match step.direction {
            Some(Direction::Up) => (SemanticScrollDirection::Up, "Scroll up"),
            Some(Direction::Down) | None => (SemanticScrollDirection::Down, "Scroll down"),
        };
        return Ok(PlannedStep {
            resolved: ResolvedStep {
                label: label.to_owned(),
                effect: StepEffect::Read,
            },
            proposal: propose(
                SemanticActionIntent::Scroll {
                    target: page.reference(),
                    direction,
                    amount: SemanticScrollAmount::Page,
                },
                SemanticEffectClass::Read,
                SemanticVerification::ScrollPositionChanged,
            )?,
        });
    }

    let target = target.ok_or(StepFailure::Unsuitable)?;
    let node = observation
        .resolve_node(target, frame)
        .map_err(|_| StepFailure::Unsuitable)?;
    if node.role() == SemanticRole::Password || node.sensitivity() == SemanticSensitivity::Secret {
        return Err(StepFailure::Refused);
    }
    let role = node.role();
    let clickable = matches!(
        role,
        SemanticRole::Button
            | SemanticRole::Link
            | SemanticRole::MenuItem
            | SemanticRole::Tab
            | SemanticRole::Option
            | SemanticRole::Checkbox
            | SemanticRole::Radio
            | SemanticRole::Combobox
    );
    let label = name(node);
    let (intent, kind, verification, text) = match step.verb {
        Verb::Click if clickable => (
            SemanticActionIntent::Click { target },
            SemanticActionKind::Click,
            SemanticVerification::PageChanged,
            format!("Click \"{label}\""),
        ),
        Verb::Open if role == SemanticRole::Link => {
            let destination = node.link_destination().ok_or(StepFailure::Unsuitable)?;
            let same = page_site(frame).is_some_and(|site| {
                zephium_agentic::registrable_site(destination).as_deref() == Some(site.as_str())
            });
            if !same {
                return Err(StepFailure::Refused);
            }
            (
                SemanticActionIntent::Click { target },
                SemanticActionKind::Click,
                SemanticVerification::PageChanged,
                format!("Open \"{label}\""),
            )
        }
        Verb::Expand if clickable => (
            SemanticActionIntent::Click { target },
            SemanticActionKind::Click,
            SemanticVerification::TargetState {
                state: SemanticState::Expanded,
                present: true,
            },
            format!("Expand \"{label}\""),
        ),
        Verb::Check | Verb::Uncheck
            if matches!(role, SemanticRole::Checkbox | SemanticRole::Radio) =>
        {
            let present = step.verb == Verb::Check;
            (
                SemanticActionIntent::Click { target },
                SemanticActionKind::Click,
                SemanticVerification::TargetState {
                    state: SemanticState::Checked,
                    present,
                },
                format!("{} \"{label}\"", if present { "Check" } else { "Uncheck" }),
            )
        }
        Verb::Fill
            if matches!(
                role,
                SemanticRole::Textbox
                    | SemanticRole::Searchbox
                    | SemanticRole::Spinbutton
                    | SemanticRole::Combobox
            ) =>
        {
            let value = step.value.clone().ok_or(StepFailure::Unsuitable)?;
            let value = SemanticActionText::try_new(value).map_err(|_| StepFailure::Unsuitable)?;
            (
                SemanticActionIntent::Fill { target, value },
                SemanticActionKind::Fill,
                SemanticVerification::TargetValueMatchesInput,
                format!("Type into \"{label}\""),
            )
        }
        Verb::Submit
            if matches!(
                role,
                SemanticRole::Textbox | SemanticRole::Searchbox | SemanticRole::Combobox
            ) =>
        {
            (
                SemanticActionIntent::Press {
                    target,
                    key: SemanticPressKey::Enter,
                },
                SemanticActionKind::Press,
                SemanticVerification::PageChanged,
                format!("Press Return in \"{label}\""),
            )
        }
        Verb::Select if matches!(role, SemanticRole::Combobox | SemanticRole::Listbox) => {
            let value = step.value.as_deref().ok_or(StepFailure::Unsuitable)?;
            let choice = option(snapshot, node, value).ok_or(StepFailure::Unsuitable)?;
            (
                SemanticActionIntent::Select {
                    target,
                    option: choice,
                },
                SemanticActionKind::Select,
                SemanticVerification::TargetSelectionMatchesOption,
                format!("Choose \"{}\" in \"{label}\"", name_of(value)),
            )
        }
        Verb::Scroll => (
            SemanticActionIntent::Scroll {
                target,
                direction: SemanticScrollDirection::Down,
                amount: SemanticScrollAmount::IntoView,
            },
            SemanticActionKind::Scroll,
            SemanticVerification::ScrollPositionChanged,
            format!("Scroll to \"{label}\""),
        ),
        _ => return Err(StepFailure::Unsuitable),
    };
    let key = (kind == SemanticActionKind::Press).then_some(SemanticPressKey::Enter);
    let (effect, class) = match classify(kind, key, node, snapshot) {
        SiteEffect::Read => (StepEffect::Read, SemanticEffectClass::Read),
        SiteEffect::Draft => (StepEffect::Draft, SemanticEffectClass::LocalWrite),
        SiteEffect::Commit(consequence) => (
            StepEffect::Commit(category(consequence)),
            consequence.class(),
        ),
    };
    Ok(PlannedStep {
        resolved: ResolvedStep {
            label: text,
            effect,
        },
        proposal: propose(intent, class, verification)?,
    })
}

fn name_of(value: &str) -> String {
    value
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_LABEL_CHARS)
        .collect::<String>()
        .replace('"', "'")
}

#[cfg(test)]
mod tests {
    use super::*;
    use zephium_agentic::{
        decode_semantic_snapshot, ContextCapabilities, ContextCapability, ContextId,
        ContextIdentity, ContextKind, ContextOperationId, ContextRegistry, ContextRunId,
        ContextSettlement, FrameId, SemanticDecodeContext, SemanticFrameTrust,
        SemanticInvocationId, SemanticObservationAssembler, SemanticObservationBudget,
        SemanticObservationId, SemanticObservationRequest, SemanticOrigin,
        SemanticSnapshotGeneration,
    };

    fn observed(nodes: &str) -> (SemanticObservation, SemanticFrameJoin) {
        let identity = ContextIdentity::new(
            ContextId::generate(),
            ContextRunId::generate(),
            1_u128.into(),
            ContextKind::Owned,
        );
        let mut registry = ContextRegistry::new();
        registry
            .reserve(
                identity,
                ContextCapabilities::try_new(ContextKind::Owned, &[ContextCapability::Observe])
                    .expect("capabilities"),
            )
            .expect("reserve");
        let operation = registry
            .begin_context(
                identity.id(),
                ContextOperationId::new(1).expect("operation"),
            )
            .expect("begin");
        registry
            .settle_construction(identity.id(), operation, ContextSettlement::Applied)
            .expect("settle");
        let context = registry.join(identity.id()).expect("join");
        let frame = SemanticFrameJoin::try_new(
            context,
            FrameId::MAIN,
            context.frame_generation(),
            SemanticOrigin::parse("https://shop.example.com/cart").expect("origin"),
            SemanticFrameTrust::SameOrigin,
        )
        .expect("frame");
        let wire = format!(r#"{{"v":1,"i":1,"g":1,"c":"complete","n":[{nodes}]}}"#);
        let snapshot = decode_semantic_snapshot(
            SemanticDecodeContext::new(
                SemanticInvocationId::new(1).expect("invocation"),
                frame.clone(),
                SemanticSnapshotGeneration::new(1).expect("generation"),
            ),
            wire.as_bytes(),
        )
        .expect("snapshot");
        let observation = SemanticObservationAssembler::new(
            SemanticObservationRequest::initial(
                SemanticObservationId::new(1).expect("observation"),
                context,
                SemanticObservationBudget::WHOLE_PAGE,
            ),
            snapshot,
        )
        .expect("assembler")
        .finish()
        .expect("observation");
        (observation, frame)
    }

    fn reference(observation: &SemanticObservation, index: usize) -> SemanticReferenceId {
        observation.frames()[0].nodes()[index].reference()
    }

    fn step(verb: Verb, value: Option<&str>) -> StepRequest {
        StepRequest {
            verb,
            read: 1,
            target: Some(1),
            direction: None,
            value: value.map(str::to_owned),
        }
    }

    const PAGE: &str = r#"{"k":1,"r":"document","o":16},
        {"k":2,"p":0,"r":"button","n":"Place order","ak":1,"o":9,"fc":true,"b":{"x":10,"y":10,"w":100,"h":30}},
        {"k":3,"p":0,"r":"password","n":"Password","q":"secret","v":{"k":"redacted"}},
        {"k":4,"p":0,"r":"link","n":"Elsewhere","o":9,"u":"https://other.example.org/"},
        {"k":5,"p":0,"r":"textbox","n":"Note","s":64,"o":2,"fc":true,"v":{"k":"text","value":""},"b":{"x":10,"y":130,"w":100,"h":30}}"#;

    #[test]
    fn a_committing_control_waits_for_confirm() {
        let (observation, frame) = observed(PAGE);
        let planned = plan(
            &observation,
            &frame,
            Some(reference(&observation, 1)),
            &step(Verb::Click, None),
        )
        .expect("planned");
        assert_eq!(planned.resolved.label, "Click \"Place order\"");
        assert_eq!(
            planned.resolved.effect,
            StepEffect::Commit(WorkConfirmCategoryV1::Purchase)
        );
        assert_eq!(planned.proposal.effect(), SemanticEffectClass::Purchase);
    }

    #[test]
    fn an_unknown_sidebar_button_is_never_a_local_draft_action() {
        let (observation, frame) = observed(
            r#"{"k":1,"r":"document","o":16},
            {"k":2,"p":0,"r":"button","n":"送信する","ak":1,"o":9,"fc":true,"b":{"x":10,"y":10,"w":100,"h":30}}"#,
        );
        let planned = plan(
            &observation,
            &frame,
            Some(reference(&observation, 1)),
            &step(Verb::Click, None),
        )
        .unwrap();
        assert_eq!(
            planned.resolved.effect,
            StepEffect::Commit(WorkConfirmCategoryV1::Save)
        );
        assert_eq!(planned.proposal.effect(), Consequence::Save.class());
    }

    #[test]
    fn protected_fields_and_other_sites_are_refused() {
        let (observation, frame) = observed(PAGE);
        assert_eq!(
            plan(
                &observation,
                &frame,
                Some(reference(&observation, 2)),
                &step(Verb::Fill, Some("hunter2")),
            )
            .err(),
            Some(StepFailure::Refused)
        );
        assert_eq!(
            plan(
                &observation,
                &frame,
                Some(reference(&observation, 3)),
                &step(Verb::Open, None),
            )
            .err(),
            Some(StepFailure::Refused)
        );
        assert_eq!(
            plan(
                &observation,
                &frame,
                Some(reference(&observation, 1)),
                &step(Verb::Fill, Some("x")),
            )
            .err(),
            Some(StepFailure::Unsuitable)
        );
    }

    #[test]
    fn everyday_steps_run_directly() {
        let (observation, frame) = observed(PAGE);
        let typed = plan(
            &observation,
            &frame,
            Some(reference(&observation, 4)),
            &step(Verb::Fill, Some("Leave at the door")),
        )
        .expect("typed");
        assert_eq!(typed.resolved.effect, StepEffect::Draft);
        assert_eq!(typed.resolved.label, "Type into \"Note\"");
        let scrolled = plan(
            &observation,
            &frame,
            None,
            &StepRequest {
                verb: Verb::Scroll,
                read: 1,
                target: None,
                direction: Some(Direction::Down),
                value: None,
            },
        )
        .expect("scrolled");
        assert_eq!(scrolled.resolved.effect, StepEffect::Read);
    }
}
