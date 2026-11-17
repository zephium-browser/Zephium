//! Dictation engines for the Work bar and the cloud half of a take: which
//! engines can run, which one the person chose, and the upload with the keys
//! and session Work's models already hold. Nothing here logs audio or words.

use zephium_agentic::lead::{self, TranscriptionTarget};
use zephium_core::work::model::{WorkModelError, WorkModelProvider};

use crate::work_models;

/// Who turns speech into words.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum VoiceEngine {
    /// Apple's recognizer on this Mac.
    OnDevice,
    /// The signed-in account's hosted transcription.
    Zephium,
    /// The person's own OpenAI key.
    OpenAi,
    /// The person's own Groq key.
    Groq,
}

const ENGINE_KEY: &str = "voice.engine";
const LANGUAGE_KEY: &str = "voice.language";
/// The order a fallback tries the cloud: the account first, then the keys.
const CLOUD_ORDER: [VoiceEngine; 3] =
    [VoiceEngine::Zephium, VoiceEngine::OpenAi, VoiceEngine::Groq];

impl VoiceEngine {
    fn id(self) -> &'static str {
        match self {
            Self::OnDevice => "on_device",
            Self::Zephium => "zephium",
            Self::OpenAi => "openai",
            Self::Groq => "groq",
        }
    }

    fn from_id(id: &str) -> Option<Self> {
        [Self::OnDevice, Self::Zephium, Self::OpenAi, Self::Groq]
            .into_iter()
            .find(|engine| engine.id() == id)
    }

    /// Whether the audio leaves this device.
    pub fn cloud(self) -> bool {
        self != Self::OnDevice
    }
}

/// The engine the person chose in Settings → AI, if any.
pub fn chosen() -> Option<VoiceEngine> {
    work_models::setting(ENGINE_KEY).and_then(|id| VoiceEngine::from_id(&id))
}

pub fn choose(engine: Option<VoiceEngine>) -> bool {
    work_models::store_setting(
        ENGINE_KEY.to_owned(),
        engine.map(VoiceEngine::id).unwrap_or_default().to_owned(),
    )
}

/// A BCP 47 tag such as `pl` or `en-GB`, or `None` for the system's.
fn valid_language(tag: &str) -> bool {
    (2..=35).contains(&tag.len())
        && tag.split('-').all(|part| {
            (1..=8).contains(&part.len()) && part.bytes().all(|b| b.is_ascii_alphanumeric())
        })
        && tag
            .bytes()
            .take_while(|b| *b != b'-')
            .all(|b| b.is_ascii_alphabetic())
}

/// The language the person chose for dictation; `None` follows the system.
pub fn language() -> Option<String> {
    work_models::setting(LANGUAGE_KEY).filter(|tag| valid_language(tag))
}

pub fn choose_language(tag: Option<&str>) -> bool {
    match tag {
        Some(tag) if !valid_language(tag) => false,
        tag => work_models::store_setting(LANGUAGE_KEY.to_owned(), tag.unwrap_or_default().into()),
    }
}

/// The cloud engines usable now, in fallback order.
pub async fn cloud_engines() -> Vec<VoiceEngine> {
    let mut usable = Vec::new();
    for engine in CLOUD_ORDER {
        let ready = match engine {
            VoiceEngine::Zephium => work_models::hosted_transcriber().await.is_some(),
            VoiceEngine::OpenAi => work_models::key_usable(WorkModelProvider::OpenAi).await,
            VoiceEngine::Groq => work_models::key_usable(WorkModelProvider::Groq).await,
            VoiceEngine::OnDevice => false,
        };
        if ready {
            usable.push(engine);
        }
    }
    usable
}

/// The engine a take uses: a cloud engine the person chose while it is
/// usable; otherwise this device when it recognizes the language, and then
/// the first usable cloud engine.
pub fn effective(
    chosen: Option<VoiceEngine>,
    on_device: bool,
    cloud: &[VoiceEngine],
) -> Option<VoiceEngine> {
    if let Some(engine) = chosen.filter(|engine| engine.cloud() && cloud.contains(engine)) {
        return Some(engine);
    }
    if on_device {
        return Some(VoiceEngine::OnDevice);
    }
    CLOUD_ORDER
        .into_iter()
        .find(|engine| cloud.contains(engine))
}

/// Uploads one WAV take to `engine`. `language` is a hint; without it the
/// model detects the language.
pub async fn transcribe(
    engine: VoiceEngine,
    wav: &[u8],
    language: Option<&str>,
) -> Result<String, WorkModelError> {
    let provider = match engine {
        VoiceEngine::OnDevice => return Err(WorkModelError::BadRequest),
        VoiceEngine::Zephium => {
            let (session, transcriber) = work_models::hosted_transcriber()
                .await
                .ok_or(WorkModelError::MissingKey)?;
            let target = TranscriptionTarget::cloud(
                &session.base(),
                &transcriber.id,
                transcriber.max_request_bytes,
            )?;
            let credential = session.credential();
            // The take's usage settles server-side: the account refreshes and
            // checks the receipt however the take ended.
            let observer: lead::LeadCloudObserver = {
                let session = session.clone();
                std::sync::Arc::new(move |record| session.recorded(record))
            };
            let transcript = lead::transcribe_observed(
                &target,
                credential.as_ref(),
                wav,
                language,
                Some(&observer),
            )
            .await?;
            if let Some(id) = &transcript.request_id {
                session.sourced(id, &work_models::WorkCallSource::Voice);
            }
            return Ok(transcript.text);
        }
        VoiceEngine::OpenAi => WorkModelProvider::OpenAi,
        VoiceEngine::Groq => WorkModelProvider::Groq,
    };
    let target = TranscriptionTarget::direct(provider)?;
    let credential = work_models::key_credential(provider);
    let result = lead::transcribe(&target, credential.as_ref(), wav, language).await;
    if result == Err(WorkModelError::Unauthorized) {
        work_models::key_refused(provider);
    }
    result.map(|transcript| transcript.text)
}

/// Talking to the assistant: spoken replies, the voice, the language and
/// the earcons, as stored. `language` is an ISO 639-1 code; none follows
/// the system.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TalkSettings {
    pub spoken: bool,
    pub voice: Option<String>,
    pub language: Option<String>,
    pub earcons: bool,
}

const TALK_SPOKEN_KEY: &str = "voice.talk.spoken";
const TALK_VOICE_KEY: &str = "voice.talk.voice";
const TALK_LANGUAGE_KEY: &str = "voice.talk.language";
const TALK_EARCONS_KEY: &str = "voice.talk.earcons";

fn valid_voice(voice: &str) -> bool {
    (1..=64).contains(&voice.len())
        && voice
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

pub fn talk_settings() -> TalkSettings {
    let on = |key| work_models::setting(key).as_deref() != Some("false");
    TalkSettings {
        spoken: on(TALK_SPOKEN_KEY),
        voice: work_models::setting(TALK_VOICE_KEY).filter(|voice| valid_voice(voice)),
        language: work_models::setting(TALK_LANGUAGE_KEY)
            .filter(|code| code.len() == 2 && code.bytes().all(|b| b.is_ascii_lowercase())),
        earcons: on(TALK_EARCONS_KEY),
    }
}

pub fn store_talk_settings(settings: &TalkSettings) -> bool {
    if settings
        .voice
        .as_deref()
        .is_some_and(|voice| !valid_voice(voice))
        || settings
            .language
            .as_deref()
            .is_some_and(|code| code.len() != 2 || !code.bytes().all(|b| b.is_ascii_lowercase()))
    {
        return false;
    }
    let flag = |on: bool| if on { "true" } else { "false" }.to_owned();
    [
        (TALK_SPOKEN_KEY, flag(settings.spoken)),
        (TALK_VOICE_KEY, settings.voice.clone().unwrap_or_default()),
        (
            TALK_LANGUAGE_KEY,
            settings.language.clone().unwrap_or_default(),
        ),
        (TALK_EARCONS_KEY, flag(settings.earcons)),
    ]
    .into_iter()
    .all(|(key, value)| work_models::store_setting(key.to_owned(), value))
}

/// Whether a Zephium account is signed in now.
pub fn cloud_signed_in() -> bool {
    work_models::cloud().is_some_and(|session| session.signed_in())
}

/// Opens the account's voice socket. Every utterance's and speech's usage
/// record reaches the account the way a hosted take's does, as voice.
pub async fn talk_link() -> Result<
    (
        lead::voice::VoiceLink,
        lead::voice::VoiceReady,
        tokio::sync::mpsc::Receiver<lead::voice::VoiceEvent>,
    ),
    WorkModelError,
> {
    let session = work_models::cloud()
        .filter(|session| session.signed_in())
        .ok_or(WorkModelError::MissingKey)?;
    let root = url::Url::parse(&session.base()).map_err(|_| WorkModelError::BadRequest)?;
    let target = lead::voice::VoiceTarget::cloud(&root)?;
    let credential = session.credential();
    let observer: lead::LeadCloudObserver = {
        let session = session.clone();
        std::sync::Arc::new(move |record: lead::LeadCloudRecord| {
            if let Some(id) = &record.request_id {
                session.sourced(id, &work_models::WorkCallSource::Voice);
            }
            session.recorded(record);
        })
    };
    lead::voice::VoiceLink::open(&target, credential.as_ref(), Some(observer)).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use VoiceEngine::*;

    #[test]
    fn this_device_comes_first_and_the_cloud_only_when_it_cannot() {
        assert_eq!(effective(None, true, &[Zephium, OpenAi]), Some(OnDevice));
        assert_eq!(effective(Some(OnDevice), true, &[OpenAi]), Some(OnDevice));
        assert_eq!(effective(None, false, &[OpenAi, Zephium]), Some(Zephium));
        assert_eq!(effective(Some(OnDevice), false, &[Groq]), Some(Groq));
        assert_eq!(effective(None, false, &[]), None);
    }

    #[test]
    fn a_chosen_cloud_engine_wins_while_it_is_usable() {
        assert_eq!(effective(Some(Groq), true, &[OpenAi, Groq]), Some(Groq));
        assert_eq!(effective(Some(Groq), true, &[OpenAi]), Some(OnDevice));
        assert_eq!(effective(Some(Zephium), false, &[OpenAi]), Some(OpenAi));
    }

    #[test]
    fn engine_ids_round_trip_and_unknown_ones_read_as_unset() {
        for engine in [OnDevice, Zephium, OpenAi, Groq] {
            assert_eq!(VoiceEngine::from_id(engine.id()), Some(engine));
        }
        assert_eq!(VoiceEngine::from_id("whisper"), None);
        assert!(!OnDevice.cloud() && Zephium.cloud());
    }

    #[test]
    fn only_plain_language_tags_are_kept() {
        for tag in ["pl", "en-GB", "zh-Hant-TW", "es-419"] {
            assert!(valid_language(tag), "{tag}");
        }
        for tag in [
            "",
            "p",
            "en_GB",
            "en-",
            "419",
            "x".repeat(40).as_str(),
            "pl;rm",
        ] {
            assert!(!valid_language(tag), "{tag}");
        }
    }
}
