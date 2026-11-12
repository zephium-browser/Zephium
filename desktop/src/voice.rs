//! Dictation in the Work bar: one take at a time, its live level and words
//! as events, and the transcript back to the bar. Audio stays in memory, is
//! never logged, and goes only to the engine the take names. The take ends
//! on stop, cancel or the window losing focus; a pause to think never ends it.
use super::*;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, specta::Type, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum VoiceEngineV1 {
    OnDevice,
    Zephium,
    OpenAi,
    Groq,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, specta::Type, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum VoicePermissionV1 {
    Granted,
    Undetermined,
    Denied,
}

#[derive(Clone, Debug, Serialize, Deserialize, specta::Type)]
pub(crate) struct VoiceStatusV1 {
    pub(crate) version: u16,
    /// Engines usable now, this device's first.
    pub(crate) engines: Vec<VoiceEngineV1>,
    pub(crate) chosen: Option<VoiceEngineV1>,
    /// What a take uses now; null when nothing can.
    pub(crate) effective: Option<VoiceEngineV1>,
    /// The dictation language the person chose; null follows the system.
    pub(crate) language: Option<String>,
    /// The system's preferred languages, most preferred first.
    pub(crate) languages: Vec<String>,
    /// The on-device recognizer's locale for the language; null when this
    /// device cannot recognize it.
    pub(crate) on_device: Option<String>,
    pub(crate) microphone: VoicePermissionV1,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, specta::Type, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum VoicePhaseV1 {
    Listening,
    Transcribing,
    Done,
    Failed,
    Cancelled,
}

/// Why a take could not start or produce words. Closed; no system text.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, specta::Type, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum VoiceFaultV1 {
    MicrophoneDenied,
    RecognitionDenied,
    NoMicrophone,
    LanguageUnavailable,
    /// Siri and Dictation are off in System Settings, so this Mac's
    /// recognizer refuses every take.
    DictationOff,
    /// No engine can run: nothing on this device and no account or key.
    NoEngine,
    KeyRefused,
    SignedOut,
    OutOfCredit,
    /// The chosen Zephium AI speech model needs another plan.
    PlanRequired,
    /// The Zephium account is under billing review.
    BillingReview,
    /// The account's daily Zephium AI limit is reached.
    DailyLimit,
    /// Free Zephium AI usage is paused for now.
    FreeUsagePaused,
    /// Zephium AI is paused as a spending protection.
    HostedPaused,
    /// The server could not read the recording.
    InvalidAudio,
    /// The server does not take the recording's encoding.
    UnsupportedAudio,
    /// The recording is longer than Zephium AI takes.
    AudioTooLong,
    RateLimited,
    Offline,
    ProviderDown,
    TooLong,
    Busy,
    Unavailable,
}

/// A take moved on. `text` is set once, when it is done.
#[derive(Clone, Debug, Serialize, Deserialize, specta::Type, tauri_specta::Event)]
#[tauri_specta(event_name = "zephium:voice-changed")]
pub(crate) struct VoiceChanged {
    pub(crate) take: u32,
    pub(crate) request: String,
    pub(crate) phase: VoicePhaseV1,
    pub(crate) engine: Option<VoiceEngineV1>,
    /// The words so far, while listening, when the engine streams them.
    pub(crate) partial: Option<String>,
    pub(crate) text: Option<String>,
    pub(crate) fault: Option<VoiceFaultV1>,
}

/// Input levels in percent, one per 40 ms of audio, oldest first; they
/// arrive about ten times a second.
#[derive(Clone, Debug, Serialize, Deserialize, specta::Type, tauri_specta::Event)]
#[tauri_specta(event_name = "zephium:voice-levels")]
pub(crate) struct VoiceLevels {
    pub(crate) take: u32,
    pub(crate) request: String,
    pub(crate) levels: Vec<u8>,
}

#[derive(Clone, Debug, Serialize, Deserialize, specta::Type)]
pub(crate) struct VoiceStartV1 {
    /// The take's id, which its events carry; null when it did not start.
    pub(crate) take: Option<u32>,
    pub(crate) engine: Option<VoiceEngineV1>,
    pub(crate) fault: Option<VoiceFaultV1>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, specta::Type, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum VoicePrivacyV1 {
    Microphone,
    Recognition,
    /// The keyboard settings, where Dictation is switched on.
    Dictation,
}

const MAX_PARTIAL_BYTES: usize = 4 * 1024;

#[cfg(any(test, feature = "work-product"))]
fn owns_take(request: &str, take: Option<u32>, current_request: &str, current_take: u32) -> bool {
    !request.is_empty()
        && request == current_request
        && take.is_none_or(|take| take == current_take)
}

fn valid_request(request: &str) -> bool {
    request.len() == 36
        && request
            .bytes()
            .any(|byte| byte.is_ascii_hexdigit() && byte != b'0')
        && request.bytes().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
}
#[cfg(target_os = "macos")]
const PRIVACY_PANE: &str = "x-apple.systempreferences:com.apple.preference.security?";

fn fault_of(fault: zephium_voice::Fault) -> VoiceFaultV1 {
    use zephium_voice::Fault;
    match fault {
        Fault::MicrophoneDenied => VoiceFaultV1::MicrophoneDenied,
        Fault::RecognitionDenied => VoiceFaultV1::RecognitionDenied,
        Fault::NoMicrophone => VoiceFaultV1::NoMicrophone,
        Fault::LanguageUnavailable => VoiceFaultV1::LanguageUnavailable,
        Fault::DictationOff => VoiceFaultV1::DictationOff,
        Fault::Recognizer | Fault::Unsupported => VoiceFaultV1::Unavailable,
    }
}

fn permission(permission: zephium_voice::Permission) -> VoicePermissionV1 {
    match permission {
        zephium_voice::Permission::Granted => VoicePermissionV1::Granted,
        zephium_voice::Permission::Undetermined => VoicePermissionV1::Undetermined,
        zephium_voice::Permission::Denied => VoicePermissionV1::Denied,
    }
}

/// The tail of the words so far, at a character boundary.
fn bounded(text: &str) -> String {
    if text.len() <= MAX_PARTIAL_BYTES {
        return text.to_owned();
    }
    let mut start = text.len() - MAX_PARTIAL_BYTES;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    text[start..].to_owned()
}

fn percent(level: f32) -> u8 {
    (level.clamp(0.0, 1.0) * 100.0).round() as u8
}

fn emit(app: &tauri::AppHandle, event: &VoiceChanged) {
    emit_to_privileged(app, MAIN_LABEL, "zephium:voice-changed", event);
}

#[cfg(feature = "work-product")]
mod product {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Arc, Mutex, MutexGuard};
    use zephium_app::voice::{self, VoiceEngine};
    use zephium_core::work::model::{HostedRefusal, WorkModelError};
    use zephium_voice::{Capture, Ending, Listener, Outcome, Recognition};

    pub(super) fn engine_v1(engine: VoiceEngine) -> VoiceEngineV1 {
        match engine {
            VoiceEngine::OnDevice => VoiceEngineV1::OnDevice,
            VoiceEngine::Zephium => VoiceEngineV1::Zephium,
            VoiceEngine::OpenAi => VoiceEngineV1::OpenAi,
            VoiceEngine::Groq => VoiceEngineV1::Groq,
        }
    }

    pub(super) fn engine_of(engine: VoiceEngineV1) -> VoiceEngine {
        match engine {
            VoiceEngineV1::OnDevice => VoiceEngine::OnDevice,
            VoiceEngineV1::Zephium => VoiceEngine::Zephium,
            VoiceEngineV1::OpenAi => VoiceEngine::OpenAi,
            VoiceEngineV1::Groq => VoiceEngine::Groq,
        }
    }

    pub(super) fn cloud_fault(engine: VoiceEngine, error: WorkModelError) -> VoiceFaultV1 {
        match error {
            WorkModelError::Unauthorized | WorkModelError::MissingKey
                if engine == VoiceEngine::Zephium =>
            {
                VoiceFaultV1::SignedOut
            }
            WorkModelError::Unauthorized | WorkModelError::MissingKey => VoiceFaultV1::KeyRefused,
            WorkModelError::OverBudget => VoiceFaultV1::OutOfCredit,
            WorkModelError::Hosted { refusal, .. } => match refusal {
                HostedRefusal::OutOfCredits => VoiceFaultV1::OutOfCredit,
                HostedRefusal::PlanRequired => VoiceFaultV1::PlanRequired,
                HostedRefusal::BillingReview => VoiceFaultV1::BillingReview,
                HostedRefusal::DailyLimit => VoiceFaultV1::DailyLimit,
                HostedRefusal::FreeUsagePaused => VoiceFaultV1::FreeUsagePaused,
                HostedRefusal::ConfirmationLimited => VoiceFaultV1::RateLimited,
                HostedRefusal::Paused => VoiceFaultV1::HostedPaused,
                HostedRefusal::InvalidAudio => VoiceFaultV1::InvalidAudio,
                HostedRefusal::UnsupportedAudio => VoiceFaultV1::UnsupportedAudio,
                HostedRefusal::AudioTooLong => VoiceFaultV1::AudioTooLong,
                HostedRefusal::AlreadySent => VoiceFaultV1::Unavailable,
            },
            WorkModelError::RateLimited { .. } => VoiceFaultV1::RateLimited,
            WorkModelError::Network => VoiceFaultV1::Offline,
            WorkModelError::Overloaded => VoiceFaultV1::ProviderDown,
            WorkModelError::ContextTooLong => VoiceFaultV1::TooLong,
            _ => VoiceFaultV1::Unavailable,
        }
    }

    enum Phase {
        Resolving,
        Starting { cancelled: bool },
        Listening(Capture),
        Finishing(Option<tauri::async_runtime::JoinHandle<()>>),
    }

    struct Live {
        take: u32,
        request: String,
        engine: Option<VoiceEngine>,
        phase: Phase,
    }

    enum Cancellation {
        Refused,
        Opening,
        Taken(Live),
    }

    fn cancel_current(
        current: &mut Option<Live>,
        request: &str,
        take: Option<u32>,
    ) -> Cancellation {
        if !current
            .as_ref()
            .is_some_and(|live| owns_take(request, take, &live.request, live.take))
        {
            return Cancellation::Refused;
        }
        if let Some(Live {
            phase: Phase::Starting { cancelled },
            ..
        }) = current.as_mut()
        {
            // Native capture is still opening; retain its owner until it drains.
            *cancelled = true;
            return Cancellation::Opening;
        }
        current
            .take()
            .map_or(Cancellation::Refused, Cancellation::Taken)
    }

    static LIVE: Mutex<Option<Live>> = Mutex::new(None);
    static NEXT: AtomicU32 = AtomicU32::new(1);

    fn live() -> MutexGuard<'static, Option<Live>> {
        LIVE.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    struct Relay {
        app: tauri::AppHandle,
        take: u32,
        request: String,
        engine: VoiceEngine,
    }

    impl Listener for Relay {
        fn levels(&self, levels: &[f32]) {
            emit_to_privileged(
                &self.app,
                MAIN_LABEL,
                "zephium:voice-levels",
                &VoiceLevels {
                    take: self.take,
                    request: self.request.clone(),
                    levels: levels.iter().map(|level| percent(*level)).collect(),
                },
            );
        }

        fn partial(&self, text: &str) {
            emit(
                &self.app,
                &VoiceChanged {
                    take: self.take,
                    request: self.request.clone(),
                    phase: VoicePhaseV1::Listening,
                    engine: Some(engine_v1(self.engine)),
                    partial: Some(bounded(text)),
                    text: None,
                    fault: None,
                },
            );
        }

        fn ended(&self, ending: Ending) {
            write_diagnostic(format_args!("voice: take={} ended={ending:?}", self.take));
            finish(self.app.clone(), self.take);
        }

        fn note(&self, fact: &str) {
            write_diagnostic(format_args!("voice: take={} {fact}", self.take));
        }
    }

    pub(super) struct Resolved {
        pub(super) status: VoiceStatusV1,
        locale: Option<String>,
        engine: Option<VoiceEngine>,
    }

    pub(super) async fn resolve() -> Resolved {
        tauri::async_runtime::spawn(zephium_app::work_models::refresh_cloud_catalog());
        let chosen = voice::chosen();
        let language = voice::language();
        let cloud = voice::cloud_engines().await;
        let wanted = language.clone();
        let (locale, languages, microphone) = tauri::async_runtime::spawn_blocking(move || {
            (
                zephium_voice::on_device_locale(wanted.as_deref()),
                zephium_voice::preferred_languages(),
                zephium_voice::microphone_permission(),
            )
        })
        .await
        .unwrap_or((None, Vec::new(), zephium_voice::Permission::Granted));
        let engine = voice::effective(chosen, locale.is_some(), &cloud);
        let mut engines = Vec::new();
        if cfg!(target_os = "macos") {
            engines.push(VoiceEngineV1::OnDevice);
        }
        engines.extend(cloud.into_iter().map(engine_v1));
        Resolved {
            status: VoiceStatusV1 {
                version: 1,
                engines,
                chosen: chosen.map(engine_v1),
                effective: engine.map(engine_v1),
                language,
                languages,
                on_device: locale.clone(),
                microphone: permission(microphone),
            },
            locale,
            engine,
        }
    }

    pub(super) async fn start(app: tauri::AppHandle, request: String) -> VoiceStartV1 {
        let refused = |fault| {
            write_diagnostic(format_args!("voice: phase=refused fault={fault:?}"));
            VoiceStartV1 {
                take: None,
                engine: None,
                fault: Some(fault),
            }
        };
        let take = {
            let mut current = live();
            if current.is_some() {
                return refused(VoiceFaultV1::Busy);
            }
            let Ok(take) = NEXT.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                value.checked_add(1)
            }) else {
                return refused(VoiceFaultV1::Unavailable);
            };
            *current = Some(Live {
                take,
                request: request.clone(),
                engine: None,
                phase: Phase::Resolving,
            });
            take
        };
        let discard_reservation = || {
            let mut current = live();
            if current.as_ref().is_some_and(|live| live.take == take) {
                *current = None;
            }
        };
        let resolved = resolve().await;
        let Some(engine) = resolved.engine else {
            discard_reservation();
            return refused(VoiceFaultV1::NoEngine);
        };
        let recognition = match (engine, resolved.locale) {
            (VoiceEngine::OnDevice, Some(locale)) => Recognition::OnDevice { locale },
            (VoiceEngine::OnDevice, None) => {
                discard_reservation();
                return refused(VoiceFaultV1::LanguageUnavailable);
            }
            _ => Recognition::Upload,
        };
        {
            let mut current = live();
            let Some(current) = current
                .as_mut()
                .filter(|live| live.take == take && live.request == request)
            else {
                return VoiceStartV1 {
                    take: None,
                    engine: None,
                    fault: None,
                };
            };
            current.engine = Some(engine);
            current.phase = Phase::Starting { cancelled: false };
        }
        let relay = Arc::new(Relay {
            app: app.clone(),
            take,
            request: request.clone(),
            engine,
        });
        let started =
            tauri::async_runtime::spawn_blocking(move || Capture::start(recognition, false, relay))
                .await
                .unwrap_or(Err(zephium_voice::Fault::Unsupported));
        let mut live = live();
        let current = live.as_mut().filter(|live| live.take == take);
        match (started, current) {
            (
                Ok(capture),
                Some(Live {
                    phase: phase @ Phase::Starting { cancelled: false },
                    ..
                }),
            ) => {
                *phase = Phase::Listening(capture);
                drop(live);
                write_diagnostic(format_args!(
                    "voice: take={take} phase=listening engine={engine:?}"
                ));
                emit(
                    &app,
                    &VoiceChanged {
                        take,
                        request: request.clone(),
                        phase: VoicePhaseV1::Listening,
                        engine: Some(engine_v1(engine)),
                        partial: None,
                        text: None,
                        fault: None,
                    },
                );
                VoiceStartV1 {
                    take: Some(take),
                    engine: Some(engine_v1(engine)),
                    fault: None,
                }
            }
            (started, current) => {
                if current.is_some() {
                    *live = None;
                }
                drop(live);
                match started {
                    Ok(capture) => {
                        capture.cancel();
                        VoiceStartV1 {
                            take: None,
                            engine: None,
                            fault: None,
                        }
                    }
                    Err(fault) => {
                        write_diagnostic(format_args!(
                            "voice: take={take} phase=start engine={engine:?} fault={fault:?}"
                        ));
                        refused(fault_of(fault))
                    }
                }
            }
        }
    }

    /// Ends the current take's capture and turns it into words. Only the
    /// take named may be finished, so a late pause cannot end a newer one.
    pub(super) fn finish(app: tauri::AppHandle, take: u32) -> bool {
        let (capture, engine, request) = {
            let mut live = live();
            let Some(current) = live.as_mut().filter(|live| live.take == take) else {
                return false;
            };
            let Some(engine) = current.engine else {
                return false;
            };
            if !matches!(current.phase, Phase::Listening(_)) {
                return false;
            }
            let Phase::Listening(capture) =
                std::mem::replace(&mut current.phase, Phase::Finishing(None))
            else {
                return false;
            };
            (capture, engine, current.request.clone())
        };
        if engine.cloud() {
            emit(
                &app,
                &VoiceChanged {
                    take,
                    request: request.clone(),
                    phase: VoicePhaseV1::Transcribing,
                    engine: Some(engine_v1(engine)),
                    partial: None,
                    text: None,
                    fault: None,
                },
            );
        }
        let settle_app = app.clone();
        let handle = tauri::async_runtime::spawn(async move {
            let finished = tauri::async_runtime::spawn_blocking(move || capture.finish())
                .await
                .unwrap_or(Err(zephium_voice::Fault::Unsupported));
            let outcome = match &finished {
                Ok(Outcome::Text(_)) => "text",
                Ok(Outcome::Nothing) => "nothing",
                Ok(Outcome::Audio { .. }) => "audio",
                Err(_) => "fault",
            };
            let result = match finished {
                Ok(Outcome::Text(text)) => Ok(text),
                Ok(Outcome::Nothing) => Ok(String::new()),
                Ok(Outcome::Audio { wav, .. }) => {
                    let language = voice::language();
                    voice::transcribe(engine, &wav, language.as_deref())
                        .await
                        .map_err(|error| cloud_fault(engine, error))
                }
                Err(fault) => Err(fault_of(fault)),
            };
            {
                let mut live = live();
                if live.as_ref().is_none_or(|live| live.take != take) {
                    return;
                }
                *live = None;
            }
            let fault = result.as_ref().err().copied();
            write_diagnostic(format_args!(
                "voice: take={take} phase=done engine={engine:?} outcome={outcome} fault={fault:?}"
            ));
            emit(
                &settle_app,
                &VoiceChanged {
                    take,
                    request,
                    phase: if fault.is_some() {
                        VoicePhaseV1::Failed
                    } else {
                        VoicePhaseV1::Done
                    },
                    engine: Some(engine_v1(engine)),
                    partial: None,
                    text: result.ok(),
                    fault,
                },
            );
        });
        let mut live = live();
        if let Some(Live {
            phase: Phase::Finishing(slot),
            ..
        }) = live.as_mut().filter(|live| live.take == take)
        {
            *slot = Some(handle);
        }
        true
    }

    /// The take that is listening now, if any.
    pub(super) fn listening() -> Option<u32> {
        live()
            .as_ref()
            .filter(|live| matches!(live.phase, Phase::Listening(_)))
            .map(|live| live.take)
    }

    pub(super) fn cancel(app: &tauri::AppHandle, request: &str, take: Option<u32>) -> bool {
        let taken = {
            match cancel_current(&mut live(), request, take) {
                Cancellation::Refused => return false,
                Cancellation::Opening => return true,
                Cancellation::Taken(taken) => taken,
            }
        };
        match taken.phase {
            Phase::Listening(capture) => {
                drop(tauri::async_runtime::spawn_blocking(move || {
                    capture.cancel()
                }));
            }
            Phase::Finishing(Some(handle)) => handle.abort(),
            Phase::Finishing(None) | Phase::Starting { .. } | Phase::Resolving => {}
        }
        emit(
            app,
            &VoiceChanged {
                take: taken.take,
                request: taken.request,
                phase: VoicePhaseV1::Cancelled,
                engine: taken.engine.map(engine_v1),
                partial: None,
                text: None,
                fault: None,
            },
        );
        true
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn stale_cancellation_preserves_replacement_and_matching_cancel_retires_reservation() {
            let mut current = Some(Live {
                take: 2,
                request: "new".into(),
                engine: None,
                phase: Phase::Resolving,
            });
            assert!(matches!(
                cancel_current(&mut current, "old", Some(1)),
                Cancellation::Refused
            ));
            assert_eq!(current.as_ref().unwrap().take, 2);
            assert!(matches!(
                cancel_current(&mut current, "new", Some(1)),
                Cancellation::Refused
            ));
            assert_eq!(current.as_ref().unwrap().take, 2);
            assert!(matches!(
                cancel_current(&mut current, "new", None),
                Cancellation::Taken(_)
            ));
            assert!(current.is_none());
        }

        #[test]
        fn cancelling_an_opening_capture_retains_its_owner_until_start_settles() {
            let mut current = Some(Live {
                take: 2,
                request: "new".into(),
                engine: None,
                phase: Phase::Starting { cancelled: false },
            });
            assert!(matches!(
                cancel_current(&mut current, "new", Some(2)),
                Cancellation::Opening
            ));
            assert!(matches!(
                current.as_ref().unwrap().phase,
                Phase::Starting { cancelled: true }
            ));
        }
    }
}

/// The main window lost focus: a listening take stops and its words still
/// land. A take still opening is left alone, since a permission prompt takes
/// focus on first use.
pub(crate) fn window_blurred(app: &tauri::AppHandle) {
    #[cfg(feature = "work-product")]
    if let Some(take) = product::listening() {
        product::finish(app.clone(), take);
    }
    #[cfg(not(feature = "work-product"))]
    let _ = app;
}

fn unavailable_status() -> VoiceStatusV1 {
    VoiceStatusV1 {
        version: 1,
        engines: Vec::new(),
        chosen: None,
        effective: None,
        language: None,
        languages: Vec::new(),
        on_device: None,
        microphone: VoicePermissionV1::Undetermined,
    }
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn voice_status(caller: WebviewWindow) -> VoiceStatusV1 {
    if !authorize(&caller, CallerPolicy::Main, "voice_status") {
        return unavailable_status();
    }
    #[cfg(feature = "work-product")]
    return product::resolve().await.status;
    #[cfg(not(feature = "work-product"))]
    unavailable_status()
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn voice_choose_engine(
    caller: WebviewWindow,
    engine: Option<VoiceEngineV1>,
) -> VoiceStatusV1 {
    if !authorize(&caller, CallerPolicy::Main, "voice_choose_engine") {
        return unavailable_status();
    }
    #[cfg(feature = "work-product")]
    {
        zephium_app::voice::choose(engine.map(product::engine_of));
        product::resolve().await.status
    }
    #[cfg(not(feature = "work-product"))]
    {
        let _ = engine;
        unavailable_status()
    }
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn voice_choose_language(
    caller: WebviewWindow,
    language: Option<String>,
) -> VoiceStatusV1 {
    if !authorize(&caller, CallerPolicy::Main, "voice_choose_language") {
        return unavailable_status();
    }
    #[cfg(feature = "work-product")]
    {
        zephium_app::voice::choose_language(language.as_deref());
        product::resolve().await.status
    }
    #[cfg(not(feature = "work-product"))]
    {
        let _ = language;
        unavailable_status()
    }
}

/// Opens the microphone and starts a take. On first use this waits while
/// the system asks for permission.
#[tauri::command]
#[specta::specta]
pub(crate) async fn voice_start(
    caller: WebviewWindow,
    app: tauri::AppHandle,
    request: String,
) -> VoiceStartV1 {
    let refused = |fault| VoiceStartV1 {
        take: None,
        engine: None,
        fault: Some(fault),
    };
    if !authorize(&caller, CallerPolicy::Main, "voice_start") || !valid_request(&request) {
        return refused(VoiceFaultV1::Unavailable);
    }
    #[cfg(feature = "work-product")]
    return product::start(app, request).await;
    #[cfg(not(feature = "work-product"))]
    {
        let _ = (app, request);
        refused(VoiceFaultV1::Unavailable)
    }
}

/// Stops listening; the words arrive as a `done` change.
#[tauri::command]
#[specta::specta]
pub(crate) fn voice_stop(caller: WebviewWindow, app: tauri::AppHandle, take: u32) -> bool {
    if !authorize(&caller, CallerPolicy::Main, "voice_stop") {
        return false;
    }
    #[cfg(feature = "work-product")]
    return product::finish(app, take);
    #[cfg(not(feature = "work-product"))]
    {
        let _ = (app, take);
        false
    }
}

/// Discards only this start request's take, whatever its phase.
#[tauri::command]
#[specta::specta]
pub(crate) fn voice_cancel(
    caller: WebviewWindow,
    app: tauri::AppHandle,
    request: String,
    take: Option<u32>,
) -> bool {
    if !authorize(&caller, CallerPolicy::Main, "voice_cancel") || !valid_request(&request) {
        return false;
    }
    #[cfg(feature = "work-product")]
    return product::cancel(&app, &request, take);
    #[cfg(not(feature = "work-product"))]
    {
        let _ = (app, request, take);
        false
    }
}

/// Opens the system's privacy settings for the microphone or speech
/// recognition, after the person declined one.
#[tauri::command]
#[specta::specta]
pub(crate) fn voice_open_privacy(caller: WebviewWindow, kind: VoicePrivacyV1) -> bool {
    if !authorize(&caller, CallerPolicy::Main, "voice_open_privacy") {
        return false;
    }
    #[cfg(target_os = "macos")]
    {
        use objc2_app_kit::NSWorkspace;
        use objc2_foundation::{NSString, NSURL};
        let address = match kind {
            VoicePrivacyV1::Microphone => PRIVACY_PANE.to_owned() + "Privacy_Microphone",
            VoicePrivacyV1::Recognition => PRIVACY_PANE.to_owned() + "Privacy_SpeechRecognition",
            VoicePrivacyV1::Dictation => {
                "x-apple.systempreferences:com.apple.Keyboard-Settings.extension?Dictation"
                    .to_owned()
            }
        };
        let url = NSURL::URLWithString(&NSString::from_str(&address));
        url.is_some_and(|url| NSWorkspace::sharedWorkspace().openURL(&url))
    }
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::UI::Shell::ShellExecuteW;
        use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
        if kind != VoicePrivacyV1::Microphone {
            return false;
        }
        // SAFETY: constant NUL-terminated strings for the plain "open" verb;
        // ShellExecute reports success as any value above 32.
        let result = unsafe {
            ShellExecuteW(
                None,
                windows::core::w!("open"),
                windows::core::w!("ms-settings:privacy-microphone"),
                None,
                None,
                SW_SHOWNORMAL,
            )
        };
        result.0 as isize > 32
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = kind;
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancellation_authority_binds_both_request_and_optional_take() {
        assert!(owns_take("current", Some(2), "current", 2));
        assert!(owns_take("current", None, "current", 2));
        assert!(!owns_take("old", Some(1), "current", 2));
        assert!(!owns_take("current", Some(1), "current", 2));
        assert!(!owns_take("", None, "current", 2));
    }

    #[test]
    fn request_identity_is_bounded_nonzero_uuid_text() {
        assert!(valid_request("12345678-1234-1234-1234-123456789abc"));
        for request in [
            "",
            "request",
            "00000000-0000-0000-0000-000000000000",
            "12345678/1234-1234-1234-123456789abc",
            "12345678-1234-1234-1234-123456789abg",
        ] {
            assert!(!valid_request(request), "{request}");
        }
    }

    #[test]
    fn changes_and_status_have_stable_wire_shapes() {
        let change = VoiceChanged {
            take: 3,
            request: "request".into(),
            phase: VoicePhaseV1::Failed,
            engine: Some(VoiceEngineV1::OpenAi),
            partial: None,
            text: None,
            fault: Some(VoiceFaultV1::KeyRefused),
        };
        assert_eq!(
            serde_json::to_value(change).unwrap(),
            serde_json::json!({
                "take": 3, "request": "request", "phase": "failed", "engine": "open_ai", "partial": null,
                "text": null, "fault": "key_refused"
            })
        );
        let status = unavailable_status();
        let value = serde_json::to_value(status).unwrap();
        assert_eq!(value["engines"], serde_json::json!([]));
        assert_eq!(value["microphone"], "undetermined");
    }

    #[test]
    fn a_switched_off_dictation_is_its_own_fault() {
        use zephium_voice::Fault;
        assert_eq!(fault_of(Fault::DictationOff), VoiceFaultV1::DictationOff);
        assert_eq!(fault_of(Fault::Recognizer), VoiceFaultV1::Unavailable);
        assert_eq!(
            serde_json::to_value(VoiceFaultV1::DictationOff).unwrap(),
            "dictation_off"
        );
    }

    #[test]
    fn levels_travel_as_whole_percentages() {
        assert_eq!(percent(0.0), 0);
        assert_eq!(percent(0.444), 44);
        assert_eq!(percent(1.7), 100);
        assert_eq!(percent(f32::NAN), 0);
    }

    #[test]
    fn partial_words_keep_their_tail_at_a_character_boundary() {
        assert_eq!(bounded("short"), "short");
        let long = "ż".repeat(MAX_PARTIAL_BYTES);
        let tail = bounded(&long);
        assert!(tail.len() <= MAX_PARTIAL_BYTES);
        assert!(tail.chars().all(|c| c == 'ż'));
    }

    #[cfg(feature = "work-product")]
    #[test]
    fn cloud_refusals_name_what_the_person_can_fix() {
        use zephium_app::voice::VoiceEngine;
        use zephium_core::work::model::{HostedRefusal, WorkModelError};
        let fault = product::cloud_fault;
        assert_eq!(
            fault(VoiceEngine::Zephium, WorkModelError::Unauthorized),
            VoiceFaultV1::SignedOut
        );
        assert_eq!(
            fault(VoiceEngine::OpenAi, WorkModelError::Unauthorized),
            VoiceFaultV1::KeyRefused
        );
        assert_eq!(
            fault(VoiceEngine::Groq, WorkModelError::OverBudget),
            VoiceFaultV1::OutOfCredit
        );
        assert_eq!(
            fault(VoiceEngine::Zephium, WorkModelError::ContextTooLong),
            VoiceFaultV1::TooLong
        );
        let hosted = |refusal| WorkModelError::Hosted {
            refusal,
            retry_after_ms: None,
        };
        for (refusal, expected) in [
            (HostedRefusal::OutOfCredits, VoiceFaultV1::OutOfCredit),
            (HostedRefusal::PlanRequired, VoiceFaultV1::PlanRequired),
            (HostedRefusal::BillingReview, VoiceFaultV1::BillingReview),
            (HostedRefusal::DailyLimit, VoiceFaultV1::DailyLimit),
            (
                HostedRefusal::FreeUsagePaused,
                VoiceFaultV1::FreeUsagePaused,
            ),
            (
                HostedRefusal::ConfirmationLimited,
                VoiceFaultV1::RateLimited,
            ),
            (HostedRefusal::Paused, VoiceFaultV1::HostedPaused),
            (HostedRefusal::InvalidAudio, VoiceFaultV1::InvalidAudio),
            (
                HostedRefusal::UnsupportedAudio,
                VoiceFaultV1::UnsupportedAudio,
            ),
            (HostedRefusal::AudioTooLong, VoiceFaultV1::AudioTooLong),
            (HostedRefusal::AlreadySent, VoiceFaultV1::Unavailable),
        ] {
            assert_eq!(fault(VoiceEngine::Zephium, hosted(refusal)), expected);
        }
        for engine in [
            VoiceEngineV1::OnDevice,
            VoiceEngineV1::Zephium,
            VoiceEngineV1::OpenAi,
            VoiceEngineV1::Groq,
        ] {
            assert_eq!(product::engine_v1(product::engine_of(engine)), engine);
        }
    }
}
