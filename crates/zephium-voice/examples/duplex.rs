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

/// Noise band-limited to roughly 300–3400 Hz.
fn speech_noise(amplitude: f32, ms: u32) -> Vec<i16> {
    let count = VOICE_RATE * ms / 1_000;
    let mut seed = 0x2545_f491_u32;
    let (mut low, mut high) = (0.0_f32, 0.0_f32);
    let a_low = 1.0 - (-std::f32::consts::TAU * 3_400.0 / VOICE_RATE as f32).exp();
    let a_high = 1.0 - (-std::f32::consts::TAU * 300.0 / VOICE_RATE as f32).exp();
    (0..count)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            let white = seed as f32 / u32::MAX as f32 * 2.0 - 1.0;
            low += a_low * (white - low);
            high += a_high * (low - high);
            ((low - high) * amplitude * 3.0 * 32_767.0).clamp(-32_767.0, 32_767.0) as i16
        })
        .collect()
}

fn rms_between(state: &State, from: Instant, to: Instant) -> f32 {
    let picked: Vec<f32> = state
        .frames
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
    let duplex = Duplex::open(DuplexConfig { voice_processing }, probe.clone()).expect("open");
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

fn flush_latency() {
    let (duplex, probe, _) = open(false);
    std::thread::sleep(Duration::from_millis(400));
    let quiet_end = Instant::now();
    duplex.play(&tone(1_000.0, 0.03, 1_000));
    std::thread::sleep(Duration::from_millis(400));
    let flushed = Instant::now();
    duplex.flush();
    std::thread::sleep(Duration::from_millis(600));
    drop(duplex);
    let state = probe.state.lock().unwrap();
    let floor = rms_between(&state, quiet_end - Duration::from_millis(300), quiet_end);
    let playing = rms_between(&state, flushed - Duration::from_millis(200), flushed);
    let settled = state
        .frames
        .iter()
        .filter(|(at, _)| *at > flushed)
        .find(|(_, rms)| *rms <= floor * 2.0)
        .map(|(at, _)| *at - flushed);
    println!(
        "flush output_level_zero_ms={} captured_floor_ms={} floor_db={:.1} tone_db={:.1}",
        state.silent_at.map_or("none".into(), |at| format!(
            "{:.1}",
            (at - flushed).as_secs_f64() * 1e3
        )),
        settled.map_or("none".into(), |d| format!("{:.1}", d.as_secs_f64() * 1e3)),
        db(floor),
        db(playing)
    );
}

fn echo(voice_processing: bool) -> (f32, f32) {
    let (duplex, probe, _) = open(voice_processing);
    std::thread::sleep(Duration::from_millis(500));
    let start = Instant::now();
    duplex.play(&speech_noise(0.02, 2_000));
    std::thread::sleep(Duration::from_millis(2_100));
    drop(duplex);
    let state = probe.state.lock().unwrap();
    let floor = rms_between(&state, start - Duration::from_millis(300), start);
    let during = rms_between(
        &state,
        start + Duration::from_millis(200),
        start + Duration::from_millis(1_900),
    );
    (floor, during)
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
    cold_open(false);
    cold_open(true);
    flush_latency();
    let (floor_off, off) = echo(false);
    let (floor_on, on) = echo(true);
    println!(
        "echo off_db={:.1} (floor {:.1}) on_db={:.1} (floor {:.1}) residue_db={:.1}",
        db(off),
        db(floor_off),
        db(on),
        db(floor_on),
        db(on) - db(off)
    );
}
