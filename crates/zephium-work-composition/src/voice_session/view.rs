//! The reducer's view as the frame receives it.
use zephium_ipc::voice_session::*;
use zephium_page_assist::voice::{Fault, Outcome, Press, State, View, Work};

fn press(press: Press) -> VoicePressV1 {
    match press {
        Press::Held => VoicePressV1::Held,
        Press::Latched => VoicePressV1::Latched,
    }
}

fn outcome(outcome: Outcome) -> VoiceOutcomeV1 {
    match outcome {
        Outcome::Answered => VoiceOutcomeV1::Answered,
        Outcome::Acted => VoiceOutcomeV1::Acted,
        Outcome::Partial => VoiceOutcomeV1::Partial,
        Outcome::NotConfirmed => VoiceOutcomeV1::NotConfirmed,
        Outcome::Failed => VoiceOutcomeV1::Failed,
        Outcome::Declined => VoiceOutcomeV1::Declined,
        Outcome::Cancelled => VoiceOutcomeV1::Cancelled,
        Outcome::NeedsYou => VoiceOutcomeV1::NeedsYou,
    }
}

/// The closed fault the frame names.
pub fn fault(fault: Fault) -> VoiceSessionFaultV1 {
    match fault {
        Fault::MicrophoneDenied => VoiceSessionFaultV1::MicrophoneDenied,
        Fault::NoMicrophone => VoiceSessionFaultV1::NoMicrophone,
        Fault::Offline => VoiceSessionFaultV1::Offline,
        Fault::PlanRequired => VoiceSessionFaultV1::PlanRequired,
        Fault::OutOfCredit => VoiceSessionFaultV1::OutOfCredit,
        Fault::DailyLimit => VoiceSessionFaultV1::DailyLimit,
        Fault::RateLimited => VoiceSessionFaultV1::RateLimited,
        Fault::ProviderDown => VoiceSessionFaultV1::ProviderDown,
        Fault::Timeout => VoiceSessionFaultV1::Timeout,
        Fault::Interrupted => VoiceSessionFaultV1::Interrupted,
        Fault::Busy => VoiceSessionFaultV1::Busy,
        Fault::Unavailable => VoiceSessionFaultV1::Unavailable,
    }
}

/// The projection of `view`.
pub fn project(view: &View, availability: VoiceAvailabilityV1, seq: u32) -> VoiceSessionViewV1 {
    let state = match view.state {
        State::Idle => VoiceSessionStateV1::Idle,
        State::Arming(p) => VoiceSessionStateV1::Arming { press: press(p) },
        State::Listening(p) => VoiceSessionStateV1::Listening { press: press(p) },
        State::Finalizing => VoiceSessionStateV1::Finalizing,
        State::Working(work) => VoiceSessionStateV1::Working {
            work: match work {
                Work::Thinking => VoiceWorkV1::Thinking,
                Work::Reading => VoiceWorkV1::Reading,
                Work::Acting => VoiceWorkV1::Acting,
                Work::Confirming => VoiceWorkV1::Confirming,
            },
        },
        State::Settled(o) => VoiceSessionStateV1::Settled {
            outcome: outcome(o),
        },
    };
    VoiceSessionViewV1 {
        version: VOICE_SESSION_VERSION,
        seq,
        availability,
        state,
        mic: view.mic,
        warm: view.warm,
        interim: view.interim.clone(),
        heard: view.heard.clone(),
        phase: view.phase.clone(),
        step: view.step.clone(),
        confirm: view.confirm.as_ref().map(|confirm| VoiceConfirmV1 {
            block: confirm.step.block,
            index: confirm.step.index,
            category: confirm.step.category,
            label: confirm.label.clone(),
        }),
        speaking: view.speaking,
        outcome: view.outcome.map(outcome),
        fault: view.fault.map(fault),
    }
}
