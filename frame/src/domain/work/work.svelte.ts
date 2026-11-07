import { commands } from "$shared/ipc/bindings";
import type {
  WorkCallV1,
  WorkReplyV1,
  WorkRuntimeProjection,
  WorkSummary,
  WorkUserEdit,
  WorkRuntimeIntent,
  WorkEvidenceLink,
  WorkEvidencePreviewV1,
  WorkSignalV1,
  WorkPageV1,
  WorkArtifactDataV1,
  WorkContextSelectionV1,
} from "$shared/ipc/bindings";
import { events } from "$shared/ipc/native-events";
import { observe } from "$shared/lib/observe";
import { registerCloseTask } from "$shared/lib/close";
import { SvelteMap, SvelteSet } from "svelte/reactivity";
import {
  admitProjection,
  commandId,
  sameProjection,
  validRevision,
  AGENT_GRANT,
  AGENT_LIMITS,
} from "./work-model";
import { currentActivity } from "./work-activity";
import { WorkOperations, type OperationObservation } from "./work-operations.svelte";

export type WorkDelivery =
  "ready" | "pending" | "rejected" | "conflict" | "unknown" | "resynchronizing";
type Mutation = Extract<WorkCallV1, { kind: "author" | "execute" }>;
type ArtifactDraft = {
  basis: string;
  execution: string;
  data: WorkArtifactDataV1;
  evidence: WorkEvidenceLink[];
};

type ContinuationScope = { work: string; selection: number; generation: number };
type ContinuationResult = {
  accepted: boolean;
  operation: OperationObservation | null;
  failure: string | null;
};
type QueuedRequest = { text: string; operation?: string };
type RequestQueue = { items: QueuedRequest[]; failure: string | null };

function runAccepted(operation: OperationObservation | undefined | null): boolean {
  const state = operation?.state;
  return (
    state?.kind === "pending" ||
    (state?.kind === "settled" && state.response.reply.kind === "projection")
  );
}
function runFailure(operation: OperationObservation | null): string {
  const state = operation?.state;
  if (state?.kind === "unknown") return "outcome_unknown";
  if (state?.kind === "refused") return state.error;
  if (state?.kind === "settled" && state.response.reply.kind === "error")
    return state.response.reply.error;
  return "unavailable";
}

/** Profile-scoped Rust projections. Pending operands survive view unmount. */
export class WorkSession {
  readonly operations: WorkOperations;
  readonly profile: string;
  works = $state.raw<WorkSummary[]>([]);
  next = $state<string | null>(null);
  selected = $state<string | null>(null);
  projection = $state.raw<WorkRuntimeProjection | null>(null);
  delivery = $state<WorkDelivery>("ready");
  failure = $state<string | null>(null);
  loading = $state(false);
  pending = $state.raw<Mutation | null>(null);
  activity = $state.raw<WorkSignalV1[]>([]);
  /** Pages the latest runs opened, with their newest frames; kept after a run ends. */
  pages = $state.raw<WorkPageV1[]>([]);
  /** Folders the canvas grants to the runs it starts; empty until a person grants one. */
  folders = $state.raw<string[]>([]);
  /** Queued requests keep their originating Work when the selected canvas changes. */
  private readonly queues = new SvelteMap<string, RequestQueue>();
  private readonly sendingQueued = new SvelteSet<string>();
  private selection = 0;
  get queue(): readonly string[] {
    return this.selected
      ? (this.queues.get(this.selected)?.items.map((item) => item.text) ?? [])
      : [];
  }
  /** Requests waiting on an origin grant, by work: the context rides with the run once allowed. */
  private activityRefresh: ReturnType<typeof setTimeout> | undefined;
  private readonly artifacts = new SvelteMap<string, ArtifactDraft>();
  private active = false;
  private generation = 0;
  private reads = 0;
  private listing = 0;
  private stop: (() => void) | null = null;
  private refresh: ReturnType<typeof setTimeout> | undefined;
  private writing: Promise<boolean> | null = null;
  private lifetime = new AbortController();
  private readonly removeCloseTask: () => void;

  constructor(profile: string) {
    this.profile = profile;
    this.operations = new WorkOperations(profile, async (work) => {
      if (!this.active) return;
      await this.reload();
      if (this.selected === work) await this.open(work);
    });
    // Only a save that has not landed holds up a close. An artifact edit is
    // on its way as that save, so it is the pending mutation that is asked.
    this.removeCloseTask = registerCloseTask(async () => {
      if (this.writing) await this.writing;
      return this.pending ? await this.retry() : true;
    });
  }
  async start() {
    if (this.active) return;
    this.active = true;
    const generation = ++this.generation;
    this.lifetime = new AbortController();
    const stop = await events.workChanged.listen(({ payload }) => {
      if (generation !== this.generation || payload.profile !== this.profile) return;
      clearTimeout(this.refresh);
      this.refresh = setTimeout(() => {
        void this.reload();
        if (this.selected) void this.open(this.selected);
      }, 100);
    });
    if (!this.active || generation !== this.generation) {
      stop();
      return;
    }
    this.stop = stop;
    await this.reload();
    if (this.selected) await this.open(this.selected);
    this.operations.start();
  }
  stopObserving() {
    this.operations.stop();
    this.active = false;
    ++this.generation;
    ++this.reads;
    ++this.listing;
    this.lifetime.abort();
    this.stop?.();
    this.stop = null;
    clearTimeout(this.refresh);
    clearTimeout(this.activityRefresh);
    // Pages survive: the workspace remounts on every return to Work.
    this.activity = [];
    this.works = [];
    this.projection = null;
    this.loading = false;
    if (this.pending) this.delivery = "unknown";
  }
  dispose() {
    this.stopObserving();
    this.removeCloseTask();
  }

  private async call(call: WorkCallV1, signal?: AbortSignal): Promise<WorkReplyV1> {
    try {
      const response = await observe(
        Promise.resolve().then(() => commands.workCall(this.profile, call)),
        9_000,
        signal,
      );
      if (response.state !== "received") return { kind: "error", error: "outcome_unknown" };
      if (response.value.version !== 1 || response.value.profile !== this.profile)
        return { kind: "error", error: "outcome_unknown" };
      return response.value.reply;
    } catch {
      return { kind: "error", error: "outcome_unknown" };
    }
  }
  async reload(more = false) {
    const generation = ++this.listing;
    const result = await this.call(
      {
        kind: "query",
        request: { version: 1, query: { kind: "list", after: more ? this.next : null, limit: 32 } },
      },
      this.lifetime.signal,
    );
    if (generation !== this.listing || !this.active) return;
    if (result.kind === "page") {
      this.works = [
        ...new SvelteMap(
          [...(more ? this.works : []), ...result.works].map((w) => [w.id, w]),
        ).values(),
      ];
      this.next = result.next;
    } else if (result.kind === "error") this.failure = result.error;
  }
  async open(work: string): Promise<boolean> {
    const generation = ++this.reads;
    clearTimeout(this.activityRefresh);
    if (work !== this.selected) {
      ++this.selection;
      this.selected = work;
      this.projection = null;
      this.pages = [];
      if (!this.pending) {
        this.delivery = "ready";
        this.failure = null;
      }
    }
    this.loading = true;
    const result = await this.call(
      { kind: "query", request: { version: 1, query: { kind: "projection", work } } },
      this.lifetime.signal,
    );
    if (generation !== this.reads || !this.active || this.selected !== work) return false;
    this.loading = false;
    if (
      result.kind === "projection" &&
      admitProjection(result.projection, this.profile, work, this.projection)
    ) {
      // A live run is read every second; the same revision is the same
      // projection, and a new object would rebuild everything drawn from it.
      if (!sameProjection(this.projection, result.projection)) this.projection = result.projection;
      this.activity = currentActivity(result.projection, this.activity);
      // Pages and their frames outlive the run that opened them, and the
      // workspace remounts whenever Work is left: read them on every open,
      // not only while an execution is live.
      if (result.projection.executions.length) {
        void this.readActivity(result.projection, generation);
        if (
          result.projection.executions.some(
            (entry) =>
              ["running", "cancel_requested"].includes(entry.status) &&
              !result.projection.interrupted.includes(entry.id),
          )
        )
          this.followActivity(work, generation, 0);
      }
      if (this.delivery === "ready") {
        const failure = this.queues.get(work)?.failure;
        if (failure) this.refused(failure);
        else this.failure = null;
      }
      return true;
    }
    if (result.kind === "error") this.failure = result.error;
    else this.failure = "unsupported_projection";
    return false;
  }
  /**
   * While a run is live, what it is doing and its pages' newest frames are
   * read every second; they are not durable, so nothing announces them. The
   * projection itself is re-read when native says the work changed, and only
   * every tenth second besides, should an announcement be lost.
   */
  private followActivity(work: string, generation: number, tick: number) {
    clearTimeout(this.activityRefresh);
    this.activityRefresh = setTimeout(() => {
      if (!this.active || this.selected !== work || generation !== this.reads) return;
      const state = this.projection;
      if (!state || tick >= 9) {
        void this.open(work);
        return;
      }
      void this.readActivity(state, generation).then(() =>
        this.followActivity(work, generation, tick + 1),
      );
    }, 1_000);
  }
  private async readActivity(state: WorkRuntimeProjection, generation: number) {
    try {
      const result = await observe(
        Promise.resolve().then(() => commands.workActivity(this.profile, state.work.id)),
        9_000,
        this.lifetime.signal,
      );
      if (!this.active || this.selected !== state.work.id) return;
      const valid =
        result.state === "received" &&
        result.value.version === 1 &&
        result.value.profile === this.profile &&
        result.value.work === state.work.id &&
        !result.value.error;
      // Pages answer for the work, not for one read of it: an open that
      // overtook this one must not throw away frames it did not ask for.
      if (valid) this.pages = result.value.pages ?? [];
      if (generation !== this.reads) return;
      this.activity = valid ? currentActivity(state, result.value.signals) : [];
    } catch {
      if (generation === this.reads) this.activity = [];
    }
  }
  back() {
    ++this.selection;
    ++this.reads;
    this.selected = null;
    this.projection = null;
    this.loading = false;
  }
  async reconcile() {
    const scope = this.continuationScope();
    await this.operations.reconcile();
    if (scope && !this.sameContinuation(scope)) return false;
    this.delivery = "resynchronizing";
    let okay: boolean;
    if (this.pending) okay = await this.retry();
    else {
      await this.reload();
      if (scope && !this.sameContinuation(scope)) return false;
      okay = this.selected ? await this.open(this.selected) : true;
    }
    if (scope && !this.sameContinuation(scope)) return false;
    const queued = scope ? this.queues.get(scope.work) : undefined;
    const head = queued?.items[0];
    if (okay && scope && queued && head) {
      const operation = head.operation ? this.operations.jobs.get(head.operation) : undefined;
      if (head.operation && (!operation || runFailure(operation) === "outcome_unknown")) {
        this.refused("outcome_unknown");
        return false;
      }
      if (runAccepted(operation)) this.removeQueued(scope.work, head);
      else {
        // Only a definite refusal permits a fresh admission after explicit recovery.
        delete head.operation;
        this.queues.set(scope.work, { items: queued.items, failure: null });
      }
    }
    if (okay) this.delivery = "ready";
    else if (this.delivery === "resynchronizing") this.delivery = "unknown";
    if (okay) this.failure = null;
    return okay;
  }
  create(objective: string, command = commandId()) {
    return this.mutate({
      kind: "author",
      command: { version: 1, command, intent: { kind: "create", objective } },
    });
  }
  get canRelease() {
    return !this.pending && !this.selected && this.artifacts.size === 0 && this.queues.size === 0;
  }
  artifactDraft(artifact: string) {
    return this.artifacts.get(`${this.selected}:${artifact}`);
  }
  editArtifact(execution: string, artifact: string, data?: WorkArtifactDataV1) {
    const work = this.projection?.work;
    const fact = this.projection?.executions.find((item) => item.id === execution);
    const original = fact?.artifacts.find((item) => item.id === artifact);
    if (!work || !fact || !original || !["needs_review", "completed"].includes(fact.status)) return;
    const key = `${work.id}:${artifact}`;
    const previous = this.artifacts.get(key);
    const user = fact.user_artifacts?.find((item) => item.artifact === artifact);
    this.artifacts.set(key, {
      basis: previous?.basis ?? work.revision,
      execution,
      data: structuredClone(data ?? previous?.data ?? user?.edited_data ?? original.data),
      evidence: previous?.evidence ?? (user?.edited_data ? user.evidence : original.evidence),
    });
  }
  discardArtifact(artifact: string) {
    if (!this.pending) this.artifacts.delete(`${this.selected}:${artifact}`);
  }
  saveArtifact(artifact: string) {
    const draft = this.artifactDraft(artifact);
    return draft
      ? this.execute(
          {
            kind: "edit_artifact",
            execution: draft.execution,
            artifact,
            data: draft.data,
            evidence: draft.evidence,
          },
          draft.basis,
        )
      : Promise.resolve(false);
  }
  edit(edit: WorkUserEdit, expected?: string) {
    const work = this.projection?.work;
    if (!work) return Promise.resolve(false);
    return this.mutate({
      kind: "author",
      command: {
        version: 1,
        command: commandId(),
        intent: { kind: "edit", work: work.id, expected_revision: expected ?? work.revision, edit },
      },
    });
  }
  /**
   * The routine loop: sending the objective runs the agent in the person's
   * own sessions; a site it has not worked on yet asks first, on the canvas.
   */
  private continuationScope(): ContinuationScope | null {
    const work = this.projection?.work.id;
    return work && this.selected === work
      ? { work, selection: this.selection, generation: this.generation }
      : null;
  }
  private sameContinuation(scope: ContinuationScope): boolean {
    return (
      this.selected === scope.work &&
      this.projection?.work.id === scope.work &&
      this.selection === scope.selection &&
      this.generation === scope.generation
    );
  }
  private refused(failure: string) {
    this.failure = failure;
    this.delivery =
      failure === "conflict" ? "conflict" : failure === "outcome_unknown" ? "unknown" : "rejected";
  }
  async run(
    context: WorkContextSelectionV1 | null = null,
    options: { private?: boolean; skill?: string | null; draft?: string } = {},
  ): Promise<boolean> {
    const scope = this.continuationScope();
    return scope ? (await this.beginRun(scope, context, options)).accepted : false;
  }
  private async beginRun(
    scope: ContinuationScope,
    context: WorkContextSelectionV1 | null,
    options: { private?: boolean; skill?: string | null; draft?: string },
  ): Promise<ContinuationResult> {
    const notStarted = { accepted: false, operation: null, failure: "unavailable" };
    if (!this.sameContinuation(scope) || this.pending || this.operations.busy(scope.work))
      return notStarted;
    // Acknowledgements are also bound to the canvas selected before the first await.
    for (const execution of [...(this.projection?.interrupted ?? [])]) {
      if (
        !this.sameContinuation(scope) ||
        !(await this.execute({ kind: "acknowledge_interruption", execution }))
      )
        return notStarted;
    }
    if (!this.sameContinuation(scope) || this.pending || this.operations.busy(scope.work))
      return notStarted;
    const work = this.projection!.work;
    const operation = await this.operations.begin({
      kind: "run",
      ...(context ? { context } : {}),
      command: {
        version: 1,
        work: work.id,
        expected_revision: work.revision,
        command: commandId(),
        intent: {
          kind: "begin_agent",
          grant: {
            ...AGENT_GRANT,
            ...(this.folders.length ? { folders: this.folders } : {}),
            ...(options.private ? { private: true } : {}),
            ...(options.skill ? { skill: options.skill } : {}),
            ...(options.draft ? { draft: options.draft } : {}),
          },
          limits: AGENT_LIMITS,
        },
      },
    });
    const accepted = runAccepted(operation);
    const failure = accepted ? null : runFailure(operation);
    if (failure && this.sameContinuation(scope)) this.refused(failure);
    return { accepted, operation, failure };
  }
  /** The next message starts a fresh run only on the Work selected when submitted. */
  async continueWith(
    text: string,
    context: WorkContextSelectionV1 | null = null,
    options: { private?: boolean; draft?: string } = {},
  ): Promise<boolean> {
    const scope = this.continuationScope();
    return scope ? (await this.continueRequest(scope, text, context, options)).accepted : false;
  }
  private async continueRequest(
    scope: ContinuationScope,
    text: string,
    context: WorkContextSelectionV1 | null = null,
    options: { private?: boolean; draft?: string } = {},
  ): Promise<ContinuationResult> {
    const message = text.trim();
    const notStarted = () => ({
      accepted: false,
      operation: null,
      failure: this.sameContinuation(scope) ? (this.failure ?? "unavailable") : "unavailable",
    });
    if (!message || !this.sameContinuation(scope)) return notStarted();
    if (!(await this.edit({ kind: "set_objective", objective: message }))) return notStarted();
    if (!this.sameContinuation(scope)) return notStarted();
    return this.beginRun(scope, context, options);
  }
  /** Up to three next requests the finished run offers; empty while it works. */
  get followups(): readonly string[] {
    const steps = this.projection?.executions.at(-1)?.steps ?? [];
    for (let index = steps.length - 1; index >= 0; index--) {
      const kind = steps[index]!.kind;
      if (kind.kind === "finish") return kind.followups ?? [];
    }
    return [];
  }
  /** The live execution a person can still speak to. */
  private get running() {
    return this.projection?.executions.find(
      (entry) =>
        ["running", "approved"].includes(entry.status) &&
        !this.projection?.interrupted.includes(entry.id),
    );
  }
  /** Hands a message to the agent at its next move; refused when no run is live. */
  async steer(text: string): Promise<boolean> {
    const message = text.trim();
    const execution = this.running;
    if (!message || !execution) return false;
    return this.execute({ kind: "steer", execution: execution.id, text: message });
  }
  /** Holds a request for this Work; false is surfaced as a capacity/admission failure. */
  enqueue(text: string): boolean {
    const message = text.trim();
    const work = this.selected;
    if (!message || !work || this.projection?.work.id !== work) {
      this.refused("unavailable");
      return false;
    }
    const queued = this.queues.get(work);
    if (queued?.failure && queued.items[0]?.text === message) {
      // A repeat click retries the retained head, never duplicates it. Unknown
      // admissions are reconciled by original operation id before any new run.
      void this.reconcile();
      return true;
    }
    if ([...this.queues.values()].reduce((count, queue) => count + queue.items.length, 0) >= 16) {
      this.refused("capacity");
      return false;
    }
    this.queues.set(work, {
      items: [...(queued?.items ?? []), { text: message }],
      failure: queued?.failure ?? null,
    });
    return true;
  }
  private removeQueued(work: string, head: QueuedRequest) {
    const queued = this.queues.get(work);
    if (queued?.items[0] !== head) return;
    const rest = queued.items.slice(1);
    if (rest.length) this.queues.set(work, { items: rest, failure: null });
    else this.queues.delete(work);
  }
  /** Refused/unknown sends keep the head paused until an explicit retry/reconcile. */
  async sendQueued(): Promise<boolean> {
    const scope = this.continuationScope();
    if (!scope || this.sendingQueued.has(scope.work)) return false;
    const queued = this.queues.get(scope.work);
    const head = queued?.items[0];
    if (!head || queued.failure || this.running || this.operations.busy(scope.work)) return false;
    this.sendingQueued.add(scope.work);
    try {
      const result = await this.continueRequest(scope, head.text);
      // Only uncertain admission ownership must outlive the disposable job cache.
      // A known refusal remains retryable even after its finished job is evicted.
      if (result.operation && result.failure === "outcome_unknown")
        head.operation = result.operation.id;
      else delete head.operation;
      if (result.accepted) {
        this.removeQueued(scope.work, head);
        return true;
      }
      const current = this.queues.get(scope.work);
      if (current?.items[0] === head)
        this.queues.set(scope.work, {
          items: current.items,
          failure: result.failure ?? "unavailable",
        });
      return false;
    } finally {
      this.sendingQueued.delete(scope.work);
    }
  }
  /** Accepts or refuses one proposed file change the running agent is waiting on. */
  approveStep(step: string, approve: boolean): Promise<boolean> {
    const execution = this.running;
    if (!execution) return Promise.resolve(false);
    return this.execute({ kind: "approve_step", execution: execution.id, step, approve });
  }
  answerStep(execution: string, step: string, answer: string) {
    return this.execute({ kind: "answer_step", execution, step, answer });
  }
  execute(
    intent: Exclude<WorkRuntimeIntent, { kind: "read_public" | "begin_agent" }>,
    expected?: string,
  ) {
    const work = this.projection?.work;
    if (!work) return Promise.resolve(false);
    return this.mutate({
      kind: "execute",
      command: {
        version: 1,
        work: work.id,
        expected_revision: expected ?? work.revision,
        command: commandId(),
        intent,
      },
    });
  }
  private mutate(call: Mutation) {
    if (this.pending || this.writing) return Promise.resolve(false);
    this.pending = call;
    return this.retry();
  }
  retry(): Promise<boolean> {
    if (this.writing) return this.writing;
    const call = this.pending;
    if (!call) return Promise.resolve(true);
    this.delivery = "pending";
    const write = this.settle(call);
    this.writing = write;
    void write.then(() => {
      if (this.writing === write) this.writing = null;
    });
    return write;
  }
  private async settle(call: Mutation): Promise<boolean> {
    // Observation disposal cannot cancel a submitted durable mutation.
    const reply = await this.call(call);
    if (this.pending !== call) return false;
    const receipt =
      reply.kind === "authoring_applied" || reply.kind === "execution_applied"
        ? reply.receipt
        : null;
    const matching =
      receipt &&
      receipt.command === call.command.command &&
      validRevision(receipt.applied_revision) &&
      (call.kind === "author" && reply.kind === "authoring_applied"
        ? (call.command.intent.kind === "create" ||
            reply.receipt.work === call.command.intent.work) &&
          reply.receipt.deleted === (call.command.intent.kind === "delete")
        : call.kind === "execute" &&
          reply.kind === "execution_applied" &&
          reply.projection.work.id === call.command.work &&
          "execution" in call.command.intent &&
          reply.receipt.execution === call.command.intent.execution);
    if (!matching) {
      const failure = reply.kind === "error" ? reply.error : "outcome_unknown";
      this.failure = failure;
      this.delivery =
        failure === "conflict"
          ? "conflict"
          : failure === "outcome_unknown"
            ? "unknown"
            : "rejected";
      if (failure !== "outcome_unknown") this.pending = null;
      return false;
    }
    this.pending = null;
    this.delivery = "ready";
    this.failure = null;
    if (call.kind === "execute" && call.command.intent.kind === "edit_artifact") {
      const key = `${call.command.work}:${call.command.intent.artifact}`;
      const draft = this.artifacts.get(key);
      if (
        draft?.basis === call.command.expected_revision &&
        JSON.stringify(draft.data) === JSON.stringify(call.command.intent.data)
      )
        this.artifacts.delete(key);
    }
    if (reply.kind === "authoring_applied") {
      if (call.kind === "author" && call.command.intent.kind === "create") {
        this.selected = reply.receipt.work;
      }
      if (reply.receipt.deleted && this.selected === reply.receipt.work) this.back();
    }
    if (
      reply.kind === "execution_applied" &&
      this.selected &&
      this.selected === reply.projection.work.id &&
      admitProjection(reply.projection, this.profile, this.selected, this.projection)
    )
      this.projection = reply.projection;
    if (this.active) {
      await this.reload();
      if (this.selected) await this.open(this.selected);
    }
    return true;
  }
  async evidence(link: WorkEvidenceLink): Promise<WorkEvidencePreviewV1 | null> {
    const work = this.selected;
    if (!work) return null;
    const result = await this.call(
      { kind: "query", request: { version: 1, query: { kind: "evidence", work, link } } },
      this.lifetime.signal,
    );
    return this.selected === work &&
      result.kind === "evidence" &&
      result.evidence.link.extraction_id === link.extraction_id &&
      result.evidence.link.source_id === link.source_id
      ? result.evidence
      : null;
  }
}

// Bounded document-lifetime ownership, retained across Browse/Work transitions.
const sessions = new SvelteMap<string, WorkSession>();
export function workSession(profile: string): WorkSession {
  let session = sessions.get(profile);
  if (!session) {
    if (sessions.size >= 8) {
      const idle = [...sessions].find(([, value]) => value.canRelease);
      if (idle) {
        idle[1].dispose();
        sessions.delete(idle[0]);
      } else throw new Error("Work session capacity reached");
    }
    session = new WorkSession(profile);
    sessions.set(profile, session);
  }
  return session;
}
