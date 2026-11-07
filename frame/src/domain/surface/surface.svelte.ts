import { listenAll } from "$shared/lib/lifecycle";
import { flushSync } from "svelte";
import { commands } from "$shared/ipc/bindings";
import { settle } from "$domain/operations";
import { events } from "$shared/ipc/native-events";

export type BrowserPage =
  | "settings"
  | "extensions"
  | "history"
  | "downloads"
  | "work"
  | "tasks"
  | "notes"
  | "time"
  | "source";
let page = $state<BrowserPage | null>(null);
// The page a browser page was opened over, so closing it returns there: Settings
// opened from Work gives the window back to Work, not to Browse.
let underneath: BrowserPage | null = null;
let error = $state(false);
let generation = 0;
let navigationEpoch = 0;
let stop: (() => void) | null = null;
let observationLifetime = new AbortController();
let initialized = false;
let initializing: Promise<void> | null = null;
export const currentPage = () => page;
export const navigationFailed = () => error;
async function initialize(epoch: number) {
  const listeners = await listenAll([
    events.browserReturn.listen(() => {
      if (epoch !== generation) return;
      underneath = null;
      flushSync(() => {
        page = null;
      });
    }),
    events.uiCommand.listen(({ payload }) => {
      if (epoch !== generation) return;
      if (payload === "browser.return-failed") {
        navigationEpoch++;
        error = true;
        return;
      }
      const destination = payload.startsWith("browser.") ? payload.slice(8) : "";
      const confirmed: BrowserPage | null | undefined =
        destination === "return"
          ? null
          : destination === "work" ||
              destination === "settings" ||
              destination === "extensions" ||
              destination === "history" ||
              destination === "downloads" ||
              destination === "tasks" ||
              destination === "notes" ||
              destination === "time" ||
              destination === "source"
            ? destination
            : undefined;
      if (confirmed !== undefined) {
        navigationEpoch++;
        if (confirmed === null || confirmed === "work") underneath = null;
        else if (page === "work") underneath = "work";
        flushSync(() => {
          error = false;
          page = confirmed;
        });
      }
    }),
  ]);
  if (epoch !== generation) {
    for (const unsubscribe of listeners) unsubscribe();
  } else {
    initialized = true;
    stop = () => {
      for (const unsubscribe of listeners) unsubscribe();
    };
  }
}
export function init(): Promise<void> {
  if (initializing) return initializing;
  if (initialized) return Promise.resolve();
  if (observationLifetime.signal.aborted) observationLifetime = new AbortController();
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
  observationLifetime.abort();
  initialized = false;
  initializing = null;
  navigationEpoch++;
  stop?.();
  stop = null;
  page = null;
  underneath = null;
  error = false;
}
export async function open(next: BrowserPage | null) {
  const request = ++navigationEpoch;
  const fail = () => {
    if (request === navigationEpoch) error = true;
  };
  error = false;
  try {
    const result = await settle(
      commands.runCommand(`browser.${next ?? "return"}`),
      5000,
      observationLifetime.signal,
    );
    if (result.outcome === "failed" || result.outcome === "rejected") fail();
  } catch {
    fail();
  }
}

/** Closes the current page, returning to the one it was opened over. */
export function close() {
  return open(page === "work" ? null : underneath);
}
