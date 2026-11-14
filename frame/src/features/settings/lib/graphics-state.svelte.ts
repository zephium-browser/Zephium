import * as m from "$shared/i18n/messages";
import { commands, type GpuReport, type RendererPreference } from "$shared/ipc/bindings";
import { observe, type Observation } from "$shared/lib/observe";
import { graphicsStatus } from "./graphics";

const LIMIT = 5000;

const read = () =>
  commands.gpuReport().then((result) => (result.status === "ok" ? result.data : null));

/** One Graphics page: the report it shows, the choice being saved and the restart it offers. */
export function createGraphics() {
  let report = $state.raw<GpuReport | null>(null);
  let failed = $state(false);
  let wanted = $state<RendererPreference | null>(null);
  let saving = $state(false);
  let saveFailed = $state(false);
  let announcement = $state("");
  let restarting = $state(false);
  let restartFailed = $state(false);
  let live = true;

  async function load() {
    failed = false;
    const result = await observe(read(), LIMIT);
    if (!live) return;
    if (result.state === "received" && result.value) report = result.value;
    else if (report === null) failed = true;
  }

  /** Saves the newest choice; picks made while one is saving follow it in order. */
  async function choose(preference: RendererPreference) {
    if (report === null) return;
    wanted = preference;
    saveFailed = false;
    announcement = "";
    if (saving) return;
    saving = true;
    while (live && wanted !== null && wanted !== report?.preference) {
      const target: RendererPreference = wanted;
      const result: Observation<Awaited<ReturnType<typeof commands.rendererPreferenceSet>>> =
        await observe(commands.rendererPreferenceSet(target), LIMIT);
      if (!live) return;
      if (result.state !== "received" || result.value.status === "error") {
        saveFailed = true;
        break;
      }
      report = result.value.data;
    }
    wanted = null;
    saving = false;
    if (saveFailed) {
      // A save that timed out may still have landed; show what is stored.
      await load();
      return;
    }
    const status = report && graphicsStatus(report).kind;
    announcement =
      status === "restart"
        ? m.graphics_saved_restart()
        : status === "environment"
          ? m.graphics_saved_environment()
          : m.graphics_saved();
  }

  /** "Try the faster mode again": the next automatic start ignores the repeated failures. */
  async function tryFaster() {
    if (report === null || saving) return;
    saving = true;
    saveFailed = false;
    announcement = "";
    const result = await observe(commands.rendererRetryFaster(), LIMIT);
    if (!live) return;
    saving = false;
    if (result.state === "received" && result.value.status === "ok") {
      report = result.value.data;
      announcement = m.graphics_saved_restart();
      return;
    }
    saveFailed = true;
    await load();
  }

  async function restart() {
    if (restarting) return;
    restarting = true;
    restartFailed = false;
    const started = await commands.appRelaunch().catch(() => false);
    if (!live) return;
    restarting = false;
    restartFailed = !started;
  }

  void load();

  return {
    report: () => report,
    failed: () => failed,
    /** The choice shown as selected: one being saved, else the stored one. */
    chosen: () => wanted ?? report?.preference ?? null,
    saving: () => saving,
    saveFailed: () => saveFailed,
    announcement: () => announcement,
    restarting: () => restarting,
    restartFailed: () => restartFailed,
    retry: () => void load(),
    choose,
    tryFaster,
    restart,
    /** Adopts a report read for the clipboard, which is newer than the one shown. */
    adopt: (next: GpuReport) => {
      if (!saving) report = next;
    },
    read,
    dispose: () => {
      live = false;
    },
  };
}

export type Graphics = ReturnType<typeof createGraphics>;
