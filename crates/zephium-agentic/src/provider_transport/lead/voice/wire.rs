use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use zephium_core::work::model::WorkModelError;

use super::{
    LeadCloudAccounting, LeadCloudObserver, LeadCloudRecord, Shared, VoiceClosing, VoiceError,
    VoiceEvent, VoiceReady, TEXT_LIMIT,
};

#[derive(Serialize)]
#[serde(tag = "type")]
pub(super) enum ClientFrame<'a> {
    #[serde(rename = "listen.start")]
    ListenStart {
        id: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        language: Option<&'a str>,
    },
    #[serde(rename = "listen.commit")]
    ListenCommit { id: &'a str },
    #[serde(rename = "listen.cancel")]
    ListenCancel { id: &'a str },
    #[serde(rename = "speak")]
    Speak {
        id: &'a str,
        text: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        voice: Option<&'a str>,
    },
    #[serde(rename = "speak.cancel")]
    SpeakCancel {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<&'a str>,
    },
}

#[derive(Deserialize)]
pub(super) struct ListenPolicy {
    pub max_ms: u32,
}

#[derive(Deserialize)]
pub(super) struct SpeakPolicy {
    pub max_chars: u32,
    pub voices: Vec<String>,
    pub voice: String,
}

#[derive(Deserialize)]
pub(super) struct Usage {
    request_id: String,
    usage: String,
    charged: Option<String>,
    audio_ms: Option<u64>,
    characters: Option<u64>,
}

impl Usage {
    fn accounting(&self) -> Result<LeadCloudAccounting, ()> {
        if !token(&self.request_id)
            || self.charged.as_ref().is_some_and(|s| {
                s.is_empty() || s.len() > 20 || !s.bytes().all(|b| b.is_ascii_digit())
            })
        {
            return Err(());
        }
        let _ = (self.audio_ms, self.characters);
        match self.usage.as_str() {
            "settled" => Ok(LeadCloudAccounting::Settled {
                charged: self.charged.clone(),
            }),
            "pending" => Ok(LeadCloudAccounting::Pending),
            _ => Err(()),
        }
    }
}

#[derive(Deserialize)]
#[serde(tag = "type")]
pub(super) enum ServerFrame {
    #[serde(rename = "ready")]
    Ready {
        policy_revision: Option<String>,
        listen: ListenPolicy,
        speak: SpeakPolicy,
    },
    #[serde(rename = "listen.partial")]
    Partial { id: String, text: String },
    #[serde(rename = "listen.final")]
    Final {
        id: String,
        text: String,
        usage: Usage,
    },
    #[serde(rename = "listen.failed")]
    ListenFailed {
        id: String,
        error: String,
        usage: Option<Usage>,
    },
    #[serde(rename = "speak.start")]
    SpeakStart { id: String },
    #[serde(rename = "speak.end")]
    SpeakEnd { id: String, usage: Usage },
    #[serde(rename = "speak.failed")]
    SpeakFailed {
        id: String,
        error: String,
        usage: Option<Usage>,
    },
    #[serde(rename = "pong")]
    Pong,
    #[serde(rename = "closing")]
    Closing { reason: VoiceClosing },
}

pub(super) fn parse(text: &str) -> Result<ServerFrame, ()> {
    if text.len() > TEXT_LIMIT {
        return Err(());
    }
    serde_json::from_str(text).map_err(|_| ())
}

fn token(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

pub(super) fn validate_ready(ready: &VoiceReady) -> Result<(), ()> {
    if ready.policy_revision.as_ref().is_some_and(|p| !token(p))
        || ready.listen_max_ms == 0
        || ready.listen_max_ms > 240_000
        || ready.speak_max_chars == 0
        || ready.speak_max_chars > 600
        || ready.voices.is_empty()
        || ready.voices.len() > 64
        || ready.voices.iter().any(|v| !token(v))
        || !ready.voices.contains(&ready.voice)
    {
        return Err(());
    }
    Ok(())
}

fn failure(code: &str) -> Result<VoiceError, ()> {
    let status = match code {
        "cancelled" => return Ok(VoiceError::Cancelled),
        "timeout" => return Ok(VoiceError::Timeout),
        "interrupted" => return Ok(VoiceError::Interrupted),
        "too_long" => return Ok(VoiceError::TooLong),
        "audio_too_long" => return Ok(VoiceError::AudioTooLong),
        "invalid_audio" => return Ok(VoiceError::InvalidAudio),
        "offline" => return Ok(VoiceError::Model(WorkModelError::Network)),
        "unauthenticated" | "session_ended" => StatusCode::UNAUTHORIZED,
        "rate_limited" | "concurrency_limited" | "provider_rate_limited" => {
            StatusCode::TOO_MANY_REQUESTS
        }
        "provider_unavailable"
        | "provider_error"
        | "provider_failure"
        | "provider_failed"
        | "cloud_disabled" => StatusCode::SERVICE_UNAVAILABLE,
        "insufficient_credits"
        | "daily_limit_reached"
        | "plan_required"
        | "billing_review_required"
        | "free_usage_limit_reached"
        | "usage_confirmation_limited"
        | "cloud_budget_exhausted"
        | "unsupported_audio_format" => StatusCode::BAD_REQUEST,
        _ => return Err(()),
    };
    let body = serde_json::json!({"error": {"code": code}}).to_string();
    Ok(VoiceError::Model(super::super::cloud::classify_cloud(
        status,
        body.as_bytes(),
    )))
}

pub(super) fn record(
    shared: &Shared,
    observer: Option<&LeadCloudObserver>,
    id: &str,
    usage: Option<&Usage>,
    fallback: LeadCloudAccounting,
) {
    if let Some(observer) = observer {
        observer(LeadCloudRecord {
            idempotency_key: id.to_owned(),
            request_id: usage.map(|u| u.request_id.clone()),
            policy_revision: shared.ready.policy_revision.clone(),
            accounting: usage.and_then(|u| u.accounting().ok()).unwrap_or(fallback),
        });
    }
}

fn end_record(
    shared: &Shared,
    observer: Option<&LeadCloudObserver>,
    id: &str,
    usage: Option<&Usage>,
    error: Option<VoiceError>,
) {
    if error == Some(VoiceError::Interrupted) {
        if let Some(observer) = observer {
            observer(LeadCloudRecord {
                idempotency_key: id.to_owned(),
                request_id: usage.map(|u| u.request_id.clone()),
                policy_revision: shared.ready.policy_revision.clone(),
                accounting: LeadCloudAccounting::Interrupted,
            });
        }
    } else {
        record(shared, observer, id, usage, LeadCloudAccounting::Refused);
    }
}

pub(super) fn receive(
    frame: ServerFrame,
    shared: &Shared,
    observer: Option<&LeadCloudObserver>,
) -> Result<Option<VoiceEvent>, ()> {
    let mut state = shared.state();
    if state.closed.is_some() {
        return Ok(None);
    }
    let event = match frame {
        ServerFrame::Ready { .. } => return Err(()),
        ServerFrame::Pong => return Ok(None),
        ServerFrame::Closing { reason } => {
            shared.close_locked(&mut state, VoiceError::Interrupted);
            VoiceEvent::Closing { reason }
        }
        ServerFrame::Partial { id, text } => {
            if !state
                .listen
                .as_ref()
                .is_some_and(|l| l.id == id && !l.cancelled)
                || text.len() > TEXT_LIMIT
            {
                return Err(());
            }
            VoiceEvent::Partial { id, text }
        }
        ServerFrame::Final { id, text, usage } => {
            usage.accounting()?;
            if !state
                .listen
                .as_ref()
                .is_some_and(|l| l.id == id && l.deadline.is_some() && !l.cancelled)
                || text.len() > TEXT_LIMIT
            {
                return Err(());
            }
            state.listen.take();
            drop(state);
            end_record(shared, observer, &id, Some(&usage), None);
            return Ok(Some(VoiceEvent::Final { id, text }));
        }
        ServerFrame::ListenFailed { id, error, usage } => {
            let error = failure(&error)?;
            if let Some(usage) = &usage {
                usage.accounting()?;
            }
            if state.listen.as_ref().is_none_or(|l| l.id != id) {
                return Err(());
            }
            state.listen.take();
            drop(state);
            end_record(shared, observer, &id, usage.as_ref(), Some(error));
            return Ok(Some(VoiceEvent::ListenFailed { id, error }));
        }
        ServerFrame::SpeakStart { id } => {
            let speech = state
                .speeches
                .front_mut()
                .filter(|s| s.id == id && !s.started)
                .ok_or(())?;
            speech.started = true;
            VoiceEvent::SpeakStart { id }
        }
        ServerFrame::SpeakEnd { id, usage } => {
            usage.accounting()?;
            if !state
                .speeches
                .front()
                .is_some_and(|s| s.id == id && s.started)
            {
                return Err(());
            }
            state.speeches.pop_front();
            drop(state);
            end_record(shared, observer, &id, Some(&usage), None);
            return Ok(Some(VoiceEvent::SpeakEnd { id }));
        }
        ServerFrame::SpeakFailed { id, error, usage } => {
            let error = failure(&error)?;
            if let Some(usage) = &usage {
                usage.accounting()?;
            }
            let index = state.speeches.iter().position(|s| s.id == id).ok_or(())?;
            state.speeches.remove(index);
            drop(state);
            end_record(shared, observer, &id, usage.as_ref(), Some(error));
            return Ok(Some(VoiceEvent::SpeakFailed { id, error }));
        }
    };
    Ok(Some(event))
}
