//! Talking to the sidebar assistant in the main window: the duplex device,
//! the account's voice socket and the assistant behind the voice session
//! service, the talk key, and the projection as two events. Main-window
//! callers only; the service owns every decision.
use super::*;
use zephium_ipc::page_assist::PageAssistDecisionV1;
use zephium_ipc::voice_session::*;

/// The voice session's view changed.
#[derive(Clone, Debug, Serialize, Deserialize, specta::Type, tauri_specta::Event)]
#[tauri_specta(event_name = "zephium:voice-session-changed")]
pub(crate) struct VoiceSessionChanged(pub(crate) VoiceSessionViewV1);

/// Microphone and speech levels, about 25 a second while live.
#[derive(Clone, Debug, Serialize, Deserialize, specta::Type, tauri_specta::Event)]
#[tauri_specta(event_name = "zephium:voice-session-levels")]
pub(crate) struct VoiceSessionLevels(pub(crate) VoiceSessionLevelsV1);

#[cfg(feature = "work-product")]
const CHANGED: &str = "zephium:voice-session-changed";
#[cfg(feature = "work-product")]
const LEVELS: &str = "zephium:voice-session-levels";

fn allowed(caller: &WebviewWindow, command: &'static str) -> bool {
    authorize(caller, CallerPolicy::Main, command)
}

#[cfg(feature = "work-product")]
mod product {
    use super::*;
    use std::sync::{Arc, Mutex, OnceLock};
    use std::time::Instant;
    use zephium_core::work::model::WorkModelError;
    use zephium_engine::page_reader::PageVoice;
    use zephium_ipc::page_assist::{PageAssistAskV1, PageAssistAttachV1, PageAssistAvailabilityV1};
    use zephium_work_composition::page_assist::core::voice::{Fault, Language};
    use zephium_work_composition::page_assist::Delivery;
    use zephium_work_composition::voice_session::port::*;
    use zephium_work_composition::voice_session::{Command, Ports, Setup, VoiceSession};

    static SESSION: OnceLock<VoiceSession> = OnceLock::new();

    pub(super) fn session() -> Option<&'static VoiceSession> {
        SESSION.get()
    }

    /// The main window's Space, front tab and whether it is private.
    #[derive(Default)]
    struct Front {
        space: Option<String>,
        tab: Option<String>,
        private: bool,
    }

    static FRONT: Mutex<Front> = Mutex::new(Front {
        space: None,
        tab: None,
        private: false,
    });
    /// The Space of the conversation voice last asked in.
    static ASKED: Mutex<Option<(String, String)>> = Mutex::new(None);
    /// The reply started ahead of its final words, with the Space, tab and
    /// conversation it was started in.
    static HELD: Mutex<Option<HeldIn>> = Mutex::new(None);

    struct HeldIn {
        held: u64,
        space: String,
        tab: String,
        conversation: String,
    }

    fn front() -> std::sync::MutexGuard<'static, Front> {
        FRONT.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn availability(app: &tauri::AppHandle) -> VoiceAvailabilityV1 {
        if cfg!(not(any(target_os = "macos", target_os = "windows"))) {
            return VoiceAvailabilityV1::Unsupported;
        }
        if !app
            .try_state::<keymap::Keymap>()
            .is_some_and(|keymap| keymap.ai_enabled())
        {
            return VoiceAvailabilityV1::AiOff;
        }
        if front().private {
            return VoiceAvailabilityV1::Private;
        }
        if !zephium_app::voice::cloud_signed_in() {
            return VoiceAvailabilityV1::SignedOut;
        }
        VoiceAvailabilityV1::Available
    }

    pub(crate) fn refresh(app: &tauri::AppHandle) {
        if let Some(session) = session() {
            session.send(Command::Availability(availability(app)));
        }
    }

    pub(crate) fn observe_items(state: &zephium_ipc::ItemsState) {
        let Some(profile) = &state.profile else {
            return;
        };
        let mut front = front();
        front.private = profile.kind == zephium_ipc::ProfileKindView::Incognito;
        front.space = state.active_space_id.clone();
        front.tab = state.active.clone();
        drop(front);
        edge(None);
    }

    /// The voice edge on the main window's front page: the look the session
    /// last asked for, and the tab showing it.
    struct Edge {
        look: PageVoice,
        shown: Option<(zephium_core::ids::ItemId, PageVoice)>,
    }

    static EDGE: Mutex<Edge> = Mutex::new(Edge {
        look: PageVoice::Off,
        shown: None,
    });

    /// Listening while the microphone listens, speaking while speech plays;
    /// nothing where voice is unavailable.
    fn look_of(view: &VoiceSessionViewV1) -> PageVoice {
        let listening = matches!(
            view.state,
            VoiceSessionStateV1::Arming { .. } | VoiceSessionStateV1::Listening { .. }
        ) || (view.mic && !view.warm);
        match view.availability {
            VoiceAvailabilityV1::Available if listening => PageVoice::Listening,
            VoiceAvailabilityV1::Available if view.speaking => PageVoice::Speaking,
            _ => PageVoice::Off,
        }
    }

    /// Moves the edge to the front tab with `look` (or the last look), off
    /// the tab that showed it; never in a private window.
    pub(super) fn edge(look: Option<PageVoice>) {
        let Some(engine) = crate::page_assist::engine() else {
            return;
        };
        let tab = {
            let front = front();
            (!front.private)
                .then(|| {
                    front
                        .tab
                        .as_deref()
                        .and_then(zephium_core::ids::ItemId::parse)
                })
                .flatten()
        };
        let mut edge = EDGE.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(look) = look {
            edge.look = look;
        }
        let want = tab
            .filter(|_| edge.look != PageVoice::Off)
            .map(|tab| (tab, edge.look));
        if edge.shown == want {
            return;
        }
        let calm = crate::page_assist::reduce_motion();
        if let Some((old, _)) = edge.shown {
            if want.is_none_or(|(tab, _)| tab != old) {
                engine.page_voice_glow(old, PageVoice::Off, calm);
            }
        }
        if let Some((tab, look)) = want {
            engine.page_voice_glow(tab, look, calm);
        }
        edge.shown = want;
    }

    fn edge_level(levels: VoiceSessionLevelsV1) {
        let shown = EDGE.lock().unwrap_or_else(|p| p.into_inner()).shown;
        let (Some((tab, look)), Some(engine)) = (shown, crate::page_assist::engine()) else {
            return;
        };
        let level = match look {
            PageVoice::Listening => levels.input,
            PageVoice::Speaking => levels.output,
            PageVoice::Off => return,
        };
        if !crate::page_assist::reduce_motion() {
            engine.page_voice_level(tab, level);
        }
    }

    fn fault_of(fault: zephium_voice::Fault) -> Fault {
        match fault {
            zephium_voice::Fault::MicrophoneDenied => Fault::MicrophoneDenied,
            zephium_voice::Fault::NoMicrophone => Fault::NoMicrophone,
            _ => Fault::Unavailable,
        }
    }

    struct Relay(Arc<AudioInbox>);

    impl zephium_voice::DuplexListener for Relay {
        fn frame(&self, pcm: &[i16]) {
            self.0.frame(pcm);
        }
        fn input_levels(&self, levels: &[f32]) {
            if let Some(level) = levels.last() {
                self.0.input_level(*level);
            }
        }
        fn output_level(&self, level: f32) {
            self.0.output_level(level);
        }
        fn played(&self, mark: u64) {
            self.0.played(mark);
        }
        fn route_changed(&self) {
            self.0.route_changed();
        }
        fn lost(&self, fault: zephium_voice::Fault) {
            self.0.lost(fault_of(fault));
        }
    }

    type Slot = Arc<Mutex<Option<zephium_voice::Duplex>>>;

    struct Device {
        app: tauri::AppHandle,
        duplex: Slot,
    }

    impl Device {
        fn with(&self, apply: impl FnOnce(&zephium_voice::Duplex)) {
            if let Some(duplex) = self
                .duplex
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .as_ref()
            {
                apply(duplex);
            }
        }
    }

    impl AudioPort for Device {
        fn prepare(&self, inbox: Arc<AudioInbox>) -> BoxFuture<'static, Result<(), Fault>> {
            let slot = self.duplex.clone();
            Box::pin(async move {
                tauri::async_runtime::spawn_blocking(move || {
                    if slot.lock().unwrap_or_else(|p| p.into_inner()).is_some() {
                        return Ok(());
                    }
                    let duplex = zephium_voice::Duplex::open(
                        zephium_voice::DuplexConfig {
                            voice_processing: true,
                        },
                        Arc::new(Relay(inbox)),
                    )
                    .map_err(fault_of)?;
                    *slot.lock().unwrap_or_else(|p| p.into_inner()) = Some(duplex);
                    Ok(())
                })
                .await
                .unwrap_or(Err(Fault::NoMicrophone))
            })
        }

        fn release(&self) {
            let duplex = self.duplex.lock().unwrap_or_else(|p| p.into_inner()).take();
            if let Some(duplex) = duplex {
                drop(tauri::async_runtime::spawn_blocking(move || drop(duplex)));
            }
        }

        fn start(&self) -> BoxFuture<'static, Result<(), Fault>> {
            crate::voice::cancel_dictation(&self.app);
            let mut started = false;
            self.with(|duplex| {
                duplex.start();
                started = true;
            });
            Box::pin(async move { started.then_some(()).ok_or(Fault::NoMicrophone) })
        }

        fn standby(&self) {
            self.with(zephium_voice::Duplex::standby);
        }

        fn gate(&self, on: bool) {
            self.with(|duplex| duplex.listen(on));
        }

        fn play(&self, pcm: &[i16]) {
            self.with(|duplex| duplex.play(pcm));
        }

        fn mark(&self, mark: u64) {
            self.with(|duplex| duplex.mark(mark));
        }

        fn flush(&self) {
            self.with(zephium_voice::Duplex::flush);
        }
    }

    struct Cloud;

    impl LinkPort for Cloud {
        fn open(&self) -> BoxFuture<'static, Result<Opened, WorkModelError>> {
            Box::pin(async {
                let (link, ready, events) = zephium_app::voice::talk_link().await?;
                Ok(Opened {
                    socket: Arc::new(link),
                    ready,
                    events,
                })
            })
        }
    }

    /// Asks, or steers the running reply, in the main window's Space with
    /// its front tab attached when it can be read; the reply is spoken when
    /// spoken replies are on and only shown otherwise.
    async fn ask(text: String, steer: bool, spoken: bool) -> Result<Asked, Fault> {
        let service = crate::page_assist::service().ok_or(Fault::Unavailable)?;
        let (space, tab) = {
            let front = front();
            (
                front.space.clone().ok_or(Fault::Unavailable)?,
                front.tab.clone(),
            )
        };
        let status = service.status(&space, tab.as_deref()).await;
        let conversation = status.conversation.id;
        if conversation.is_empty() {
            return Err(Fault::Unavailable);
        }
        let attachments = match (status.availability, tab) {
            (PageAssistAvailabilityV1::Ready, Some(tab)) => vec![PageAssistAttachV1::Page { tab }],
            _ => Vec::new(),
        };
        let request = PageAssistAskV1 {
            space: space.clone(),
            conversation: conversation.clone(),
            text,
            suggestion: None,
            attachments,
        };
        let delivery = if spoken {
            Delivery::Spoken
        } else {
            Delivery::Shown
        };
        let admitted = if steer {
            service.steer(request, delivery).await
        } else {
            service.ask_as(request, delivery).await
        };
        let (reply, task) = admitted.map_err(|fault| Fault::from_assist(&fault))?;
        tauri::async_runtime::spawn(task);
        *ASKED.lock().unwrap_or_else(|p| p.into_inner()) = Some((space, conversation.clone()));
        Ok(Asked {
            conversation,
            reply,
        })
    }

    /// Starts a held reply on words the final transcript has not confirmed
    /// yet; only a readable front tab qualifies.
    async fn ask_ahead(text: String, spoken: bool) -> Result<u64, Fault> {
        let service = crate::page_assist::service().ok_or(Fault::Unavailable)?;
        let (space, tab) = {
            let front = front();
            (
                front.space.clone().ok_or(Fault::Unavailable)?,
                front.tab.clone().ok_or(Fault::Unavailable)?,
            )
        };
        let status = service.status(&space, Some(&tab)).await;
        let conversation = status.conversation.id;
        if conversation.is_empty() || status.availability != PageAssistAvailabilityV1::Ready {
            return Err(Fault::Unavailable);
        }
        let request = PageAssistAskV1 {
            space: space.clone(),
            conversation: conversation.clone(),
            text,
            suggestion: None,
            attachments: vec![PageAssistAttachV1::Page { tab: tab.clone() }],
        };
        let delivery = if spoken {
            Delivery::Spoken
        } else {
            Delivery::Shown
        };
        let (held, task) = service
            .ask_ahead(request, delivery)
            .await
            .map_err(|fault| Fault::from_assist(&fault))?;
        tauri::async_runtime::spawn(task);
        let earlier = HELD
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .replace(HeldIn {
                held: held.id,
                space,
                tab,
                conversation,
            });
        if let Some(earlier) = earlier {
            service.discard(earlier.held);
        }
        Ok(held.id)
    }

    /// Releases a held reply when the front Space and tab are still the ones
    /// it was started in; otherwise discards it.
    async fn release(held: u64, text: String) -> Result<Asked, Fault> {
        let service = crate::page_assist::service().ok_or(Fault::Unavailable)?;
        let started = {
            let mut slot = HELD.lock().unwrap_or_else(|p| p.into_inner());
            if slot.as_ref().is_some_and(|entry| entry.held == held) {
                slot.take()
            } else {
                None
            }
        };
        let Some(started) = started else {
            service.discard(held);
            return Err(Fault::Unavailable);
        };
        let same = {
            let front = front();
            front.space.as_deref() == Some(started.space.as_str())
                && front.tab.as_deref() == Some(started.tab.as_str())
        };
        if !same {
            service.discard(held);
            return Err(Fault::Unavailable);
        }
        let reply = service
            .release(held, &text)
            .await
            .map_err(|fault| Fault::from_assist(&fault))?;
        *ASKED.lock().unwrap_or_else(|p| p.into_inner()) =
            Some((started.space, started.conversation.clone()));
        Ok(Asked {
            conversation: started.conversation,
            reply,
        })
    }

    fn discard(held: u64) {
        let mut slot = HELD.lock().unwrap_or_else(|p| p.into_inner());
        if slot.as_ref().is_some_and(|entry| entry.held == held) {
            *slot = None;
        }
        drop(slot);
        if let Some(service) = crate::page_assist::service() {
            service.discard(held);
        }
    }

    fn space_of(conversation: &str) -> Option<String> {
        ASKED
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .filter(|(_, asked)| asked == conversation)
            .map(|(space, _)| space.clone())
    }

    struct Assistant;

    impl AssistantPort for Assistant {
        fn ask(&self, text: String, spoken: bool) -> BoxFuture<'static, Result<Asked, Fault>> {
            Box::pin(ask(text, false, spoken))
        }

        fn ask_ahead(&self, text: String, spoken: bool) -> BoxFuture<'static, Result<u64, Fault>> {
            Box::pin(ask_ahead(text, spoken))
        }

        fn release(&self, held: u64, text: String) -> BoxFuture<'static, Result<Asked, Fault>> {
            Box::pin(release(held, text))
        }

        fn discard(&self, held: u64) {
            discard(held);
        }

        fn steer(
            &self,
            _conversation: String,
            _reply: u32,
            text: String,
            spoken: bool,
        ) -> BoxFuture<'static, Result<Asked, Fault>> {
            Box::pin(ask(text, true, spoken))
        }

        fn stop(&self, conversation: &str, _reply: u32) {
            if let (Some(service), Some(space)) =
                (crate::page_assist::service(), space_of(conversation))
            {
                service.stop(&space);
            }
        }

        fn decide(
            &self,
            conversation: &str,
            reply: u32,
            block: u16,
            decision: PageAssistDecisionV1,
        ) -> bool {
            crate::page_assist::service()
                .is_some_and(|service| service.decide(conversation, reply, block, decision))
        }
    }

    struct Frame(tauri::AppHandle);

    impl zephium_work_composition::voice_session::port::Projection for Frame {
        fn view(&self, view: &VoiceSessionViewV1) {
            if let Some(talk) = self.0.try_state::<talk_key::TalkKey>() {
                let busy = view.state != VoiceSessionStateV1::Idle || view.fault.is_some();
                talk.claim(busy || view.speaking, view.confirm.is_some());
            }
            edge(Some(look_of(view)));
            emit_to_privileged(&self.0, MAIN_LABEL, CHANGED, view);
        }

        fn levels(&self, levels: VoiceSessionLevelsV1) {
            edge_level(levels);
            emit_to_privileged(&self.0, MAIN_LABEL, LEVELS, &levels);
        }
    }

    struct Monotonic(Instant);

    impl Clock for Monotonic {
        fn now_ms(&self) -> u64 {
            u64::try_from(self.0.elapsed().as_millis()).unwrap_or(u64::MAX)
        }
    }

    fn uuid() -> String {
        let mut bytes = [0u8; 16];
        if getrandom::fill(&mut bytes).is_err() {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_nanos());
            bytes = nanos.to_le_bytes();
        }
        bytes[6] = (bytes[6] & 0x0f) | 0x40;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        format!(
            "{}-{}-{}-{}-{}",
            &hex[..8],
            &hex[8..12],
            &hex[12..16],
            &hex[16..20],
            &hex[20..]
        )
    }

    fn system_language() -> Language {
        zephium_voice::preferred_languages()
            .iter()
            .find_map(|tag| Language::from_code(tag))
            .unwrap_or_default()
    }

    pub(super) fn language_v1(code: Option<&str>) -> VoiceLanguageV1 {
        match code.and_then(Language::from_code) {
            None => VoiceLanguageV1::Automatic,
            Some(Language::En) => VoiceLanguageV1::En,
            Some(Language::Es) => VoiceLanguageV1::Es,
            Some(Language::De) => VoiceLanguageV1::De,
            Some(Language::Fr) => VoiceLanguageV1::Fr,
            Some(Language::Pl) => VoiceLanguageV1::Pl,
            Some(Language::Pt) => VoiceLanguageV1::Pt,
            Some(Language::Ru) => VoiceLanguageV1::Ru,
            Some(Language::It) => VoiceLanguageV1::It,
            Some(Language::Ja) => VoiceLanguageV1::Ja,
            Some(Language::Zh) => VoiceLanguageV1::Zh,
        }
    }

    pub(super) fn code_of(language: VoiceLanguageV1) -> Option<&'static str> {
        Some(match language {
            VoiceLanguageV1::Automatic => return None,
            VoiceLanguageV1::En => "en",
            VoiceLanguageV1::Es => "es",
            VoiceLanguageV1::De => "de",
            VoiceLanguageV1::Fr => "fr",
            VoiceLanguageV1::Pl => "pl",
            VoiceLanguageV1::Pt => "pt",
            VoiceLanguageV1::Ru => "ru",
            VoiceLanguageV1::It => "it",
            VoiceLanguageV1::Ja => "ja",
            VoiceLanguageV1::Zh => "zh",
        })
    }

    pub(super) fn stored() -> VoiceSessionPreferencesV1 {
        let stored = zephium_app::voice::talk_settings();
        VoiceSessionPreferencesV1 {
            spoken: stored.spoken,
            voice: stored.voice,
            language: language_v1(stored.language.as_deref()),
            earcons: stored.earcons,
        }
    }

    pub(super) fn store(preferences: &VoiceSessionPreferencesV1) -> bool {
        zephium_app::voice::store_talk_settings(&zephium_app::voice::TalkSettings {
            spoken: preferences.spoken,
            voice: preferences.voice.clone(),
            language: code_of(preferences.language).map(str::to_owned),
            earcons: preferences.earcons,
        })
    }

    /// The talk key's signals, as the voice session's commands.
    pub(super) fn signal(signal: talk_key::TalkSignal) -> Command {
        match signal {
            talk_key::TalkSignal::Down => Command::KeyDown,
            talk_key::TalkSignal::Up => Command::KeyUp,
            talk_key::TalkSignal::Cancel => Command::Escape,
            talk_key::TalkSignal::Confirm => Command::Decide(PageAssistDecisionV1::Approve),
        }
    }

    pub(crate) fn install(app: &tauri::AppHandle) {
        let (voice, driver) = VoiceSession::new(
            Ports {
                audio: Arc::new(Device {
                    app: app.clone(),
                    duplex: Slot::default(),
                }),
                link: Arc::new(Cloud),
                assistant: Arc::new(Assistant),
                projection: Arc::new(Frame(app.clone())),
                clock: Arc::new(Monotonic(Instant::now())),
                ids: Arc::new(uuid),
            },
            Setup {
                preferences: stored(),
                availability: availability(app),
                system_language: system_language(),
            },
        );
        if SESSION.set(voice).is_err() {
            return;
        }
        tauri::async_runtime::spawn(driver);
        if let Some(talk) = app.try_state::<talk_key::TalkKey>() {
            talk.attach(|key| {
                if let Some(session) = session() {
                    session.send(signal(key));
                }
            });
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn talk_signals_map_to_session_commands() {
            assert_eq!(signal(talk_key::TalkSignal::Down), Command::KeyDown);
            assert_eq!(signal(talk_key::TalkSignal::Up), Command::KeyUp);
            assert_eq!(signal(talk_key::TalkSignal::Cancel), Command::Escape);
            assert_eq!(
                signal(talk_key::TalkSignal::Confirm),
                Command::Decide(PageAssistDecisionV1::Approve)
            );
        }

        #[test]
        fn the_edge_shows_the_live_microphone_and_speech_only() {
            let view = |state, mic, warm, speaking, availability| VoiceSessionViewV1 {
                version: 1,
                seq: 1,
                availability,
                state,
                mic,
                warm,
                interim: String::new(),
                heard: None,
                phase: None,
                step: None,
                confirm: None,
                speaking,
                outcome: None,
                fault: None,
            };
            let held = VoicePressV1::Held;
            let available = VoiceAvailabilityV1::Available;
            let cases = [
                (
                    VoiceSessionStateV1::Idle,
                    false,
                    false,
                    false,
                    PageVoice::Off,
                ),
                (
                    VoiceSessionStateV1::Arming { press: held },
                    false,
                    false,
                    false,
                    PageVoice::Listening,
                ),
                (
                    VoiceSessionStateV1::Listening { press: held },
                    true,
                    false,
                    true,
                    PageVoice::Listening,
                ),
                (
                    VoiceSessionStateV1::Finalizing,
                    true,
                    true,
                    false,
                    PageVoice::Off,
                ),
                (
                    VoiceSessionStateV1::Finalizing,
                    true,
                    false,
                    false,
                    PageVoice::Listening,
                ),
                (
                    VoiceSessionStateV1::Idle,
                    true,
                    true,
                    true,
                    PageVoice::Speaking,
                ),
                (
                    VoiceSessionStateV1::Idle,
                    false,
                    false,
                    true,
                    PageVoice::Speaking,
                ),
            ];
            for (state, mic, warm, speaking, look) in cases {
                assert_eq!(
                    look_of(&view(state, mic, warm, speaking, available)),
                    look,
                    "{state:?}"
                );
            }
            for refused in [
                VoiceAvailabilityV1::Private,
                VoiceAvailabilityV1::AiOff,
                VoiceAvailabilityV1::SignedOut,
                VoiceAvailabilityV1::Unsupported,
            ] {
                let live = view(
                    VoiceSessionStateV1::Listening { press: held },
                    true,
                    false,
                    true,
                    refused,
                );
                assert_eq!(look_of(&live), PageVoice::Off);
            }
        }

        #[test]
        fn ids_are_uuid_v4_and_languages_round_trip() {
            let id = uuid();
            let bytes = id.as_bytes();
            assert_eq!(id.len(), 36);
            assert_eq!(bytes[14], b'4');
            assert!(matches!(bytes[19], b'8' | b'9' | b'a' | b'b'));
            assert_ne!(uuid(), id);
            for language in [
                VoiceLanguageV1::Automatic,
                VoiceLanguageV1::Pl,
                VoiceLanguageV1::Zh,
            ] {
                assert_eq!(language_v1(code_of(language)), language);
            }
        }
    }
}

#[cfg(feature = "work-product")]
pub(crate) use product::{install, observe_items, refresh};

#[cfg(not(feature = "work-product"))]
pub(crate) fn install(_app: &tauri::AppHandle) {}
#[cfg(not(feature = "work-product"))]
pub(crate) fn observe_items(_state: &zephium_ipc::ItemsState) {}
#[cfg(not(feature = "work-product"))]
pub(crate) fn refresh(_app: &tauri::AppHandle) {}

/// Tees one assistant event to the voice session; the frame's copy is untouched.
pub(crate) fn assist(event: &zephium_ipc::page_assist::PageAssistChangedV1) {
    #[cfg(feature = "work-product")]
    if let Some(session) = product::session() {
        session.assist(event);
    }
    #[cfg(not(feature = "work-product"))]
    let _ = event;
}

/// The main window became active, or was minimized.
pub(crate) fn foreground(active: bool) {
    #[cfg(feature = "work-product")]
    if let Some(session) = product::session() {
        session.send(zephium_work_composition::voice_session::Command::Foreground(active));
    }
    #[cfg(not(feature = "work-product"))]
    let _ = active;
}

/// Dictation is starting: a voice utterance gives up the microphone.
pub(crate) fn yield_microphone() {
    #[cfg(feature = "work-product")]
    if let Some(session) = product::session() {
        if session.view().is_some_and(|view| {
            matches!(
                view.state,
                VoiceSessionStateV1::Arming { .. } | VoiceSessionStateV1::Listening { .. }
            )
        }) {
            session.send(zephium_work_composition::voice_session::Command::Escape);
        }
    }
}

/// The window closes or the app quits.
pub(crate) fn dispose() {
    #[cfg(feature = "work-product")]
    if let Some(session) = product::session() {
        session.send(zephium_work_composition::voice_session::Command::Dispose);
    }
    #[cfg(feature = "work-product")]
    product::edge(Some(zephium_engine::page_reader::PageVoice::Off));
}

#[cfg(feature = "work-product")]
fn send(command: zephium_work_composition::voice_session::Command) -> bool {
    product::session().is_some_and(|session| session.send(command))
}

/// The voice session's view; null when voice is not part of this build.
#[tauri::command]
#[specta::specta]
pub(crate) async fn voice_session_view(caller: WebviewWindow) -> Option<VoiceSessionViewV1> {
    if !allowed(&caller, "voice_session_view") {
        return None;
    }
    #[cfg(feature = "work-product")]
    return product::session().and_then(|session| session.view());
    #[cfg(not(feature = "work-product"))]
    None
}

/// The on-screen talk button: start, stop (send) or cancel.
#[tauri::command]
#[specta::specta]
pub(crate) async fn voice_session_intent(
    caller: WebviewWindow,
    intent: VoiceSessionIntentV1,
) -> bool {
    if !allowed(&caller, "voice_session_intent") {
        return false;
    }
    #[cfg(feature = "work-product")]
    return send(zephium_work_composition::voice_session::Command::Intent(
        intent,
    ));
    #[cfg(not(feature = "work-product"))]
    {
        let _ = intent;
        false
    }
}

/// Approves or declines the confirm voice is waiting on, as a click does.
#[tauri::command]
#[specta::specta]
pub(crate) async fn voice_session_decide(
    caller: WebviewWindow,
    decision: PageAssistDecisionV1,
) -> bool {
    if !allowed(&caller, "voice_session_decide") {
        return false;
    }
    #[cfg(feature = "work-product")]
    return send(zephium_work_composition::voice_session::Command::Decide(
        decision,
    ));
    #[cfg(not(feature = "work-product"))]
    {
        let _ = decision;
        false
    }
}

/// Voice settings and the voices the service offers.
#[tauri::command]
#[specta::specta]
pub(crate) async fn voice_session_settings(
    caller: WebviewWindow,
) -> Option<VoiceSessionSettingsV1> {
    if !allowed(&caller, "voice_session_settings") {
        return None;
    }
    #[cfg(feature = "work-product")]
    return product::session().and_then(|session| session.settings());
    #[cfg(not(feature = "work-product"))]
    None
}

/// Stores voice settings; null when they were refused.
#[tauri::command]
#[specta::specta]
pub(crate) async fn voice_session_set_settings(
    caller: WebviewWindow,
    preferences: VoiceSessionPreferencesV1,
) -> Option<VoiceSessionSettingsV1> {
    if !allowed(&caller, "voice_session_set_settings")
        || preferences
            .voice
            .as_ref()
            .is_some_and(|voice| voice.is_empty() || voice.len() > 64)
    {
        return None;
    }
    #[cfg(feature = "work-product")]
    {
        let session = product::session()?;
        if !product::store(&preferences) {
            return None;
        }
        session
            .send(zephium_work_composition::voice_session::Command::Preferences(product::stored()));
        tokio::task::yield_now().await;
        session.settings().map(|mut settings| {
            settings.preferences = product::stored();
            settings
        })
    }
    #[cfg(not(feature = "work-product"))]
    {
        let _ = preferences;
        None
    }
}
