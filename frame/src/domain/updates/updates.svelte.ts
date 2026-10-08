/**
 * Zephium's own updates, as native reports them. A check downloads a newer
 * release in the background; nothing installs until the person relaunches.
 * Development builds report `unavailable` and are never checked.
 */
import { commands, type UpdateStatus } from "$shared/ipc/bindings";
import { preferences } from "$domain/preferences";
import { events } from "$shared/ipc/native-events";

/** After launch, long enough to stay out of startup's way. */
export const FIRST_CHECK_MS = 30_000;
export const CHECK_INTERVAL_MS = 6 * 60 * 60 * 1000;
/** While a check is in flight, how often its progress is read back. */
const PROGRESS_MS = 1500;

const UNKNOWN: UpdateStatus = { state: "unavailable" };

let current = $state.raw<UpdateStatus>(UNKNOWN);
let known = false;
let relaunching = $state(false);
let inFlight: Promise<void> | null = null;
let epoch = 0;
let lifetime = 0;
let timer: ReturnType<typeof setTimeout> | undefined;
let lastScheduled = 0;
let installProgress: ReturnType<typeof setTimeout> | undefined;

export const status = () => current;
export const available = () => current.state !== "unavailable";
export const pendingRelaunch = () => relaunching;

function admit(next: UpdateStatus | null, read: number) {
  if (next === null || read !== epoch) return;
  current = next;
  known = true;
}

export async function refresh(): Promise<void> {
  const read = epoch;
  try {
    admit(await commands.updateStatus(), read);
  } catch {
    // The last known status stands.
  }
}

function idle(state: UpdateStatus["state"]) {
  return state === "idle" || state === "upToDate" || state === "failed";
}

async function settleCheck(read: number) {
  try {
    admit(await commands.updateCheck(), read);
  } catch {
    if (read === epoch) current = { state: "failed" };
  }
}

/** Checks now, or joins the check already running. */
export function check(): Promise<void> {
  if (inFlight) return inFlight;
  if (known && !available()) return Promise.resolve();
  const read = ++epoch;
  if (idle(current.state)) current = { state: "checking" };
  const progress = setInterval(() => {
    void commands.updateStatus().then(
      (next) => {
        if (read === epoch && inFlight !== null && next?.state === "downloading") current = next;
      },
      () => {},
    );
  }, PROGRESS_MS);
  const task = settleCheck(read).finally(() => {
    clearInterval(progress);
    // A status read sent during the check must not land over its result.
    if (read === epoch) epoch += 1;
    inFlight = null;
  });
  inFlight = task;
  return task;
}

/** Installs the waiting update; native quits and the app comes back. */
export async function relaunch(): Promise<boolean> {
  if (relaunching || current.state !== "ready") return false;
  relaunching = true;
  const accepted = await commands.updateRelaunch().catch(() => false);
  await refresh();
  relaunching = false;
  if (accepted && status().state === "installing") watchInstallation(lifetime);
  return accepted;
}

function watchInstallation(generation: number) {
  clearTimeout(installProgress);
  installProgress = setTimeout(() => {
    if (generation !== lifetime) return;
    void refresh().finally(() => {
      if (generation === lifetime && current.state === "installing") watchInstallation(generation);
    });
  }, PROGRESS_MS);
}

async function scheduled(generation: number) {
  timer = undefined;
  if (generation !== lifetime) return;
  lastScheduled = Date.now();
  if (!known) await refresh();
  if (generation !== lifetime || (known && !available())) return;
  if (preferences.value("updates.auto-check") !== "false") await check();
  if (generation === lifetime)
    timer = setTimeout(() => void scheduled(generation), CHECK_INTERVAL_MS);
}

// A machine that slept through the interval checks as soon as it is back.
function onVisible() {
  if (document.visibilityState !== "visible" || lastScheduled === 0) return;
  if (Date.now() - lastScheduled < CHECK_INTERVAL_MS) return;
  clearTimeout(timer);
  void scheduled(lifetime);
}

let started = false;

let stopNative: (() => void) | null = null;

/** Reads the status and starts the background schedule. Main window only. */
export function init(): Promise<void> {
  if (started) return Promise.resolve();
  started = true;
  const generation = ++lifetime;
  // Native checks on its own schedule too, and says when it has.
  void events.uiCommand
    .listen(({ payload }) => {
      if (payload === "updates.changed" && generation === lifetime) void refresh();
    })
    .then((stop) => {
      if (generation === lifetime) stopNative = stop;
      else stop();
    });
  timer = setTimeout(() => void scheduled(generation), FIRST_CHECK_MS);
  document.addEventListener("visibilitychange", onVisible);
  return refresh();
}

export function dispose() {
  if (!started) return;
  started = false;
  lifetime += 1;
  epoch += 1;
  clearTimeout(timer);
  clearTimeout(installProgress);
  stopNative?.();
  stopNative = null;
  installProgress = undefined;
  timer = undefined;
  lastScheduled = 0;
  document.removeEventListener("visibilitychange", onVisible);
  inFlight = null;
  relaunching = false;
  known = false;
  current = UNKNOWN;
}
