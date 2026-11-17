import * as m from "$shared/i18n/messages";
import type {
  VoiceConfirmV1,
  VoiceOutcomeV1,
  VoicePressV1,
  VoiceSessionFaultV1,
  VoiceSessionViewV1,
} from "$shared/ipc/bindings";

/** What the island shows, decided from Rust's view alone. */
export type Island =
  | { kind: "listening"; press: VoicePressV1; arming: boolean; words: string }
  | { kind: "finalizing"; words: string }
  | {
      kind: "working";
      line: string;
      /** The step the reply is on, for display only: the page names its control. */
      step: string | null;
      heard: string | null;
      speaking: boolean;
    }
  | { kind: "confirming"; confirm: VoiceConfirmV1 }
  | { kind: "speaking"; outcome: VoiceOutcomeV1 | null; heard: string | null }
  | { kind: "settled"; outcome: VoiceOutcomeV1 }
  | { kind: "fault"; fault: VoiceSessionFaultV1 }
  | { kind: "warm" };

export function island(view: VoiceSessionViewV1 | null): Island | null {
  if (!view || view.availability !== "available") return null;
  const state = view.state;
  switch (state.state) {
    case "arming":
    case "listening":
      return {
        kind: "listening",
        press: state.press,
        arming: state.state === "arming" || !view.mic,
        words: view.interim.trim(),
      };
    case "finalizing":
      return { kind: "finalizing", words: (view.heard ?? view.interim).trim() };
    case "working":
      if (state.work === "confirming" && view.confirm)
        return { kind: "confirming", confirm: view.confirm };
      if (view.fault) return { kind: "fault", fault: view.fault };
      return {
        kind: "working",
        line: workLine(view),
        step: view.step?.trim() || null,
        heard: view.heard?.trim() || null,
        speaking: view.speaking,
      };
    case "settled":
      if (view.fault) return { kind: "fault", fault: view.fault };
      if (view.speaking)
        return { kind: "speaking", outcome: state.outcome, heard: view.heard?.trim() || null };
      return { kind: "settled", outcome: state.outcome };
    case "idle":
      if (view.fault) return { kind: "fault", fault: view.fault };
      if (view.speaking)
        return { kind: "speaking", outcome: view.outcome, heard: view.heard?.trim() || null };
      return view.mic ? { kind: "warm" } : null;
  }
}

/** The one living line: what the assistant is doing now. */
function workLine(view: VoiceSessionViewV1): string {
  const phase = view.phase;
  if (phase?.phase === "reading") return m.assist_phase_reading({ site: phase.site });
  if (phase?.phase === "opening") return m.assist_phase_opening();
  if (
    phase?.phase === "acting" ||
    view.step?.trim() ||
    (view.state.state === "working" && view.state.work === "acting")
  )
    return m.assist_phase_acting();
  return m.assist_phase_thinking();
}

export type Tone = "done" | "quiet" | "warn";

/** A settled outcome in Rust's own terms: success only when Rust says every step was seen to take. */
export function outcomeView(outcome: VoiceOutcomeV1): {
  title: string;
  detail: string | null;
  tone: Tone;
} {
  switch (outcome) {
    case "answered":
      return { title: m.talk_outcome_answered(), detail: null, tone: "done" };
    case "acted":
      return { title: m.talk_outcome_acted(), detail: null, tone: "done" };
    case "partial":
      return {
        title: m.talk_outcome_partial(),
        detail: m.talk_outcome_detail_partial(),
        tone: "warn",
      };
    case "not_confirmed":
      return {
        title: m.talk_outcome_not_confirmed(),
        detail: m.talk_outcome_detail_not_confirmed(),
        tone: "warn",
      };
    case "failed":
      return {
        title: m.talk_outcome_failed(),
        detail: m.talk_outcome_detail_failed(),
        tone: "warn",
      };
    case "declined":
      return { title: m.talk_outcome_declined(), detail: null, tone: "quiet" };
    case "cancelled":
      return { title: m.talk_outcome_cancelled(), detail: null, tone: "quiet" };
    case "needs_you":
      return {
        title: m.talk_outcome_needs_you(),
        detail: m.talk_outcome_detail_needs_you(),
        tone: "warn",
      };
  }
}

/** The one thing that fixes a fault: words to act on, or a way into the account. */
export type Remedy = "plans" | "credits" | null;

export function faultView(
  fault: VoiceSessionFaultV1,
  mac: boolean,
): { title: string; fix: string | null; remedy: Remedy } {
  switch (fault) {
    case "microphone_denied":
      return {
        title: m.talk_fault_microphone_denied(),
        fix: mac
          ? m.talk_fault_microphone_denied_fix_mac()
          : m.talk_fault_microphone_denied_fix_windows(),
        remedy: null,
      };
    case "no_microphone":
      return {
        title: m.talk_fault_no_microphone(),
        fix: m.talk_fault_no_microphone_fix(),
        remedy: null,
      };
    case "offline":
      return { title: m.talk_fault_offline(), fix: m.talk_fault_offline_fix(), remedy: null };
    case "plan_required":
      return { title: m.talk_fault_plan_required(), fix: null, remedy: "plans" };
    case "out_of_credit":
      return { title: m.talk_fault_out_of_credit(), fix: null, remedy: "credits" };
    case "daily_limit":
      return {
        title: m.talk_fault_daily_limit(),
        fix: m.talk_fault_daily_limit_fix(),
        remedy: null,
      };
    case "rate_limited":
      return { title: m.talk_fault_rate_limited(), fix: m.talk_fault_retry(), remedy: null };
    case "provider_down":
      return { title: m.talk_fault_provider_down(), fix: m.talk_fault_retry(), remedy: null };
    case "timeout":
      return { title: m.talk_fault_timeout(), fix: m.talk_fault_retry(), remedy: null };
    case "interrupted":
      return { title: m.talk_fault_interrupted(), fix: m.talk_fault_retry(), remedy: null };
    case "busy":
      return { title: m.talk_fault_busy(), fix: m.talk_fault_busy_fix(), remedy: null };
    case "unavailable":
      return { title: m.talk_fault_unavailable(), fix: m.talk_fault_retry(), remedy: null };
  }
}

/**
 * The transcript as two runs: what was said a moment ago, and the newest
 * words, which read strongest. Split at a word boundary.
 */
export function freshSplit(words: string, fresh = 3): { said: string; newest: string } {
  const parts = words.split(/(\s+)/u);
  let count = 0;
  let at = parts.length;
  while (at > 0 && count < fresh) {
    at -= 1;
    if (parts[at]!.trim()) count += 1;
  }
  return { said: parts.slice(0, at).join(""), newest: parts.slice(at).join("") };
}

/**
 * Bar heights for a level, 0 to 1. Each reading gets its own shape, seeded by
 * the level itself, so the meter reads as a voice rather than a gauge while
 * staying a pure function of what native measured: no clock, no randomness.
 */
export function bars(level: number, count = 5): number[] {
  const clamped = Math.min(1, Math.max(0, Number.isFinite(level) ? level : 0));
  const shaped = Math.sqrt(clamped);
  const seed = Math.round(clamped * 997) + 1;
  const middle = (count - 1) / 2;
  return Array.from({ length: count }, (_, at) => {
    const noise = Math.sin(seed * (at + 1) * 12.9898) * 43758.5453;
    const wobble = 0.5 + 0.5 * (noise - Math.floor(noise));
    const crown = 1 - (Math.abs(at - middle) / (middle + 1)) * 0.4;
    return Math.min(1, 0.16 + shaped * 0.84 * crown * wobble);
  });
}

const QUOTED = /[“"]([^”"]*)[”"]/gu;

/** A step's words with what it quotes (the control) apart from the verb around it. */
export function labelParts(label: string): { text: string; named: boolean }[] {
  const parts: { text: string; named: boolean }[] = [];
  let at = 0;
  for (const match of label.matchAll(QUOTED)) {
    if (match.index > at) parts.push({ text: label.slice(at, match.index), named: false });
    if (match[1]) parts.push({ text: match[1], named: true });
    at = match.index + match[0].length;
  }
  if (at < label.length) parts.push({ text: label.slice(at), named: false });
  return parts;
}
