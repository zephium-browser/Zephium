import type { WorkComposerDraft, WorkEnvironmentCall, WorkReplyV1 } from "$shared/ipc/bindings";
import { validRevision } from "$domain/work";

export type ComposerSubmission = Readonly<{ text: string; revision: string; generation: number }>;
type Call = (request: WorkEnvironmentCall) => Promise<WorkReplyV1>;
const MAX_CHARS = 64 * 1024;

/** Document words remain local until the scoped native revision acknowledges them. */
export class WorkComposer {
  text = $state("");
  failure = $state<string | null>(null);
  private revision: string | null = null;
  private generation = 0;
  private saved = 0;
  private durableText = "";
  private attempt: { expected: string; text: string; generation: number; clear?: boolean } | null =
    null;
  private retired = false;
  private timer: ReturnType<typeof setTimeout> | undefined;
  private writing: Promise<boolean> | null = null;
  private readonly hydration: Promise<boolean>;
  constructor(
    private readonly profile: string,
    private readonly space: string,
    private readonly call: Call,
  ) {
    this.hydration = this.hydrate();
  }
  private admit(reply: WorkReplyV1): WorkComposerDraft | null {
    if (!reply || reply.kind !== "environment" || reply.reply.kind !== "composer") return null;
    const draft = reply.reply.draft;
    return draft &&
      draft.profile === this.profile &&
      draft.space === this.space &&
      typeof draft.text === "string" &&
      typeof draft.revision === "string" &&
      validRevision(draft.revision) &&
      draft.text.length <= MAX_CHARS
      ? draft
      : null;
  }
  private async request(request: WorkEnvironmentCall): Promise<WorkReplyV1> {
    try {
      return await this.call(request);
    } catch {
      return { kind: "error", error: "outcome_unknown" };
    }
  }
  private async hydrate() {
    const draft = this.admit(await this.request({ kind: "composer_read", space: this.space }));
    if (this.retired) return false;
    if (!draft) {
      this.failure = "outcome_unknown";
      return false;
    }
    this.revision = draft.revision;
    this.durableText = draft.text;
    if (!this.generation) this.text = draft.text;
    else if (draft.text && draft.text !== this.text) this.failure = "conflict";
    return true;
  }
  set(text: string) {
    if (this.retired || text === this.text) return;
    this.text = text;
    ++this.generation;
    clearTimeout(this.timer);
    this.timer = setTimeout(() => void this.flush(), 500);
  }
  get disposable() {
    return (
      this.retired ||
      (this.revision !== null &&
        this.saved === this.generation &&
        !this.writing &&
        this.failure === null &&
        this.attempt === null)
    );
  }
  flush(): Promise<boolean> {
    if (this.retired) return Promise.resolve(false);
    clearTimeout(this.timer);
    if (this.writing) return this.writing.then((okay) => okay && this.flush());
    const task = this.save();
    this.writing = task;
    void task.finally(() => {
      if (this.writing === task) this.writing = null;
    });
    return task;
  }
  private async save(): Promise<boolean> {
    await this.hydration;
    if (this.retired || this.revision === null) return false;
    if (this.saved === this.generation) return !this.failure;
    if (this.failure === "conflict" || this.failure === "outcome_unknown") return false;
    const generation = this.generation;
    const text = this.text;
    if (text.length > MAX_CHARS) {
      this.failure = "capacity";
      return false;
    }
    const expected = this.revision;
    this.attempt = { expected, text, generation };
    const reply = await this.request({ kind: "composer_save", space: this.space, expected, text });
    if (this.retired) return false;
    const draft = this.admit(reply);
    if (!draft || draft.text !== text || BigInt(draft.revision) !== BigInt(expected) + 1n) {
      this.failure = reply.kind === "error" ? reply.error : "outcome_unknown";
      return false;
    }
    this.revision = draft.revision;
    this.durableText = text;
    this.attempt = null;
    this.saved = generation;
    this.failure = null;
    // A later keystroke is a separate write based on this acknowledged revision.
    if (this.generation !== generation) return this.save();
    return true;
  }
  async prepare(): Promise<ComposerSubmission | null> {
    if (!(await this.flush()) || this.revision === null || this.retired) return null;
    if (this.saved !== this.generation || this.text !== this.durableText) return null;
    return { text: this.text, revision: this.revision, generation: this.generation };
  }
  clear(submitted: ComposerSubmission): Promise<boolean> {
    if (this.retired) return Promise.resolve(false);
    clearTimeout(this.timer);
    if (this.writing) return this.writing.then((okay) => okay && this.clear(submitted));
    const task = this.clearAcknowledged(submitted);
    this.writing = task;
    void task.finally(() => {
      if (this.writing === task) this.writing = null;
    });
    return task;
  }
  private async clearAcknowledged(submitted: ComposerSubmission): Promise<boolean> {
    if (this.retired) return false;
    if (this.generation !== submitted.generation || this.text !== submitted.text) return true;
    if (this.revision !== submitted.revision) return false;
    this.attempt = {
      expected: submitted.revision,
      text: "",
      generation: submitted.generation,
      clear: true,
    };
    const reply = await this.request({
      kind: "composer_save",
      space: this.space,
      expected: submitted.revision,
      text: "",
    });
    const draft = this.admit(reply);
    if (this.retired) return false;
    if (!draft || draft.text !== "" || BigInt(draft.revision) !== BigInt(submitted.revision) + 1n) {
      this.failure = reply.kind === "error" ? reply.error : "outcome_unknown";
      return false;
    }
    this.revision = draft.revision;
    this.failure = null;
    this.durableText = "";
    this.attempt = null;
    if (this.generation === submitted.generation) this.text = "";
    else this.timer = setTimeout(() => void this.flush(), 500);
    return true;
  }
  /** Explicit reconciliation never silently replaces another document's words. */
  resolve(mode: "retry" | "load" | "replace"): Promise<boolean> {
    if (this.retired) return Promise.resolve(false);
    clearTimeout(this.timer);
    if (this.writing) return this.writing.then(() => this.resolve(mode));
    const task = this.reconcile(mode);
    this.writing = task;
    void task.finally(() => {
      if (this.writing === task) this.writing = null;
    });
    return task;
  }
  private async reconcile(mode: "retry" | "load" | "replace"): Promise<boolean> {
    await this.hydration;
    if (this.retired) return false;
    const generation = this.generation;
    const reply = await this.request({ kind: "composer_read", space: this.space });
    const draft = this.admit(reply);
    if (this.retired) return false;
    if (!draft) {
      this.failure = reply.kind === "error" ? reply.error : "outcome_unknown";
      return false;
    }
    const attempt = this.attempt;
    const settled =
      attempt &&
      draft.text === attempt.text &&
      BigInt(draft.revision) === BigInt(attempt.expected) + 1n;
    if (mode === "load") {
      if (this.generation !== generation) return false;
      this.text = draft.text;
      this.saved = ++this.generation;
    } else if (settled) {
      this.saved = attempt.generation;
      if (attempt.clear && this.generation === attempt.generation) this.text = "";
    } else if (
      mode === "retry" &&
      !(draft.revision === this.revision && draft.text === this.durableText) &&
      !(this.revision === null && (!draft.text || !this.generation))
    ) {
      this.failure = "conflict";
      return false;
    } else if (this.revision === null && !this.generation) this.text = draft.text;
    if (mode === "replace" && this.text !== draft.text) ++this.generation;
    this.revision = draft.revision;
    this.durableText = draft.text;
    this.attempt = null;
    this.failure = null;
    return mode === "load" || this.save();
  }
  forget() {
    if (this.retired) return;
    this.retired = true;
    clearTimeout(this.timer);
    this.text = "";
    this.durableText = "";
    this.attempt = null;
    this.failure = null;
    ++this.generation;
  }
}
