import { tick } from "svelte";
import type { IconSvgElement } from "@hugeicons/svelte";
import { CheckListIcon } from "@hugeicons/core-free-icons";
import * as m from "$shared/i18n/messages";
import type { PanelState, SearchResult, ToolKind } from "$shared/ipc/bindings";
import { commands } from "$shared/ipc/bindings";
import { events } from "$shared/ipc/native-events";
import { settle } from "$domain/operations";
import { createSearchController, type SearchSnapshot } from "./search-controller";
import { calculate, type Calculation } from "./calculator";
import {
  completionSuffix,
  completionTarget,
  resultIdentity,
  resultSection,
  type ResultSection,
} from "./search-model";

export type Destination = {
  kind: ToolKind;
  label: string;
  icon: IconSvgElement;
  /** Shown beside the destination wherever it is offered. */
  keys?: string[];
};

/** Short confirmation of something done without leaving the launcher. */
export type SurfaceNotice = "opened" | "copied" | "answer";

export type SurfaceRow = {
  id: string;
  title: string;
  section: ResultSection;
  result: SearchResult | null;
  tool: ToolKind | null;
  icon: IconSvgElement | null;
  /** A shortcut that runs this row from anywhere in the launcher. */
  keys?: string[];
  /** Set on the row that answers arithmetic typed into the field. */
  calculation?: Calculation;
};

/** Shared behaviour for both search hosts. The floating launcher and the New
 *  Tab capsule differ only in chrome; selection, coalescing, execution and
 *  cancellation are one implementation. */
export function createSearchSurface(options: {
  /** Set for New Tab, which binds a specific blank tab and anchors its list.
   *  The launcher instead binds each presentation through `begin`. */
  tabId: string | null;
  destinations: Destination[];
  onTool: (tool: ToolKind) => void;
  /** Saves the typed line as a task; resolves to the title saved, or null. */
  onCapture?: (text: string) => Promise<string | null>;
  captureKeys?: string[];
}) {
  const anchored = options.tabId !== null;

  let query = $state("");
  let results = $state<SearchResult[]>([]);
  let completion = $state<string | null>(null);
  /** The completion currently standing in the field, or null when the field
   *  holds only what was typed. */
  let applied = $state<string | null>(null);
  let answered = $state("");
  let pending = $state(false);
  let settled = $state(false);
  let error = $state<SearchSnapshot["error"]>("none");
  let failed = $state(false);
  let running = $state(false);
  let open = $state(!anchored);
  let selected = $state<string | null>(null);
  /** The user has aimed at a row. Until then the default is row 0, which is
   *  always the action for what they typed. */
  let moved = $state(false);
  let deleting = $state(false);
  /** What the field is actually showing, completion included. Deletion has to
   *  be judged against this: comparing against the typed text made backspace
   *  look broken, because removing the completion leaves the typed text the
   *  same length and the completion was immediately re-applied. */
  let displayed = $state("");
  let composing = $state(false);
  /** The title of a task just saved from this query, while the launcher says so. */
  let captured = $state<string | null>(null);
  let notice = $state<SurfaceNotice | null>(null);
  let noticeTimer: ReturnType<typeof setTimeout> | undefined;
  const CAPTURE_ID = "capture:task";
  const CALCULATION_ID = "calculation";
  /** The launcher's current presentation; null while it is put away. */
  let context: PanelState | null = null;

  /** Intent expressed before the answer arrived. Resolved against the settled
   *  result set so a click or Enter is never silently dropped, and never runs
   *  an action the newer query no longer offers. `""` means "the first row",
   *  which is the typed action. */
  let queued: string | null = null;
  /** Long enough to read, short enough to be gone before the next action. */
  const NOTICE_MS = 1600;
  /** How long a dismissed launcher keeps what was typed. */
  const RESUME_MS = 30_000;

  let disposed = false;
  let list: HTMLElement | undefined;
  let controller: ReturnType<typeof createSearchController> | undefined;

  // At rest the host offers destinations on their own; typing their name
  // brings them into the list.
  const matchingTools = $derived(
    anchored || !query.trim()
      ? []
      : options.destinations.filter((destination) =>
          destination.label.toLocaleLowerCase().includes(query.trim().toLocaleLowerCase()),
        ),
  );

  // Native emits results already in section order, so rows keep that order and
  // destinations follow as their own run.
  // Answered here rather than natively: it is instant, needs no round trip,
  // and New Tab, which offers places rather than answers, never asks.
  const calculation = $derived(anchored ? null : calculate(query));

  const rows = $derived<SurfaceRow[]>([
    // A sum typed into the field is a question; its answer is what Enter
    // should act on, ahead of searching the sum itself.
    ...(calculation
      ? [
          {
            id: CALCULATION_ID,
            title: calculation.text,
            section: "calculator" as const,
            result: null,
            tool: null,
            icon: null,
            calculation,
          },
        ]
      : []),
    // Editing a page's address, that page is no destination of its own.
    ...results
      .filter(
        (result) =>
          !(result.action.type === "ActivateTab" && result.action.id === context?.seed_tab),
      )
      .map((result) => ({
        id: resultIdentity(result),
        // The row searching for what was typed reads the field as it is, so
        // it never lags a key behind while the next answer is on its way. A
        // row titled otherwise (a scoped search) keeps its own words.
        title:
          result.kind === "search" && result.title === answered.trim() && query.trim()
            ? query.trim()
            : result.title,
        section: resultSection(result),
        result,
        tool: null,
        icon: null,
      })),
    ...matchingTools.map((destination) => ({
      id: `tool:${destination.kind}`,
      title: destination.label,
      section: "destinations" as const,
      result: null,
      tool: destination.kind,
      icon: destination.icon,
      keys: destination.keys,
    })),
    // Anything typed can become a task, last so it never displaces the page or
    // search the line most often means.
    ...(query.trim() && !anchored && options.onCapture
      ? [
          {
            id: CAPTURE_ID,
            title: m.launcher_capture({ text: query.trim() }),
            section: "commands" as const,
            result: null,
            tool: null,
            icon: CheckListIcon,
            keys: options.captureKeys,
          },
        ]
      : []),
  ]);

  /** The row the field is currently showing the user, when a completion has
   *  been applied. Safari highlights this row, and Enter opens it. */
  const completed = $derived(completionTarget(rows, applied));

  const selectedId = $derived(
    rows.length === 0
      ? null
      : moved && selected && rows.some((row) => row.id === selected)
        ? selected
        : (completed?.id ?? rows[0]!.id),
  );

  async function capture() {
    const text = query.trim();
    if (running || !text || !options.onCapture) return;
    running = true;
    failed = false;
    try {
      const title = await options.onCapture(text);
      if (disposed) return;
      running = false;
      if (title) {
        captured = title;
        fulfilled = true;
      } else failed = true;
    } catch {
      if (!disposed) {
        running = false;
        failed = true;
      }
    }
  }

  function announce(value: SurfaceNotice) {
    clearTimeout(noticeTimer);
    notice = value;
    noticeTimer = setTimeout(() => (notice = null), NOTICE_MS);
  }

  /** Whether a row leads to an address, which is what can be opened behind the
   *  current tab or copied. */
  function addressOf(row: SurfaceRow | undefined): string | null {
    return row?.result?.action.type === "OpenUrl" ? row.result.action.url : null;
  }

  async function run(row: SurfaceRow, background = false) {
    if (running) return;
    if (row.id === CAPTURE_ID) {
      await capture();
      return;
    }
    if (row.calculation) {
      await copyText(String(row.calculation.value), "answer");
      return;
    }
    if (row.tool) {
      fulfilled = true;
      options.onTool(row.tool);
      return;
    }
    const expected = controller?.context();
    if (!row.result || !expected) return;
    background = background && !anchored && !!addressOf(row);
    running = true;
    failed = false;
    try {
      const admission = anchored
        ? await settle(commands.newtabRun(row.result.action, expected))
        : await commands.launcherRun(row.result.action, expected, background);
      if (disposed) return;
      const rejected =
        "accepted" in admission
          ? !admission.accepted
          : admission.outcome === "failed" || admission.outcome === "rejected";
      running = false;
      if (rejected) failed = true;
      else if (anchored) open = false;
      else if (background) announce("opened");
      else fulfilled = true;
    } catch {
      if (!disposed) {
        running = false;
        failed = true;
      }
    }
  }

  /** Runs now when the visible rows answer the current query, otherwise waits
   *  for the answer rather than doing nothing. */
  /** A destination, a capture or an answer does not depend on native. */
  const local = (row: SurfaceRow) =>
    !!row.tool || row.id === CAPTURE_ID || row.id === CALCULATION_ID;

  function activate(row: SurfaceRow, background = false) {
    if (settled || local(row)) void run(row, background);
    else if (!background) queued = row.id;
  }

  function resolveQueued() {
    if (queued === null || !settled || running) return;
    const target = queued === "" ? rows[0] : rows.find((row) => row.id === queued);
    queued = null;
    if (target) void run(target);
  }

  async function move(offset: number) {
    if (!rows.length) return;
    const at = rows.findIndex((row) => row.id === selectedId);
    const next = at + offset;
    // No wrap. Past the top the selection returns to the typed action; past
    // the bottom it stays put, so holding a key never cycles the list.
    if (next < 0) {
      moved = false;
      selected = null;
      return;
    }
    if (next >= rows.length) return;
    moved = true;
    selected = rows[next]!.id;
    await tick();
    list?.querySelector<HTMLElement>(`[aria-posinset="${next + 1}"]`)?.scrollIntoView({
      block: "nearest",
    });
  }

  // The page's own address, when the launcher opened to edit it. It is not
  // something typed, so it is never offered as a task.
  let seededQuery = "";
  function seed(value: string) {
    changed(value);
    seededQuery = value;
  }

  function changed(value: string) {
    captured = null;
    // Replacing the selected suffix with another typed character shortens
    // the displayed value too. Only a value that did not extend the typed
    // prefix is a deletion; otherwise completion would disappear every other key.
    deleting =
      value.length <= query.length &&
      value.length < displayed.length &&
      displayed.startsWith(value);
    displayed = value;
    query = value;
    applied = null;
    if (anchored) open = true;
    queued = null;
    moved = false;
    selected = null;
    failed = false;
    running = false;
    controller?.change(value);
  }

  function submit(background = false) {
    if (composing) return;
    const row = rows.find((candidate) => candidate.id === selectedId);
    if (row && (settled || row.id === CALCULATION_ID || (moved && local(row)))) {
      void run(row, background);
      return;
    }
    // Sending something behind the current tab is a deliberate choice about
    // a row the user can see; it is never deferred onto an answer they cannot.
    // Until the user aims at a row, the row on screen is the previous query's
    // typed action, whose identity carries the previous text. Queuing that id
    // would match nothing in the answer and drop the Enter.
    if (query.trim() && !background) queued = moved ? (row?.id ?? "") : "";
  }

  async function copy() {
    const row = rows.find((candidate) => candidate.id === selectedId);
    // The search row reads the field before its answer arrives; its address
    // is the previous query's until then.
    if (!settled && row?.result?.kind === "search") return;
    const address = addressOf(row);
    if (address) await copyText(address, "copied");
  }

  async function copyText(text: string, notice: SurfaceNotice) {
    try {
      await navigator.clipboard.writeText(text);
      if (!disposed) announce(notice);
    } catch {
      if (!disposed) failed = true;
    }
  }

  /** Appends the offered host and selects the appended part, so the next
   *  keystroke replaces it. Never fights the typist: not while composing, not
   *  after a deletion, and not when the caret sits inside the text. */
  function applyCompletion(input: HTMLInputElement | undefined) {
    if (!input || deleting || composing || !completion) return;
    const suffix = completionSuffix(query, completion);
    if (!suffix) return;
    if (input.value !== query) return;
    if (input.selectionStart !== query.length || input.selectionEnd !== query.length) return;
    input.value = query + suffix;
    input.setSelectionRange(query.length, query.length + suffix.length);
    applied = completion;
    displayed = input.value;
  }

  function dismiss() {
    // First press puts the list away and keeps the query; a second clears it.
    if (anchored && open) {
      open = false;
      queued = null;
      return true;
    }
    if (query) {
      changed("");
      return true;
    }
    return false;
  }

  function focused() {
    if (!open && query) controller?.change(query);
    open = true;
  }

  function blurred() {
    if (!anchored) return;
    open = false;
    queued = null;
    const active = controller?.request();
    controller?.pause();
    if (active) void commands.newtabCancel(active).catch(() => {});
  }

  function retry() {
    failed = false;
    controller?.change(query);
  }

  let listening = false;
  /** A foreground action or a capture finished what the user came for, so the
   *  next presentation starts fresh rather than resuming. */
  let fulfilled = false;
  let expiry: ReturnType<typeof setTimeout> | undefined;
  let expiresAt = 0;
  /** The last answer to an empty field, shown again when the launcher resets
   *  so its home list is complete on the frame it appears. */
  let home: SearchResult[] = [];

  /** Binds the launcher to one presentation, resuming whatever it was left
   *  showing unless that has since expired. */
  function begin(next: PanelState) {
    if (expiresAt && Date.now() >= expiresAt) toHome();
    expiresAt = 0;
    clearTimeout(expiry);
    context = next;
    // Before the controller exists, so its first request is for the address
    // and not for an empty field that the seed immediately replaces.
    if (next.seed) seed(next.seed);
    if (listening) void initialize();
  }

  /** Puts the launcher away. Native work stops now; what was typed is kept
   *  for a short while, and the reset to home happens while still hidden so
   *  the window never changes size on the frame it is shown. */
  function end() {
    context = null;
    controller?.dispose();
    controller = undefined;
    queued = null;
    running = false;
    failed = false;
    pending = false;
    settled = false;
    clearTimeout(noticeTimer);
    notice = null;
    clearTimeout(expiry);
    if (fulfilled || !query) toHome();
    else {
      expiresAt = Date.now() + RESUME_MS;
      expiry = setTimeout(toHome, RESUME_MS);
    }
  }

  function toHome() {
    seededQuery = "";
    fulfilled = false;
    query = "";
    displayed = "";
    answered = "";
    applied = null;
    completion = null;
    results = home;
    moved = false;
    selected = null;
    error = "none";
    captured = null;
  }

  async function initialize() {
    const owner = options.tabId
      ? await commands.newtabSearchContext(options.tabId)
      : context?.window_id && context.profile_id && context.space_id
        ? {
            window_id: context.window_id,
            profile_id: context.profile_id,
            space_id: context.space_id,
            session_id: context.session_id,
          }
        : null;
    if (disposed || (!anchored && !context)) return;
    if (!owner) {
      failed = true;
      return;
    }
    controller?.dispose();
    controller = createSearchController({
      owner,
      emptyQuery: anchored ? "skip" : "search",
      shown: { results, answered },
      send: (value, requestId) =>
        options.tabId
          ? commands.newtabSearch(value, { ...owner, request_id: requestId })
          : commands.launcherSearch(value, requestId),
      update: (snapshot) => {
        results = snapshot.results;
        if (!anchored && snapshot.settled && !snapshot.answered) home = snapshot.results;
        completion = snapshot.completion;
        answered = snapshot.answered;
        pending = snapshot.pending;
        settled = snapshot.settled;
        error = snapshot.error;
        resolveQueued();
      },
    });
    if (query && (!anchored || open)) controller.change(query);
    else if (!anchored) controller.start();
  }

  /** Called from the host's `onMount`; returns its teardown. */
  function mount() {
    const listener = events.searchChanged.listen((event) => controller?.receive(event.payload));
    void listener
      .then(() => {
        if (disposed) return;
        listening = true;
        if (anchored || context) return initialize();
      })
      .catch(() => {
        if (!disposed) failed = true;
      });

    return () => {
      disposed = true;
      clearTimeout(noticeTimer);
      clearTimeout(expiry);
      const active = controller?.request();
      if (anchored && active) void commands.newtabCancel(active).catch(() => {});
      controller?.dispose();
      void listener.then((stop) => stop()).catch(() => {});
    };
  }

  return {
    mount,
    begin,
    end,
    changed,
    submit,
    capture,
    copy,
    activate,
    move,
    dismiss,
    focused,
    blurred,
    retry,
    applyCompletion,
    hover: (id: string) => {
      moved = true;
      selected = id;
    },
    compositionStart() {
      composing = true;
      controller?.compositionStart();
    },
    compositionEnd(value: string) {
      composing = false;
      query = value;
      controller?.compositionEnd(value);
    },
    setList(element: HTMLElement | undefined) {
      list = element;
    },
    get query() {
      return query;
    },
    /** The query the visible rows answer. Emphasis is measured against this
     *  rather than the live text, so a row's highlight never drops out for the
     *  moment between a keystroke and its answer. */
    get answered() {
      return answered;
    },
    get rows() {
      return rows;
    },
    get selectedId() {
      return selectedId;
    },
    get selectedIndex() {
      return rows.findIndex((row) => row.id === selectedId);
    },
    get open() {
      return open;
    },
    get pending() {
      return pending;
    },
    get running() {
      return running;
    },
    get captured() {
      return captured;
    },
    get notice() {
      return notice;
    },
    /** The selected row leads to an address, so it can go to a background tab
     *  or the clipboard. */
    get addressable() {
      return !!addressOf(rows.find((row) => row.id === selectedId));
    },
    /** The row a visible inline completion stands for. */
    get completedRow() {
      return completed;
    },
    get selectedRow() {
      return rows.find((row) => row.id === selectedId) ?? null;
    },
    get capturable() {
      return !anchored && !!options.onCapture && !!query.trim() && query !== seededQuery;
    },
    get failed() {
      return failed;
    },
    get error() {
      return error;
    },
    get composing() {
      return composing;
    },
    /** Nothing matched and every native provider has reported. */
    get empty() {
      return !rows.length && !pending && !!query.trim() && !failed && error === "none";
    },
  };
}
