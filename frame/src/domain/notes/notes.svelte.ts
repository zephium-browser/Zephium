import { SvelteMap } from "svelte/reactivity";
import type {
  NoteCall,
  NoteError,
  NoteRecord,
  NoteResponse,
  NoteSummary,
} from "$shared/ipc/bindings";
import { events } from "$shared/ipc/native-events";
import { registerCloseTask } from "$shared/lib/close";
import { blank, outline } from "./outline";
import { noteCall } from "./transport";

/** Typing pauses this long before a save. Leaving the note or the window
 *  saves at once. */
const SAVE_IDLE = 1200;
/** Continuous typing still saves at least this often. */
const SAVE_LONGEST = 4000;
const RETRY_DELAYS = [1500, 4000, 10_000, 30_000];
/** A retitle left alone this long is finished, and the file may take the name. */
const TITLE_SETTLE = 5000;
const PAGE = 100;
/** Listing refreshes after file changes wait for a burst to settle. */
const REFRESH_DELAY = 120;
const SEARCH_DELAY = 140;
const NOTICE_MS = 6000;
/** Top-level blocks read for a row's title and preview while typing. */
const PREVIEW_BLOCKS = 24;

export type SaveState = "saved" | "unsaved" | "saving" | "retrying" | "conflict" | "failed";

/** The note in the editor. `id` is null until a new note's first save. */
export type OpenNote = {
  id: string | null;
  summary: NoteSummary | null;
  /** The Markdown the editor loads. It changes only when the note is loaded
   *  again, never as the person types. */
  source: string;
  /** Increments whenever `source` is replaced, so the editor reloads. */
  version: number;
  editable: boolean;
  trashed: boolean;
};

export type NoteNotice = { kind: "trashed"; id: string; title: string; at: number };

/** The editor's text as Markdown, at most `blocks` top-level blocks of it. */
export type NoteReader = (blocks?: number) => string;

/** A `[[target]]` as native matches it to a title or a file name. */
function linkKey(target: string): string {
  const text = target.split(/[#^]/u)[0]!.trim();
  const stem = text.endsWith(".md") || text.endsWith(".MD") ? text.slice(0, -3) : text;
  return stem.normalize("NFC").toLowerCase().replace(/\s+/gu, " ").trim();
}

/** The listing's order: pinned first, then most recently changed. */
function listed(a: NoteSummary, b: NoteSummary): number {
  return (
    Number(b.pinned) - Number(a.pinned) ||
    Number(b.modified_at) - Number(a.modified_at) ||
    (a.id < b.id ? 1 : -1)
  );
}

let editions = 0;

/** One host's view of a profile's notes. The files are the record; this
 *  holds a page of their descriptions and the one note being edited. */
export class NoteSession {
  readonly profile: string;
  readonly host: string;
  items = $state.raw<NoteSummary[]>([]);
  next = $state<string | null>(null);
  loading = $state(false);
  loaded = $state(false);
  error = $state<NoteError | null>(null);
  query = $state("");
  trash = $state(false);
  note = $state.raw<OpenNote | null>(null);
  saveState = $state<SaveState>("saved");
  saveError = $state<NoteError | null>(null);
  /** The file's version when a save found it changed elsewhere. */
  conflict = $state.raw<NoteRecord | null>(null);
  notice = $state.raw<NoteNotice | null>(null);
  /** Why the chosen note could not be opened, when it could not. */
  openError = $state<NoteError | null>(null);
  backlinks = $state.raw<NoteSummary[]>([]);
  /** Increments when link targets may resolve differently. */
  linksRevision = $state(0);

  #markdown = "";
  /** Reads the editor's text when it is next needed; set while typing has
   *  moved on from `#markdown`. */
  #reader: NoteReader | null = null;
  #base: string | null = null;
  #edits = 0;
  #savedEdits = 0;
  #createRequest: string | null = null;
  #writing: Promise<boolean> | null = null;
  #timer: ReturnType<typeof setTimeout> | undefined;
  #firstUnsaved = 0;
  #retries = 0;
  /** The title the open note's file was last allowed to follow. */
  #settledTitle: string | null = null;
  /** A save changed the title since, so a settling write is owed. */
  #unsettled = false;
  /** The title the settling timer was last started for. */
  #timedTitle: string | null = null;
  #settleTimer: ReturnType<typeof setTimeout> | undefined;
  /** Notes announced while a new note's first save was on its way, taken to
   *  be that note until its reply says which it is. */
  #assumed: string[] = [];
  #refreshTimer: ReturnType<typeof setTimeout> | undefined;
  #searchTimer: ReturnType<typeof setTimeout> | undefined;
  #noticeTimer: ReturnType<typeof setTimeout> | undefined;
  #listing = 0;
  #epoch = 0;
  /** Increments whenever a different note, or a fresh load, is in the editor. */
  #document = 0;
  #active = false;
  #lifetime = new AbortController();
  #stop: (() => void) | null = null;
  #resume: string | null = null;
  #requested: string | null = null;
  #targets = new SvelteMap<string, NoteSummary | null>();
  readonly removeCloseTask: () => void;

  constructor(profile: string, host: string) {
    this.profile = profile;
    this.host = host;
    this.removeCloseTask = registerCloseTask(() => this.#flushForClose());
  }

  get markdown(): string {
    return this.#read();
  }

  #read(): string {
    if (this.#reader) {
      this.#markdown = this.#reader();
      this.#reader = null;
    }
    return this.#markdown;
  }

  #text(markdown: string): void {
    this.#markdown = markdown;
    this.#reader = null;
  }

  get active(): boolean {
    return this.#active;
  }

  get disposable(): boolean {
    return !this.#active && this.saveState === "saved";
  }

  #call(call: NoteCall): Promise<NoteResponse> {
    return noteCall(this.profile, call, this.#lifetime.signal);
  }

  async start(): Promise<void> {
    if (this.#active) return;
    this.#active = true;
    const epoch = ++this.#epoch;
    this.#lifetime = new AbortController();
    const stop = await events.notesChanged.listen(({ payload }) => {
      if (epoch !== this.#epoch || payload.profile !== this.profile) return;
      this.#changed(payload.notes, payload.reset, payload.links);
    });
    if (!this.#active || epoch !== this.#epoch) {
      stop();
      return;
    }
    this.#stop = stop;
    await this.reload();
    const requested = this.#requested ?? (this.note ? null : this.#resume);
    this.#requested = null;
    if (requested) await this.open(requested);
    else if (this.saveState === "unsaved") void this.flush();
  }

  /** Stops following changes while the host is hidden. A note with unsaved
   *  text keeps it; a saved one is let go and read again on return. */
  stop(): void {
    this.#active = false;
    ++this.#epoch;
    ++this.#listing;
    clearTimeout(this.#refreshTimer);
    clearTimeout(this.#searchTimer);
    this.#stop?.();
    this.#stop = null;
    // Pending text is saved before the host's calls are cut off.
    const lifetime = this.#lifetime;
    void this.settle().finally(() => {
      if (!this.#active) lifetime.abort();
    });
    clearTimeout(this.#settleTimer);
    this.items = [];
    this.next = null;
    this.loaded = false;
    this.loading = false;
    this.#targets.clear();
    if (this.saveState === "saved" && this.note?.id) {
      this.#resume = this.note.id;
      this.note = null;
      this.#text("");
      this.backlinks = [];
    }
  }

  async #flushForClose(): Promise<boolean> {
    const inactive = !this.#active;
    if (inactive) this.#lifetime = new AbortController();
    try {
      if (this.saveState === "retrying" || this.saveState === "failed") this.saveState = "unsaved";
      return await this.settle();
    } finally {
      if (inactive) this.#lifetime.abort();
    }
  }

  /** Opens a note once the host is showing. */
  async requestOpen(id: string): Promise<void> {
    if (this.#active) await this.open(id);
    else this.#requested = id;
  }

  // Listing

  #scheduleRefresh(delay = REFRESH_DELAY): void {
    if (!this.#active) return;
    clearTimeout(this.#refreshTimer);
    this.#refreshTimer = setTimeout(() => void this.reload(), delay);
  }

  async reload(more = false): Promise<void> {
    clearTimeout(this.#refreshTimer);
    const listing = ++this.#listing;
    this.loading = true;
    const result = await this.#call({
      kind: "list",
      query: {
        search: this.query,
        trashed: this.trash,
        after: more ? this.next : null,
        limit: PAGE,
      },
    });
    if (listing !== this.#listing || !this.#active) return;
    this.loading = false;
    if (result.kind !== "page") {
      this.error = result.kind === "error" ? result.error : "unavailable";
      return;
    }
    const fresh = result.items.map((item) => item.id);
    this.items = more
      ? [...this.items.filter((item) => !fresh.includes(item.id)), ...result.items]
      : result.items;
    this.next = result.next;
    this.error = null;
    this.loaded = true;
  }

  search(value: string): void {
    const query = value.slice(0, 512);
    if (query === this.query) return;
    this.query = query;
    clearTimeout(this.#searchTimer);
    this.#searchTimer = setTimeout(() => this.#scheduleRefresh(0), SEARCH_DELAY);
  }

  async showTrash(trash: boolean): Promise<void> {
    if (trash === this.trash) return;
    if (!(await this.close())) return;
    this.trash = trash;
    this.items = [];
    this.loaded = false;
    await this.reload();
  }

  #changed(
    notes: { id: string; revision: string | null }[],
    reset: boolean,
    links: string[],
  ): void {
    const creating = this.note !== null && this.note.id === null && this.#writing !== null;
    if (creating)
      for (const changed of notes)
        if (
          !this.items.some((item) => item.id === changed.id) &&
          !this.#assumed.includes(changed.id)
        )
          this.#assumed.push(changed.id);
    // Its own saves are news to everyone but this session, which has put the
    // row where it belongs already.
    const own = (changed: { id: string; revision: string | null }) =>
      changed.id === this.note?.id
        ? this.#writing !== null || changed.revision === this.#base
        : creating && this.#assumed.includes(changed.id);
    const others = notes.filter((changed) => !own(changed));
    // A rename keeps the revision, so a listed note named by links is stale too.
    const stale = others.some((changed) =>
      this.items.some(
        (item) =>
          item.id === changed.id && (item.revision !== changed.revision || links.length > 0),
      ),
    );
    if ((reset && (others.length > 0 || notes.length === 0)) || stale) this.#scheduleRefresh();
    if (reset) {
      this.#targets.clear();
      this.linksRevision++;
    } else if (links.length) {
      for (const [target, found] of this.#targets)
        if (
          links.includes(linkKey(target)) ||
          (found && notes.some((changed) => changed.id === found.id))
        )
          this.#targets.delete(target);
      this.linksRevision++;
    }
    const id = this.note?.id;
    const change = id ? notes.find((changed) => changed.id === id) : undefined;
    if (!change || this.#writing || change.revision === this.#base) return;
    if (change.revision === null) {
      // Removed elsewhere. Unsaved text will make a new file on its next save.
      if (this.saveState === "saved") this.#release();
      return;
    }
    if (this.saveState === "saved") void this.#refresh(id!);
    // With unsaved text, the next save meets the change as a conflict.
  }

  /** Reads the open note again after another app changed it. */
  async #refresh(id: string): Promise<void> {
    const result = await this.#call({ kind: "get", id });
    if (result.kind !== "record" || this.note?.id !== id || this.saveState !== "saved") return;
    if (result.record.summary.revision === this.#base) return;
    this.#load(result.record);
  }

  // Opening

  #load(record: NoteRecord): void {
    this.#document++;
    this.openError = null;
    clearTimeout(this.#timer);
    this.#unsettle(record.summary.title);
    this.#text(record.markdown);
    this.#base = record.summary.revision;
    this.#edits = this.#savedEdits = 0;
    this.#createRequest = null;
    this.conflict = null;
    this.saveState = "saved";
    this.saveError = null;
    this.note = {
      id: record.summary.id,
      summary: record.summary,
      source: record.markdown,
      version: ++editions,
      editable: record.summary.editable && !record.summary.trashed,
      trashed: record.summary.trashed,
    };
    void this.#loadBacklinks(record.summary.id);
  }

  #release(): void {
    this.#document++;
    this.openError = null;
    clearTimeout(this.#timer);
    this.#unsettle(null);
    this.note = null;
    this.#text("");
    this.#base = null;
    this.#resume = null;
    this.backlinks = [];
    this.conflict = null;
    this.saveState = "saved";
    this.saveError = null;
  }

  /** Leaves the open note: saves it, and puts away a note left empty. */
  async close(): Promise<boolean> {
    const note = this.note;
    if (!note) return true;
    if (!note.id && blank(this.#read())) {
      this.#release();
      return true;
    }
    if (!(await this.settle())) return false;
    if (this.note !== note && this.note?.id !== note.id) return true;
    const id = this.note?.id;
    const empty = id && note.editable && blank(this.#read());
    this.#release();
    // An emptied note is not worth a file; it goes where deleted notes go.
    if (empty) {
      await this.#call({ kind: "trash", id });
      this.#scheduleRefresh(0);
    }
    return true;
  }

  async open(id: string): Promise<boolean> {
    if (this.note?.id === id) return true;
    if (!(await this.close())) return false;
    const epoch = this.#epoch;
    const result = await this.#call({ kind: "get", id });
    if (epoch !== this.#epoch) return false;
    if (result.kind !== "record") {
      // Shown in place of the note, so choosing it is never a silent no-op.
      const summary = this.items.find((item) => item.id === id) ?? null;
      this.#document++;
      this.#text("");
      this.note = {
        id,
        summary,
        source: "",
        version: ++editions,
        editable: false,
        trashed: !!summary?.trashed,
      };
      this.openError = result.kind === "error" ? result.error : "unavailable";
      return false;
    }
    this.#load(result.record);
    return true;
  }

  /** Starts a note that has no file until it has content. */
  async create(markdown = ""): Promise<boolean> {
    if (!(await this.close())) return false;
    if (this.trash) {
      this.trash = false;
      void this.reload();
    }
    this.#document++;
    this.openError = null;
    this.#unsettle(null);
    this.#text(markdown);
    this.#base = null;
    this.#edits = markdown ? 1 : 0;
    this.#savedEdits = 0;
    this.#createRequest = null;
    this.saveState = markdown ? "unsaved" : "saved";
    this.#firstUnsaved = performance.now();
    this.note = {
      id: null,
      summary: null,
      source: markdown,
      version: ++editions,
      editable: true,
      trashed: false,
    };
    this.backlinks = [];
    if (markdown) await this.flush();
    return true;
  }

  // Editing

  /** Takes typing. The editor passes a reader rather than its text, so a
   *  keystroke costs a few blocks of serializing for the row's preview and
   *  the whole note is written out only when it is saved. */
  edit(input: string | NoteReader): void {
    if (!this.note?.editable) return;
    if (typeof input !== "string") this.#reader = input;
    else if (input === this.#read()) return;
    else this.#markdown = input;
    this.#edits++;
    this.#preview();
    if (this.saveState === "conflict") return;
    if (this.saveState !== "unsaved") {
      this.saveState = "unsaved";
      this.#firstUnsaved = performance.now();
    }
    this.#schedule(SAVE_IDLE);
  }

  /** Updates the open note's row as it is typed, before the save lands. */
  #preview(): void {
    const summary = this.note?.summary;
    if (!summary) return;
    const { heading, preview } = outline(this.#head());
    const title = heading ?? summary.title;
    if (title === summary.title && preview === summary.preview) return;
    const next = { ...summary, title, preview };
    this.note = { ...this.note!, summary: next };
    this.items = this.items.map((item) => (item.id === next.id ? next : item));
  }

  /** Enough of the text for a title and a preview. */
  #head(): string {
    return this.#reader ? this.#reader(PREVIEW_BLOCKS) : this.#markdown;
  }

  #schedule(delay: number): void {
    clearTimeout(this.#timer);
    const overdue = SAVE_LONGEST - (performance.now() - this.#firstUnsaved);
    this.#timer = setTimeout(() => void this.flush(false), Math.max(0, Math.min(delay, overdue)));
  }

  /** Saves everything typed and lets the file take its note's new title, as
   *  leaving the note does. */
  settle(): Promise<boolean> {
    return this.flush(true, true);
  }

  /** Saves until the file holds everything typed, or a save cannot proceed.
   *  A background save (`drain: false`) writes once and leaves typing that
   *  continued meanwhile to the next pause, so a long burst is not a burst
   *  of writes. */
  async flush(drain = true, settle = false): Promise<boolean> {
    clearTimeout(this.#timer);
    let followed = false;
    for (;;) {
      if (this.#writing) {
        if (!drain) return false;
        await this.#writing;
        continue;
      }
      if (this.saveState === "conflict" || this.saveState === "failed") return false;
      if (!this.note || this.#savedEdits === this.#edits) {
        if (this.saveState !== "retrying") this.saveState = "saved";
        if (settle && this.#unsettled && this.saveState === "saved" && !followed) {
          followed = true;
          await this.#follow();
          continue;
        }
        return this.saveState === "saved";
      }
      if (!(await this.#write(settle))) return false;
      if (!drain) {
        if (this.saveState === "unsaved") this.#schedule(SAVE_IDLE);
        return this.saveState === "saved";
      }
    }
  }

  #write(settle = false): Promise<boolean> {
    const note = this.note!;
    const edits = this.#edits;
    const markdown = this.#read();
    const epoch = this.#epoch;
    const document = this.#document;
    this.saveState = "saving";
    const work = (async (): Promise<boolean> => {
      if (!note.id && blank(markdown)) {
        this.#savedEdits = edits;
        return true;
      }
      const request = note.id ? crypto.randomUUID() : (this.#createRequest ??= crypto.randomUUID());
      const result = await this.#call(
        note.id
          ? {
              kind: "write",
              request_id: request,
              id: note.id,
              base_revision: this.#base!,
              markdown,
              settle,
            }
          : { kind: "create", request_id: request, markdown },
      );
      if (!note.id) this.#identify(result.kind === "applied" ? result.summary.id : null);
      // The editor moved on to another note; this one is saved as far as it goes.
      if (document !== this.#document) return true;
      switch (result.kind) {
        case "applied":
          this.#applied(result.summary, edits, settle || !note.id);
          return true;
        case "conflict":
          if (result.current.markdown === markdown) {
            this.#applied(result.current.summary, edits, false);
            return true;
          }
          this.conflict = result.current;
          this.saveState = "conflict";
          return false;
        case "error":
          return this.#failed(result.error, epoch);
        default:
          return this.#failed("unavailable", epoch);
      }
    })();
    this.#writing = work;
    void work.finally(() => {
      if (this.#writing === work) this.#writing = null;
    });
    return work;
  }

  /** `settled`: the file name now follows the title, or never has to. */
  #applied(summary: NoteSummary, edits: number, settled: boolean): void {
    const created = !this.note?.id;
    this.#base = summary.revision;
    if (settled) this.#unsettle(summary.title);
    else if (summary.title !== this.#settledTitle) {
      this.#unsettled = true;
      // Timed from the last change to the title, not from every save.
      if (summary.title !== this.#timedTitle) {
        this.#timedTitle = summary.title;
        clearTimeout(this.#settleTimer);
        this.#settleTimer = setTimeout(() => void this.settle(), TITLE_SETTLE);
      }
    }
    this.#savedEdits = Math.max(this.#savedEdits, edits);
    this.#createRequest = null;
    this.#retries = 0;
    this.saveError = null;
    if (this.note) {
      const { heading, preview } = outline(this.#head());
      const current =
        this.#savedEdits === this.#edits
          ? summary
          : { ...summary, title: heading ?? summary.title, preview };
      this.note = { ...this.note, id: summary.id, summary: current };
      this.saveState = this.#savedEdits === this.#edits ? "saved" : "unsaved";
      if (this.saveState === "unsaved") this.#firstUnsaved = performance.now();
      this.#upsert(current);
      if (created) void this.#loadBacklinks(summary.id);
    }
  }

  /** Puts a saved note's row where the listing would, without a reload. */
  #upsert(summary: NoteSummary): void {
    const at = this.items.findIndex((item) => item.id === summary.id);
    const before = this.items[at - 1];
    const after = this.items[at + 1];
    // Saving the note already at the top of its place, the usual case while
    // typing, leaves the order as it is; only a note that moved is sorted.
    if (
      at >= 0 &&
      (this.trash ||
        this.query ||
        ((!before || listed(before, summary) < 0) && (!after || listed(summary, after) < 0)))
    ) {
      const items = [...this.items];
      items[at] = summary;
      this.items = items;
      return;
    }
    if (this.trash || this.query) return;
    this.items = [...this.items.filter((item) => item.id !== summary.id), summary].sort(listed);
  }

  /** Lets a saved note's file take its title. Only the name is at stake, and
   *  native keeps what it was named after, so a failure waits for the next
   *  time the note is left rather than holding anything up. */
  async #follow(): Promise<void> {
    const note = this.note;
    const base = this.#base;
    if (!note?.id || !note.editable || base === null) return;
    const document = this.#document;
    this.#unsettled = false;
    const result = await this.#call({
      kind: "write",
      request_id: crypto.randomUUID(),
      id: note.id,
      base_revision: base,
      markdown: this.#read(),
      settle: true,
    });
    if (document !== this.#document) return;
    if (result.kind !== "applied" || result.summary.revision !== this.#base) {
      this.#unsettled ||= result.kind !== "applied";
      return;
    }
    this.#settledTitle = result.summary.title;
    const path = result.summary.path;
    if (this.note?.summary && this.note.summary.path !== path) {
      this.note = { ...this.note, summary: { ...this.note.summary, path } };
      this.items = this.items.map((item) => (item.id === note.id ? { ...item, path } : item));
    }
  }

  #unsettle(title: string | null): void {
    clearTimeout(this.#settleTimer);
    this.#settledTitle = title;
    this.#timedTitle = null;
    this.#unsettled = false;
  }

  /** A new note's first save has landed, or failed: announcements taken to be
   *  it that were another note's are a listing change after all. */
  #identify(id: string | null): void {
    const strangers = this.#assumed.some((assumed) => assumed !== id);
    this.#assumed = [];
    if (strangers) this.#scheduleRefresh();
  }

  #failed(error: NoteError, epoch: number): boolean {
    this.saveError = error;
    if (error === "not_found" && this.note?.id) {
      // The file was removed while this text was open; keep the text as a
      // new note rather than lose it.
      this.note = { ...this.note, id: null, summary: null };
      this.#base = null;
      this.saveState = "unsaved";
      this.#schedule(0);
      return false;
    }
    if (error === "read_only" || error === "too_large" || error === "invalid") {
      this.saveState = "failed";
      return false;
    }
    // Unknown outcome or a busy service: the same write is safe to repeat.
    this.saveState = "retrying";
    const delay = RETRY_DELAYS[Math.min(this.#retries++, RETRY_DELAYS.length - 1)]!;
    clearTimeout(this.#timer);
    this.#timer = setTimeout(() => {
      if (epoch !== this.#epoch && !this.#active) return;
      this.saveState = "unsaved";
      void this.flush(false);
    }, delay);
    return false;
  }

  /** Settles a conflict: `mine` writes this text over the other version,
   *  `theirs` takes the other version and drops this text. */
  async resolve(choice: "mine" | "theirs"): Promise<void> {
    const current = this.conflict;
    if (!current || !this.note) return;
    this.conflict = null;
    if (choice === "theirs") {
      this.#load(current);
      return;
    }
    this.#base = current.summary.revision;
    this.saveState = "unsaved";
    await this.flush();
  }

  /** Tries a failed save again. */
  async retry(): Promise<void> {
    if (this.saveState === "failed" || this.saveState === "retrying") {
      this.saveState = "unsaved";
      await this.flush();
    }
  }

  // Note actions

  async setPinned(id: string, pinned: boolean): Promise<void> {
    const result = await this.#call({ kind: "set_pinned", id, pinned });
    if (result.kind !== "applied") return;
    if (this.note?.id === id)
      this.note = {
        ...this.note,
        summary: { ...result.summary, title: this.note.summary?.title ?? result.summary.title },
      };
    this.#upsert(result.summary);
  }

  async moveToTrash(id: string): Promise<boolean> {
    const title =
      this.items.find((item) => item.id === id)?.title ?? this.note?.summary?.title ?? "";
    if (this.note?.id === id) {
      if (!(await this.flush())) return false;
      this.#release();
    }
    const result = await this.#call({ kind: "trash", id });
    if (result.kind !== "applied") return false;
    this.items = this.items.filter((item) => item.id !== id);
    this.#notify({ kind: "trashed", id, title, at: Date.now() });
    return true;
  }

  async restore(id: string): Promise<boolean> {
    const result = await this.#call({ kind: "restore", id });
    if (result.kind !== "applied") return false;
    if (this.notice?.id === id) this.dismissNotice();
    if (this.trash) this.items = this.items.filter((item) => item.id !== id);
    else this.#upsert(result.summary);
    if (this.note?.id === id) this.#load({ summary: result.summary, markdown: this.#read() });
    return true;
  }

  async deleteForever(id: string): Promise<boolean> {
    const result = await this.#call({ kind: "delete", id });
    if (result.kind !== "done") return false;
    this.items = this.items.filter((item) => item.id !== id);
    if (this.note?.id === id) this.#release();
    return true;
  }

  /** A note's Markdown as it is on disk, or as typed if it is open. */
  async markdownOf(id: string): Promise<string | null> {
    if (this.note?.id === id) return this.#read();
    const result = await this.#call({ kind: "get", id });
    return result.kind === "record" ? result.record.markdown : null;
  }

  async reveal(id: string | null = null): Promise<void> {
    await this.#call({ kind: "reveal", id });
  }

  #notify(notice: NoteNotice): void {
    clearTimeout(this.#noticeTimer);
    this.notice = notice;
    this.#noticeTimer = setTimeout(() => {
      if (this.notice === notice) this.notice = null;
    }, NOTICE_MS);
  }

  /** Keeps a notice on screen while it is being read. */
  holdNotice(held: boolean): void {
    const notice = this.notice;
    clearTimeout(this.#noticeTimer);
    if (!held && notice)
      this.#noticeTimer = setTimeout(() => {
        if (this.notice === notice) this.notice = null;
      }, NOTICE_MS / 2);
  }

  dismissNotice(): void {
    clearTimeout(this.#noticeTimer);
    this.notice = null;
  }

  async undo(): Promise<void> {
    const notice = this.notice;
    if (notice?.kind === "trashed") await this.restore(notice.id);
  }

  // Links

  /** Notes matching a partly typed `[[` link, most recent first. */
  async find(query: string): Promise<NoteSummary[]> {
    const result = await this.#call({
      kind: "list",
      query: { search: query.slice(0, 512), trashed: false, after: null, limit: 8 },
    });
    return result.kind === "page" ? result.items.filter((item) => item.id !== this.note?.id) : [];
  }

  /** What each `[[target]]` currently points at; cached until notes appear
   *  or disappear. */
  async resolveTargets(targets: string[]): Promise<Record<string, NoteSummary | null>> {
    const key = (target: string) => target.normalize("NFC").toLowerCase().trim();
    const missing = targets
      .map(key)
      .filter((target, index, all) => all.indexOf(target) === index && !this.#targets.has(target));
    for (let at = 0; at < missing.length; at += 128) {
      const result = await this.#call({ kind: "resolve", targets: missing.slice(at, at + 128) });
      if (result.kind !== "targets") break;
      for (const item of result.items) this.#targets.set(key(item.target), item.note);
    }
    return Object.fromEntries(
      targets.map((target) => [target, this.#targets.get(key(target)) ?? null]),
    );
  }

  async #loadBacklinks(id: string): Promise<void> {
    const result = await this.#call({ kind: "backlinks", id });
    if (this.note?.id === id) this.backlinks = result.kind === "page" ? result.items : [];
  }
}
