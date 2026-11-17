import type { KeymapEntry, VoiceSessionViewV1 } from "../ipc/bindings";
import { emitNativeEvent } from "./native-events";

let seq = 100;

/** A voice session view as Rust projects it, available and idle unless told otherwise. */
export function voiceView(over: Partial<VoiceSessionViewV1> = {}): VoiceSessionViewV1 {
  return {
    version: 1,
    seq: (seq += 1),
    availability: "available",
    state: { state: "idle" },
    mic: false,
    warm: false,
    interim: "",
    heard: null,
    phase: null,
    step: null,
    confirm: null,
    speaking: false,
    outcome: null,
    fault: null,
    ...over,
  };
}

/** Rust's next view and, while live, a level reading. */
export function emitVoice(over: Partial<VoiceSessionViewV1> = {}, levels?: [number, number]) {
  emitNativeEvent("voiceSessionChanged", voiceView(over));
  if (levels) emitNativeEvent("voiceSessionLevels", { input: levels[0], output: levels[1] });
}

export const TALK_KEY: KeymapEntry = {
  id: "voice.talk",
  title: "Talk to Zephium",
  group: "keys",
  accelerator: "CmdOrCtrl+Shift+A",
  default_accelerator: "CmdOrCtrl+Shift+A",
  customizable: true,
  customized: false,
};
