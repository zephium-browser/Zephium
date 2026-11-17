//! Measures the duplex engine. Device runs need the microphone permission
//! already granted to the launching app; nothing is ever asked or recorded.
//!
//! ```text
//! cargo run -p zephium-voice --features file-probe --example duplex            # device runs
//! cargo run -p zephium-voice --features file-probe --example duplex -- --file speech.wav
//! ```

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use zephium_voice::{Duplex, DuplexConfig, DuplexListener, Fault, Permission, VOICE_RATE};

#[derive(Default)]
struct Probe {
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    first_frame: Option<Instant>,
    frames: Vec<(Instant, f32)>,
    /// 1 kHz amplitude per frame, to see a quiet tone above room noise.
    tones: Vec<(Instant, f32)>,
    silent_at: Option<Instant>,
    played: Vec<(Instant, u64)>,
    notes: Vec<String>,
}

impl DuplexListener for Probe {
    fn frame(&self, pcm: &[i16]) {
        let rms = (pcm.iter().map(|s| f32::from(*s).powi(2)).sum::<f32>() / pcm.len() as f32)
            .sqrt()
            / 32_768.0;
        let mut state = self.state.lock().unwrap();
        let now = Instant::now();
        state.first_frame.get_or_insert(now);
        state.frames.push((now, rms));
        state.tones.push((now, goertzel(pcm, 1_000.0)));
    }
    fn input_levels(&self, _levels: &[f32]) {}
    fn output_level(&self, level: f32) {
        if level == 0.0 {
            self.state
                .lock()
                .unwrap()
                .silent_at
                .get_or_insert(Instant::now());
        }
    }
    fn played(&self, mark: u64) {
        self.state
            .lock()
            .unwrap()
            .played
            .push((Instant::now(), mark));
    }
    fn route_changed(&self) {
        println!("route changed");
    }
    fn lost(&self, fault: Fault) {
        println!("lost: {fault:?}");
    }
    fn note(&self, fact: &str) {
        self.state.lock().unwrap().notes.push(fact.to_owned());
    }
}

fn tone(hz: f32, amplitude: f32, ms: u32) -> Vec<i16> {
    let count = VOICE_RATE * ms / 1_000;
    (0..count)
        .map(|n| {
            let t = n as f32 / VOICE_RATE as f32;
            (amplitude * (std::f32::consts::TAU * hz * t).sin() * 32_767.0) as i16
        })
        .collect()
}

/// Noise band-limited to roughly 300–3400 Hz, gated at a syllable rate (4 Hz)
/// so noise suppression does not treat it as stationary background.
fn speech_noise(amplitude: f32, ms: u32) -> Vec<i16> {
    let count = VOICE_RATE * ms / 1_000;
    let mut seed = 0x2545_f491_u32;
    let (mut low, mut high) = (0.0_f32, 0.0_f32);
    let a_low = 1.0 - (-std::f32::consts::TAU * 3_400.0 / VOICE_RATE as f32).exp();
    let a_high = 1.0 - (-std::f32::consts::TAU * 300.0 / VOICE_RATE as f32).exp();
    (0..count)
        .map(|n| {
            let syllable = (n * 4 / VOICE_RATE).is_multiple_of(2);
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            let white = seed as f32 / u32::MAX as f32 * 2.0 - 1.0;
            low += a_low * (white - low);
            high += a_high * (low - high);
            let gain = if syllable { amplitude * 3.0 } else { 0.0 };
            ((low - high) * gain * 32_767.0).clamp(-32_767.0, 32_767.0) as i16
        })
        .collect()
}

fn goertzel(pcm: &[i16], hz: f32) -> f32 {
    let w = std::f32::consts::TAU * hz / VOICE_RATE as f32;
    let coefficient = 2.0 * w.cos();
    let (mut s1, mut s2) = (0.0_f32, 0.0_f32);
    for sample in pcm {
        let s0 = f32::from(*sample) / 32_768.0 + coefficient * s1 - s2;
        s2 = s1;
        s1 = s0;
    }
    (s1 * s1 + s2 * s2 - coefficient * s1 * s2).max(0.0).sqrt() * 2.0 / pcm.len() as f32
}

fn rms_of(series: &[(Instant, f32)], from: Instant, to: Instant) -> f32 {
    let picked: Vec<f32> = series
        .iter()
        .filter(|(at, _)| *at >= from && *at < to)
        .map(|(_, rms)| *rms)
        .collect();
    if picked.is_empty() {
        return 0.0;
    }
    (picked.iter().map(|r| r * r).sum::<f32>() / picked.len() as f32).sqrt()
}

fn db(value: f32) -> f32 {
    20.0 * value.max(1e-9).log10()
}

fn open(voice_processing: bool) -> (Duplex, Arc<Probe>, Instant) {
    let probe = Arc::new(Probe::default());
    let started = Instant::now();
    let duplex = match Duplex::open(DuplexConfig { voice_processing }, probe.clone()) {
        Ok(duplex) => duplex,
        Err(fault) => {
            let notes = probe.state.lock().unwrap().notes.join("; ");
            panic!("open vp={voice_processing}: {fault:?} [{notes}]");
        }
    };
    duplex.listen(true);
    (duplex, probe, started)
}

fn cold_open(voice_processing: bool) {
    for run in 0..5 {
        let (duplex, probe, started) = open(voice_processing);
        let returned = started.elapsed();
        let deadline = Instant::now() + Duration::from_secs(3);
        while probe.state.lock().unwrap().first_frame.is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(1));
        }
        let first = probe
            .state
            .lock()
            .unwrap()
            .first_frame
            .map(|at| at - started);
        drop(duplex);
        let notes = probe.state.lock().unwrap().notes.join("; ");
        println!(
            "cold open vp={voice_processing} run={run} open_ms={:.1} first_frame_ms={} [{notes}]",
            returned.as_secs_f64() * 1e3,
            first.map_or("none".into(), |d| format!("{:.1}", d.as_secs_f64() * 1e3))
        );
        std::thread::sleep(Duration::from_millis(300));
    }
}

/// The engine is already running with the gate closed; time from opening the
/// gate to the first frame (the pre-roll) and to the first live frame.
fn gate(voice_processing: bool) {
    let probe = Arc::new(Probe::default());
    let duplex = Duplex::open(DuplexConfig { voice_processing }, probe.clone()).expect("open");
    for run in 0..5 {
        std::thread::sleep(Duration::from_millis(700));
        probe.state.lock().unwrap().frames.clear();
        let opened = Instant::now();
        duplex.listen(true);
        let deadline = opened + Duration::from_secs(1);
        while probe.state.lock().unwrap().frames.len() <= 10 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(1));
        }
        duplex.listen(false);
        let state = probe.state.lock().unwrap();
        let first = state.frames.first().map(|(at, _)| *at - opened);
        let live = state.frames.get(10).map(|(at, _)| *at - opened);
        let ms = |d: Option<Duration>| {
            d.map_or("none".into(), |d| format!("{:.1}", d.as_secs_f64() * 1e3))
        };
        println!(
            "gate vp={voice_processing} run={run} first_frame_ms={} first_live_frame_ms={}",
            ms(first),
            ms(live)
        );
    }
}

fn flush_latency() {
    let (duplex, probe, _) = open(false);
    std::thread::sleep(Duration::from_millis(400));
    let quiet_end = Instant::now();
    duplex.play(&tone(1_000.0, 0.9, 1_000));
    std::thread::sleep(Duration::from_millis(400));
    let flushed = Instant::now();
    duplex.flush();
    std::thread::sleep(Duration::from_millis(600));
    drop(duplex);
    let state = probe.state.lock().unwrap();
    let floor = rms_of(
        &state.tones,
        quiet_end - Duration::from_millis(300),
        quiet_end,
    );
    let playing = rms_of(&state.tones, flushed - Duration::from_millis(200), flushed);
    let onset = state
        .tones
        .iter()
        .filter(|(at, _)| *at > quiet_end)
        .find(|(_, level)| *level > floor * 2.0)
        .map(|(at, _)| *at - quiet_end);
    let settled = state
        .tones
        .iter()
        .filter(|(at, _)| *at > flushed)
        .find(|(_, rms)| *rms <= floor * 2.0)
        .map(|(at, _)| *at - flushed);
    println!(
        "flush output_level_zero_ms={} captured_floor_ms={} play_to_captured_onset_ms={} floor_1khz_db={:.1} tone_1khz_db={:.1}",
        state.silent_at.map_or("none".into(), |at| format!(
            "{:.1}",
            (at - flushed).as_secs_f64() * 1e3
        )),
        settled.map_or("none".into(), |d| format!("{:.1}", d.as_secs_f64() * 1e3)),
        onset.map_or("none".into(), |d| format!("{:.1}", d.as_secs_f64() * 1e3)),
        db(floor),
        db(playing)
    );
}

/// Captured level before and during 2 s of playback: broadband for speech
/// noise, at 1 kHz for a tone.
fn echo(voice_processing: bool, tone_probe: bool) -> (f32, f32) {
    let (duplex, probe, _) = open(voice_processing);
    std::thread::sleep(Duration::from_millis(500));
    let start = Instant::now();
    if tone_probe {
        duplex.play(&tone(1_000.0, 0.9, 2_000));
    } else {
        duplex.play(&speech_noise(0.3, 2_000));
    }
    std::thread::sleep(Duration::from_millis(2_100));
    drop(duplex);
    let state = probe.state.lock().unwrap();
    let series = if tone_probe {
        &state.tones
    } else {
        &state.frames
    };
    println!(
        "  echo vp={voice_processing} frames={} [{}]",
        series.len(),
        state.notes.join("; ")
    );
    let floor = rms_of(series, start - Duration::from_millis(300), start);
    let during = rms_of(
        series,
        start + Duration::from_millis(200),
        start + Duration::from_millis(1_900),
    );
    (floor, during)
}

/// Captured 1 kHz level in 250 ms windows across 4 s of tone, to see whether
/// the echo canceller converges.
fn converge(voice_processing: bool) {
    let (duplex, probe, _) = open(voice_processing);
    std::thread::sleep(Duration::from_millis(500));
    let start = Instant::now();
    duplex.play(&tone(1_000.0, 0.9, 4_000));
    std::thread::sleep(Duration::from_millis(4_300));
    drop(duplex);
    let state = probe.state.lock().unwrap();
    let windows: Vec<String> = (0..17)
        .map(|n| {
            let from = start + Duration::from_millis(250 * n);
            format!(
                "{:.0}",
                db(rms_of(
                    &state.tones,
                    from,
                    from + Duration::from_millis(250)
                ))
            )
        })
        .collect();
    println!(
        "converge vp={voice_processing} 1khz_db_per_250ms={}",
        windows.join(" ")
    );
}

/// A raw open right after a voice-processing engine closes.
fn switch() {
    for run in 0..5 {
        let (vp, _, _) = open(true);
        std::thread::sleep(Duration::from_millis(800));
        drop(vp);
        let (raw, probe, started) = open(false);
        std::thread::sleep(Duration::from_millis(1_500));
        drop(raw);
        let state = probe.state.lock().unwrap();
        let first = state.first_frame.map(|at| at - started);
        println!(
            "switch run={run} frames={} first_frame_ms={} [{}]",
            state.frames.len(),
            first.map_or("none".into(), |d| format!("{:.0}", d.as_secs_f64() * 1e3)),
            state.notes.join("; ")
        );
    }
}

/// Another app's 1 kHz tone, heard by a raw engine, before and while a
/// voice-processing engine is open: a drop means voice processing ducked it.
fn ducking(other: &str) {
    let (raw, probe, _) = open(false);
    std::thread::sleep(Duration::from_millis(300));
    let mut player = std::process::Command::new("/usr/bin/afplay")
        .arg(other)
        .spawn()
        .expect("afplay");
    let start = Instant::now();
    std::thread::sleep(Duration::from_millis(1_300));
    let (vp, vp_probe, _) = open(true);
    let vp_open = Instant::now();
    std::thread::sleep(Duration::from_millis(1_700));
    drop(vp);
    let _ = player.wait();
    drop(raw);
    let state = probe.state.lock().unwrap();
    let vp_state = vp_probe.state.lock().unwrap();
    let before = rms_of(
        &state.tones,
        start + Duration::from_millis(300),
        start + Duration::from_millis(1_200),
    );
    let during = rms_of(
        &state.tones,
        vp_open + Duration::from_millis(500),
        vp_open + Duration::from_millis(1_600),
    );
    println!(
        "ducking other_1khz_before_db={:.1} with_vp_db={:.1} change_db={:.1} heard_by_vp_db={:.1} raw_frames={} vp_frames={} [{}]",
        db(before),
        db(during),
        db(during) - db(before),
        db(rms_of(
            &vp_state.tones,
            vp_open + Duration::from_millis(500),
            vp_open + Duration::from_millis(1_600)
        )),
        state.frames.len(),
        vp_state.frames.len(),
        state.notes.join("; ")
    );
}

fn file(path: &str) {
    let probe = Arc::new(Probe::default());
    let duplex = Duplex::open_file(std::path::Path::new(path), probe.clone()).expect("file");
    duplex.listen(true);
    std::thread::sleep(Duration::from_millis(500));
    duplex.play(&tone(1_000.0, 0.1, 1_000));
    duplex.mark(1);
    std::thread::sleep(Duration::from_millis(300));
    let flushed = Instant::now();
    duplex.flush();
    std::thread::sleep(Duration::from_millis(300));
    duplex.play(&tone(500.0, 0.1, 100));
    duplex.mark(2);
    std::thread::sleep(Duration::from_millis(400));
    drop(duplex);
    let state = probe.state.lock().unwrap();
    println!(
        "file frames={} notes={:?} flush_to_zero_ms={} played={:?}",
        state.frames.len(),
        state.notes,
        state.silent_at.map_or("none".into(), |at| format!(
            "{:.1}",
            (at - flushed).as_secs_f64() * 1e3
        )),
        state
            .played
            .iter()
            .map(|(_, mark)| *mark)
            .collect::<Vec<_>>()
    );
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--file") {
        file(args.get(1).expect("a WAV file"));
        return;
    }
    let permission = zephium_voice::microphone_permission();
    println!("microphone: {permission:?}");
    if permission != Permission::Granted {
        println!("device runs skipped: the microphone permission is not granted");
        return;
    }
    match args.first().map(String::as_str) {
        Some("--vp") => return cold_open(true),
        Some("--raw") => return cold_open(false),
        Some("--switch") => return switch(),
        Some("--flush") => return flush_latency(),
        Some("--ducking") => return ducking(args.get(1).expect("a WAV for the other app")),
        Some("--converge") => {
            converge(false);
            std::thread::sleep(Duration::from_millis(500));
            return converge(true);
        }
        Some("--gate") => {
            gate(false);
            return gate(true);
        }
        _ => {}
    }
    if args.first().map(String::as_str) != Some("--acoustic") {
        cold_open(false);
        cold_open(true);
    }
    flush_latency();
    for (name, tone_probe) in [("speech_noise", false), ("tone_1khz", true)] {
        let (floor_off, off) = echo(false, tone_probe);
        let (floor_on, on) = echo(true, tone_probe);
        println!(
            "echo {name} off_db={:.1} (floor {:.1}) on_db={:.1} (floor {:.1}) residue_db={:.1}",
            db(off),
            db(floor_off),
            db(on),
            db(floor_on),
            db(on) - db(off)
        );
    }
}
