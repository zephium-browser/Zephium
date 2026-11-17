//! Talking to the sidebar assistant: hold the key, speak, hear the reply.
//! Rust owns the microphone, the voice socket, the speech queue and every
//! decision; the frame shows this projection and sends typed intent only.
use serde::{Deserialize, Serialize};
use specta::Type;

use crate::page_assist::{PageAssistPhaseV1, WorkConfirmCategoryV1};

/// The projection's shape version.
pub const VOICE_SESSION_VERSION: u16 = 1;
/// Longest interim or heard transcript the view carries, in characters.
pub const VOICE_SESSION_MAX_SHOWN_CHARS: usize = 500;

/// How the person is talking.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, Eq, PartialEq, Type)]
#[serde(rename_all = "snake_case")]
pub enum VoicePressV1 {
    /// The key is held down.
    Held,
    /// A short press latched listening on until the next press.
    Latched,
}

/// What the assistant is doing.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, Eq, PartialEq, Type)]
#[serde(rename_all = "snake_case")]
pub enum VoiceWorkV1 {
    Thinking,
    Reading,
    Acting,
    /// Waiting on the person.
    Confirming,
}

/// How the last reply ended.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, Eq, PartialEq, Type)]
#[serde(rename_all = "snake_case")]
pub enum VoiceOutcomeV1 {
    /// Done with no steps.
    Answered,
    /// Every step that ran is done with its effect verified.
    Acted,
    Partial,
    /// A step ran, but its effect was not observed.
    NotConfirmed,
    Failed,
    Declined,
    Cancelled,
    /// It waits on the person.
    NeedsYou,
}

/// The session's state.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, Eq, PartialEq, Type)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum VoiceSessionStateV1 {
    Idle,
    /// Listening was asked for; the microphone is opening.
    Arming {
        press: VoicePressV1,
    },
    Listening {
        press: VoicePressV1,
    },
    /// The person finished; waiting for the final words.
    Finalizing,
    Working {
        work: VoiceWorkV1,
    },
    Settled {
        outcome: VoiceOutcomeV1,
    },
}

/// Why voice could not do something. Closed; never provider text.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, Eq, PartialEq, Type)]
#[serde(rename_all = "snake_case")]
pub enum VoiceSessionFaultV1 {
    MicrophoneDenied,
    NoMicrophone,
    /// The voice service cannot be reached.
    Offline,
    /// The plan does not include voice.
    PlanRequired,
    OutOfCredit,
    DailyLimit,
    RateLimited,
    /// The speech provider is down.
    ProviderDown,
    /// No answer in time.
    Timeout,
    /// The connection dropped.
    Interrupted,
    /// The assistant is busy with another reply.
    Busy,
    Unavailable,
}

/// Whether voice can be used in this window now.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, Eq, PartialEq, Type)]
#[serde(rename_all = "snake_case")]
pub enum VoiceAvailabilityV1 {
    Available,
    /// AI is switched off in Settings.
    AiOff,
    /// No Zephium account is signed in.
    SignedOut,
    /// Private windows never use voice.
    Private,
    /// This platform or build has no voice engine.
    Unsupported,
}

/// A step waiting on the person's confirm.
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq, Type)]
#[serde(deny_unknown_fields)]
pub struct VoiceConfirmV1 {
    /// The reply block the step is in.
    pub block: u16,
    /// Its index within the block.
    pub index: u8,
    /// What it commits.
    pub category: Option<WorkConfirmCategoryV1>,
    /// Its label, for display only (untrusted, bounded).
    pub label: String,
}

/// Payload of `zephium:voice-session-changed`, and the view command's answer.
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq, Type)]
#[serde(deny_unknown_fields)]
pub struct VoiceSessionViewV1 {
    pub version: u16,
    /// Grows with every change; an older view is stale.
    pub seq: u32,
    pub availability: VoiceAvailabilityV1,
    pub state: VoiceSessionStateV1,
    /// The microphone is open.
    pub mic: bool,
    /// The microphone is open with listening gated off.
    pub warm: bool,
    /// The interim transcript while listening.
    pub interim: String,
    /// The last final transcript.
    pub heard: Option<String>,
    pub phase: Option<PageAssistPhaseV1>,
    /// The step the reply is on, for display only.
    pub step: Option<String>,
    pub confirm: Option<VoiceConfirmV1>,
    /// Speech is queued or playing.
    pub speaking: bool,
    pub outcome: Option<VoiceOutcomeV1>,
    pub fault: Option<VoiceSessionFaultV1>,
}

/// Payload of `zephium:voice-session-levels`, about 25 a second, only while
/// the microphone listens or speech plays.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, Eq, PartialEq, Type)]
#[serde(deny_unknown_fields)]
pub struct VoiceSessionLevelsV1 {
    /// Microphone level in percent.
    pub input: u8,
    /// Speech level in percent.
    pub output: u8,
}

/// The language the person speaks to voice.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, Eq, PartialEq, Type)]
#[serde(rename_all = "snake_case")]
pub enum VoiceLanguageV1 {
    /// The system's preferred language.
    #[default]
    Automatic,
    En,
    Es,
    De,
    Fr,
    Pl,
    Pt,
    Ru,
    It,
    Ja,
    Zh,
}

/// What the person can change.
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq, Type)]
#[serde(deny_unknown_fields)]
pub struct VoiceSessionPreferencesV1 {
    /// Replies are spoken aloud.
    pub spoken: bool,
    /// The speaking voice; none uses the service's default.
    pub voice: Option<String>,
    pub language: VoiceLanguageV1,
    /// The short listening and error sounds.
    pub earcons: bool,
}

/// The settings and what can be chosen.
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq, Type)]
#[serde(deny_unknown_fields)]
pub struct VoiceSessionSettingsV1 {
    pub version: u16,
    pub preferences: VoiceSessionPreferencesV1,
    /// The voices the service offers, once it has been reached; empty before.
    pub voices: Vec<String>,
    /// The service's default voice, once known.
    pub default_voice: Option<String>,
}

/// The on-screen button, mirroring the talk key.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, Eq, PartialEq, Type)]
#[serde(rename_all = "snake_case")]
pub enum VoiceSessionIntentV1 {
    /// Start listening (latched).
    Start,
    /// Stop listening and send the words.
    Stop,
    /// Stop listening and discard, or stop the reply, or silence speech.
    Cancel,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_view_has_a_stable_wire_shape() {
        let view = VoiceSessionViewV1 {
            version: VOICE_SESSION_VERSION,
            seq: 3,
            availability: VoiceAvailabilityV1::Available,
            state: VoiceSessionStateV1::Listening {
                press: VoicePressV1::Held,
            },
            mic: true,
            warm: false,
            interim: "find".into(),
            heard: None,
            phase: None,
            step: None,
            confirm: Some(VoiceConfirmV1 {
                block: 1,
                index: 0,
                category: Some(WorkConfirmCategoryV1::Purchase),
                label: "Click \"Pay\"".into(),
            }),
            speaking: false,
            outcome: None,
            fault: Some(VoiceSessionFaultV1::PlanRequired),
        };
        let json = serde_json::to_value(&view).unwrap();
        assert_eq!(json["state"]["state"], "listening");
        assert_eq!(json["state"]["press"], "held");
        assert_eq!(json["confirm"]["category"], "purchase");
        assert_eq!(json["fault"], "plan_required");
        assert_eq!(
            serde_json::from_value::<VoiceSessionViewV1>(json).unwrap(),
            view
        );
    }

    #[test]
    fn preferences_refuse_unknown_fields_and_languages() {
        let good = serde_json::json!({
            "spoken": true, "voice": null, "language": "automatic", "earcons": false
        });
        assert!(serde_json::from_value::<VoiceSessionPreferencesV1>(good).is_ok());
        let extra = serde_json::json!({
            "spoken": true, "voice": null, "language": "en", "earcons": true, "model": "x"
        });
        assert!(serde_json::from_value::<VoiceSessionPreferencesV1>(extra).is_err());
        let language = serde_json::json!({
            "spoken": true, "voice": null, "language": "xx", "earcons": true
        });
        assert!(serde_json::from_value::<VoiceSessionPreferencesV1>(language).is_err());
    }
}
