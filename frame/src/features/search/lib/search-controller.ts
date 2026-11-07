import type { SearchContext, SearchResults, SearchResult } from "$shared/ipc/bindings";
import { sameSearch } from "./search-model";

export type SearchSnapshot = {
  results: SearchResult[];
  /** Host native offers the field, present only on a response for this exact query. */
  completion: string | null;
  /** Native providers are still working. Drives the empty state, never a spinner. */
  pending: boolean;
  /** The visible rows answer the current query and may be executed now. */
  settled: boolean;
  /** The query the visible rows actually answer, which lags the typed text
   *  while a replacement is in flight. Emphasis is measured against this, so a
   *  row's highlight cannot drop out and return as the next answer lands. */
  answered: string;
  error: "none" | "failed" | "too_long";
};

/** A burst of keystrokes is one search. Short enough to be imperceptible when
 *  the typist pauses, long enough that a fast burst is a single native request
 *  instead of one per character — each of which costs an actor pass, a history
 *  read, a note-title read and a network observation. */
const COALESCE_MS = 60;

/** Failsafe only. Native carries real provider progress in `pending`; this
 *  exists so a native that never answers at all cannot wait forever. */
const RESPONSE_DEADLINE_MS = 5000;
const RETRY_MS = 150;
/** How long a first, partial answer shorter than the rows on screen waits for
 *  the rest of it. Local matches land before online suggestions; shown as
 *  they come, the list shrinks and grows back on every key. */
const PARTIAL_HOLD_MS = 160;

export function createSearchController(options: {
  owner: Omit<SearchContext, "request_id"> | null;
  send: (query: string, id: string) => Promise<boolean>;
  update: (snapshot: SearchSnapshot) => void;
  /** New Tab shows nothing for an empty field, so it never asks native. */
  emptyQuery?: "search" | "skip";
  /** What the host is already showing. A controller made for a new presentation
   *  starts from it, so its first publication keeps those rows on screen
   *  instead of emptying the list until the first answer lands. */
  shown?: { results: SearchResult[]; answered: string };
  id?: () => string;
}) {
  let query = "";
  let composing = false;
  let disposed = false;
  let active: SearchContext | null = null;
  let settledContext: SearchContext | null = null;
  let received = false;
  let visible: SearchResult[] = options.shown?.results ?? [];
  let answered = options.shown?.answered ?? "";
  let completion: string | null = null;
  let debounce: ReturnType<typeof setTimeout> | undefined;
  let deadline: ReturnType<typeof setTimeout> | undefined;
  let hold: ReturnType<typeof setTimeout> | undefined;
  /** A request for this query has already been sent again after failing. A
   *  refused or unanswered search is almost always transient, and saying so
   *  on the first miss turns a hiccup into an error the user has to handle. */
  let retried = false;

  const publish = (snapshot: SearchSnapshot) => {
    if (!disposed) options.update(snapshot);
  };
  const clear = () => {
    clearTimeout(debounce);
    clearTimeout(deadline);
    clearTimeout(hold);
  };
  /** Retained rows stay on screen so the list does not blink between queries,
   *  but they are not `settled`: the surface queues intent against them rather
   *  than executing an action the newer query may no longer offer. */
  const retained = (error: SearchSnapshot["error"] = "none") => ({
    results: visible,
    completion: null,
    pending: true,
    settled: false,
    answered,
    error,
  });

  function fail(request: SearchContext) {
    if (disposed || !sameSearch(active, request) || received) return;
    clearTimeout(deadline);
    active = null;
    if (!retried) {
      retried = true;
      debounce = setTimeout(() => void send(), RETRY_MS);
      return;
    }
    publish({
      results: [],
      completion: null,
      pending: false,
      settled: false,
      answered: "",
      error: "failed",
    });
  }

  async function send() {
    if (disposed || composing) return;
    if (new TextEncoder().encode(query).byteLength > 2048) {
      publish({
        results: [],
        completion: null,
        pending: false,
        settled: false,
        answered: "",
        error: "too_long",
      });
      return;
    }
    if (!options.owner || (!query.trim() && options.emptyQuery === "skip")) {
      visible = [];
      completion = null;
      answered = "";
      settledContext = null;
      publish({
        results: [],
        completion: null,
        pending: false,
        settled: false,
        answered: "",
        error: "none",
      });
      return;
    }
    const request = { ...options.owner, request_id: (options.id ?? (() => crypto.randomUUID()))() };
    active = request;
    received = false;
    publish(retained());
    deadline = setTimeout(() => {
      if (disposed || !sameSearch(active, request)) return;
      if (!received) fail(request);
      else
        publish({
          results: visible,
          completion: null,
          pending: false,
          settled: true,
          answered,
          error: "none",
        });
    }, RESPONSE_DEADLINE_MS);
    try {
      if (!(await options.send(query, request.request_id))) fail(request);
    } catch {
      fail(request);
    }
  }

  function change(value: string) {
    const first = query === "" || value === "";
    query = value;
    retried = false;
    clear();
    active = null;
    settledContext = null;
    received = false;
    publish(retained());
    // The first character after an idle field answers at once; inside a burst
    // the requests coalesce.
    if (!composing && !disposed) debounce = setTimeout(() => void send(), first ? 0 : COALESCE_MS);
  }

  return {
    start: () => void send(),
    change,
    compositionStart() {
      composing = true;
      clear();
      active = null;
      settledContext = null;
      received = false;
      publish(retained());
    },
    compositionEnd(value: string) {
      composing = false;
      change(value);
    },
    /** The context a row may currently be executed against, or null while a
     *  replacement is in flight. */
    context: () => settledContext,
    request: () => active,
    query: () => query,
    receive(result: SearchResults) {
      if (disposed || !sameSearch(active, result.context) || result.query !== query) return;
      if (!result.pending) clearTimeout(deadline);
      // An empty intermediate response is a provider that had nothing, not an
      // answer. Keeping the previous rows is what stops the list blinking
      // "No matching results" between keystrokes.
      if (result.pending && result.results.length === 0) {
        received = false;
        publish(retained());
        return;
      }
      clearTimeout(hold);
      const apply = () => {
        if (disposed || !sameSearch(active, result.context)) return;
        received = true;
        settledContext = active;
        visible = result.results;
        answered = result.query;
        completion = result.completion;
        publish({
          results: visible,
          completion,
          pending: result.pending,
          settled: true,
          answered,
          error: "none",
        });
      };
      // A partial answer that would shrink the list waits briefly for the rest
      // of it, so the list changes once per answer instead of shrinking and
      // growing back; one that lengthens it, or the final one, shows at once.
      if (result.pending && result.results.length < visible.length) {
        hold = setTimeout(apply, PARTIAL_HOLD_MS);
        return;
      }
      apply();
    },
    pause() {
      clear();
      active = null;
      settledContext = null;
      received = false;
      publish(retained());
    },
    dispose() {
      disposed = true;
      clear();
      active = null;
      settledContext = null;
    },
  };
}
