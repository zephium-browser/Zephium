import { commands } from "$shared/ipc/bindings";
import type {
  WorkContextSelectionV1,
  WorkEnvironmentCall,
  WorkEnvironmentIntent,
  WorkEnvironmentEdit,
  WorkEnvironmentSnapshot,
  WorkEnvironmentSummary,
  WorkEnvironmentView,
  WorkReplyV1,
} from "$shared/ipc/bindings";
import { commandId, validRevision } from "$domain/work";
import { events } from "$shared/ipc/native-events";
import { observe } from "$shared/lib/observe";
import { registerCloseTask } from "$shared/lib/close";
import { onDraftsForgotten, readDraft, writeDraft } from "$shared/lib/drafts";
import { SvelteMap } from "svelte/reactivity";

type Pending = Extract<WorkEnvironmentCall, { kind: "command" | "checkpoint" }>;
const DRAFT_KIND = "work-composer";
const DRAFT_PAUSE_MS = 500;
/** Profile/Space-scoped native projections plus document-lifetime unresolved operands. */
export class WorkEnvironmentSession {
  readonly profile: string;
  readonly space: string;
  snapshot = $state.raw<WorkEnvironmentSnapshot | null>(null);
  works = $state.raw<WorkEnvironmentSummary[]>([]);
  selected = $state<string | null>(null);
  next = $state<string | null>(null);
  pending = $state.raw<Pending | null>(null);
  delivery = $state<"ready" | "pending" | "unknown" | "conflict" | "rejected">("ready");
  failure = $state<string | null>(null);
  loading = $state(false);
  /** The Work whose run the canvas last saw live; the sidebar marks its row. */
  running = $state<string | null>(null);
  remoteView = $state.raw<{ sequence: number; view: WorkEnvironmentView } | null>(null);
  private remoteSequence = 0;
  /** The words in the ask bar. Kept across a relaunch, per profile and space. */
  #composer = $state("");
  #draftTimer: ReturnType<typeof setTimeout> | undefined;
  objectiveSubmission = $state.raw<{
    objective: string;
    command: string;
    attached: boolean;
    /** Selected context is connected to the goal card once; retries skip it. */
    related?: boolean;
    context: WorkContextSelectionV1 | null;
  } | null>(null);
  objectiveToAttach = $state<string | null>(null);
  viewDraft = $state.raw<{ id: string; expected: string; view: WorkEnvironmentView } | null>(null);
  /** Where the person is looking, kept here until the next write or until the Work is left. */
  private camera: { id: string; x: number; y: number; zoom_milli: number } | null = null;
  private active = false;
  private launched = false;
  private generation = 0;
  private reads = 0;
  private listing = 0;
  private stop: (() => void) | null = null;
  private invalidation: ReturnType<typeof setTimeout> | undefined;
  private timer: ReturnType<typeof setTimeout> | undefined;
  private writing: Promise<boolean> | null = null;
  private checkpointRetries = 0;
  private readonly removeCloseTask: () => void;
  constructor(profile: string, space: string) {
    this.profile = profile;
    this.space = space;
    this.#composer = readDraft(DRAFT_KIND, [profile, space]) ?? "";
    this.removeCloseTask = registerCloseTask(async () => {
      // Words in the ask bar are kept and restored, so they never hold up a
      // close; only a save that has not landed does.
      this.keepComposer();
      if (this.writing) await this.writing;
      if (this.pending && !(await this.retry())) return false;
      return this.flushView();
    });
  }
  get composer() {
    return this.#composer;
  }
  set composer(text: string) {
    if (text === this.#composer) return;
    this.#composer = text;
    clearTimeout(this.#draftTimer);
    // Sending or clearing is final at once; typing is written once it pauses.
    if (text) this.#draftTimer = setTimeout(() => this.keepComposer(), DRAFT_PAUSE_MS);
    else this.keepComposer();
  }
  private keepComposer() {
    clearTimeout(this.#draftTimer);
    writeDraft(DRAFT_KIND, [this.profile, this.space], this.#composer);
  }
  get disposable() {
    return (
      !this.active && !this.pending && !this.viewDraft && !this.writing && !this.objectiveToAttach
    );
  }
  async start(defaultTitle: string) {
    if (this.active) return;
    this.active = true;
    const generation = ++this.generation;
    const refresh = () => {
      clearTimeout(this.invalidation);
      this.invalidation = setTimeout(() => {
        if (this.active && generation === this.generation) {
          void this.reload();
          void this.refresh();
        }
      }, 80);
    };
    const stop = await events.workEnvironmentChanged.listen(({ payload }) => {
      if (payload.profile === this.profile) refresh();
    });
    if (!this.active || generation !== this.generation) {
      stop();
      return;
    }
    if (typeof window !== "undefined") window.addEventListener("focus", refresh);
    this.stop = () => {
      stop();
      if (typeof window !== "undefined") window.removeEventListener("focus", refresh);
    };
    if (this.pending) await this.retry();
    if (!this.active || generation !== this.generation || this.pending) return;
    const listed = await this.reload();
    if (!this.active || generation !== this.generation || !listed) return;
    // The first visit after launch starts on a clean work; an empty one is reused.
    const launch = !this.launched;
    this.launched = true;
    const last =
      this.selected ??
      this.works
        .filter((work) => work.lifecycle === "active")
        .sort((a, b) => Number(b.touched_ms) - Number(a.touched_ms))[0]?.id;
    if (!last) {
      await this.create(defaultTitle);
      return;
    }
    const opened = await this.open(last);
    if (
      launch &&
      opened &&
      this.snapshot?.lifecycle === "active" &&
      this.snapshot.elements.length > 0
    )
      await this.create(defaultTitle);
  }
  /** Shows `id` when the canvas opens next, or at once while it is open. */
  show(id: string) {
    this.selected = id;
    this.launched = true;
    if (this.active) void this.open(id);
  }
  stopObserving() {
    this.active = false;
    ++this.generation;
    ++this.reads;
    ++this.listing;
    clearTimeout(this.timer);
    clearTimeout(this.invalidation);
    this.stop?.();
    this.stop = null;
    void this.flushView();
    this.loading = false;
  }
  dispose() {
    this.stopObserving();
    this.keepComposer();
    this.removeCloseTask();
  }
  private async call(request: WorkEnvironmentCall): Promise<WorkReplyV1> {
    try {
      const result = await observe(
        Promise.resolve().then(() =>
          commands.workCall(this.profile, { kind: "environment", version: 1, request }),
        ),
        9000,
      );
      if (
        result.state !== "received" ||
        result.value.version !== 1 ||
        result.value.profile !== this.profile
      )
        return { kind: "error", error: "outcome_unknown" };
      return result.value.reply;
    } catch {
      return { kind: "error", error: "outcome_unknown" };
    }
  }
  private admit(snapshot: WorkEnvironmentSnapshot, id?: string): boolean {
    if (
      snapshot.version !== 1 ||
      snapshot.profile !== this.profile ||
      snapshot.space !== this.space ||
      (id && snapshot.id !== id) ||
      !validRevision(snapshot.revision) ||
      !validRevision(snapshot.view.revision) ||
      snapshot.elements.length > 500 ||
      snapshot.areas.length > 64
    )
      return false;
    const current = this.snapshot;
    if (
      current?.id === snapshot.id &&
      (BigInt(snapshot.revision) < BigInt(current.revision) ||
        BigInt(snapshot.view.revision) < BigInt(current.view.revision))
    )
      return false;
    if (
      current?.id === snapshot.id &&
      current.view.revision !== snapshot.view.revision &&
      !this.viewDraft &&
      !this.pending
    )
      this.publishRemoteView(snapshot.view);
    this.snapshot = snapshot;
    this.selected = snapshot.id;
    const known = this.works.find((work) => work.id === snapshot.id);
    this.works = [
      {
        name: null,
        requests: [],
        touched_ms: "0",
        ...known,
        id: snapshot.id,
        space: snapshot.space,
        title: snapshot.title,
        lifecycle: snapshot.lifecycle,
        revision: snapshot.revision,
        empty: !snapshot.elements.length && !snapshot.areas.length,
      },
      ...this.works.filter((work) => work.id !== snapshot.id),
    ].slice(0, 256);
    return true;
  }
  /** Every work of the Space, page by page, so the list and its search see them all. */
  async reload(): Promise<boolean> {
    const request = ++this.listing;
    const works: WorkEnvironmentSummary[] = [];
    let after: string | null = null;
    let selected: string | null = null;
    for (let page = 0; page < 8; page++) {
      const reply = await this.call({ kind: "list", space: this.space, after, limit: 32 });
      if (!this.active || request !== this.listing) return false;
      if (
        reply.kind !== "environment" ||
        reply.reply.kind !== "page" ||
        !reply.reply.works.every((work) => work.space === this.space)
      ) {
        this.failure = reply.kind === "error" ? reply.error : "outcome_unknown";
        return false;
      }
      works.push(...reply.reply.works);
      selected ??= reply.reply.selected;
      after = reply.reply.next;
      if (!after) break;
    }
    this.works = [...new SvelteMap(works.map((work) => [work.id, work])).values()].slice(0, 256);
    this.next = after;
    if (!this.selected) this.selected = selected;
    return true;
  }
  /** The one untouched work still carrying its first name, if there is one. */
  private draft(title: string) {
    return this.works.find(
      (work) => work.lifecycle === "active" && work.empty && work.title === title,
    );
  }
  async open(id: string): Promise<boolean> {
    if (this.pending || !(await this.flushView())) return false;
    const request = ++this.reads;
    this.loading = true;
    const reply = await this.call({ kind: "open", id });
    if (!this.active || request !== this.reads) return false;
    this.loading = false;
    if (
      reply.kind === "environment" &&
      reply.reply.kind === "snapshot" &&
      this.admit(reply.reply.snapshot, id)
    ) {
      this.failure = null;
      this.delivery = "ready";
      return true;
    }
    this.failure = reply.kind === "error" ? reply.error : "outcome_unknown";
    return false;
  }
  async refresh(): Promise<boolean> {
    const id = this.selected;
    if (!id || this.pending) return false;
    const request = ++this.reads;
    const reply = await this.call({ kind: "read", id });
    if (!this.active || request !== this.reads) return false;
    if (
      reply.kind === "environment" &&
      reply.reply.kind === "snapshot" &&
      this.admit(reply.reply.snapshot, id)
    ) {
      if (!this.viewDraft) {
        this.delivery = "ready";
        this.failure = null;
      } else if (this.delivery === "rejected") {
        this.delivery = "ready";
        this.scheduleView();
      }
      return true;
    }
    this.failure = reply.kind === "error" ? reply.error : "outcome_unknown";
    return false;
  }
  async discardView() {
    if (this.pending || this.writing) return false;
    this.viewDraft = null;
    this.delivery = "ready";
    const okay = await this.refresh();
    if (okay && this.snapshot) this.publishRemoteView(this.snapshot.view);
    return okay;
  }
  private publishRemoteView(view: WorkEnvironmentView) {
    const camera = this.camera?.id === this.snapshot?.id ? this.camera : null;
    this.remoteView = {
      sequence: ++this.remoteSequence,
      view: camera ? { ...view, x: camera.x, y: camera.y, zoom_milli: camera.zoom_milli } : view,
    };
  }
  /** A new work: the one untouched draft when there is one, so empty works never pile up. */
  async create(title: string) {
    if (!(await this.flushView())) return false;
    const draft = this.draft(title);
    if (draft) return draft.id === this.snapshot?.id || this.open(draft.id);
    return this.mutate({ kind: "create", space: this.space, title });
  }
  /** Rename or archive a Work from the list: it is opened first, since an edit applies to the open one. */
  async editWork(
    id: string,
    edit: Extract<WorkEnvironmentEdit, { kind: "rename" | "set_lifecycle" }>,
  ) {
    if (this.snapshot?.id !== id && !(await this.open(id))) return false;
    return this.edit(edit);
  }
  async edit(edit: WorkEnvironmentEdit) {
    if (!(await this.flushView()) || !this.snapshot) return false;
    return this.mutate({
      kind: "edit",
      id: this.snapshot.id,
      expected: this.snapshot.revision,
      edit,
    });
  }
  private mutate(intent: WorkEnvironmentIntent) {
    if (this.pending || this.writing) return Promise.resolve(false);
    this.pending = { kind: "command", command: commandId(), intent };
    return this.retry();
  }
  retry(): Promise<boolean> {
    if (this.writing) return this.writing;
    const pending = this.pending;
    if (!pending) return Promise.resolve(true);
    this.delivery = "pending";
    const task = this.settle(pending);
    this.writing = task;
    void task.finally(() => {
      if (this.writing === task) this.writing = null;
    });
    return task;
  }
  private async settle(pending: Pending): Promise<boolean> {
    const reply = await this.call(pending);
    if (this.pending !== pending) return false;
    if (
      pending.kind === "checkpoint" &&
      reply.kind === "environment" &&
      reply.reply.kind === "checkpointed"
    ) {
      const receipt = reply.reply;
      if (
        receipt.expected === pending.expected &&
        validRevision(receipt.expected) &&
        validRevision(receipt.applied_view_revision) &&
        validRevision(receipt.snapshot.view.revision) &&
        BigInt(receipt.applied_view_revision) === BigInt(receipt.expected) + 1n &&
        BigInt(receipt.snapshot.view.revision) >= BigInt(receipt.applied_view_revision) &&
        this.admit(receipt.snapshot, pending.id)
      ) {
        const draft = this.viewDraft;
        const advanced = receipt.snapshot.view.revision !== receipt.applied_view_revision;
        this.pending = null;
        if (draft?.view === pending.view) {
          this.viewDraft = null;
          if (advanced) this.publishRemoteView(receipt.snapshot.view);
        } else if (draft?.id === pending.id) {
          // New local gestures were based on this checkpoint. A later remote view
          // cannot be silently replaced merely because its earlier receipt replayed.
          this.viewDraft = {
            ...draft,
            expected: receipt.applied_view_revision,
            view: { ...draft.view, revision: receipt.applied_view_revision },
          };
          if (advanced) {
            this.delivery = "conflict";
            this.failure = "conflict";
            return true;
          }
        }
        this.delivery = "ready";
        this.failure = null;
        this.checkpointRetries = 0;
        this.scheduleView();
        return true;
      }
    }
    if (
      pending.kind === "command" &&
      reply.kind === "environment" &&
      reply.reply.kind === "applied" &&
      reply.reply.command === pending.command &&
      validRevision(reply.reply.applied_revision) &&
      validRevision(reply.reply.applied_view_revision) &&
      validRevision(reply.reply.snapshot.revision) &&
      validRevision(reply.reply.snapshot.view.revision) &&
      BigInt(reply.reply.applied_revision) <= BigInt(reply.reply.snapshot.revision) &&
      BigInt(reply.reply.applied_view_revision) <= BigInt(reply.reply.snapshot.view.revision) &&
      this.admit(
        reply.reply.snapshot,
        pending.intent.kind === "create" ? undefined : pending.intent.id,
      )
    ) {
      this.pending = null;
      if (this.viewDraft && this.viewDraft.expected !== reply.reply.snapshot.view.revision) {
        this.delivery = "conflict";
        this.failure = "conflict";
      } else {
        this.delivery = "ready";
        this.failure = null;
        this.scheduleView();
      }
      return true;
    }
    const error = reply.kind === "error" ? reply.error : "outcome_unknown";
    this.failure = error;
    this.delivery =
      error === "conflict" ? "conflict" : error === "outcome_unknown" ? "unknown" : "rejected";
    if (this.delivery !== "unknown") this.pending = null;
    if (pending.kind === "checkpoint" && this.delivery === "rejected") this.releaseDraft(error);
    return false;
  }
  /** A refused arrangement save never holds the person: a busy refusal is
   * retried briefly, then the saved arrangement stands and is re-read. */
  private releaseDraft(error: string) {
    const busy = error === "capacity" || error === "unavailable";
    if (busy && this.checkpointRetries < 2) {
      this.checkpointRetries += 1;
      this.delivery = "ready";
      this.failure = null;
      clearTimeout(this.timer);
      this.timer = setTimeout(() => {
        void this.flushView();
      }, 1500);
      return;
    }
    this.checkpointRetries = 0;
    this.viewDraft = null;
    if (this.active) void this.refresh();
  }
  private scheduleView() {
    clearTimeout(this.timer);
    if (this.active && this.delivery === "ready" && this.viewDraft)
      this.timer = setTimeout(() => {
        void this.flushView();
      }, 1200);
  }
  /**
   * Where things stand is saved once the hands have been still a moment; where
   * the person is looking only rides along with the next write, or is saved as
   * the Work is left, so panning never writes.
   */
  checkpoint(view: WorkEnvironmentView) {
    const snapshot = this.snapshot;
    if (!snapshot || snapshot.lifecycle !== "active") return;
    const drafted = this.viewDraft?.id === snapshot.id;
    if (!drafted && !arranged(snapshot.view, view)) {
      this.camera = { id: snapshot.id, x: view.x, y: view.y, zoom_milli: view.zoom_milli };
      return;
    }
    const expected = drafted ? this.viewDraft!.expected : snapshot.view.revision;
    this.camera = null;
    this.viewDraft = { id: snapshot.id, expected, view: { ...view, revision: expected } };
    this.scheduleView();
  }
  /** A held camera becomes a write against the latest saved arrangement. */
  private draftCamera() {
    const camera = this.camera;
    const snapshot = this.snapshot;
    this.camera = null;
    if (!camera || this.viewDraft || snapshot?.id !== camera.id || snapshot.lifecycle !== "active")
      return;
    const { x, y, zoom_milli } = snapshot.view;
    if (x === camera.x && y === camera.y && zoom_milli === camera.zoom_milli) return;
    const expected = snapshot.view.revision;
    this.viewDraft = {
      id: snapshot.id,
      expected,
      view: {
        ...snapshot.view,
        revision: expected,
        x: camera.x,
        y: camera.y,
        zoom_milli: camera.zoom_milli,
      },
    };
  }
  async flushView(): Promise<boolean> {
    clearTimeout(this.timer);
    if (this.writing && !(await this.writing)) return false;
    if (this.pending) return false;
    this.draftCamera();
    const draft = this.viewDraft;
    if (!draft) return true;
    if (this.delivery === "conflict" || this.delivery === "unknown") return false;
    this.pending = { kind: "checkpoint", id: draft.id, expected: draft.expected, view: draft.view };
    const okay = await this.retry();
    if (okay && this.viewDraft && this.viewDraft !== draft && this.delivery === "ready")
      return this.flushView();
    return !this.viewDraft && (okay || this.delivery === "rejected");
  }
}
function arranged(before: WorkEnvironmentView, after: WorkEnvironmentView) {
  return (
    JSON.stringify([before.placements, before.areas ?? []]) !==
    JSON.stringify([after.placements, after.areas ?? []])
  );
}
const sessions = new SvelteMap<string, WorkEnvironmentSession>();
// A deleted profile or space takes the words in its ask bar with it.
onDraftsForgotten((profile, space) => {
  for (const session of sessions.values())
    if (session.profile === profile && (space === undefined || session.space === space))
      session.composer = "";
});
export function environmentSession(profile: string, space: string): WorkEnvironmentSession | null {
  const key = `${profile}:${space}`;
  const existing = sessions.get(key);
  if (existing) return existing;
  if (sessions.size >= 16) {
    const candidate = [...sessions].find(([, session]) => session.disposable);
    if (!candidate) return null;
    candidate[1].dispose();
    sessions.delete(candidate[0]);
  }
  const session = new WorkEnvironmentSession(profile, space);
  sessions.set(key, session);
  return session;
}
