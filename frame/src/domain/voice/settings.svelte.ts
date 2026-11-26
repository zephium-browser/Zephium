import { commands, type VoiceEngineV1, type VoiceStatusV1 } from "$shared/ipc/bindings";
import { events } from "$shared/ipc/native-events";
import * as m from "$shared/i18n/messages";
import { listenAll } from "$shared/lib/lifecycle";
import { observe } from "$shared/lib/observe";
import { IS_MAC } from "$shared/platform";

const TIMEOUT = 9000;

export function engineName(engine: VoiceEngineV1): string {
  switch (engine) {
    case "on_device":
      return IS_MAC ? m.voice_engine_mac() : m.voice_engine_device();
    case "zephium":
      return m.voice_engine_zephium();
    case "open_ai":
      return m.voice_engine_openai();
    case "groq":
      return m.voice_engine_groq();
  }
}

/** A language tag in its own language, such as "polski" for `pl`. */
export function languageName(tag: string, locale: string): string {
  try {
    const name = new Intl.DisplayNames([locale], { type: "language" }).of(tag);
    return name ? name.charAt(0).toLocaleUpperCase(locale) + name.slice(1) : tag;
  } catch {
    return tag;
  }
}

/**
 * Settings → AI's view of dictation: the engines that can run, the person's
 * choice and language. Rust owns all of it; this mirrors its last answer and
 * reads again when keys or the account change.
 */
export class VoiceSettings {
  status = $state.raw<VoiceStatusV1 | null>(null);
  unavailable = $state(false);
  busy = $state(false);
  private stops: (() => void)[] = [];
  private active = false;
  private generation = 0;
  private sequence = 0;
  private lifetime = new AbortController();

  async start() {
    if (this.active) return;
    this.active = true;
    const generation = ++this.generation;
    this.lifetime = new AbortController();
    let ready = false;
    const refresh = () => {
      if (ready && this.active && generation === this.generation) void this.read();
    };
    let stops: (() => void)[];
    try {
      stops = await listenAll([
        events.workModelsChanged.listen(refresh),
        events.accountChanged.listen(refresh),
      ]);
    } catch {
      if (generation === this.generation) {
        this.active = false;
        this.unavailable = true;
      }
      return;
    }
    if (!this.active || generation !== this.generation) {
      for (const stop of stops) stop();
      return;
    }
    this.stops = stops;
    ready = true;
    await this.read();
  }

  private async adopt(request: () => Promise<VoiceStatusV1>) {
    const sequence = ++this.sequence;
    const response = await observe(Promise.resolve().then(request), TIMEOUT, this.lifetime.signal);
    if (sequence !== this.sequence || response.state === "aborted") return;
    if (response.state !== "received") {
      this.unavailable = true;
      return;
    }
    this.unavailable = false;
    this.status = response.value;
  }

  read() {
    return this.adopt(() => commands.voiceStatus());
  }

  async choose(engine: VoiceEngineV1 | null) {
    this.busy = true;
    await this.adopt(() => commands.voiceChooseEngine(engine));
    this.busy = false;
  }

  async chooseLanguage(language: string | null) {
    this.busy = true;
    await this.adopt(() => commands.voiceChooseLanguage(language));
    this.busy = false;
  }

  dispose() {
    this.active = false;
    this.generation++;
    this.lifetime.abort();
    for (const stop of this.stops) stop();
    this.stops = [];
  }
}
