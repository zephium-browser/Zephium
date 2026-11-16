import { surface as browser } from "$domain/surface";
import { setPanelExtent } from "./sidebar-mode.svelte";
import { events } from "$shared/ipc/native-events";
export type ToolKind = "notes" | "tasks" | "ai" | "time" | "history" | "downloads" | "bookmarks";
let tool = $state<ToolKind | null>(null);
/** The assistant's conversation breathes wider than a list. */
const extentOf = (kind: ToolKind) => (kind === "ai" ? 372 : 336);
let queued: ToolKind | null = null;
let stop: (() => void) | null = null;
let generation = 0;
let initialized = false;
let initializing: Promise<void> | null = null;
export const activeTool = () => tool;
export function open(kind: ToolKind) {
  // Work keeps the column, so a tool opens beside its canvas; any other page gives way to Browse.
  const page = browser.currentPage();
  if (page !== null && page !== "work") {
    queued = kind;
    void browser.open(null);
    return;
  }
  // The column widens at once; native slides the page aside to make room.
  tool = kind;
  setPanelExtent(extentOf(kind));
}
export function close() {
  queued = null;
  tool = null;
  setPanelExtent(0);
}
async function initialize(owner: number) {
  const unsubscribe = await events.uiCommand.listen(({ payload }) => {
    if (owner !== generation) return;
    const kind = payload.slice(5);
    if (
      payload.startsWith("tool.") &&
      ["notes", "tasks", "ai", "time", "history", "downloads", "bookmarks"].includes(kind)
    ) {
      // A tool's own shortcut closes it again.
      if (tool === kind && browser.currentPage() === null) close();
      else open(kind as ToolKind);
    }
    if (payload === "browser.return" && queued) {
      const next = queued;
      queued = null;
      open(next);
    }
    if (payload === "browser.return-failed") queued = null;
    if (
      [
        "browser.settings",
        "browser.history",
        "browser.downloads",
        "browser.tasks",
        "browser.notes",
        "browser.time",
      ].includes(payload)
    ) {
      tool = null;
      setPanelExtent(0);
      queued = null;
    }
  });
  if (owner !== generation) unsubscribe();
  else {
    stop = unsubscribe;
    initialized = true;
  }
}

export function init(): Promise<void> {
  if (initializing) return initializing;
  if (initialized) return Promise.resolve();
  const task = initialize(++generation);
  initializing = task;
  void task.then(
    () => {
      if (initializing === task) initializing = null;
    },
    () => {
      if (initializing === task) initializing = null;
    },
  );
  return task;
}
export function dispose() {
  generation++;
  initialized = false;
  initializing = null;
  queued = null;
  stop?.();
  stop = null;
  tool = null;
}
