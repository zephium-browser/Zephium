import { SvelteDate, SvelteMap, SvelteSet } from "svelte/reactivity";
import { registerCloseTask } from "$shared/lib/close";
import { events } from "$shared/ipc/native-events";
import { resourceCall } from "./transport";
import { mergePage, newerRevision, settleInto } from "./resource-model";
import type {
  ResourceCall_Deserialize as ResourceCall,
  ResourceDraft_Deserialize as ResourceDraft,
  ResourceIntent_Deserialize as ResourceIntent,
  ResourceRecord_Serialize as ResourceRecord,
  ResourceSummary,
  ResourceCommand_Deserialize as ResourceCommand,
  TaskCounts,
  TaskField,
  TaskView,
  TaskActor,
  TaskContext,
  TaskStatus,
  TaskMetadata,
  TaskList,
  TaskStep,
  TaskPriority,
} from "$shared/ipc/bindings";

const PAGE_SIZE = 100;
const SEARCH_DEBOUNCE_MS = 180;
const REFRESH_DEBOUNCE_MS = 100;
/** A pause long enough to be the end of a thought rather than of a word.
 *  Leaving the field, the task or the window saves at once, so closing
 *  mid-sentence never waits on it. */
const TEXT_DEBOUNCE_MS = 1000;
const UNDO_DEPTH = 16;
/** A busy or unanswered write is sent again after each of these. Short, so a
 *  closing window still sees it settle. */
const RETRY_DELAYS = [500, 1500, 4000];
/** The fields whose change can move a task between views, lists or counts. */
const COUNTED = new Set<TaskField["field"]>(["status", "schedule", "deadline", "organization"]);
/** A row drawn for a task native has not created yet. It has no id to act on. */
const PLACEHOLDER = "pending:";

/** Everything a task row draws, flattened out of the wire shape.
 *
 *  A row is a projection, never authority: `pending` marks one drawn from the
 *  user's intent while native has yet to settle it.
 */
export type TaskRow = {
  id: string;
  revision: string;
  title: string;
  /** Null until the body has been read: a listing does not carry one. */
  description: string | null;
  createdAt: string | null;
  pinned: boolean;
  updatedAt: string;
  status: TaskStatus;
  assignee: TaskActor;
  origin: TaskActor;
  dueDate: string | null;
  dueTime: string | null;
  deadline: string | null;
  duration: number | null;
  context: TaskContext | null;
  sortKey: string | null;
  work: string | null;
  pending: boolean;
  list: string | null;
  inbox: boolean;
  priority: TaskPriority;
  steps?: TaskStep[];
  stepCount: number;
  stepDone: number;
  completedAt: string | null;
};

export type TaskInput = {
  title: string;
  dueDate?: string | null;
  dueTime?: string | null;
  deadline?: string | null;
  duration?: number | null;
  context?: TaskContext | null;
  origin?: TaskActor;
  list?: string | null;
  inbox?: boolean;
  priority?: TaskPriority;
  /** The capture field as it read when this was submitted, when it has been
   *  emptied since. */
  draft?: string;
};

/** What a reversible action did, so a surface can offer to take it back. */
export type TaskAction = "complete" | "delete" | "restore" | "change";
export type TaskNotice = { serial: number; action: TaskAction; title: string };

/** A reversal the reader can still ask for. It re-enters the same write path as
 *  the action it undoes, so an undo is settled by native exactly like one. */
type Undoable = { id: string; action: TaskAction; title: string; run: () => Promise<boolean> };

type Patch = Partial<TaskRow>;
type Job = {
  patch: Patch;
  /** A field update, written onto whatever revision native holds. */
  set?: TaskField[];
  expect?: TaskField[];
  /** A whole-record intent, which needs the current revision to build. */
  build?: (record: ResourceRecord) => ResourceIntent | null;
  command?: ResourceCommand;
  error: string | null;
  undo?: Undoable;
  hide?: boolean;
};
type Body = { description: string; createdAt: string; revision: string; steps: TaskStep[] };
type TaskContentOf = Extract<ResourceDraft["content"], { kind: "task" }>;

const emptyCounts = (): TaskCounts => ({
  inbox: 0,
  today: 0,
  overdue: 0,
  upcoming: 0,
  all: 0,
  completed: 0,
  trash: 0,
});

function localDay(): string {
  const date = new SvelteDate();
  return `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, "0")}-${String(date.getDate()).padStart(2, "0")}`;
}

function detailsOf(task: TaskContentOf) {
  return {
    list: task.details?.list ?? null,
    inbox: task.details?.inbox ?? false,
    priority: task.details?.priority ?? "none",
    steps: task.details?.steps ?? [],
    completed_at: task.details?.completed_at ?? null,
    deadline: task.details?.deadline ?? null,
    duration: task.details?.duration ?? null,
  };
}

function metadataOf(id: string, task: TaskContentOf): TaskMetadata {
  const details = detailsOf(task);
  return {
    id,
    list: details.list,
    inbox: details.inbox,
    priority: details.priority,
    steps: details.steps.length,
    steps_done: details.steps.filter((step) => step.completed).length,
    completed_at: details.completed_at,
    deadline: details.deadline,
    duration: details.duration,
  };
}

function toRow(item: ResourceSummary, meta?: TaskMetadata, body?: Body): TaskRow {
  return {
    id: item.id,
    revision: item.revision,
    title: item.title,
    description: body?.description ?? null,
    createdAt: body?.createdAt ?? null,
    pinned: item.pinned,
    updatedAt: item.updated_at,
    status: item.status ?? (item.completed ? "done" : "open"),
    assignee: item.assignee ?? "user",
    origin: item.origin ?? "user",
    dueDate: item.due_date,
    dueTime: item.due_time,
    deadline: meta?.deadline ?? null,
    duration: meta?.duration ?? null,
    context: item.context,
    sortKey: item.sort_key,
    work: item.work,
    pending: false,
    list: meta?.list ?? null,
    inbox: meta?.inbox ?? false,
    priority: meta?.priority ?? "none",
    steps: body?.steps,
    stepCount: meta?.steps ?? 0,
    stepDone: meta?.steps_done ?? 0,
    completedAt: meta?.completed_at ?? null,
  };
}

function summarize(record: ResourceRecord): ResourceSummary {
  const task = record.draft.content.kind === "task" ? record.draft.content : null;
  return {
    id: record.id,
    revision: record.revision,
    title: record.draft.title,
    pinned: record.draft.pinned,
    updated_at: record.updated_at,
    completed: task?.completed ?? null,
    due_date: task?.due_date ?? null,
    due_time: task?.due_time ?? null,
    status: task?.status ?? null,
    assignee: task?.assignee ?? null,
    origin: task?.origin ?? null,
    context: task?.context ?? null,
    sort_key: task?.sort_key ?? null,
    work: task?.work ?? null,
  };
}

/** How a written field reads back on a row, so it can be drawn before native settles. */
function patchOf(field: TaskField): Patch {
  switch (field.field) {
    case "title":
      return { title: field.value };
    case "description":
      return { description: field.value };
    case "status":
      return { status: field.value };
    case "schedule":
      return { dueDate: field.date, dueTime: field.time };
    case "deadline":
      return { deadline: field.date };
    case "duration":
      return { duration: field.minutes };
    case "organization":
      return { list: field.list, inbox: field.inbox };
    case "priority":
      return { priority: field.value };
    case "steps":
      return {
        steps: field.value,
        stepCount: field.value.length,
        stepDone: field.value.filter((step) => step.completed).length,
      };
    case "pinned":
      return { pinned: field.value };
    case "position":
      return { sortKey: field.sort_key };
  }
}

/** The value a row holds for the same property, which is what undo writes back. */
function current(row: TaskRow, field: TaskField): TaskField | null {
  switch (field.field) {
    case "title":
      return { field: "title", value: row.title };
    case "description":
      return row.description === null ? null : { field: "description", value: row.description };
    case "status":
      return { field: "status", value: row.status };
    case "schedule":
      return { field: "schedule", date: row.dueDate, time: row.dueTime };
    case "deadline":
      return { field: "deadline", date: row.deadline };
    case "duration":
      return { field: "duration", minutes: row.duration };
    case "organization":
      return { field: "organization", list: row.list, inbox: row.inbox };
    case "priority":
      return { field: "priority", value: row.priority };
    case "steps":
      return row.steps ? { field: "steps", value: row.steps } : null;
    case "pinned":
      return { field: "pinned", value: row.pinned };
    case "position":
      return { field: "position", sort_key: row.sortKey };
  }
}

function byId<T extends { id: string }>(items: readonly T[]): ReadonlyMap<string, T> {
  return new Map(items.map((item) => [item.id, item]));
}

function sameSteps(left: readonly TaskStep[], right: readonly TaskStep[]): boolean {
  return (
    left.length === right.length &&
    left.every(
      (step, index) =>
        step.id === right[index]!.id &&
        step.title === right[index]!.title &&
        step.completed === right[index]!.completed,
    )
  );
}

/** A host owns a query and transient edits. Rust owns task identity and outcomes.
 * Jobs keep their request identity until settlement; failed drafts survive hiding
 * the host. An earlier reply only removes its own overlay, never newer input. */
export class TaskSession {
  readonly profile: string;
  items = $state.raw<ResourceSummary[]>([]);
  query = $state("");
  scope = $state<TaskView>("all");
  listId = $state<string | null>(null);
  lists = $state.raw<TaskList[]>([]);
  #metadata = new SvelteMap<string, TaskMetadata>();
  #listMutation = $state.raw<ResourceCommand | null>(null);
  #listFailure = $state<string | null>(null);
  #listBusy: Promise<TaskList | null> | null = null;
  day = $state("");
  trash = $state(false);
  loading = $state(false);
  error = $state<string | null>(null);
  next = $state<string | null>(null);
  counts = $state.raw<TaskCounts>(emptyCounts());
  captureDraft = $state("");
  captureContext = $state.raw<TaskContext | null>(null);
  selectedId = $state<string | null>(null);
  /** The latest reversible action worth announcing, until it is undone or replaced. */
  notice = $state.raw<TaskNotice | null>(null);
  #bodies = new SvelteMap<string, Body>();
  #drafts = new SvelteMap<string, Patch>();
  #jobs = new SvelteMap<string, Job[]>();
  #chain = new SvelteMap<string, Promise<boolean>>();
  #textTimers = new SvelteMap<string, ReturnType<typeof setTimeout>>();
  /** A text field's pending save. `settle` is the reader leaving the field,
   *  which may tidy what they typed; a pause in typing may not. */
  #commit = new SvelteMap<string, (settle: boolean) => void>();
  #undo = $state.raw<Undoable[]>([]);
  #serial = 0;
  #creation = $state.raw<{
    command: ResourceCommand;
    input: TaskInput;
    /** The capture field as it read when this was submitted. */
    draft: string;
    error: string | null;
  } | null>(null);
  #creating: Promise<string | null> | null = null;
  #active = false;
  #epoch = 0;
  #listing = 0;
  #stop: (() => void) | null = null;
  #refreshTimer: ReturnType<typeof setTimeout> | undefined;
  #overviewTimer: ReturnType<typeof setTimeout> | undefined;
  #observeTimer: ReturnType<typeof setTimeout> | undefined;
  #observed = new SvelteSet<string>();
  #searchTimer: ReturnType<typeof setTimeout> | undefined;
  readonly removeCloseTask: () => void;

  constructor(profile: string) {
    this.profile = profile;
    this.removeCloseTask = registerCloseTask(() => this.flush());
  }

  /** The last projection of each listed task and the inputs it was drawn from.
   *  A row whose inputs are untouched keeps its identity, so typing in one title
   *  does not redraw every other row. Keyed by the listing entry itself, so a
   *  replaced or dropped entry takes its projection with it. */
  #drawn = new WeakMap<
    ResourceSummary,
    {
      meta?: TaskMetadata;
      body?: Body;
      jobs?: Job[];
      draft?: Patch;
      row: TaskRow;
    }
  >();

  #index = $derived(byId(this.items));

  rows: TaskRow[] = $derived.by(() => {
    const rows = this.items.flatMap((item) => {
      const meta = this.#metadata.get(item.id);
      const body = this.#bodies.get(item.id);
      const jobs = this.#jobs.get(item.id);
      const draft = this.#drafts.get(item.id);
      if (jobs?.some((job) => job.hide && !job.error)) return [];
      const cached = this.#drawn.get(item);
      let row: TaskRow;
      if (
        cached &&
        cached.meta === meta &&
        cached.body === body &&
        cached.jobs === jobs &&
        cached.draft === draft
      )
        row = cached.row;
      else {
        row = toRow(item, meta, body);
        for (const job of jobs ?? []) row = { ...row, ...job.patch };
        row = { ...row, ...draft, pending: jobs !== undefined || draft !== undefined };
        this.#drawn.set(item, { meta, body, jobs, draft, row });
      }
      // A failed edit stays reachable even once its row has left the view.
      if (!this.#member(row) && !jobs?.some((job) => job.error)) return [];
      return [row];
    });
    return rows;
  });

  failure = $derived(
    this.#listFailure ??
      this.#creation?.error ??
      [...this.#jobs.values()].flat().find((job) => job.error)?.error ??
      this.error,
  );
  /** Whether a write to this task is on its way to native. */
  saving(id: string): boolean {
    return this.#chain.has(id);
  }
  get active(): boolean {
    return this.#active;
  }
  get undoable() {
    return this.#undo.length > 0;
  }
  get empty() {
    return !this.loading && this.rows.length === 0 && this.failure === null;
  }
  get retained() {
    return (
      this.#jobs.size > 0 ||
      this.#drafts.size > 0 ||
      this.#creation !== null ||
      this.#listMutation !== null ||
      this.captureDraft.length > 0
    );
  }

  /** Membership native decides by query, re-checked for rows edited since. Day
   *  and completion are placed by the list's sections, which animate the exit. */
  #member(row: TaskRow): boolean {
    if (this.trash || this.query.trim()) return true;
    if (this.listId) return row.list === this.listId;
    if (this.scope === "inbox") return row.inbox;
    return true;
  }

  async start(query = "") {
    if (this.#active) return;
    this.query = query.slice(0, 512);
    this.#active = true;
    const epoch = ++this.#epoch;
    try {
      const stop = await events.resourceChanged.listen(({ payload }) => {
        if (epoch !== this.#epoch || payload.profile !== this.profile) return;
        if (payload.kind === "task") this.#observe(payload.id, payload.revision);
        else if (payload.kind === "task_list") this.#scheduleOverview();
      });
      if (!this.#active || epoch !== this.#epoch) {
        stop();
        return;
      }
      this.#stop = stop;
      await this.reload();
    } catch {
      this.error = "unavailable";
    }
  }

  stop(): void {
    if (!this.#active) return;
    this.#active = false;
    const epoch = ++this.#epoch;
    this.#listing++;
    clearTimeout(this.#searchTimer);
    clearTimeout(this.#refreshTimer);
    clearTimeout(this.#overviewTimer);
    clearTimeout(this.#observeTimer);
    this.#observed.clear();
    this.#stop?.();
    this.#stop = null;
    // The rows stay, so a host shown again draws them while they are read
    // afresh instead of flashing empty; the registry lets go of a clean,
    // stopped session once others need the room.
    void this.flush().then((saved) => {
      if (epoch !== this.#epoch) return;
      this.loading = false;
      if (saved && !this.retained) this.error = null;
    });
  }

  async flush(): Promise<boolean> {
    for (const timer of this.#textTimers.values()) clearTimeout(timer);
    this.#textTimers.clear();
    const commits = [...this.#commit.values()];
    this.#commit.clear();
    for (const commit of commits) commit(true);
    await Promise.allSettled([
      ...this.#chain.values(),
      ...(this.#creating ? [this.#creating] : []),
      ...(this.#listBusy ? [this.#listBusy] : []),
    ]);
    return (
      this.#jobs.size === 0 &&
      this.#drafts.size === 0 &&
      this.#creation === null &&
      this.#listMutation === null
    );
  }

  search(value: string) {
    this.query = value.slice(0, 512);
    clearTimeout(this.#searchTimer);
    this.#searchTimer = setTimeout(() => void this.reload(false, true), SEARCH_DEBOUNCE_MS);
  }

  setView(scope: TaskView, day: string, list: string | null = this.listId) {
    if (scope === this.scope && day === this.day && list === this.listId) return;
    this.listId = list;
    this.scope = scope;
    this.day = day;
    if (this.#active) void this.reload(false, true);
  }

  showTrash(trashed: boolean) {
    if (this.trash === trashed) return;
    this.trash = trashed;
    if (this.#active) void this.reload(false, true);
  }

  async retry() {
    this.error = null;
    if (this.#creation) await this.#saveCreation();
    if (this.#listMutation) {
      if (this.#listFailure === "conflict") {
        await this.reload();
        const intent = this.#listMutation.intent;
        if (intent.kind === "rename_task_list" || intent.kind === "delete_task_list") {
          const found = this.lists.find((list) => list.id === intent.id);
          if (found)
            this.#listMutation = {
              ...this.#listMutation,
              request_id: crypto.randomUUID(),
              intent: { ...intent, expected_revision: found.revision },
            };
        }
      }
      await this.#saveList();
    }
    for (const [id, jobs] of this.#jobs) {
      // Explicit retry after a conflict means saving the retained local version.
      // An uncertain write instead replays its exact command and request ID.
      for (const job of jobs) {
        if (job.error !== "outcome_unknown") {
          job.command = undefined;
          job.expect = undefined;
        }
        job.error = null;
      }
      this.#jobs.set(id, [...jobs]);
      await this.#drain(id);
    }
    await this.reload();
  }

  /** Gives up every unsaved edit and returns to what native holds. */
  async discard() {
    for (const timer of this.#textTimers.values()) clearTimeout(timer);
    this.#textTimers.clear();
    this.#commit.clear();
    this.#jobs.clear();
    this.#drafts.clear();
    this.#creation = null;
    this.#listMutation = null;
    this.#listFailure = null;
    this.error = null;
    await this.reload();
  }

  async reload(more = false, reset = false) {
    clearTimeout(this.#refreshTimer);
    if (!this.#active || (more && (!this.next || this.loading))) return;
    const generation = ++this.#listing;
    this.loading = true;
    const today = this.day || localDay();
    const searching = this.query.trim();
    const query = {
      list: searching ? null : this.listId,
      view: this.trash ? "trash" : searching ? "all" : this.scope,
      today,
      search: searching,
      limit: PAGE_SIZE,
    } as const;
    let response = await this.#call({
      kind: "list_tasks",
      query: { ...query, after: more ? this.next : null },
    });
    if (generation !== this.#listing || !this.#active) return;
    if (response.kind !== "task_page") {
      this.loading = false;
      this.error = response.kind === "error" ? response.error : "unavailable";
      return;
    }
    const page = response;
    const target = more || reset ? PAGE_SIZE : Math.max(PAGE_SIZE, this.items.length);
    while (page.next && page.items.length < target) {
      response = await this.#call({ kind: "list_tasks", query: { ...query, after: page.next } });
      if (generation !== this.#listing || !this.#active) return;
      if (response.kind !== "task_page") {
        this.loading = false;
        this.error = response.kind === "error" ? response.error : "unavailable";
        return;
      }
      page.items = mergePage(page.items, response.items);
      page.next = response.next;
      page.counts = response.counts;
      page.lists = response.lists;
      page.metadata = [...page.metadata, ...response.metadata];
    }
    this.error = null;
    this.loading = false;
    this.counts = page.counts;
    this.lists = page.lists;
    for (const meta of page.metadata) this.#metadata.set(meta.id, meta);
    // Keep a failed edit reachable even when its authoritative row left the query.
    const retained = this.items.filter(
      (item) =>
        (this.#jobs.has(item.id) || this.#drafts.has(item.id)) &&
        !page.items.some((next) => next.id === item.id),
    );
    this.items = more ? mergePage(this.items, page.items) : mergePage(page.items, retained);
    this.next = page.next;
    if (!more) this.#prune();
    const selected = this.items.find((item) => item.id === this.selectedId);
    if (selected && this.#bodies.get(selected.id)?.revision !== selected.revision)
      void this.load(selected.id);
  }

  async load(id: string): Promise<void> {
    this.selectedId = id;
    const revision = this.items.find((item) => item.id === id)?.revision;
    if (this.#bodies.get(id)?.revision === revision) return;
    const found = await this.#call({ kind: "get", id });
    if (found.kind !== "record" || found.record.draft.content.kind !== "task") {
      this.error = found.kind === "error" ? found.error : "unavailable";
      return;
    }
    this.#accept(found.record);
  }

  async create(input: TaskInput): Promise<string | null> {
    const title = input.title.trim().slice(0, 256);
    const pending = this.#creation;
    if (pending) {
      // The same text again is a retry of the capture that failed, never a
      // second task; different text waits until the earlier one has landed.
      const settled = await this.#saveCreation();
      if (settled === null || pending.input.title.trim() === title) return settled;
    }
    if (!title) return null;
    const list = input.list === undefined ? this.listId : input.list;
    const draft: ResourceDraft = {
      title,
      pinned: false,
      related: [],
      content: {
        kind: "task",
        details: {
          list,
          inbox: input.inbox ?? !list,
          priority: input.priority ?? "none",
          steps: [],
          completed_at: null,
          deadline: input.deadline ?? null,
          duration: input.duration ?? null,
        },
        description: "",
        completed: false,
        status: "open",
        assignee: "user",
        origin: input.origin ?? "user",
        due_date: input.dueDate ?? null,
        due_time: input.dueDate ? (input.dueTime ?? null) : null,
        context: input.context
          ? { url: input.context.url, title: input.context.title.slice(0, 256) }
          : null,
        sort_key: null,
        work: null,
      },
    };
    this.#creation = {
      input: { ...input, title },
      draft: input.draft ?? this.captureDraft,
      error: null,
      command: { version: 1, request_id: crypto.randomUUID(), intent: { kind: "create", draft } },
    };
    return this.#saveCreation();
  }

  #saveCreation(): Promise<string | null> {
    if (this.#creating) return this.#creating;
    const creation = this.#creation;
    if (!creation) return Promise.resolve(null);
    this.#creating = (async () => {
      const response = await this.#mutate(creation.command);
      if (response.kind !== "applied" || response.request_id !== creation.command.request_id) {
        const error = response.kind === "error" ? response.error : "outcome_unknown";
        this.#creation = { ...creation, error };
        return null;
      }
      this.#ack(creation.command.request_id);
      this.#creation = null;
      // Settled from anywhere, a retry included, the capture is spent; words
      // typed since are a different task and stay.
      if (this.captureDraft === creation.draft) {
        this.captureDraft = "";
        this.captureContext = null;
      }
      this.#accept(response.record);
      this.#remember({
        id: response.record.id,
        action: "change",
        title: response.record.draft.title,
        run: () => this.setTrashed(response.record.id, true, false),
      });
      this.#scheduleOverview();
      return response.record.id;
    })().finally(() => {
      this.#creating = null;
    });
    return this.#creating;
  }

  setStatus(id: string, status: TaskStatus, reversible = true): Promise<boolean> {
    const row = this.#row(id);
    const action = status === "done" && row?.status !== "done" ? "complete" : "change";
    return this.#update(id, [{ field: "status", value: status }], { reversible, action });
  }

  move(id: string, status: TaskStatus, sortKey: string | null, reversible = true) {
    return this.#update(
      id,
      [
        { field: "status", value: status },
        { field: "position", sort_key: sortKey },
      ],
      { reversible, action: status === "done" ? "complete" : "change" },
    );
  }

  setPinned(id: string, pinned: boolean) {
    return this.#update(id, [{ field: "pinned", value: pinned }]);
  }

  schedule(id: string, dueDate: string | null, dueTime: string | null = null, reversible = true) {
    return this.#update(
      id,
      [{ field: "schedule", date: dueDate, time: dueDate === null ? null : dueTime }],
      { reversible },
    );
  }

  /** A manual position, which drag and its keyboard equal write. */
  setPosition(id: string, sortKey: string) {
    return this.#update(id, [{ field: "position", sort_key: sortKey }], { reversible: false });
  }

  setDeadline(id: string, deadline: string | null) {
    return this.#update(id, [{ field: "deadline", date: deadline }]);
  }

  setDuration(id: string, minutes: number | null) {
    return this.#update(id, [{ field: "duration", minutes }]);
  }

  organize(id: string, list: string | null, inbox = false) {
    return this.#update(id, [{ field: "organization", list, inbox }]);
  }

  prioritize(id: string, priority: TaskPriority) {
    return this.#update(id, [{ field: "priority", value: priority }]);
  }

  rename(id: string, title: string) {
    this.#edit(id, "title", title.slice(0, 256));
  }

  describe(id: string, description: string) {
    // Rust bounds UTF-8 bytes; 4096 UTF-16 units never exceed its 16 KiB limit.
    this.#edit(id, "description", description.slice(0, 4096));
  }

  #edit(id: string, field: "title" | "description", value: string) {
    const key = `${field}:${id}`;
    this.#drafts.set(id, { ...this.#drafts.get(id), [field]: value });
    clearTimeout(this.#textTimers.get(key));
    // The base is what this typing replaces, taken when it began and never from
    // a draft: a draft is not what native holds, so expecting it would conflict.
    if (!this.#commit.has(key)) {
      const base = this.#settled(id)?.[field] ?? null;
      const commit = (settle: boolean) => {
        const raw = this.#drafts.get(id)?.[field];
        if (raw === undefined || raw === null) return;
        if (field === "title" && !raw.trim()) {
          // A blank title is a word being retyped, not a save; leaving it
          // blank gives the old title back.
          if (settle) this.#dropDraft(id, field);
          else this.#commit.set(key, commit);
          return;
        }
        this.#dropDraft(id, field);
        // Trimming while the field is in use would rewrite it under the
        // caret, so a trailing space is kept until the reader leaves.
        const text = field === "title" && settle ? raw.trim() : raw;
        if (text === base) return;
        void this.#update(id, [{ field, value: text }], {
          reversible: false,
          expect: base === null ? [] : [{ field, value: base }],
        });
      };
      this.#commit.set(key, commit);
    }
    this.#debounce(key);
  }

  renameStep(id: string, stepId: string, title: string) {
    const base = this.#settled(id)?.steps;
    if (!base) return;
    const steps = (this.#drafts.get(id)?.steps ?? base).map((step) =>
      step.id === stepId ? { ...step, title: title.slice(0, 256) } : step,
    );
    this.#drafts.set(id, { ...this.#drafts.get(id), steps });
    const key = `steps:${id}`;
    clearTimeout(this.#textTimers.get(key));
    if (!this.#commit.has(key)) {
      const commit = (settle: boolean) => {
        const draft = this.#drafts.get(id)?.steps;
        if (!draft) return;
        if (!settle && draft.some((step) => !step.title.trim())) {
          this.#commit.set(key, commit);
          return;
        }
        this.#dropDraft(id, "steps");
        // A step left blank keeps the title it had.
        const value = settle
          ? draft.flatMap((step) => {
              const title = step.title.trim() || base.find((old) => old.id === step.id)?.title;
              return title ? [{ ...step, title }] : [];
            })
          : draft;
        if (sameSteps(value, base)) return;
        void this.#update(id, [{ field: "steps", value }], {
          reversible: false,
          expect: [{ field: "steps", value: base }],
        });
      };
      this.#commit.set(key, commit);
    }
    this.#debounce(key);
  }

  /** Saves whatever is being typed into a task now, as leaving its fields
   *  does; every task's when no id is given. */
  commitText(id?: string) {
    for (const [key, commit] of [...this.#commit]) {
      if (id !== undefined && key.slice(key.indexOf(":") + 1) !== id) continue;
      clearTimeout(this.#textTimers.get(key));
      this.#textTimers.delete(key);
      this.#commit.delete(key);
      commit(true);
    }
  }

  #dropDraft(id: string, field: keyof Patch) {
    const remaining = { ...this.#drafts.get(id) };
    delete remaining[field];
    if (Object.keys(remaining).length) this.#drafts.set(id, remaining);
    else this.#drafts.delete(id);
  }

  /** What native holds plus writes on their way, without anything still
   *  being typed. */
  #settled(id: string): TaskRow | undefined {
    const item = this.#index.get(id);
    if (!item) return undefined;
    let row = toRow(item, this.#metadata.get(id), this.#bodies.get(id));
    for (const job of this.#jobs.get(id) ?? []) row = { ...row, ...job.patch };
    return row;
  }

  async updateSteps(id: string, steps: TaskStep[]) {
    if (!(await this.flush())) return false;
    const value = steps.map((step) => ({
      id: step.id,
      title: step.title,
      completed: step.completed,
    }));
    return this.#update(id, [{ field: "steps", value }], { reversible: false });
  }

  setTrashed(id: string, trashed: boolean, reversible = true): Promise<boolean> {
    const title = this.#row(id)?.title ?? "";
    return this.#enqueue(id, {
      patch: {},
      build: (record) => ({
        kind: trashed ? "trash" : "restore",
        id,
        expected_revision: record.revision,
      }),
      error: null,
      hide: trashed !== this.trash,
      undo: reversible
        ? {
            id,
            action: trashed ? "delete" : "restore",
            title,
            run: () => this.setTrashed(id, !trashed, false),
          }
        : undefined,
    });
  }

  async undo(): Promise<void> {
    const entry = this.#undo.at(-1);
    if (!entry) return;
    this.#undo = this.#undo.slice(0, -1);
    this.notice = null;
    await entry.run();
  }

  dismissNotice() {
    this.notice = null;
  }

  #remember(entry: Undoable) {
    this.#undo = [...this.#undo, entry].slice(-UNDO_DEPTH);
    if (entry.action !== "change")
      this.notice = { serial: ++this.#serial, action: entry.action, title: entry.title };
  }

  #row(id: string): TaskRow | undefined {
    return this.rows.find((row) => row.id === id);
  }

  #debounce(key: string) {
    this.#textTimers.set(
      key,
      setTimeout(() => {
        this.#textTimers.delete(key);
        const commit = this.#commit.get(key);
        this.#commit.delete(key);
        commit?.(false);
      }, TEXT_DEBOUNCE_MS),
    );
  }

  #update(
    id: string,
    set: TaskField[],
    options: { reversible?: boolean; action?: TaskAction; expect?: TaskField[] } = {},
  ): Promise<boolean> {
    const row = this.#row(id);
    const inverse = row ? set.map((field) => current(row, field)) : [];
    const undo =
      options.reversible !== false && row && inverse.every((field) => field !== null)
        ? {
            id,
            action: options.action ?? "change",
            title: row.title,
            run: () => this.#update(id, inverse as TaskField[], { reversible: false }),
          }
        : undefined;
    return this.#enqueue(id, {
      patch: Object.assign({}, ...set.map(patchOf)) as Patch,
      set,
      expect: options.expect,
      error: null,
      undo,
    });
  }

  #enqueue(id: string, job: Job): Promise<boolean> {
    if (id.startsWith(PLACEHOLDER)) return Promise.resolve(false);
    this.#jobs.set(id, [...(this.#jobs.get(id) ?? []), job]);
    return this.#drain(id);
  }

  #drain(id: string): Promise<boolean> {
    const running = this.#chain.get(id);
    if (running) return running;
    const run = (async () => {
      let counted = false;
      while (this.#jobs.get(id)?.length) {
        const job = this.#jobs.get(id)![0]!;
        if (job.error) return false;
        if (!job.command) {
          let intent: ResourceIntent | null;
          if (job.set) intent = { kind: "update_task", id, set: job.set, expect: job.expect ?? [] };
          else {
            const found = await this.#call({ kind: "get", id });
            if (found.kind !== "record" || found.record.draft.content.kind !== "task") {
              this.#fail(id, job, found.kind === "error" ? found.error : "unavailable");
              return false;
            }
            intent = job.build?.(found.record) ?? null;
          }
          if (!intent) {
            this.#fail(id, job, "invalid");
            return false;
          }
          job.command = { version: 1, request_id: crypto.randomUUID(), intent };
        }
        const response = await this.#mutate(job.command);
        if (response.kind !== "applied" || response.request_id !== job.command.request_id) {
          this.#fail(id, job, response.kind === "error" ? response.error : "outcome_unknown");
          return false;
        }
        this.#ack(job.command.request_id);
        this.#accept(response.record);
        const rest = this.#jobs.get(id)!.slice(1);
        if (rest.length) this.#jobs.set(id, rest);
        else this.#jobs.delete(id);
        if (job.undo) this.#remember(job.undo);
        counted ||= !job.set || job.set.some((field) => COUNTED.has(field.field));
      }
      // Totals only move with what places a task; typing a title moves none.
      if (counted) this.#scheduleOverview();
      return true;
    })().finally(() => this.#chain.delete(id));
    this.#chain.set(id, run);
    return run;
  }

  #fail(id: string, job: Job, error: string) {
    job.error = error;
    this.#jobs.set(id, [...this.#jobs.get(id)!]);
  }

  #accept(record: ResourceRecord) {
    const task = record.draft.content;
    // A session nobody is viewing, such as the launcher's capture, keeps no
    // projection: it would only grow with every task it saves.
    if (task.kind !== "task" || !this.#active) return;
    const old = this.#index.get(record.id);
    if (old && BigInt(old.revision) > BigInt(record.revision)) return;
    if (record.trashed !== this.trash) {
      this.items = this.items.filter((item) => item.id !== record.id);
      this.#prune();
      return;
    }
    this.items = settleInto(this.items, summarize(record));
    this.#metadata.set(record.id, metadataOf(record.id, task));
    this.#bodies.set(record.id, {
      steps: detailsOf(task).steps,
      description: task.description,
      createdAt: record.created_at,
      revision: record.revision,
    });
  }

  /** Another host or actor changed a task. Fetch just that task rather than
   *  reloading every loaded page; a search needs native's matching, so reloads. */
  #observe(id: string, revision: string) {
    const known = this.#index.get(id);
    if (known && !newerRevision(revision, known.revision)) return;
    if (this.#chain.has(id)) return;
    // A task created here is announced before its reply arrives, and the reply
    // carries the record; only what is still unknown after it needs reading.
    if (!known && this.#creating) {
      void this.#creating.then(() => this.#observe(id, revision));
      return;
    }
    if (!known && this.query.trim()) {
      this.#scheduleRefresh();
      return;
    }
    this.#observed.add(id);
    clearTimeout(this.#observeTimer);
    this.#observeTimer = setTimeout(() => void this.#fetchObserved(), REFRESH_DEBOUNCE_MS);
  }

  async #fetchObserved() {
    const ids = [...this.#observed];
    this.#observed.clear();
    const epoch = this.#epoch;
    for (const id of ids) {
      const found = await this.#call({ kind: "get", id });
      if (epoch !== this.#epoch) return;
      if (found.kind === "record") this.#accept(found.record);
      else if (found.kind === "error" && found.error === "not_found") {
        this.items = this.items.filter((item) => item.id !== id);
        this.#prune();
      }
    }
    this.#scheduleOverview();
  }

  /** Lets go of what was read for tasks no longer listed. */
  #prune() {
    const listed = this.#index;
    for (const id of [...this.#metadata.keys()]) if (!listed.has(id)) this.#metadata.delete(id);
    for (const id of [...this.#bodies.keys()]) if (!listed.has(id)) this.#bodies.delete(id);
  }

  async saveList(title: string, id?: string): Promise<TaskList | null> {
    if (this.#listMutation) return this.#saveList();
    const found = this.lists.find((list) => list.id === id);
    const value = title.trim().slice(0, 64);
    if (!value) return null;
    this.#listMutation = {
      version: 1,
      request_id: crypto.randomUUID(),
      intent: found
        ? {
            kind: "rename_task_list",
            id: found.id,
            expected_revision: found.revision,
            title: value,
          }
        : { kind: "create_task_list", title: value },
    };
    return this.#saveList();
  }

  async deleteList(id: string): Promise<boolean> {
    if (this.#listMutation) return false;
    const found = this.lists.find((list) => list.id === id);
    if (!found) return false;
    this.#listMutation = {
      version: 1,
      request_id: crypto.randomUUID(),
      intent: { kind: "delete_task_list", id, expected_revision: found.revision },
    };
    return (await this.#saveList()) !== null;
  }

  #saveList(): Promise<TaskList | null> {
    if (this.#listBusy) return this.#listBusy;
    const command = this.#listMutation;
    if (!command) return Promise.resolve(null);
    this.#listBusy = (async () => {
      const response = await this.#mutate(command);
      if (response.kind !== "task_list_applied" || response.request_id !== command.request_id) {
        this.#listFailure = response.kind === "error" ? response.error : "outcome_unknown";
        return null;
      }
      this.#ack(command.request_id);
      this.#listFailure = null;
      this.#listMutation = null;
      this.lists = this.lists.filter((list) => list.id !== response.list.id);
      if (!response.list.deleted)
        this.lists = [...this.lists, response.list].sort((a, b) => a.title.localeCompare(b.title));
      // Deleting a list moves its tasks, which only a reload can show.
      if (response.list.deleted) this.#scheduleRefresh();
      return response.list;
    })().finally(() => (this.#listBusy = null));
    return this.#listBusy;
  }

  /** A busy or unanswered write is sent again as the same request, which
   *  native settles once however often it arrives. */
  async #mutate(command: ResourceCommand) {
    for (let attempt = 0; ; attempt++) {
      const response = await this.#call({ kind: "mutate", command });
      const delay = RETRY_DELAYS[attempt];
      if (
        delay === undefined ||
        response.kind !== "error" ||
        (response.error !== "capacity" && response.error !== "outcome_unknown")
      )
        return response;
      await new Promise((resolve) => setTimeout(resolve, delay));
    }
  }

  #ack(request: string) {
    void this.#call({ kind: "acknowledge", request_id: request });
  }

  #scheduleRefresh() {
    if (!this.#active) return;
    clearTimeout(this.#refreshTimer);
    this.#refreshTimer = setTimeout(() => void this.reload(), REFRESH_DEBOUNCE_MS);
  }

  /** Counts and lists only: a settled write already carries its own record. */
  #scheduleOverview() {
    if (!this.#active) return;
    clearTimeout(this.#overviewTimer);
    this.#overviewTimer = setTimeout(async () => {
      const epoch = this.#epoch;
      const response = await this.#call({ kind: "task_overview", today: this.day || localDay() });
      if (epoch !== this.#epoch || response.kind !== "task_overview") return;
      this.counts = response.counts;
      this.lists = response.lists;
    }, REFRESH_DEBOUNCE_MS);
  }

  #call(call: ResourceCall) {
    return resourceCall(this.profile, call);
  }
}

/** What is due by `today`, overdue included, without a page of rows: for a
 *  surface that shows how much is left rather than what it is. */
export async function taskCounts(profile: string, today: string): Promise<TaskCounts | null> {
  const response = await resourceCall(profile, { kind: "task_overview", today });
  return response.kind === "task_overview" ? response.counts : null;
}

/** The profile's lists, without a page of rows, for a capture made outside any
 *  task view. */
export async function taskLists(profile: string, today: string): Promise<TaskList[]> {
  const response = await resourceCall(profile, { kind: "task_overview", today });
  return response.kind === "task_overview" ? response.lists : [];
}

/** Sessions kept for hosts to come back to, beyond which the one used least
 *  recently is let go. One that is showing, or that holds anything unsaved,
 *  is never let go to make room. */
const MAX_SESSIONS = 6;
type Kept = { session: TaskSession; dispose: () => void; used: number };
const sessions = new SvelteMap<string, Kept>();
let uses = 0;
let watching = false;
/** A hidden window may be closed or suspended without another chance to
 *  save, so what is being typed is saved as it hides. */
function watchVisibility() {
  if (watching || typeof document === "undefined") return;
  watching = true;
  document.addEventListener("visibilitychange", () => {
    if (document.visibilityState !== "hidden") return;
    for (const { session } of sessions.values()) session.commitText();
  });
}
function makeRoom() {
  while (sessions.size >= MAX_SESSIONS) {
    let oldest: [string, Kept] | undefined;
    for (const entry of sessions)
      if (
        !entry[1].session.active &&
        !entry[1].session.retained &&
        (!oldest || entry[1].used < oldest[1].used)
      )
        oldest = entry;
    if (!oldest) return;
    const [key, { session, dispose }] = oldest;
    sessions.delete(key);
    session.removeCloseTask();
    dispose();
  }
}
export function taskSession(profile: string, host: string): TaskSession {
  const key = `${profile}:${host}`;
  const existing = sessions.get(key);
  if (existing) {
    existing.used = ++uses;
    return existing.session;
  }
  watchVisibility();
  makeRoom();
  // Built under its own root: a session outlives the view that first asked for
  // it, and deriveds created during that view's setup would die with it.
  let built: TaskSession | undefined;
  const dispose = $effect.root(() => {
    built = new TaskSession(profile);
  });
  // Without a DOM there is no reactive owner to escape, and no root either.
  const session = built ?? new TaskSession(profile);
  sessions.set(key, { session, dispose, used: ++uses });
  return session;
}
