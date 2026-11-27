import { SvelteMap, SvelteSet } from "svelte/reactivity";
import type {
  ResourceCommand_Deserialize as ResourceCommand,
  ResourceRecord_Serialize as ResourceRecord,
  ResourceResponse_Serialize as ResourceResponse,
  WorkArtifactDataV1_Deserialize as ObjectData,
  WorkArtifactDataV1_Serialize as ObjectRead,
} from "$shared/ipc/bindings";
import { resourceCall } from "./transport";
import { events } from "$shared/ipc/native-events";
import { registerCloseTask } from "$shared/lib/close";
import { newerRevision } from "./resource-model";

const RETRY_DELAYS = [500, 1500, 4000];
/** A pause in typing before the words are written: one write per thought, not per key. */
const WRITE_AFTER_MS = 450;

export type OwnedObject = { id: string; revision: string; title: string; data: ObjectRead };

/** What is kept of an edit: a row still being typed (no words yet) is shown, never written. */
function kept(data: ObjectData): ObjectData {
  if (data.kind === "list")
    return { ...data, items: data.items.filter((item) => item.title.trim()) };
  if (data.kind === "plan")
    return { ...data, steps: data.steps.filter((step) => step.title.trim()) };
  if (data.kind === "sheet")
    return { ...data, rows: data.rows.filter((row) => row.cells.some((cell) => cell.trim())) };
  return data;
}

/**
 * The person's own objects on a canvas — the tables, checklists and
 * calendars they made — read once and written as they change. What they
 * see is always their latest edit; native settles each write once.
 */
export class OwnedObjects {
  readonly #profile: string;
  #stop: (() => void) | null = null;
  #epoch = 0;
  readonly #records = new SvelteMap<string, ResourceRecord>();
  /** Edits not yet settled, shown over the record they will replace. */
  readonly #pending = new SvelteMap<string, ObjectData>();
  readonly #timers = new Map<string, ReturnType<typeof setTimeout>>();
  readonly #writing = new Map<string, Promise<void>>();
  readonly #loading = new Set<string>();
  /** Resources read once, whatever they turned out to be. */
  readonly #read = new SvelteSet<string>();
  readonly #failed = new SvelteSet<string>();
  readonly #removeCloseTask: () => void;
  #stopped = false;

  constructor(profile: string) {
    this.#profile = profile;
    this.#removeCloseTask = registerCloseTask(async () => {
      const saved = await this.flushAll();
      if (saved && this.#stopped) this.#removeCloseTask();
      return saved;
    });
  }

  /** Agent edits arrive through the same authoritative resource event as user edits. */
  async start() {
    this.#stopped = false;
    const epoch = ++this.#epoch;
    const stop = await events.resourceChanged.listen(({ payload }) => {
      if (
        epoch !== this.#epoch ||
        payload.profile !== this.#profile ||
        payload.kind !== "object" ||
        (!this.#read.has(payload.id) &&
          !this.#records.has(payload.id) &&
          !this.#loading.has(payload.id))
      )
        return;
      if (this.#pending.has(payload.id)) return; // Keep its old basis: a conflict must not overwrite remote edits.
      void this.#call({ kind: "get", id: payload.id }).then((response) => {
        if (epoch === this.#epoch && !this.#pending.has(payload.id) && response.kind === "record")
          this.#admit(response.record);
      });
    });
    if (epoch !== this.#epoch) stop();
    else this.#stop = stop;
  }

  stop() {
    this.#stopped = true;
    ++this.#epoch;
    this.#stop?.();
    this.#stop = null;
    void this.flushAll().then((saved) => {
      if (saved && this.#stopped) this.#removeCloseTask();
    });
  }

  get failed(): boolean {
    return this.#failed.size > 0;
  }

  #admit(record: ResourceRecord) {
    const current = this.#records.get(record.id);
    if (!current || newerRevision(record.revision, current.revision))
      this.#records.set(record.id, record);
  }

  /** The object as the person last left it, or undefined until it is read. */
  get(id: string): OwnedObject | undefined {
    const record = this.#records.get(id);
    if (record?.trashed || record?.draft.content.kind !== "object") return undefined;
    const data =
      (this.#pending.get(id) as ObjectRead | undefined) ?? record.draft.content.object.data;
    return { id, revision: record.revision, title: record.draft.title, data };
  }

  /** Durable identity tokens only; unsaved drafts are not admitted as context. */
  revisions(): ReadonlyMap<string, string> {
    return new Map(
      [...this.#records]
        .filter(([id, record]) => !record.trashed && !this.#pending.has(id))
        .map(([id, record]) => [id, record.revision]),
    );
  }

  /** Settle local typing before a request reads or edits these blocks. */
  async flushAll(): Promise<boolean> {
    await Promise.all([...this.#pending.keys()].map((id) => this.flush(id)));
    return this.#pending.size === 0;
  }

  /** Whether a resource has been read: until then, whether it is an object is not known. */
  read(id: string): boolean {
    return this.#read.has(id) || this.#records.has(id);
  }

  /** Reads the objects a canvas holds that are not read yet; anything else is left alone. */
  async load(ids: Iterable<string>) {
    for (const id of ids) {
      if (this.#records.has(id) || this.#loading.has(id)) continue;
      this.#loading.add(id);
      void this.#call({ kind: "get", id })
        .then((response) => {
          if (response.kind === "record" && response.record.draft.content.kind === "object")
            this.#admit(response.record);
        })
        .finally(() => {
          this.#loading.delete(id);
          this.#read.add(id);
        });
    }
  }

  /** A new object of the person's; its id once native holds it. */
  async create(title: string, data: ObjectData): Promise<string | null> {
    const command: ResourceCommand = {
      version: 1,
      request_id: crypto.randomUUID(),
      intent: {
        kind: "create",
        draft: {
          title,
          pinned: false,
          related: [],
          content: { kind: "object", object: { version: 1, data, evidence: [] } },
        },
      },
    };
    const response = await this.#mutate(command);
    if (response.kind !== "applied" || response.request_id !== command.request_id) return null;
    this.#ack(command.request_id);
    this.#records.set(response.record.id, response.record);
    return response.record.id;
  }

  /** Shows the change at once and writes it after a pause; a later change replaces it. */
  update(id: string, data: ObjectData) {
    this.#pending.set(id, data);
    clearTimeout(this.#timers.get(id));
    this.#timers.set(
      id,
      setTimeout(() => void this.flush(id), WRITE_AFTER_MS),
    );
  }

  /** Writes what is pending now, after any write already under way. */
  async flush(id: string): Promise<void> {
    clearTimeout(this.#timers.get(id));
    this.#timers.delete(id);
    const before = this.#writing.get(id) ?? Promise.resolve();
    const next = before.then(() => this.#write(id));
    this.#writing.set(id, next);
    await next;
  }

  async #write(id: string, retry = true): Promise<void> {
    const shown = this.#pending.get(id);
    const data = shown && kept(shown);
    const record = this.#records.get(id);
    if (!data || record?.draft.content.kind !== "object") return;
    const command: ResourceCommand = {
      version: 1,
      request_id: crypto.randomUUID(),
      intent: {
        kind: "replace",
        id,
        expected_revision: record.revision,
        draft: {
          title: record.draft.title,
          pinned: record.draft.pinned,
          related: record.draft.related,
          content: {
            kind: "object",
            object: { ...record.draft.content.object, data },
          },
        },
      },
    };
    const response = await this.#mutate(command);
    if (response.kind === "applied" && response.request_id === command.request_id) {
      this.#ack(command.request_id);
      this.#records.set(id, response.record);
      // Only the edit this write carried is settled; a newer one stays pending.
      if (this.#pending.get(id) === shown) this.#pending.delete(id);
      this.#failed.delete(id);
      return;
    }
    if (response.kind === "error" && response.error === "conflict" && retry) {
      const fresh = await this.#call({ kind: "get", id });
      if (
        fresh.kind === "record" &&
        fresh.record.id === id &&
        !fresh.record.trashed &&
        fresh.record.draft.content.kind === "object" &&
        newerRevision(fresh.record.revision, record.revision) &&
        JSON.stringify(fresh.record.draft.content.object.data) ===
          JSON.stringify(record.draft.content.object.data)
      ) {
        this.#admit(fresh.record);
        await this.#write(id, false);
        return;
      }
    }
    // Keep both versions when the content changed; guessing a merge could erase
    // another editor's words. Metadata-only changes can safely take the edit.
    this.#failed.add(id);
  }

  async #mutate(command: ResourceCommand): Promise<ResourceResponse> {
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

  #call(call: Parameters<typeof resourceCall>[1]) {
    return resourceCall(this.#profile, call);
  }
}
