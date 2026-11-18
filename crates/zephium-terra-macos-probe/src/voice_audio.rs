//! A device-free AudioPort. The clock advances in sample time; the adapter
//! pumps it from a monotonic clock. Capture, pre-roll and playback work in
//! standby exactly at the port boundary, without opening any OS audio device.
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::voice_metrics::{Kind, Metrics, Provenance};
use serde_json::json;
use zephium_work_composition::page_assist::core::voice::Fault;
use zephium_work_composition::voice_session::port::{
    AudioInbox, AudioPort, BoxFuture, Frame, FRAME,
};

const RATE: u64 = 24_000;
const PREROLL: usize = 10;
const START_BUFFER: usize = 1440;
const LIMIT: usize = RATE as usize * 180;

#[derive(Default)]
struct Tick {
    frames: Vec<Frame>,
    marks: Vec<u64>,
    first: Vec<(u64, u64, Provenance)>,
    ends: Vec<(u64, u64, Provenance)>,
    spans: Vec<(u64, u64, u64, Provenance)>,
    input_level: Option<f32>,
    output_level: f32,
    played: u64,
    stale: u64,
}

struct Block {
    pcm: Vec<i16>,
    offset: usize,
    group: u64,
    stale: bool,
    source: Provenance,
}

#[derive(Default)]
struct AudioClock {
    capturing: bool,
    gate: bool,
    was_gated: bool,
    next_frame_us: u64,
    preroll: VecDeque<Frame>,
    script: Vec<i16>,
    cursor: usize,
    delivered: bool,
    queue: VecDeque<Block>,
    queued: usize,
    enqueued: u64,
    consumed: u64,
    marks: VecDeque<(u64, u64, u64)>,
    group: u64,
    begun: Vec<u64>,
    last_sample: BTreeMap<u64, u64>,
    last_source: BTreeMap<u64, Provenance>,
    playing: bool,
    last_us: u64,
    remainder: u64,
}

impl AudioClock {
    fn start(&mut self, now: u64) {
        if !self.capturing {
            self.capturing = true;
            self.next_frame_us = now + 20_000;
            self.preroll.clear();
            self.was_gated = false;
        }
    }
    fn standby(&mut self) {
        self.capturing = false;
        self.was_gated = false;
        self.preroll.clear();
    }
    fn arm(&mut self, pcm: Vec<i16>) {
        self.script = pcm;
        self.cursor = 0;
        self.delivered = false;
    }
    fn play(&mut self, pcm: &[i16], stale: bool, source: Provenance) -> bool {
        if pcm.len() > LIMIT.saturating_sub(self.queued) {
            return false;
        }
        if !pcm.is_empty() {
            self.queue.push_back(Block {
                pcm: pcm.to_vec(),
                offset: 0,
                group: self.group,
                stale,
                source,
            });
            self.queued += pcm.len();
            self.enqueued += pcm.len() as u64;
        }
        true
    }
    fn mark(&mut self, mark: u64) {
        self.marks.push_back((self.enqueued, mark, self.group));
        self.group += 1;
    }
    fn flush(&mut self) {
        self.queue.clear();
        self.queued = 0;
        self.marks.clear();
        self.enqueued = self.consumed;
        self.playing = false;
        self.group += 1;
    }
    fn tick(&mut self, now: u64) -> Tick {
        let mut out = Tick::default();
        while self.capturing && self.next_frame_us <= now {
            self.next_frame_us += 20_000;
            if self.gate && !self.was_gated {
                out.frames.extend(self.preroll.drain(..));
            }
            self.was_gated = self.gate;
            let mut frame = [0; FRAME];
            if self.gate && self.cursor < self.script.len() {
                let end = (self.cursor + FRAME).min(self.script.len());
                frame[..end - self.cursor].copy_from_slice(&self.script[self.cursor..end]);
                self.cursor = end;
                self.delivered = end == self.script.len();
            }
            if self.gate {
                out.input_level = Some(level(&frame));
                out.frames.push(frame);
            } else {
                if self.preroll.len() == PREROLL {
                    self.preroll.pop_front();
                }
                self.preroll.push_back(frame);
            }
        }
        let elapsed = now.saturating_sub(self.last_us);
        let samples = elapsed * RATE + self.remainder;
        let mut budget = samples / 1_000_000;
        self.remainder = samples % 1_000_000;
        let mut at = self.last_us;
        self.last_us = now;
        if !self.playing
            && (self.queued >= START_BUFFER || (self.queued > 0 && !self.marks.is_empty()))
        {
            self.playing = true;
        }
        while self.playing && budget > 0 {
            let Some(block) = self.queue.front_mut() else {
                self.playing = false;
                break;
            };
            if !self.begun.contains(&block.group) {
                self.begun.push(block.group);
                out.first.push((block.group, at, block.source));
            }
            let count = (block.pcm.len() - block.offset).min(budget as usize);
            out.output_level = level(&block.pcm[block.offset..block.offset + count]);
            out.spans
                .push((block.group, at, count as u64, block.source));
            self.last_source.insert(block.group, block.source);
            block.offset += count;
            budget -= count as u64;
            self.queued -= count;
            self.consumed += count as u64;
            out.played += count as u64;
            if block.stale {
                out.stale += count as u64;
            }
            at += count as u64 * 1_000_000 / RATE;
            self.last_sample.insert(block.group, at);
            if block.offset == block.pcm.len() {
                self.queue.pop_front();
            }
            while self
                .marks
                .front()
                .is_some_and(|(samples, _, _)| *samples <= self.consumed)
            {
                if let Some((_, mark, group)) = self.marks.pop_front() {
                    out.marks.push(mark);
                    out.ends.push((
                        group,
                        at,
                        self.last_source.get(&group).copied().unwrap_or_default(),
                    ));
                }
            }
        }
        // A mark on an empty queue still settles, as a real render callback does.
        while self
            .marks
            .front()
            .is_some_and(|(samples, _, _)| *samples <= self.consumed)
        {
            if let Some((_, mark, group)) = self.marks.pop_front() {
                out.marks.push(mark);
                out.ends.push((
                    group,
                    self.last_sample.get(&group).copied().unwrap_or(now),
                    self.last_source.get(&group).copied().unwrap_or_default(),
                ));
            }
        }
        if self.queue.is_empty() {
            self.playing = false;
            out.output_level = 0.0;
        }
        out
    }
}

fn level(pcm: &[i16]) -> f32 {
    pcm.iter()
        .map(|s| f32::from(*s).abs() / 32768.0)
        .fold(0.0, f32::max)
}

struct Source {
    id: String,
    pcm: Vec<i16>,
    obsolete: bool,
    expected_fact: bool,
    source: Provenance,
}
#[derive(Default)]
struct State {
    clock: AudioClock,
    inbox: Option<Arc<AudioInbox>>,
    sources: Vec<Source>,
    obsolete_ids: BTreeSet<String>,
    obsolete_replies: BTreeSet<u32>,
    source_samples: usize,
    source_overflow: bool,
    released: bool,
}

pub(super) struct ScriptedAudio {
    state: Mutex<State>,
    metrics: Arc<Metrics>,
}

impl ScriptedAudio {
    pub fn new(metrics: Arc<Metrics>) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(State::default()),
            metrics,
        })
    }
    fn with<T>(&self, f: impl FnOnce(&mut State) -> T) -> T {
        f(&mut self.state.lock().unwrap_or_else(|p| p.into_inner()))
    }
    fn now_us(&self) -> u64 {
        self.metrics
            .origin
            .elapsed()
            .as_micros()
            .try_into()
            .unwrap_or(u64::MAX)
    }
    pub fn arm(&self, pcm: Vec<i16>) {
        self.with(|s| s.clock.arm(pcm));
    }
    pub fn delivered(&self) -> bool {
        self.with(|s| s.clock.delivered)
    }
    pub fn playing_answer(&self) -> bool {
        self.pump();
        self.with(|s| {
            s.clock.playing
                && s.clock
                    .queue
                    .front()
                    .is_some_and(|b| b.source.kind == Kind::Answer)
                && s.clock.queued > 0
        })
    }
    pub fn idle(&self) -> bool {
        self.with(|s| s.clock.queued == 0 && s.clock.marks.is_empty())
    }
    pub fn sentence(&self, id: &str, expected_fact: bool, source: Provenance) {
        self.with(|s| {
            s.sources.push(Source {
                id: id.to_owned(),
                pcm: Vec::new(),
                obsolete: source
                    .reply
                    .is_some_and(|r| s.obsolete_replies.contains(&r)),
                expected_fact,
                source,
            });
        });
    }
    pub fn source(&self, id: &str, pcm: &[u8]) {
        self.with(|s| {
            if s.source_samples + pcm.len() / 2 > LIMIT {
                s.source_overflow = true;
                return;
            }
            let at = match s.sources.iter().position(|source| source.id == id) {
                Some(at) => at,
                None => {
                    s.sources.push(Source {
                        id: id.to_owned(),
                        pcm: Vec::new(),
                        obsolete: s.obsolete_ids.contains(id),
                        expected_fact: false,
                        source: Provenance::default(),
                    });
                    s.sources.len() - 1
                }
            };
            if !pcm.is_empty() && s.sources[at].source.received.is_none() {
                s.sources[at].source.received = Some(self.metrics.now());
            }
            let source = s.sources[at].source;
            if !pcm.is_empty() && source.kind == Kind::Answer && source.sent.is_some() {
                self.metrics.with(|f| {
                    if f.received_answer_source.is_none() {
                        f.received_answer_source = Some(source);
                        if let Some(received) = source.received {
                            f.times.insert("first_answer_audio_received", received);
                        }
                        if let Some(sent) = source.sent {
                            f.times.insert("answer_speak_sent", sent);
                        }
                    }
                });
            }
            s.sources[at].pcm.extend(
                pcm.chunks_exact(2)
                    .map(|p| i16::from_le_bytes([p[0], p[1]])),
            );
            s.source_samples += pcm.len() / 2;
        });
    }
    pub fn obsolete(&self, ids: &[String]) {
        self.with(|s| {
            s.obsolete_ids.extend(ids.iter().cloned());
            for source in &mut s.sources {
                source.obsolete |= ids.contains(&source.id);
            }
        });
    }
    pub fn spawn(self: &Arc<Self>) -> tokio::task::JoinHandle<()> {
        let audio = self.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(1));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                interval.tick().await;
                if audio.with(|s| s.released) {
                    break;
                }
                audio.pump();
            }
        })
    }
    fn pump(&self) {
        self.with(|s| {
            let tick = s.clock.tick(self.now_us());
            if let Some(inbox) = &s.inbox {
                for frame in &tick.frames {
                    inbox.frame(frame);
                }
                if let Some(level) = tick.input_level {
                    inbox.input_level(level);
                }
                inbox.output_level(tick.output_level);
                for mark in tick.marks {
                    inbox.played(mark);
                }
            }
            self.metrics.with(|f| {
                *f.counts.entry("played_samples").or_default() += tick.played;
                if f.times.contains_key("barge_flush") { f.old_audio_after_flush += tick.stale; }
                for (group, at, source) in tick.first {
                    *f.counts.entry("sentences_spoken").or_default() += 1;
                    f.times.entry("first_audio_played").or_insert(at / 1000);
                    f.playback
                        .push(json!({"speech": group, "kind": source.kind.name(), "reply": source.reply, "first_sample_ms": at/1000}));
                }
                for (group, at, samples, source) in tick.spans {
                    let played_ms = at / 1000;
                    if source.kind == Kind::Line {
                        f.times.entry("first_line_played").or_insert(played_ms);
                    } else if source.sent.is_some() && source.received.is_some() {
                        if f.answer_source.is_none() {
                            f.answer_source = Some(source);
                            f.times.insert("first_answer_audio_played", played_ms);
                            if let Some(sent) = source.sent { f.times.insert("answer_speak_sent", sent); }
                            if let Some(received) = source.received { f.times.insert("first_answer_audio_received", received); }
                        }
                        if source.reply.and_then(|r| f.reply_utterances.get(&r)).is_some_and(|i| *i == 1) {
                            *f.counts.entry("second_answer_samples").or_default() += samples;
                        }
                    }
                    if let Some(final_at) = f.times.get("second_final_transcript") {
                        let before = final_at.saturating_mul(1000).saturating_sub(at).saturating_mul(RATE).div_ceil(1_000_000).min(samples);
                        let after = samples - before;
                        if after > 0 {
                            match source.kind {
                                Kind::Answer => f.after_stop_samples += after,
                                Kind::Line => { f.after_stop_lines.insert(group); }
                            }
                        }
                    }
                }
                for (group, at, source) in tick.ends {
                    f.times.insert("speech_end", at / 1000);
                    f.playback.push(json!({"speech": group, "kind": source.kind.name(), "reply": source.reply, "end_ms": at/1000}));
                }
            });
        });
    }
}

impl AudioPort for ScriptedAudio {
    fn prepare(&self, inbox: Arc<AudioInbox>) -> BoxFuture<'static, Result<(), Fault>> {
        self.with(|s| {
            s.inbox = Some(inbox);
            s.released = false;
            s.clock.last_us = self.now_us();
        });
        Box::pin(async { Ok(()) })
    }
    fn release(&self) {
        self.with(|s| {
            s.clock.standby();
            s.clock.flush();
            s.inbox = None;
            s.released = true;
        });
    }
    fn start(&self) -> BoxFuture<'static, Result<(), Fault>> {
        self.with(|s| s.clock.start(self.now_us()));
        Box::pin(async { Ok(()) })
    }
    fn standby(&self) {
        self.pump();
        self.with(|s| s.clock.standby());
    }
    fn gate(&self, on: bool) {
        self.pump();
        self.with(|s| s.clock.gate = on);
    }
    fn play(&self, pcm: &[i16]) {
        self.pump();
        self.with(|s| {
            let matches: Vec<_> = s
                .sources
                .iter()
                .filter(|source| source.pcm.windows(pcm.len().max(1)).any(|p| p == pcm))
                .collect();
            let source = matches.first().map(|s| s.source).unwrap_or_default();
            if matches.iter().any(|s| s.source != source) {
                self.metrics.count("ambiguous_provenance_blocks", 1);
            }
            // Ambiguous repeated audio is not evidence of a fresh source.
            let stale = !matches.is_empty() && matches.iter().all(|source| source.obsolete);
            if matches.iter().any(|source| source.obsolete) && !stale {
                self.metrics.count("ambiguous_provenance_blocks", 1);
            }
            if matches.is_empty() {
                self.metrics.count("unattributed_play_blocks", 1);
            }
            if matches
                .iter()
                .any(|source| source.expected_fact && !source.obsolete)
            {
                self.metrics.count("fact_audio_enqueued", 1);
            }
            if s.source_overflow {
                self.metrics.count("provenance_overflow", 1);
            }
            if !s.clock.play(pcm, stale, source) {
                self.metrics.count("playback_overflow", 1);
            }
        });
    }
    fn mark(&self, mark: u64) {
        self.pump();
        self.with(|s| s.clock.mark(mark));
    }
    fn flush(&self) {
        self.pump();
        let barge = self
            .metrics
            .with(|f| f.times.contains_key("barge_key_down"));
        self.with(|s| {
            s.clock.flush();
            if barge {
                for source in &mut s.sources {
                    if source.source.kind == Kind::Answer {
                        source.obsolete = true;
                        if let Some(reply) = source.source.reply {
                            s.obsolete_replies.insert(reply);
                        }
                    }
                }
            }
            if let Some(inbox) = &s.inbox {
                inbox.output_level(0.0);
            }
        });
        self.metrics.with(|f| {
            if f.times.contains_key("barge_key_down") {
                f.times
                    .entry("barge_flush")
                    .or_insert_with(|| self.metrics.now());
            }
        });
        self.metrics.count("flushes", 1);
    }
}

/// Remove synthesis padding and emulate a human press. No PCM leaves memory.
pub(super) fn utterance(pcm: Vec<i16>) -> Option<Vec<i16>> {
    let start = pcm.iter().position(|s| s.unsigned_abs() > 64)?;
    let end = pcm.iter().rposition(|s| s.unsigned_abs() > 64)? + 1;
    let mut trimmed = vec![0; 3600]; // 150 ms before the first word
    trimmed.extend_from_slice(&pcm[start..end]);
    trimmed.resize(trimmed.len() + 2400, 0); // 100 ms after the last word
    trimmed.resize(trimmed.len().div_ceil(FRAME) * FRAME, 0);
    Some(trimmed)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn voice_capture_clock_delivers_whole_frames_at_real_time_with_preroll() {
        let mut clock = AudioClock::default();
        clock.arm(vec![123; FRAME * 3]);
        clock.start(0);
        assert!(clock.tick(219_999).frames.is_empty());
        clock.gate = true;
        let first = clock.tick(220_000);
        assert_eq!(first.frames.len(), 11);
        assert!(first.frames[..10].iter().flatten().all(|s| *s == 0));
        assert_eq!(first.frames[10], [123; FRAME]);
        assert!(clock.tick(239_999).frames.is_empty());
        assert_eq!(clock.tick(240_000).frames.len(), 1);
        assert!(!clock.delivered);
        assert_eq!(clock.tick(260_000).frames.len(), 1);
        assert!(clock.delivered);
    }
    #[test]
    fn voice_playout_clock_buffers_and_reports_marks_in_order() {
        let mut clock = AudioClock::default();
        assert!(clock.play(&vec![100; FRAME], false, Provenance::default()));
        assert_eq!(clock.tick(100_000).played, 0);
        clock.mark(1);
        clock.play(&vec![100; FRAME], false, Provenance::default());
        clock.mark(2);
        assert!(clock.tick(119_000).marks.is_empty());
        assert_eq!(clock.tick(120_000).marks, [1]);
        assert_eq!(clock.tick(140_000).marks, [2]);
        assert!(clock.tick(150_000).marks.is_empty());
    }
    #[test]
    fn voice_flush_stops_playout_and_removes_pending_marks() {
        let mut clock = AudioClock::default();
        clock.play(&vec![100; 24000], false, Provenance::default());
        clock.mark(1);
        assert_eq!(clock.tick(20_000).played, 480);
        clock.flush();
        let after = clock.tick(1_020_000);
        assert_eq!(after.played, 0);
        assert!(after.marks.is_empty());
        clock.play(&vec![100; FRAME], false, Provenance::default());
        clock.mark(2);
        assert_eq!(clock.tick(1_040_000).marks, [2]);
    }
    #[test]
    fn voice_standby_stops_capture_but_plays_speech_and_restart_has_fresh_preroll() {
        let mut clock = AudioClock::default();
        clock.arm(vec![123; FRAME]);
        clock.start(0);
        clock.tick(200_000);
        clock.standby();
        clock.play(&vec![100; FRAME], false, Provenance::default());
        clock.mark(9);
        assert_eq!(clock.tick(220_000).marks, [9]);
        assert!(clock.tick(300_000).frames.is_empty());
        clock.start(300_000);
        clock.gate = true;
        assert_eq!(clock.tick(320_000).frames, vec![[123; FRAME]]);
    }
    #[test]
    fn voice_levels_retain_the_last_capture_value_between_frames() {
        let mut clock = AudioClock::default();
        clock.arm(vec![8000; FRAME * 2]);
        clock.start(0);
        clock.gate = true;
        assert!(clock.tick(20_000).input_level.unwrap() > 0.0);
        assert_eq!(clock.tick(21_000).input_level, None);
        assert!(clock.tick(40_000).input_level.unwrap() > 0.0);
    }
    #[test]
    fn voice_late_old_audio_is_detected_after_flush() {
        let metrics = Metrics::new(std::time::Instant::now());
        let audio = ScriptedAudio::new(metrics);
        let pcm = vec![1234_i16; 1440];
        let bytes: Vec<u8> = pcm.iter().flat_map(|s| s.to_le_bytes()).collect();
        audio.sentence("first", false, Provenance::default());
        audio.source("first", &bytes);
        audio.play(&pcm);
        audio.flush();
        audio.obsolete(&["first".to_owned()]);
        audio.source("first", &bytes);
        audio.play(&pcm);
        audio.with(|s| {
            let played = s.clock.tick(s.clock.last_us + 20_000);
            assert_eq!(played.stale, 480);
        });
    }
    #[test]
    fn voice_a_mark_after_an_underrun_reports_the_actual_last_sample_time() {
        let mut clock = AudioClock::default();
        clock.play(&vec![1234; 1440], false, Provenance::default());
        assert_eq!(clock.tick(100_000).played, 1440);
        clock.mark(1);
        let settled = clock.tick(200_000);
        assert_eq!(settled.marks, [1]);
        assert_eq!(settled.ends, [(0, 60_000, Provenance::default())]);
    }
    #[test]
    fn voice_cached_line_and_answer_keep_kind_and_timestamps_through_clock() {
        let metrics = Metrics::new(std::time::Instant::now());
        let audio = ScriptedAudio::new(metrics.clone());
        let line = Provenance {
            kind: Kind::Line,
            sent: Some(1),
            received: Some(2),
            reply: None,
        };
        let answer = Provenance {
            kind: Kind::Answer,
            sent: Some(5),
            received: Some(6),
            reply: Some(9),
        };
        for (id, value, source) in [("warm", 111_i16, line), ("reply", 222_i16, answer)] {
            audio.sentence(id, false, source);
            audio.source(id, &value.to_le_bytes().repeat(1440));
        }
        for (value, source) in [(111, line), (222, answer), (111, line)] {
            audio.play(&vec![value; 1440]);
            // Advance deterministically after admission, without a wall-clock
            // pump between enqueue and assertion (which could start playback).
            audio.with(|s| {
                assert_eq!(s.clock.queue.back().unwrap().source, source);
                s.clock.mark(1);
                let tick = s.clock.tick(s.clock.last_us + 100_000);
                assert_eq!(tick.first[0].2, source);
                assert_eq!(tick.ends[0].2, source);
            });
        }
    }
    #[test]
    fn voice_padding_preserves_first_and_last_word_samples() {
        let padded = utterance([vec![0; 9000], vec![123; 777], vec![0; 9000]].concat()).unwrap();
        assert_eq!(&padded[3600..4377], &[123; 777]);
        assert!(padded[..3600].iter().all(|s| *s == 0));
        assert!(padded[4377..].iter().all(|s| *s == 0));
        assert!(padded.len().is_multiple_of(FRAME));
        assert!(utterance(vec![0; 24000]).is_none());
    }
}
