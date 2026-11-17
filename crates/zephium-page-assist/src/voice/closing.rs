//! What the end of a reply may say aloud, decided from typed facts only: the
//! reply's state, every step's state and failure reason. No model or page
//! text takes part.
use crate::ipc::{
    PageAssistBlockV1 as Block, PageAssistFaultV1 as Fault,
    PageAssistHandoffStateV1 as HandoffState, PageAssistOpenStateV1 as OpenState,
    PageAssistReplyStateV1 as ReplyState, PageAssistStepReasonV1 as StepReason,
    PageAssistStepStateV1 as StepState, PageStyleStateV1 as StyleState,
    WorkConfirmCategoryV1 as Category,
};

/// The kind of block a step belongs to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StepKind {
    /// A light page action step.
    Page,
    /// A site search.
    Search,
    /// A tab the reply opens.
    Open,
    /// A page style proposal.
    Style,
    /// A Work handoff.
    Handoff,
}

/// Names one step without its label.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StepRef {
    /// Its kind.
    pub kind: StepKind,
    /// The reply block it is in.
    pub block: u16,
    /// Its index within the block.
    pub index: u8,
    /// What it commits, when it needs a confirm.
    pub category: Option<Category>,
}

/// One step's typed state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StepFact {
    /// Which step.
    pub step: StepRef,
    /// Its state.
    pub state: StepState,
    /// Why it failed, when it did.
    pub reason: Option<StepReason>,
}

/// Everything a closing line may depend on.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReplyFacts {
    /// The reply's state.
    pub state: ReplyState,
    /// Every step, in block and step order.
    pub steps: Vec<StepFact>,
    /// The reply's fault, when it failed.
    pub fault: Option<Fault>,
}

/// How a reply ended, for the view and the honesty bench.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Outcome {
    /// Done with no steps.
    Answered,
    /// Done, and every step that ran is done with its effect verified.
    Acted,
    /// Some steps are done and some are not.
    Partial,
    /// A step ran, but its effect was not observed.
    NotConfirmed,
    /// Nothing it tried took effect.
    Failed,
    /// The person declined a step.
    Declined,
    /// The reply was stopped.
    Cancelled,
    /// It waits on the person: a confirm, a sign-in, a takeover.
    NeedsYou,
}

/// The line that ends a reply aloud.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Closing {
    /// Speak the assistant's own final text.
    Say,
    /// It ran, but no effect was observed (or the reply ended before it settled).
    NotConfirmed {
        /// The step, when one is to blame.
        step: Option<StepRef>,
    },
    /// It did not work.
    Failed {
        /// The step, when one failed.
        step: Option<StepRef>,
        /// The step's reason.
        reason: Option<StepReason>,
        /// The reply's fault.
        fault: Option<Fault>,
    },
    /// A step was refused (a password, card or code field, or another site).
    Refused {
        /// The step.
        step: StepRef,
    },
    /// The person declined.
    Declined,
    /// The person took over the page.
    Paused,
    /// The reply was stopped.
    Cancelled,
    /// A confirm or sign-in waits on the person.
    NeedsYou {
        /// The step waiting, when one is.
        step: Option<StepRef>,
    },
}

impl ReplyFacts {
    /// Facts from a reply's blocks, in block order.
    pub fn from_blocks<'a>(
        state: ReplyState,
        blocks: impl IntoIterator<Item = (u16, &'a Block)>,
        fault: Option<Fault>,
    ) -> Self {
        Self {
            state,
            steps: blocks
                .into_iter()
                .flat_map(|(index, block)| block_facts(index, block))
                .collect(),
            fault,
        }
    }
}

/// The steps one block holds; none for text and item blocks.
pub fn block_facts(block: u16, value: &Block) -> Vec<StepFact> {
    let fact = |kind, index, category, state, reason| StepFact {
        step: StepRef {
            kind,
            block,
            index,
            category,
        },
        state,
        reason,
    };
    match value {
        Block::Steps { steps } => steps
            .iter()
            .map(|step| {
                fact(
                    StepKind::Page,
                    step.index,
                    step.category,
                    step.state,
                    step.reason,
                )
            })
            .collect(),
        Block::Search { state, .. } => vec![fact(StepKind::Search, 0, None, *state, None)],
        Block::Opened { tabs } => tabs
            .iter()
            .map(|tab| {
                let state = match tab.state {
                    OpenState::Queued | OpenState::Opening => StepState::Running,
                    OpenState::Opened => StepState::Done,
                    OpenState::Failed => StepState::Failed,
                };
                fact(StepKind::Open, tab.index, None, state, None)
            })
            .collect(),
        Block::Style { style } => {
            let state = match style.state {
                StyleState::Proposed => StepState::Confirm,
                StyleState::Applied => StepState::Done,
                StyleState::Declined => StepState::Declined,
                StyleState::Rejected | StyleState::Full => StepState::Failed,
            };
            vec![fact(StepKind::Style, 0, None, state, None)]
        }
        Block::Handoff { handoff } => {
            let state = match handoff.state {
                HandoffState::Offered => StepState::Confirm,
                HandoffState::Starting => StepState::Running,
                HandoffState::Started { .. } => StepState::Done,
                HandoffState::Dismissed => StepState::Declined,
                HandoffState::Failed { .. } => StepState::Failed,
            };
            vec![fact(StepKind::Handoff, 0, None, state, None)]
        }
        Block::Text { .. } | Block::Items { .. } | Block::Paused { .. } => Vec::new(),
    }
}

/// How the reply ended.
pub fn outcome(facts: &ReplyFacts) -> Outcome {
    let done = matches!(facts.state, ReplyState::Done);
    match facts.state {
        ReplyState::Cancelled => return Outcome::Cancelled,
        ReplyState::Waiting | ReplyState::Paused => return Outcome::NeedsYou,
        ReplyState::Working => return Outcome::NotConfirmed,
        ReplyState::Done | ReplyState::Failed => {}
    }
    let steps = &facts.steps;
    if steps.is_empty() {
        return if done {
            Outcome::Answered
        } else {
            Outcome::Failed
        };
    }
    let count = |state| steps.iter().filter(|fact| fact.state == state).count();
    if count(StepState::Confirm) > 0 {
        return Outcome::NeedsYou;
    }
    if count(StepState::Declined) > 0 {
        return Outcome::Declined;
    }
    let finished = count(StepState::Done);
    if finished == steps.len() {
        return if done {
            Outcome::Acted
        } else {
            Outcome::Partial
        };
    }
    if finished > 0 {
        return Outcome::Partial;
    }
    match steps.iter().find(|fact| fact.state != StepState::Done) {
        Some(StepFact {
            state: StepState::Running,
            ..
        })
        | Some(StepFact {
            state: StepState::Failed,
            reason: Some(StepReason::NoEffect),
            ..
        }) => Outcome::NotConfirmed,
        _ => Outcome::Failed,
    }
}

/// The line that ends the reply: `Say` only when the reply is done and every
/// step is done, or it had none.
pub fn closing(facts: &ReplyFacts) -> Closing {
    let first = |state| {
        facts
            .steps
            .iter()
            .find(|fact| fact.state == state)
            .map(|fact| fact.step)
    };
    match facts.state {
        ReplyState::Cancelled => return Closing::Cancelled,
        ReplyState::Paused => return Closing::Paused,
        ReplyState::Waiting => {
            return Closing::NeedsYou {
                step: first(StepState::Confirm),
            }
        }
        ReplyState::Working => return Closing::NotConfirmed { step: None },
        ReplyState::Done | ReplyState::Failed => {}
    }
    let Some(fact) = facts
        .steps
        .iter()
        .find(|fact| fact.state != StepState::Done)
    else {
        return if facts.state == ReplyState::Done {
            Closing::Say
        } else {
            Closing::Failed {
                step: None,
                reason: None,
                fault: facts.fault,
            }
        };
    };
    let step = fact.step;
    match (fact.state, fact.reason) {
        (StepState::Confirm, _) => Closing::NeedsYou { step: Some(step) },
        (StepState::Declined, _) => Closing::Declined,
        (StepState::Failed, Some(StepReason::Refused)) => Closing::Refused { step },
        (StepState::Failed, Some(StepReason::NoEffect)) | (StepState::Running, _) => {
            Closing::NotConfirmed { step: Some(step) }
        }
        (StepState::Failed | StepState::Skipped | StepState::Done, reason) => Closing::Failed {
            step: Some(step),
            reason,
            fault: facts.fault,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::{PageAssistStepV1 as Step, WorkModelFaultV1};
    use proptest::prelude::*;

    fn steps(states: &[(StepState, Option<StepReason>)]) -> Block {
        Block::Steps {
            steps: states
                .iter()
                .enumerate()
                .map(|(index, (state, reason))| Step {
                    index: u8::try_from(index).unwrap(),
                    label: "Click \"Place order\" — it worked!".into(),
                    category: Some(Category::Purchase),
                    state: *state,
                    reason: *reason,
                })
                .collect(),
        }
    }

    fn facts(state: ReplyState, states: &[(StepState, Option<StepReason>)]) -> ReplyFacts {
        let block = steps(states);
        let blocks = if states.is_empty() {
            vec![]
        } else {
            vec![(1, &block)]
        };
        ReplyFacts::from_blocks(state, blocks, None)
    }

    const DONE: (StepState, Option<StepReason>) = (StepState::Done, None);

    #[test]
    fn success_is_said_only_when_every_step_is_done() {
        assert_eq!(closing(&facts(ReplyState::Done, &[])), Closing::Say);
        assert_eq!(outcome(&facts(ReplyState::Done, &[])), Outcome::Answered);
        assert_eq!(
            closing(&facts(ReplyState::Done, &[DONE, DONE])),
            Closing::Say
        );
        assert_eq!(
            outcome(&facts(ReplyState::Done, &[DONE, DONE])),
            Outcome::Acted
        );
        let no_effect = facts(
            ReplyState::Done,
            &[(StepState::Failed, Some(StepReason::NoEffect))],
        );
        assert!(matches!(
            closing(&no_effect),
            Closing::NotConfirmed {
                step: Some(StepRef { index: 0, .. })
            }
        ));
        assert_eq!(outcome(&no_effect), Outcome::NotConfirmed);
        let refused = facts(
            ReplyState::Failed,
            &[
                DONE,
                (StepState::Failed, Some(StepReason::Refused)),
                (StepState::Skipped, None),
            ],
        );
        assert!(matches!(
            closing(&refused),
            Closing::Refused {
                step: StepRef { index: 1, .. }
            }
        ));
        assert_eq!(outcome(&refused), Outcome::Partial);
        let changed = facts(
            ReplyState::Failed,
            &[(StepState::Failed, Some(StepReason::PageChanged))],
        );
        assert!(matches!(
            closing(&changed),
            Closing::Failed {
                reason: Some(StepReason::PageChanged),
                ..
            }
        ));
        assert_eq!(outcome(&changed), Outcome::Failed);
        let declined = facts(ReplyState::Done, &[DONE, (StepState::Declined, None)]);
        assert_eq!(closing(&declined), Closing::Declined);
        assert_eq!(outcome(&declined), Outcome::Declined);
        let skipped = facts(ReplyState::Done, &[DONE, (StepState::Skipped, None)]);
        assert!(matches!(
            closing(&skipped),
            Closing::Failed { reason: None, .. }
        ));
        let unsettled = facts(ReplyState::Done, &[(StepState::Running, None)]);
        assert!(matches!(closing(&unsettled), Closing::NotConfirmed { .. }));
    }

    #[test]
    fn reply_states_override_steps() {
        assert_eq!(
            closing(&facts(ReplyState::Cancelled, &[DONE])),
            Closing::Cancelled
        );
        assert_eq!(
            closing(&facts(ReplyState::Paused, &[DONE])),
            Closing::Paused
        );
        assert!(matches!(
            closing(&facts(
                ReplyState::Waiting,
                &[DONE, (StepState::Confirm, None)]
            )),
            Closing::NeedsYou {
                step: Some(StepRef {
                    index: 1,
                    category: Some(Category::Purchase),
                    ..
                })
            }
        ));
        assert_eq!(outcome(&facts(ReplyState::Waiting, &[])), Outcome::NeedsYou);
        let mut failed = facts(ReplyState::Failed, &[DONE]);
        failed.fault = Some(Fault::Model {
            fault: WorkModelFaultV1::DailyLimit,
            retry_after_ms: None,
        });
        assert!(matches!(
            closing(&failed),
            Closing::Failed {
                step: None,
                fault: Some(_),
                ..
            }
        ));
        assert_eq!(outcome(&failed), Outcome::Partial);
        assert_eq!(outcome(&facts(ReplyState::Failed, &[])), Outcome::Failed);
    }

    #[test]
    fn other_blocks_count_as_steps() {
        let opened = Block::Opened {
            tabs: vec![crate::ipc::PageAssistOpenedV1 {
                index: 2,
                host: "github.com".into(),
                url: "https://github.com/".into(),
                tab: None,
                state: OpenState::Failed,
            }],
        };
        let search = Block::Search {
            destination: "github.com".into(),
            query: "q".into(),
            state: StepState::Done,
        };
        let text = Block::Text {
            text: "Opened all of them.".into(),
            cites: vec![],
        };
        let reply = ReplyFacts::from_blocks(
            ReplyState::Done,
            [(0, &text), (1, &search), (2, &opened)],
            None,
        );
        assert_eq!(reply.steps.len(), 2);
        assert!(matches!(
            closing(&reply),
            Closing::Failed {
                step: Some(StepRef {
                    kind: StepKind::Open,
                    index: 2,
                    ..
                }),
                ..
            }
        ));
        assert_eq!(outcome(&reply), Outcome::Partial);
    }

    fn any_state() -> impl Strategy<Value = (StepState, Option<StepReason>)> {
        (
            prop_oneof![
                Just(StepState::Running),
                Just(StepState::Confirm),
                Just(StepState::Done),
                Just(StepState::Declined),
                Just(StepState::Failed),
                Just(StepState::Skipped),
            ],
            prop_oneof![
                Just(None),
                Just(Some(StepReason::Refused)),
                Just(Some(StepReason::Unsuitable)),
                Just(Some(StepReason::PageChanged)),
                Just(Some(StepReason::NoEffect)),
            ],
        )
    }

    proptest! {
        #[test]
        fn say_exactly_when_the_outcome_is_a_success(
            state in prop_oneof![
                Just(ReplyState::Working),
                Just(ReplyState::Waiting),
                Just(ReplyState::Paused),
                Just(ReplyState::Done),
                Just(ReplyState::Failed),
                Just(ReplyState::Cancelled),
            ],
            states in proptest::collection::vec(any_state(), 0..6),
        ) {
            let reply = facts(state, &states);
            let say = closing(&reply) == Closing::Say;
            prop_assert_eq!(say, matches!(outcome(&reply), Outcome::Answered | Outcome::Acted));
            if say {
                prop_assert_eq!(reply.state, ReplyState::Done);
                prop_assert!(reply.steps.iter().all(|fact| fact.state == StepState::Done));
            }
        }
    }
}
