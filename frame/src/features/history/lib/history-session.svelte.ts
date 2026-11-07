import { SvelteMap } from "svelte/reactivity";
import type {
  HistoryCall,
  HistoryError,
  HistoryRange,
  HistoryVisitView,
} from "$shared/ipc/bindings";
import { commands } from "$shared/ipc/bindings";
import { observe } from "$shared/lib/observe";

const PAGE_SIZE = 100;
const SEARCH_DEBOUNCE_MS = 180;
const RESPONSE_DEADLINE_MS = 8000;

/** Visits held in memory. The list is windowed, so this bounds retained data
 *  rather than rendered nodes; beyond it the reader narrows the search. */
const MAX_LOADED_VISITS = 5000;

export class HistorySession {
  readonly profile: string;

  visits = $state.raw<HistoryVisitView[]>([]);
  query = $state("");
  /** How far back the list reaches, and what Clear removes. */
  range = $state<HistoryRange>("everything");
  loading = $state(false);
  busy = $state(false);
  error = $state<HistoryError | null>(null);

  /** Cursor for the next page, or null once the list is exhausted. */
  private cursor: string | null = null;
  private listing = 0;
  private lifetime = new AbortController();
  private searchTimer: ReturnType<typeof setTimeout> | undefined;
  private started = false;

  constructor(profile: string) {
    this.profile = profile;
  }

  get exhausted() {
    return this.cursor === null;
  }

  get capped() {
    return this.visits.length >= MAX_LOADED_VISITS;
  }

  get empty() {
    return !this.loading && this.visits.length === 0 && this.error === null;
  }

  /** `query` seeds the first request, so a surface that reopens with a search
   *  already in its field never flashes the unfiltered list first. */
  async start(query = "") {
    if (this.started) return;
    this.started = true;
    this.query = query.slice(0, 512);
    await this.reload();
  }

  stop() {
    this.started = false;
    this.listing += 1;
    clearTimeout(this.searchTimer);
    this.lifetime.abort();
    this.lifetime = new AbortController();
    // A reopened surface shows an empty field, so it must not still be
    // filtering by whatever was typed last time.
    this.visits = [];
    this.cursor = null;
    this.query = "";
    this.range = "everything";
    this.error = null;
    this.loading = false;
    this.busy = false;
  }

  search(value: string) {
    this.listing += 1;
    this.cursor = null;
    this.query = value.slice(0, 512);
    clearTimeout(this.searchTimer);
    this.searchTimer = setTimeout(() => void this.reload(), SEARCH_DEBOUNCE_MS);
  }

  scope(range: HistoryRange) {
    if (range === this.range) return;
    this.range = range;
    this.cursor = null;
    void this.reload();
  }

  /** Loads the first page, or appends the next one. */
  async reload(more = false) {
    if (more && (this.exhausted || this.capped || this.loading)) return;
    const generation = ++this.listing;
    this.loading = true;
    const response = await this.call({
      kind: "page",
      query: this.query.trim(),
      range: this.range,
      before: more ? this.cursor : null,
      limit: PAGE_SIZE,
    });
    if (generation !== this.listing) return;
    this.loading = false;
    if (!response) {
      this.error = "unavailable";
      return;
    }
    if (response.kind === "error") {
      this.error = response.error;
      return;
    }
    if (response.kind !== "page") return;
    this.error = null;
    this.cursor = response.next;
    this.visits = more ? [...this.visits, ...response.visits] : response.visits;
  }

  /** Removes every visit to each address, and the rows showing them. */
  async forget(urls: readonly string[]) {
    if (!urls.length || this.busy) return;
    this.listing += 1;
    this.loading = false;
    this.busy = true;
    const lifetime = this.lifetime;
    const response = await this.call({ kind: "forget", urls: [...urls] });
    if (lifetime.signal.aborted) return;
    this.busy = false;
    if (response?.kind === "removed") {
      this.visits = this.visits.filter((visit) => !urls.includes(visit.url));
      await this.reload();
      return;
    }
    this.error = response?.kind === "error" ? response.error : "unavailable";
  }

  /** Removes exactly what the list is showing. */
  async clear() {
    if (this.busy) return;
    this.busy = true;
    const lifetime = this.lifetime;
    const response = await this.call({ kind: "clear", range: this.range });
    if (lifetime.signal.aborted) return;
    this.busy = false;
    if (response?.kind === "removed") {
      this.cursor = null;
      await this.reload();
      return;
    }
    this.error = response?.kind === "error" ? response.error : "unavailable";
  }

  retry() {
    this.error = null;
    void this.reload();
  }

  private async call(call: HistoryCall) {
    const observation = await observe(
      commands.historyCall(this.profile, call),
      RESPONSE_DEADLINE_MS,
      this.lifetime.signal,
    );
    return observation.state === "received" ? observation.value : null;
  }
}

const sessions = new SvelteMap<string, HistorySession>();

/** One session per profile and host, so the page and the panel do not refetch
 *  each other's pages. */
export function historySession(profile: string, host: string): HistorySession {
  const key = `${profile}:${host}`;
  let session = sessions.get(key);
  if (!session) {
    session = new HistorySession(profile);
    if (sessions.size >= 8) {
      const oldest = sessions.keys().next();
      if (!oldest.done) sessions.delete(oldest.value);
    }
    sessions.set(key, session);
  }
  return session;
}
