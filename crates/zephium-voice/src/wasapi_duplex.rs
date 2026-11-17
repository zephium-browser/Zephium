//! Windows: event-driven shared-mode WASAPI on the default endpoints, serviced
//! by one worker. With voice processing the capture stream is in the
//! communications category, so the endpoint's own echo canceller and noise
//! suppression apply, bound to the endpoint we render to where Windows allows.
//! A default-device change or an invalidated device rebuilds both streams; the
//! playback queue lives in [`Engine`] and survives.

use std::collections::VecDeque;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use windows::core::{IUnknown, IUnknown_Vtbl, Interface, GUID, HRESULT, PCWSTR};
use windows::Win32::Foundation::{
    CloseHandle, E_NOINTERFACE, E_POINTER, HANDLE, PROPERTYKEY, S_OK,
};
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::{
    eCapture, eCommunications, eConsole, eRender, AudioCategory_Communications,
    AudioCategory_Other, AudioClientProperties, EDataFlow, ERole, IAcousticEchoCancellationControl,
    IAudioCaptureClient, IAudioClient, IAudioClient2, IAudioClient3, IAudioEffectsManager,
    IAudioRenderClient, IMMDeviceEnumerator, IMMNotificationClient, IMMNotificationClient_Vtbl,
    MMDeviceEnumerator, AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED,
    AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM, AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
    AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, AUDCLNT_STREAMOPTIONS_NONE, AUDCLNT_STREAMOPTIONS_RAW,
    AUDIO_EFFECT, AUDIO_EFFECT_STATE_ON, DEVICE_STATE, WAVEFORMATEX, WAVEFORMATEXTENSIBLE,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
    COINIT_MULTITHREADED,
};
use windows::Win32::System::Threading::{
    AvRevertMmThreadCharacteristics, AvSetMmThreadCharacteristicsW, CreateEventW, SetEvent,
    WaitForMultipleObjects,
};

use crate::duplex::Engine;
use crate::{DuplexConfig, Fault};

const E_ACCESSDENIED: HRESULT = HRESULT(0x8007_0005_u32 as i32);
const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;
const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;
const SUBTYPE_FLOAT: GUID = GUID::from_u128(0x0000_0003_0000_0010_8000_00aa_0038_9b71);
const EFFECTS: [(GUID, &str); 6] = [
    (
        GUID::from_u128(0x6f64_adbe_8211_11e2_8c70_2c27_d7f0_01fa),
        "aec",
    ),
    (
        GUID::from_u128(0x6f64_adbf_8211_11e2_8c70_2c27_d7f0_01fa),
        "ns",
    ),
    (
        GUID::from_u128(0x6f64_add0_8211_11e2_8c70_2c27_d7f0_01fa),
        "deep_ns",
    ),
    (
        GUID::from_u128(0x6f64_adc0_8211_11e2_8c70_2c27_d7f0_01fa),
        "agc",
    ),
    (
        GUID::from_u128(0x6f64_adc1_8211_11e2_8c70_2c27_d7f0_01fa),
        "beamforming",
    ),
    (
        GUID::from_u128(0x6f64_adcf_8211_11e2_8c70_2c27_d7f0_01fa),
        "far_field_beamforming",
    ),
];
/// The capture side keeps this much in the device; render keeps the minimum.
const CAPTURE_BUFFER: i64 = 1_000_000;
/// Wakes the worker to notice a device that went away without an error.
const IDLE_WAIT_MS: u32 = 200;
const SETTLE: Duration = Duration::from_millis(150);
const CHURN: usize = 5;
const CHURN_WINDOW: Duration = Duration::from_secs(10);

fn fault(error: &windows::core::Error) -> Fault {
    if error.code() == E_ACCESSDENIED {
        Fault::MicrophoneDenied
    } else {
        Fault::NoMicrophone
    }
}

struct Event(HANDLE);

// SAFETY: an event handle may be signalled and waited on from any thread.
unsafe impl Send for Event {}
// SAFETY: as above.
unsafe impl Sync for Event {}

impl Event {
    fn new() -> Result<Self, Fault> {
        // SAFETY: an unnamed auto-reset event, closed in `Drop`.
        unsafe { CreateEventW(None, false, false, PCWSTR::null()) }
            .map(Self)
            .map_err(|_| Fault::NoMicrophone)
    }

    fn raise(&self) {
        // SAFETY: the handle is open for this value's life.
        let _ = unsafe { SetEvent(self.0) };
    }
}

impl Drop for Event {
    fn drop(&mut self) {
        // SAFETY: closes the handle this value created.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

struct Shared {
    wake: Event,
    closing: AtomicBool,
    changed: AtomicBool,
}

struct Com;

impl Com {
    fn enter() -> Result<Self, Fault> {
        // SAFETY: balanced by `Drop` on this same thread.
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
            .ok()
            .map_err(|_| Fault::NoMicrophone)?;
        Ok(Self)
    }
}

impl Drop for Com {
    fn drop(&mut self) {
        // SAFETY: pairs the successful initialization above.
        unsafe { CoUninitialize() };
    }
}

/// Default-device changes for our role, delivered on a system thread that
/// only raises a flag and the worker's event.
#[repr(C)]
struct Notifier {
    vtable: *const IMMNotificationClient_Vtbl,
    refs: AtomicU32,
    role: ERole,
    shared: Arc<Shared>,
}

static NOTIFIER_VTABLE: IMMNotificationClient_Vtbl = IMMNotificationClient_Vtbl {
    base__: IUnknown_Vtbl {
        QueryInterface: notifier_query,
        AddRef: notifier_add_ref,
        Release: notifier_release,
    },
    OnDeviceStateChanged: notifier_state,
    OnDeviceAdded: notifier_device,
    OnDeviceRemoved: notifier_device,
    OnDefaultDeviceChanged: notifier_default,
    OnPropertyValueChanged: notifier_property,
};

unsafe extern "system" fn notifier_query(
    this: *mut c_void,
    iid: *const GUID,
    out: *mut *mut c_void,
) -> HRESULT {
    if out.is_null() || iid.is_null() {
        return E_POINTER;
    }
    // SAFETY: COM passes valid pointers; `this` is a live `Notifier`.
    unsafe {
        if *iid == IUnknown::IID || *iid == IMMNotificationClient::IID {
            notifier_add_ref(this);
            *out = this;
            S_OK
        } else {
            *out = std::ptr::null_mut();
            E_NOINTERFACE
        }
    }
}

unsafe extern "system" fn notifier_add_ref(this: *mut c_void) -> u32 {
    // SAFETY: `this` is a live `Notifier`.
    unsafe {
        (*this.cast::<Notifier>())
            .refs
            .fetch_add(1, Ordering::AcqRel)
            + 1
    }
}

unsafe extern "system" fn notifier_release(this: *mut c_void) -> u32 {
    // SAFETY: `this` is a live `Notifier` created by `Box::into_raw`; the
    // last reference frees it.
    unsafe {
        let left = (*this.cast::<Notifier>())
            .refs
            .fetch_sub(1, Ordering::AcqRel)
            - 1;
        if left == 0 {
            drop(Box::from_raw(this.cast::<Notifier>()));
        }
        left
    }
}

unsafe extern "system" fn notifier_state(_: *mut c_void, _: PCWSTR, _: DEVICE_STATE) -> HRESULT {
    S_OK
}

unsafe extern "system" fn notifier_device(_: *mut c_void, _: PCWSTR) -> HRESULT {
    S_OK
}

unsafe extern "system" fn notifier_property(_: *mut c_void, _: PCWSTR, _: PROPERTYKEY) -> HRESULT {
    S_OK
}

unsafe extern "system" fn notifier_default(
    this: *mut c_void,
    flow: EDataFlow,
    role: ERole,
    _: PCWSTR,
) -> HRESULT {
    // SAFETY: `this` is a live `Notifier`.
    let notifier = unsafe { &*this.cast::<Notifier>() };
    if role == notifier.role && (flow == eRender || flow == eCapture) {
        notifier.shared.changed.store(true, Ordering::Release);
        notifier.shared.wake.raise();
    }
    S_OK
}

struct Registration {
    devices: IMMDeviceEnumerator,
    client: IMMNotificationClient,
}

impl Registration {
    fn new(devices: &IMMDeviceEnumerator, role: ERole, shared: &Arc<Shared>) -> Option<Self> {
        let raw = Box::into_raw(Box::new(Notifier {
            vtable: &NOTIFIER_VTABLE,
            refs: AtomicU32::new(1),
            role,
            shared: shared.clone(),
        }));
        // SAFETY: a `Notifier` starts with its vtable pointer and one owned
        // reference, which `client` takes over.
        let client = unsafe { IMMNotificationClient::from_raw(raw.cast()) };
        // SAFETY: registering a live client on a live enumerator.
        unsafe { devices.RegisterEndpointNotificationCallback(&client) }.ok()?;
        Some(Self {
            devices: devices.clone(),
            client,
        })
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        // SAFETY: unregistering the client registered in `new`.
        let _ = unsafe {
            self.devices
                .UnregisterEndpointNotificationCallback(&self.client)
        };
    }
}

/// A float32 stream format at the endpoint's own rate and channel count.
struct Mix {
    rate: u32,
    channels: u16,
    float: bool,
    raw: *mut WAVEFORMATEX,
}

impl Mix {
    fn of(client: &IAudioClient) -> Result<Self, windows::core::Error> {
        // SAFETY: the returned format is freed in `Drop`.
        unsafe {
            let raw = client.GetMixFormat()?;
            let format = std::ptr::read_unaligned(raw);
            let (tag, bits) = (format.wFormatTag, format.wBitsPerSample);
            let float = bits == 32
                && (tag == WAVE_FORMAT_IEEE_FLOAT
                    || (tag == WAVE_FORMAT_EXTENSIBLE && {
                        let extended = std::ptr::read_unaligned(raw.cast::<WAVEFORMATEXTENSIBLE>());
                        let sub: GUID = extended.SubFormat;
                        sub == SUBTYPE_FLOAT
                    }));
            let (rate, channels) = (format.nSamplesPerSec, format.nChannels);
            Ok(Self {
                rate,
                channels: channels.max(1),
                float,
                raw,
            })
        }
    }

    fn float(&self) -> WAVEFORMATEX {
        let block = 4 * self.channels;
        WAVEFORMATEX {
            wFormatTag: WAVE_FORMAT_IEEE_FLOAT,
            nChannels: self.channels,
            nSamplesPerSec: self.rate,
            nAvgBytesPerSec: self.rate * u32::from(block),
            nBlockAlign: block,
            wBitsPerSample: 32,
            cbSize: 0,
        }
    }
}

impl Drop for Mix {
    fn drop(&mut self) {
        // SAFETY: frees the format GetMixFormat allocated.
        unsafe { CoTaskMemFree(Some(self.raw.cast_const().cast())) };
    }
}

const CONVERT: u32 = AUDCLNT_STREAMFLAGS_EVENTCALLBACK
    | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM
    | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY;

struct Streams {
    capture: IAudioClient,
    capture_client: IAudioCaptureClient,
    capture_event: Event,
    in_channels: usize,
    render: IAudioClient,
    render_client: IAudioRenderClient,
    render_event: Event,
    out_channels: usize,
    out_frames: u32,
    scratch: Vec<f32>,
}

impl Streams {
    fn open(
        config: DuplexConfig,
        core: &Engine,
        devices: &IMMDeviceEnumerator,
        role: ERole,
    ) -> Result<Self, Fault> {
        let listener = core.listener();
        let started = Instant::now();
        let mut facts = Vec::new();
        // SAFETY: COM is initialized on this worker; every interface is used
        // and released on it. Formats outlive the Initialize calls reading them.
        unsafe {
            let speaker = devices
                .GetDefaultAudioEndpoint(eRender, role)
                .map_err(|_| Fault::NoMicrophone)?;
            let speaker_id = speaker.GetId().map_err(|_| Fault::NoMicrophone)?;
            let speaker_wide: Vec<u16> = speaker_id
                .as_wide()
                .iter()
                .copied()
                .chain(std::iter::once(0))
                .collect();
            CoTaskMemFree(Some(speaker_id.0.cast_const().cast()));
            if let Ok(volume) = speaker.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None) {
                if let (Ok(level), Ok(muted)) =
                    (volume.GetMasterVolumeLevelScalar(), volume.GetMute())
                {
                    facts.push(format!(
                        "render_volume={level:.2} muted={}",
                        u8::from(muted.as_bool())
                    ));
                }
            }
            let render: IAudioClient = speaker
                .Activate(CLSCTX_ALL, None)
                .map_err(|_| Fault::NoMicrophone)?;
            let out_mix = Mix::of(&render).map_err(|_| Fault::NoMicrophone)?;
            let mut period = None;
            if out_mix.float {
                if let Ok(low) = render.cast::<IAudioClient3>() {
                    let (mut default, mut fundamental, mut min, mut max) = (0, 0, 0, 0);
                    if low
                        .GetSharedModeEnginePeriod(
                            out_mix.raw,
                            &mut default,
                            &mut fundamental,
                            &mut min,
                            &mut max,
                        )
                        .is_ok()
                        && low
                            .InitializeSharedAudioStream(
                                AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
                                min,
                                out_mix.raw,
                                None,
                            )
                            .is_ok()
                    {
                        period = Some(min);
                    }
                }
            }
            let render = if period.is_some() {
                render
            } else {
                let render: IAudioClient = speaker
                    .Activate(CLSCTX_ALL, None)
                    .map_err(|_| Fault::NoMicrophone)?;
                let format = out_mix.float();
                render
                    .Initialize(AUDCLNT_SHAREMODE_SHARED, CONVERT, 0, 0, &format, None)
                    .map_err(|_| Fault::NoMicrophone)?;
                render
            };
            let render_event = Event::new()?;
            render
                .SetEventHandle(render_event.0)
                .map_err(|_| Fault::NoMicrophone)?;
            let out_frames = render.GetBufferSize().map_err(|_| Fault::NoMicrophone)?;
            let render_client: IAudioRenderClient =
                render.GetService().map_err(|_| Fault::NoMicrophone)?;
            let out_rate = out_mix.rate;
            let latency = render.GetStreamLatency().unwrap_or(0).max(0) as u64
                * u64::from(out_rate)
                / 10_000_000
                + u64::from(out_frames);
            core.set_output(f64::from(out_rate), latency);

            let microphone = devices
                .GetDefaultAudioEndpoint(eCapture, role)
                .map_err(|error| fault(&error))?;
            let capture: IAudioClient = microphone
                .Activate(CLSCTX_ALL, None)
                .map_err(|error| fault(&error))?;
            facts.push(category(&capture, config.voice_processing).into());
            let in_mix = Mix::of(&capture).map_err(|error| fault(&error))?;
            let format = in_mix.float();
            capture
                .Initialize(
                    AUDCLNT_SHAREMODE_SHARED,
                    CONVERT,
                    CAPTURE_BUFFER,
                    0,
                    &format,
                    None,
                )
                .map_err(|error| fault(&error))?;
            facts.push(effects(&capture));
            if config.voice_processing {
                facts.push(
                    match capture.GetService::<IAcousticEchoCancellationControl>() {
                        Ok(control) => match control
                            .SetEchoCancellationRenderEndpoint(PCWSTR(speaker_wide.as_ptr()))
                        {
                            Ok(()) => "aec_render_endpoint=bound",
                            Err(_) => "aec_render_endpoint=refused",
                        },
                        Err(_) => "aec_render_endpoint=unavailable",
                    }
                    .into(),
                );
            }
            let capture_event = Event::new()?;
            capture
                .SetEventHandle(capture_event.0)
                .map_err(|error| fault(&error))?;
            let capture_client: IAudioCaptureClient =
                capture.GetService().map_err(|error| fault(&error))?;
            core.set_input(f64::from(in_mix.rate));

            if let Ok(silence) = render_client.GetBuffer(out_frames) {
                if !silence.is_null() {
                    let _ = render_client
                        .ReleaseBuffer(out_frames, AUDCLNT_BUFFERFLAGS_SILENT.0 as u32);
                }
            }
            render.Start().map_err(|_| Fault::NoMicrophone)?;
            capture.Start().map_err(|error| fault(&error))?;
            listener.note(&format!(
                "duplex started ms={} voice_processing={} in_rate={} in_channels={} out_rate={out_rate} out_channels={} period_frames={} buffer_frames={out_frames} latency_frames={latency}; {}",
                started.elapsed().as_millis(),
                config.voice_processing,
                in_mix.rate,
                in_mix.channels,
                out_mix.channels,
                period.map_or("default".into(), |p| p.to_string()),
                facts.join("; ")
            ));
            Ok(Self {
                capture,
                capture_client,
                capture_event,
                in_channels: usize::from(in_mix.channels),
                render,
                render_client,
                render_event,
                out_channels: usize::from(out_mix.channels),
                out_frames,
                scratch: Vec::with_capacity(8_192),
            })
        }
    }

    /// Drains captured packets and tops up the render buffer.
    fn service(&mut self, core: &Engine) -> windows::core::Result<()> {
        // SAFETY: the clients are live on this worker; each GetBuffer is
        // paired with ReleaseBuffer before the next, and the buffers hold the
        // frames the calls report in the float format they were opened with.
        unsafe {
            loop {
                if self.capture_client.GetNextPacketSize()? == 0 {
                    break;
                }
                let mut data = std::ptr::null_mut();
                let mut frames = 0_u32;
                let mut flags = 0_u32;
                self.capture_client
                    .GetBuffer(&mut data, &mut frames, &mut flags, None, None)?;
                let count = frames as usize * self.in_channels;
                self.scratch.clear();
                if flags & (AUDCLNT_BUFFERFLAGS_SILENT.0 as u32) != 0 || data.is_null() {
                    self.scratch.resize(count, 0.0);
                } else {
                    self.scratch
                        .extend_from_slice(std::slice::from_raw_parts(data.cast::<f32>(), count));
                }
                self.capture_client.ReleaseBuffer(frames)?;
                core.capture(&self.scratch, self.in_channels);
            }
            let free = self
                .out_frames
                .saturating_sub(self.render.GetCurrentPadding()?);
            if free > 0 {
                let data = self.render_client.GetBuffer(free)?;
                if !data.is_null() {
                    let out = std::slice::from_raw_parts_mut(
                        data.cast::<f32>(),
                        free as usize * self.out_channels,
                    );
                    core.render(out, self.out_channels);
                }
                self.render_client.ReleaseBuffer(free, 0)?;
            }
        }
        Ok(())
    }
}

impl Drop for Streams {
    fn drop(&mut self) {
        // SAFETY: stopping the clients this worker started.
        unsafe {
            let _ = self.capture.Stop();
            let _ = self.render.Stop();
        }
    }
}

/// Puts the capture stream in the communications category, or asks for the
/// raw signal, before it is initialized.
fn category(capture: &IAudioClient, voice_processing: bool) -> &'static str {
    let Ok(client) = capture.cast::<IAudioClient2>() else {
        return "category=unavailable";
    };
    let properties = |category, options| AudioClientProperties {
        cbSize: std::mem::size_of::<AudioClientProperties>() as u32,
        bIsOffload: false.into(),
        eCategory: category,
        Options: options,
    };
    // SAFETY: the properties outlive the calls reading them.
    unsafe {
        if voice_processing {
            match client.SetClientProperties(&properties(
                AudioCategory_Communications,
                AUDCLNT_STREAMOPTIONS_NONE,
            )) {
                Ok(()) => "category=communications",
                Err(_) => "category=communications_refused",
            }
        } else if client
            .SetClientProperties(&properties(AudioCategory_Other, AUDCLNT_STREAMOPTIONS_RAW))
            .is_ok()
        {
            "category=raw"
        } else {
            let _ = client
                .SetClientProperties(&properties(AudioCategory_Other, AUDCLNT_STREAMOPTIONS_NONE));
            "category=default"
        }
    }
}

/// The capture effects the endpoint reports for this stream.
fn effects(capture: &IAudioClient) -> String {
    // SAFETY: the effect list is read within its reported length and freed.
    unsafe {
        let Ok(manager) = capture.GetService::<IAudioEffectsManager>() else {
            return "effects=unavailable".into();
        };
        let mut list: *mut AUDIO_EFFECT = std::ptr::null_mut();
        let mut count = 0_u32;
        if manager.GetAudioEffects(&mut list, &mut count).is_err() {
            return "effects=unavailable".into();
        }
        let effects = if list.is_null() {
            &[][..]
        } else {
            std::slice::from_raw_parts(list, count as usize)
        };
        let mut named = Vec::new();
        let mut other = 0_usize;
        for effect in effects {
            let on = effect.state == AUDIO_EFFECT_STATE_ON;
            match EFFECTS.iter().find(|(id, _)| *id == effect.id) {
                Some((_, name)) => named.push(format!("{name}={}", if on { "on" } else { "off" })),
                None if on => other += 1,
                None => {}
            }
        }
        if !list.is_null() {
            CoTaskMemFree(Some(list.cast_const().cast()));
        }
        format!("effects count={count} {} other_on={other}", named.join(" "))
    }
}

struct Mmcss(HANDLE);

impl Mmcss {
    fn enter() -> Option<Self> {
        let mut index = 0_u32;
        // SAFETY: registers this thread; reverted in `Drop` on the same thread.
        unsafe { AvSetMmThreadCharacteristicsW(windows::core::w!("Pro Audio"), &mut index) }
            .ok()
            .map(Self)
    }
}

impl Drop for Mmcss {
    fn drop(&mut self) {
        // SAFETY: reverts the registration made in `enter`.
        let _ = unsafe { AvRevertMmThreadCharacteristics(self.0) };
    }
}

pub(crate) struct Device {
    shared: Arc<Shared>,
    worker: Option<JoinHandle<()>>,
}

impl Device {
    pub(crate) fn open(config: DuplexConfig, core: Arc<Engine>) -> Result<Self, Fault> {
        let shared = Arc::new(Shared {
            wake: Event::new()?,
            closing: AtomicBool::new(false),
            changed: AtomicBool::new(false),
        });
        let (ready, opened) = mpsc::sync_channel(1);
        let watch = shared.clone();
        let worker = std::thread::Builder::new()
            .name("zephium-voice-duplex".into())
            .spawn(move || run(config, &core, &watch, &ready))
            .map_err(|_| Fault::NoMicrophone)?;
        match opened.recv() {
            Ok(Ok(())) => Ok(Self {
                shared,
                worker: Some(worker),
            }),
            Ok(Err(error)) => {
                let _ = worker.join();
                Err(error)
            }
            Err(_) => {
                let _ = worker.join();
                Err(Fault::NoMicrophone)
            }
        }
    }
}

fn run(
    config: DuplexConfig,
    core: &Engine,
    shared: &Arc<Shared>,
    ready: &mpsc::SyncSender<Result<(), Fault>>,
) {
    let com = match Com::enter() {
        Ok(com) => com,
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };
    let mmcss = Mmcss::enter();
    // SAFETY: COM is initialized on this thread.
    let devices: IMMDeviceEnumerator =
        match unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) } {
            Ok(devices) => devices,
            Err(error) => {
                let _ = ready.send(Err(fault(&error)));
                return;
            }
        };
    let role = if config.voice_processing {
        eCommunications
    } else {
        eConsole
    };
    let mut streams = match Streams::open(config, core, &devices, role) {
        Ok(streams) => Some(streams),
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };
    let registration = Registration::new(&devices, role, shared);
    if registration.is_none() {
        core.listener().note("device notifications unavailable");
    }
    let _ = ready.send(Ok(()));
    let mut rebuilds: VecDeque<Instant> = VecDeque::with_capacity(CHURN + 1);
    while !shared.closing.load(Ordering::Acquire) {
        let mut rebuild = shared.changed.swap(false, Ordering::AcqRel);
        if let Some(live) = streams.as_mut() {
            let handles = [shared.wake.0, live.capture_event.0, live.render_event.0];
            // SAFETY: the handles stay open across the wait.
            unsafe { WaitForMultipleObjects(&handles, false, IDLE_WAIT_MS) };
            if shared.closing.load(Ordering::Acquire) {
                break;
            }
            rebuild |= shared.changed.swap(false, Ordering::AcqRel);
            rebuild |= live.service(core).is_err();
        }
        if !rebuild {
            continue;
        }
        drop(streams.take());
        std::thread::sleep(SETTLE);
        if shared.closing.load(Ordering::Acquire) {
            break;
        }
        shared.changed.store(false, Ordering::Release);
        let now = Instant::now();
        while rebuilds
            .front()
            .is_some_and(|at| now.duration_since(*at) > CHURN_WINDOW)
        {
            rebuilds.pop_front();
        }
        rebuilds.push_back(now);
        let rebuilt = if rebuilds.len() > CHURN {
            Err(Fault::NoMicrophone)
        } else {
            Streams::open(config, core, &devices, role)
        };
        match rebuilt {
            Ok(rebuilt) => {
                streams = Some(rebuilt);
                core.listener().route_changed();
            }
            Err(error) => {
                core.listener().lost(error);
                break;
            }
        }
    }
    drop(streams);
    drop(registration);
    drop(devices);
    drop(mmcss);
    drop(com);
}

impl Drop for Device {
    fn drop(&mut self) {
        self.shared.closing.store(true, Ordering::Release);
        self.shared.wake.raise();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
