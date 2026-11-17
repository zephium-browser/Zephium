import {
  commands,
  type PageAssistDecisionV1,
  type VoiceSessionIntentV1,
  type VoiceSessionViewV1,
} from "$shared/ipc/bindings";
import { events } from "$shared/ipc/native-events";
import { listenAll } from "$shared/lib/lifecycle";

/**
 * Talking to the assistant, as Rust's voice session reports it. Rust owns the
 * microphone, the socket, the speech and every decision; this holds its last
 * view and the levels it streams while live, and sends typed intent.
 */
let held = $state.raw<VoiceSessionViewV1 | null>(null);
let input = $state(0);
let output = $state(0);
let lifecycle = 0;
let initializing: Promise<void> | null = null;
let stops: (() => void)[] = [];

export const view = () => held;
export const available = () => held?.availability === "available";
/** Microphone level, 0 to 1, while it listens. */
export const inputLevel = () => input;
/** Speech level, 0 to 1, while it plays. */
export const outputLevel = () => output;

/** The view says something worth showing: listening, working, speaking, an outcome or a fault. */
export function active(): boolean {
  const current = held;
  if (!current || current.availability !== "available") return false;
  return current.state.state !== "idle" || current.mic || current.speaking || !!current.fault;
}

const percent = (value: number) =>
  Number.isFinite(value) ? Math.min(100, Math.max(0, value)) / 100 : 0;

function admit(next: VoiceSessionViewV1) {
  if (held && next.seq <= held.seq) return;
  held = next;
  if (next.state.state !== "listening") input = 0;
  if (!next.speaking) output = 0;
}

export function init(): Promise<void> {
  if (initializing) return initializing;
  const generation = ++lifecycle;
  const task = (async () => {
    let listening: (() => void)[];
    try {
      listening = await listenAll([
        events.voiceSessionChanged.listen(({ payload }) => {
          if (generation === lifecycle) admit(payload);
        }),
        events.voiceSessionLevels.listen(({ payload }) => {
          if (generation !== lifecycle || !held) return;
          input = held.state.state === "listening" ? percent(payload.input) : 0;
          output = held.speaking ? percent(payload.output) : 0;
        }),
      ]);
    } catch {
      return;
    }
    if (generation !== lifecycle) {
      for (const stop of listening) stop();
      return;
    }
    stops = listening;
    let first: VoiceSessionViewV1 | null;
    try {
      first = await commands.voiceSessionView();
    } catch {
      first = null;
    }
    if (generation === lifecycle && first) admit(first);
  })();
  initializing = task;
  return task;
}

export function dispose() {
  lifecycle += 1;
  initializing = null;
  for (const stop of stops) stop();
  stops = [];
  held = null;
  input = 0;
  output = 0;
}

/** Start, stop (send) or cancel, as the talk key does. Admitted is not done: the view says what happened. */
export async function intent(kind: VoiceSessionIntentV1): Promise<boolean> {
  try {
    return await commands.voiceSessionIntent(kind);
  } catch {
    return false;
  }
}

/** Approve or decline the step voice waits on, as the panel's buttons do. */
export async function decide(decision: PageAssistDecisionV1): Promise<boolean> {
  try {
    return await commands.voiceSessionDecide(decision);
  } catch {
    return false;
  }
}
