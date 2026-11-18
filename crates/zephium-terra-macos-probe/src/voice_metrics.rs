//! Content-free monotonic observations. Private partials stay in memory only;
//! result projection has no transcript, reply text, or PCM fields.
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use zephium_work_composition::page_assist::core::voice::phrases::normalize;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Kind {
    Line,
    #[default]
    Answer,
}
impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Line => "line",
            Self::Answer => "answer",
        }
    }
}

/// Identity and timestamps follow the PCM, including cached lines.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Provenance {
    pub kind: Kind,
    pub sent: Option<u64>,
    pub received: Option<u64>,
    pub reply: Option<u32>,
}

#[derive(Default)]
pub(super) struct Utterance {
    pub key_up: Option<u64>,
    pub final_at: Option<u64>,
    partials: usize,
    last_partial: Option<(u64, String)>,
    at_keyup: Option<(u64, String)>,
    late: Vec<(u64, String)>,
    late_exact: Option<String>,
    late_before_final: Vec<u64>,
    last_partial_exact: Option<bool>,
    full_before_final: Option<u64>,
    final_summary: Option<(usize, usize, Option<bool>, Option<i64>)>,
    early: Early,
}

/// Replies started ahead of the final for one utterance. The started words
/// stay in memory only until the final is compared.
#[derive(Default)]
struct Early {
    starts: u32,
    discards: u32,
    released: u32,
    latest: Option<(u64, String)>,
    released_at: Option<u64>,
    matched: Option<&'static str>,
}
impl Utterance {
    fn partial(&mut self, at: u64, text: &str) {
        self.partials += 1;
        if self.key_up.is_none() {
            self.last_partial = Some((at, normalize(text)));
        } else if self.late.len() < 64 {
            self.late.push((at, normalize(text)));
            self.late_exact = Some(text.trim().to_owned());
        }
    }
    fn keyup(&mut self, at: u64) {
        self.key_up = Some(at);
        self.at_keyup = self
            .last_partial
            .take()
            .filter(|(arrived, _)| *arrived <= at);
    }
    fn finish(&mut self, at: u64, text: &str, expected: &[&str]) {
        self.final_at = Some(at);
        let (missing, words) = transcript(text, expected);
        let final_text = normalize(text);
        self.final_summary = Some((
            missing,
            words,
            self.at_keyup.as_ref().map(|(_, p)| *p == final_text),
            self.at_keyup.as_ref().map(|(_, p)| {
                final_text.split_whitespace().count() as i64 - p.split_whitespace().count() as i64
            }),
        ));
        if let Some((_, started)) = &mut self.early.latest {
            self.early.matched = Some(if started.trim() == text.trim() {
                "exact"
            } else if normalize(started) == final_text {
                "folded"
            } else {
                "differs"
            });
            started.clear();
        }
        self.late_before_final = self
            .late
            .iter()
            .map(|(arrived, _)| at.saturating_sub(*arrived))
            .collect();
        self.last_partial_exact = self.late_exact.take().map(|partial| partial == text.trim());
        // How long before the final the whole transcript was already known.
        self.full_before_final = self
            .at_keyup
            .iter()
            .chain(&self.late)
            .find(|(_, partial)| *partial == final_text)
            .map(|(arrived, _)| at.saturating_sub(*arrived));
        self.late.clear();
        // Once compared, even the private normalized partial is unnecessary.
        if let Some((_, text)) = &mut self.at_keyup {
            text.clear();
        }
        self.last_partial = None;
    }
    fn result(&self) -> Option<Value> {
        let (missing, words, matches, missing_words) = self.final_summary?;
        Some(json!({"at_ms": self.final_at, "key_up_ms": self.key_up,
            "expected_words": missing == 0, "expected_missing": missing, "word_count": words,
            "partials": self.partials,
            "last_partial_before_keyup_ms": self.at_keyup.as_ref().and_then(|(at, _)| self.key_up?.checked_sub(*at)),
            "partial_at_keyup_matches_final": matches, "partial_words_missing": missing_words,
            "late_partials_before_final_ms": self.late_before_final,
            "last_partial_exact": self.last_partial_exact,
            "full_partial_before_final_ms": self.full_before_final,
            "early_starts": self.early.starts, "early_discards": self.early.discards,
            "early_released": self.early.released,
            "early_start_to_final_ms": between(self.final_at, self.early.released_at),
            "early_text_matched": self.early.matched}))
    }
}

#[derive(Default)]
pub(super) struct Facts {
    pub times: BTreeMap<&'static str, u64>,
    pub counts: BTreeMap<&'static str, u64>,
    pub faults: BTreeMap<String, u64>,
    pub states: Vec<String>,
    pub utterances: Vec<Utterance>,
    pub reply_utterances: BTreeMap<u32, usize>,
    pub reply_first_text: BTreeMap<u32, u64>,
    /// When a reply's model first streamed text, by call source, for
    /// replies started ahead whose events wait for the final.
    pub source_first_text: BTreeMap<String, u64>,
    pub reply_sources: BTreeMap<u32, String>,
    pub answer_source: Option<Provenance>,
    pub received_answer_source: Option<Provenance>,
    pub playback: Vec<Value>,
    pub session_faults: Vec<Value>,
    pub old_audio_after_flush: u64,
    pub after_stop_model_calls: u64,
    pub after_stop_samples: u64,
    pub after_stop_lines: std::collections::BTreeSet<u64>,
    pub reply_trace: Vec<Value>,
}

pub(super) struct Metrics {
    pub origin: Instant,
    pub facts: Mutex<Facts>,
}

impl Metrics {
    pub fn new(origin: Instant) -> Arc<Self> {
        Arc::new(Self {
            origin,
            facts: Mutex::new(Facts::default()),
        })
    }
    pub fn now(&self) -> u64 {
        self.origin
            .elapsed()
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX)
    }
    pub fn with<T>(&self, f: impl FnOnce(&mut Facts) -> T) -> T {
        f(&mut self.facts.lock().unwrap_or_else(|p| p.into_inner()))
    }
    pub fn first(&self, key: &'static str) {
        self.with(|f| {
            f.times.entry(key).or_insert_with(|| self.now());
        });
    }
    pub fn at(&self, key: &'static str) {
        self.with(|f| {
            f.times.insert(key, self.now());
        });
    }
    pub fn count(&self, key: &'static str, n: u64) {
        self.with(|f| *f.counts.entry(key).or_default() += n);
    }
    pub fn model_started(&self) {
        self.with(|f| {
            if f.times.contains_key("second_final_transcript") {
                f.after_stop_model_calls += 1;
            }
        });
    }
    pub fn begin_utterance(&self) {
        self.with(|f| f.utterances.push(Utterance::default()));
    }
    pub fn keyup(&self, barge: bool) {
        let at = self.now();
        self.with(|f| {
            f.times
                .insert(if barge { "barge_key_up" } else { "key_up" }, at);
            if let Some(u) = f.utterances.last_mut() {
                u.keyup(at);
            }
        });
    }
    pub fn partial(&self, index: usize, text: &str) {
        self.with(|f| {
            if let Some(u) = f.utterances.get_mut(index) {
                u.partial(self.now(), text);
            }
        });
    }
    pub fn early_start(&self, index: usize, text: &str) {
        let at = self.now();
        self.with(|f| {
            if let Some(u) = f.utterances.get_mut(index) {
                u.early.starts += 1;
                u.early.latest = Some((at, text.to_owned()));
            }
        });
    }
    pub fn early_discard(&self, index: usize) {
        self.with(|f| {
            if let Some(u) = f.utterances.get_mut(index) {
                u.early.discards += 1;
            }
        });
    }
    pub fn early_release(&self, index: usize, reply: u32, source: String) {
        self.with(|f| {
            if let Some(u) = f.utterances.get_mut(index) {
                u.early.released += 1;
                u.early.released_at = u.early.latest.as_ref().map(|(at, _)| *at);
            }
            f.reply_sources.insert(reply, source);
        });
    }
    pub fn source_text(&self, source: &str, at: u64) {
        self.with(|f| {
            if f.source_first_text.len() < 64 || f.source_first_text.contains_key(source) {
                f.source_first_text.entry(source.to_owned()).or_insert(at);
            }
        });
    }
    pub fn final_transcript(&self, index: usize, text: &str, expected: &[&str]) {
        self.with(|f| {
            let at = self.now();
            f.times.entry("final_transcript").or_insert(at);
            if index == 1 {
                f.times.insert("second_final_transcript", at);
            }
            if let Some(u) = f.utterances.get_mut(index) {
                u.finish(at, text, expected);
            }
        });
    }
    pub fn result(&self) -> Value {
        self.with(|f| {
            let delta = |end, start| difference(&f.times, end, start);
            let mut value = json!({
                "times_ms": f.times, "counts": f.counts, "link_faults": f.faults,
                "states": f.states, "session_faults": f.session_faults,
                "transcripts": f.utterances.iter().filter_map(Utterance::result).collect::<Vec<_>>(),
                "playback": f.playback, "reply_trace": f.reply_trace,
                "keyup_to_final": delta("final_transcript", "key_up"),
                "keyup_to_first_audio": delta("first_audio_played", "key_up"),
                "keyup_to_first_action": delta("first_action", "key_up"),
                "barge_key_down_to_silence": delta("barge_flush", "barge_key_down"),
                "old_reply_samples_after_flush": f.old_audio_after_flush,
                "second_answer_spoken": *f.counts.get("second_answer_samples").unwrap_or(&0) > 0,
                "audio_seconds_in": *f.counts.get("input_samples").unwrap_or(&0) as f64 / 24_000.0,
                "audio_seconds_out": *f.counts.get("played_samples").unwrap_or(&0) as f64 / 24_000.0,
                "received_audio_seconds": *f.counts.get("output_samples").unwrap_or(&0) as f64 / 24_000.0,
                "wall_ms": delta("loop_end", "key_down").unwrap_or_else(|| self.now().saturating_sub(*f.times.get("key_down").unwrap_or(&0))),
            });
            let source = f.answer_source.or(f.received_answer_source).unwrap_or_default();
            let utterance = source.reply.and_then(|r| f.reply_utterances.get(&r)).and_then(|i| f.utterances.get(*i));
            let keyup = utterance.and_then(|u| u.key_up);
            let final_at = utterance.and_then(|u| u.final_at);
            let shown = source.reply.and_then(|r| f.reply_first_text.get(&r).copied());
            let produced = source.reply.and_then(|r| f.reply_sources.get(&r)).and_then(|s| f.source_first_text.get(s).copied());
            let first_text = produced.or(shown);
            let played = f.times.get("first_answer_audio_played").copied();
            let stages = answer_stages(keyup, final_at, first_text, source.sent, source.received, played);
            value["first_text_before_final_ms"] = json!(between(final_at, first_text));
            value["keyup_to_answer_audio"] = json!(between(played, keyup));
            for (key, stage) in stages { value[key] = json!(stage); }
            value["answer_utterance"] = json!(source.reply.and_then(|r| f.reply_utterances.get(&r)));
            if f.times.contains_key("second_final_transcript") {
                value["after_stop_model_calls"] = json!(f.after_stop_model_calls);
                value["after_stop_speech_ms"] = json!(f.after_stop_samples as f64 / 24.0);
                value["after_stop_lines"] = json!(f.after_stop_lines.len());
            }
            value
        })
    }
}

fn between(end: Option<u64>, start: Option<u64>) -> Option<u64> {
    end?.checked_sub(start?)
}
fn difference(times: &BTreeMap<&str, u64>, end: &str, start: &str) -> Option<u64> {
    between(times.get(end).copied(), times.get(start).copied())
}
fn answer_stages(
    keyup: Option<u64>,
    final_at: Option<u64>,
    text: Option<u64>,
    sent: Option<u64>,
    received: Option<u64>,
    played: Option<u64>,
) -> [(&'static str, Option<u64>); 5] {
    // A reply started ahead may stream before the final; its stage is a
    // clamped zero and the lead is `first_text_before_final_ms`.
    let text = text.map(|text| final_at.map_or(text, |at| text.max(at)));
    [
        ("stt_final", between(final_at, keyup)),
        ("model_first_text", between(text, final_at)),
        ("sentence_ready", between(sent, text)),
        ("tts_first_audio", between(received, sent)),
        ("playout_start", between(played, received)),
    ]
}

pub(super) fn transcript(text: &str, expected: &[&str]) -> (usize, usize) {
    let words: Vec<String> = text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .map(str::to_lowercase)
        .collect();
    (
        expected
            .iter()
            .filter(|word| !words.iter().any(|heard| heard == **word))
            .count(),
        normalize(text).split_whitespace().count(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn voice_answer_stage_sum_and_missing_events() {
        let stages = answer_stages(
            Some(500),
            Some(650),
            Some(1200),
            Some(1550),
            Some(1800),
            Some(1861),
        );
        assert_eq!(
            stages.map(|(_, v)| v.unwrap()).iter().sum::<u64>(),
            1861 - 500
        );
        assert_eq!(
            stages.map(|(_, v)| v),
            [Some(150), Some(550), Some(350), Some(250), Some(61)]
        );
        let missing = answer_stages(Some(500), Some(650), None, Some(1550), None, Some(1861));
        assert_eq!(missing.map(|(_, v)| v), [Some(150), None, None, None, None]);
        assert_eq!(between(Some(1), Some(2)), None);
    }
    #[test]
    fn voice_answer_uses_its_own_utterance_and_reply() {
        let metrics = Metrics::new(Instant::now());
        metrics.with(|f| {
            let mut u = Utterance::default();
            u.keyup(700);
            u.finish(800, "new request", &["request"]);
            f.utterances = vec![Utterance::default(), u];
            f.reply_utterances.insert(42, 1);
            f.reply_first_text.insert(42, 900);
            f.answer_source = Some(Provenance {
                kind: Kind::Answer,
                sent: Some(1000),
                received: Some(1100),
                reply: Some(42),
            });
            f.times.insert("first_answer_audio_played", 1200);
            f.times.insert("key_up", 100);
        });
        let result = metrics.result();
        assert_eq!(result["keyup_to_answer_audio"], 500);
        assert_eq!(result["stt_final"], 100);
        assert_eq!(result["answer_utterance"], 1);
        let sum: u64 = [
            "stt_final",
            "model_first_text",
            "sentence_ready",
            "tts_first_audio",
            "playout_start",
        ]
        .iter()
        .map(|key| result[key].as_u64().unwrap())
        .sum();
        assert_eq!(sum, result["keyup_to_answer_audio"].as_u64().unwrap());
    }
    #[test]
    fn voice_partials_snapshot_at_keyup_and_do_not_serialize_text() {
        let mut u = Utterance::default();
        u.partial(10, "We’re on");
        u.partial(30, "WE'RE, on the page!");
        u.keyup(40);
        u.partial(50, "late unrelated");
        u.finish(60, "we’re on the page.", &["page", "missing"]);
        let result = u.result().unwrap();
        assert_eq!(result["partials"], 3);
        assert_eq!(result["last_partial_before_keyup_ms"], 10);
        assert_eq!(result["partial_at_keyup_matches_final"], true);
        assert_eq!(result["partial_words_missing"], 0);
        assert_eq!(result["expected_missing"], 1);
        assert!(!result.to_string().contains("page"));
        let mut missing = Utterance::default();
        missing.keyup(40);
        missing.partial(50, "late");
        missing.finish(60, "two words", &[]);
        assert_eq!(
            missing.result().unwrap()["partial_words_missing"],
            Value::Null
        );
        let mut fewer = Utterance::default();
        fewer.partial(1, "one");
        fewer.keyup(2);
        fewer.finish(3, "one more word", &[]);
        assert_eq!(fewer.result().unwrap()["partial_words_missing"], 2);
    }
    #[test]
    fn voice_early_start_counts_and_lead_without_text() {
        let metrics = Metrics::new(Instant::now());
        metrics.with(|f| {
            let mut u = Utterance::default();
            u.keyup(700);
            f.utterances = vec![u];
        });
        metrics.early_start(0, "what is first here");
        metrics.early_discard(0);
        metrics.early_start(0, "What is first here?");
        metrics.with(|f| {
            f.utterances[0].early.latest.as_mut().unwrap().0 = 760;
            f.utterances[0].finish(1100, "What is first here?", &[]);
        });
        metrics.early_release(0, 7, "c~2".into());
        metrics.source_text("c~2", 1000);
        metrics.with(|f| {
            f.reply_utterances.insert(7, 0);
            f.reply_first_text.insert(7, 1110);
            f.answer_source = Some(Provenance {
                kind: Kind::Answer,
                sent: Some(1300),
                received: Some(1700),
                reply: Some(7),
            });
            f.times.insert("first_answer_audio_played", 1750);
        });
        let result = metrics.result();
        let transcript = &result["transcripts"][0];
        assert_eq!(transcript["early_starts"], 2);
        assert_eq!(transcript["early_discards"], 1);
        assert_eq!(transcript["early_released"], 1);
        assert_eq!(transcript["early_start_to_final_ms"], 340);
        assert_eq!(transcript["early_text_matched"], "exact");
        assert_eq!(result["model_first_text"], 0);
        assert_eq!(result["first_text_before_final_ms"], 100);
        assert_eq!(result["sentence_ready"], 200);
        let sum: u64 = [
            "stt_final",
            "model_first_text",
            "sentence_ready",
            "tts_first_audio",
            "playout_start",
        ]
        .iter()
        .map(|key| result[key].as_u64().unwrap())
        .sum();
        assert_eq!(sum, result["keyup_to_answer_audio"].as_u64().unwrap());
        assert!(!result.to_string().contains("first here"));

        let mut folded = Utterance::default();
        folded.keyup(1);
        folded.early.latest = Some((2, "send hello world".into()));
        folded.finish(3, "Send: Hello, World!", &[]);
        assert_eq!(folded.result().unwrap()["early_text_matched"], "folded");
        assert_eq!(
            folded.result().unwrap()["early_start_to_final_ms"],
            Value::Null
        );
    }
    #[test]
    fn voice_transcript_checks_strict_whole_words() {
        assert_eq!(
            transcript("Lakeview, people?", &["lakeview", "people"]),
            (0, 2)
        );
        assert_eq!(transcript("stopping", &["stop"]), (1, 1));
        assert_eq!(transcript("", &[]), (0, 0));
    }
}
