import {
  commands,
  type VoiceChanged,
  type VoiceEngineV1,
  type VoiceFaultV1,
  type VoiceStartV1,
} from "$shared/ipc/bindings";
import { events } from "$shared/ipc/native-events";
import { listenAll } from "$shared/lib/lifecycle";

/** Why a take ended without words: native's reason, or silence. */
export type VoiceProblem = VoiceFaultV1 | "nothing_heard";
export type VoicePhase = "idle" | "starting" | "listening" | "transcribing" | "failed";
type LevelSink = (levels: readonly number[]) => void;

/**
 * One dictation take in the Work bar, mirrored from native. Rust owns the
 * microphone, the engine and the audio; this holds what the bar shows and
 * hands the words over once.
 */
export class VoiceTake {
  phase = $state<VoicePhase>("idle");
  engine = $state.raw<VoiceEngineV1 | null>(null);
  /** The words so far, when the engine streams them. */
  partial = $state("");
  problem = $state.raw<VoiceProblem | null>(null);
  /** When listening began, in `performance.now()` time. */
  since = $state<number | null>(null);
  /** The phase for this class's own decisions: a component teardown reads
   *  state as it was before the change that tore it down. */
  private current: VoicePhase = "idle";
  private take: number | null = null;
  private request: string | null = null;
  private generation = 0;
  private listening: Promise<void> | null = null;
  private stops: (() => void)[] = [];
  private sinks: LevelSink[] = [];
  private disposed = false;

  constructor(private readonly commit: (text: string) => void) {}

  get active() {
    return (
      this.current === "starting" || this.current === "listening" || this.current === "transcribing"
    );
  }

  private to(phase: VoicePhase) {
    this.current = phase;
    this.phase = phase;
  }

  /** Levels in percent, oldest first, as native measures them. */
  onLevels(sink: LevelSink): () => void {
    this.sinks.push(sink);
    return () => {
      this.sinks = this.sinks.filter((known) => known !== sink);
    };
  }

  /** Subscribes before the first start, so no change is missed. */
  private listen(): Promise<void> {
    if (this.listening) return this.listening;
    const pending = (async () => {
      const stops = await listenAll([
        events.voiceChanged.listen(({ payload }) => this.changed(payload)),
        events.voiceLevels.listen(({ payload }) => {
          if (
            payload.request !== this.request ||
            payload.take !== this.take ||
            this.current !== "listening"
          )
            return;
          for (const sink of this.sinks) sink(payload.levels);
        }),
      ]);
      if (this.disposed) for (const stop of stops) stop();
      else this.stops = stops;
    })();
    this.listening = pending;
    void pending.catch(() => {
      if (this.listening === pending) this.listening = null;
    });
    return pending;
  }

  async start() {
    if (this.active || this.disposed) return;
    const generation = ++this.generation;
    this.reset();
    const request = crypto.randomUUID();
    this.request = request;
    this.engine = null;
    this.to("starting");
    try {
      await this.listen();
    } catch {
      if (generation === this.generation && request === this.request) this.fail("unavailable");
      return;
    }
    if (generation !== this.generation) return;
    let result: VoiceStartV1;
    try {
      result = await commands.voiceStart(request);
    } catch {
      result = { take: null, engine: null, fault: "unavailable" };
    }
    if (generation !== this.generation || request !== this.request) {
      // Cancelled before native had a take to cancel: release it now.
      if (result.take !== null) void commands.voiceCancel(request, result.take).catch(() => {});
      return;
    }
    if (result.fault) return this.fail(result.fault);
    if (result.take === null) return this.reset();
    this.take = result.take;
    this.engine = result.engine;
    if (this.current === "starting") this.heard();
  }

  private heard() {
    this.to("listening");
    this.since = performance.now();
  }

  private changed(change: VoiceChanged) {
    if (change.request !== this.request) return;
    // The first change can arrive before `voice_start` answers with its id.
    if (this.take === null && this.current === "starting") this.take = change.take;
    if (change.take !== this.take) return;
    if (change.engine) this.engine = change.engine;
    switch (change.phase) {
      case "listening":
        if (this.current === "starting") this.heard();
        if (change.partial !== null) this.partial = change.partial;
        return;
      case "transcribing":
        if (this.current === "listening") this.to("transcribing");
        return;
      case "done": {
        const text = (change.text ?? "").trim();
        if (!text) return this.fail("nothing_heard");
        this.reset();
        this.commit(text);
        return;
      }
      case "failed":
        return this.fail(change.fault ?? "unavailable");
      case "cancelled":
        return this.reset();
    }
  }

  /** Ends listening; the words follow. Before the microphone opens, it cancels. */
  stop() {
    if (this.current === "starting") return this.cancel();
    if (this.current !== "listening" || this.take === null) return;
    const take = this.take;
    const request = this.request;
    void commands.voiceStop(take).catch(() => {
      if (this.take === take && this.request === request) this.fail("unavailable");
    });
  }

  /** Discards the take, whatever its phase, or clears a problem. */
  cancel() {
    const active = this.active;
    const request = this.request;
    const take = this.take;
    this.generation += 1;
    this.reset();
    if (active && request) void commands.voiceCancel(request, take).catch(() => {});
  }

  private fail(problem: VoiceProblem) {
    this.reset();
    this.problem = problem;
    this.to("failed");
  }

  private reset() {
    this.to("idle");
    this.problem = null;
    this.partial = "";
    this.since = null;
    this.take = null;
    this.request = null;
  }

  dispose() {
    this.cancel();
    this.disposed = true;
    for (const stop of this.stops) stop();
    this.stops = [];
    this.sinks = [];
  }
}
