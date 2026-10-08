import { commands } from "$shared/ipc/bindings";
import { SvelteMap, SvelteSet } from "svelte/reactivity";
import type {
  DownloadCall,
  DownloadCleanup,
  DownloadError,
  DownloadPreferences,
  DownloadView,
} from "$shared/ipc/bindings";
import { events } from "$shared/ipc/native-events";

const TERMINAL = new Set(["completed", "cancelled", "interrupted", "failed"]);

/** Whether a download has stopped for good, so it may be forgotten. */
export function finished(entry: DownloadView): boolean {
  return TERMINAL.has(entry.state);
}

/** Profile-bound projection. Native owns transfers, paths and durable history. */
export class DownloadSession {
  readonly profile: string;
  entries = $state.raw<DownloadView[]>([]);
  preferences = $state.raw<DownloadPreferences | null>(null);
  error = $state<DownloadError | null>(null);
  supported = $state(true);
  cleanup = $state.raw<DownloadCleanup>({ running: false, error: null });
  siteDownloadsRequireConfirmation = $state(false);
  loading = $state(false);
  busy = $state(false);
  next = $state<string | null>(null);
  private generation = 0;
  private active = false;
  private stopListening: (() => void) | null = null;
  private refreshAgain = false;
  private updating = false;
  private updateAgain = false;
  private removedDuringLoad = new SvelteSet<string>();
  private cleanupUpdatedDuringLoad = false;
  private updatedDuringLoad = new SvelteSet<string>();

  constructor(profile: string) {
    this.profile = profile;
  }

  async start(initial = true) {
    if (this.active) return;
    this.active = true;
    const generation = ++this.generation;
    try {
      const stop = await events.downloadsChanged.listen(({ payload }) => {
        if (!this.active || generation !== this.generation || payload.profile !== this.profile)
          return;
        void this.refresh();
      });
      if (!this.active || generation !== this.generation) {
        stop();
        return;
      }
      this.stopListening = stop;
      if (initial) await this.reload();
      else await this.refresh(); // Bounded native snapshot; no history read or idle polling.
    } catch {
      if (generation === this.generation) {
        this.active = false;
        this.error = "unavailable";
      }
    }
  }

  async retry() {
    if (!this.active) await this.start();
    else await this.reload();
  }

  stop() {
    this.active = false;
    this.generation++;
    this.stopListening?.();
    this.stopListening = null;
    this.entries = [];
    this.cleanup = { running: false, error: null };
    this.cleanupUpdatedDuringLoad = false;
    this.removedDuringLoad.clear();
    this.updatedDuringLoad.clear();
    this.next = null;
    this.loading = false;
    this.refreshAgain = false;
    this.updating = false;
    this.updateAgain = false;
    this.busy = false;
  }

  async refresh() {
    if (this.updating) {
      this.updateAgain = true;
      return;
    }
    const generation = this.generation;
    this.updating = true;
    try {
      const response = await commands.downloadCall(this.profile, { kind: "updates" });
      if (generation !== this.generation) return;
      if (response.kind === "updates") {
        this.cleanup = response.cleanup;
        if (this.loading) this.cleanupUpdatedDuringLoad = true;
        const records = new SvelteMap(this.entries.map((entry) => [entry.id, entry]));
        for (const entry of response.entries) {
          if (this.loading) this.updatedDuringLoad.add(entry.id);
          const old = records.get(entry.id);
          if (!old || entry.revision > old.revision) records.set(entry.id, entry);
        }
        for (const id of response.removed) {
          records.delete(id);
          if (this.loading) this.removedDuringLoad.add(id);
        }
        this.entries = [...records.values()]
          .sort((a, b) => b.id.localeCompare(a.id))
          .slice(0, 2000);
      } else if (response.kind === "error") {
        this.error = response.error;
      }
    } catch {
      if (generation === this.generation) this.error = "unavailable";
    } finally {
      if (generation === this.generation) {
        this.updating = false;
        if (this.updateAgain && this.active) {
          this.updateAgain = false;
          void this.refresh();
        }
      }
    }
  }

  async reload(more = false) {
    if (this.loading) {
      this.refreshAgain = true;
      return;
    }
    if (more && (!this.next || this.entries.length >= 2000)) return;
    const generation = this.generation;
    this.loading = true;
    try {
      const response = await commands.downloadCall(this.profile, {
        kind: "list",
        before: more ? this.next : null,
        limit: 50,
      });
      if (generation !== this.generation) return;
      if (response.kind === "page") {
        this.supported = response.supported;
        if (!this.cleanupUpdatedDuringLoad) this.cleanup = response.cleanup;
        const entries = response.entries
          .filter((entry) => !this.removedDuringLoad.has(entry.id))
          .map((entry) => {
            const current = this.entries.find((old) => old.id === entry.id);
            return current && current.revision > entry.revision ? current : entry;
          });
        this.entries = more
          ? [
              ...this.entries,
              ...entries.filter((entry) => !this.entries.some((old) => old.id === entry.id)),
            ]
          : [
              ...entries,
              ...this.entries.filter(
                (entry) =>
                  this.updatedDuringLoad.has(entry.id) &&
                  !this.removedDuringLoad.has(entry.id) &&
                  !entries.some((old) => old.id === entry.id),
              ),
            ]
              .sort((a, b) => b.id.localeCompare(a.id))
              .slice(0, 2000);
        this.next = response.next;
        this.error = null;
      } else if (response.kind === "error") {
        this.error = response.error;
      }
    } catch {
      if (generation === this.generation) this.error = "unavailable";
    } finally {
      if (generation === this.generation) {
        this.loading = false;
        this.removedDuringLoad.clear();
        this.updatedDuringLoad.clear();
        this.cleanupUpdatedDuringLoad = false;
        if (this.refreshAgain && this.active) {
          this.refreshAgain = false;
          void this.reload();
        }
      }
    }
  }

  /** Where the system grants folder access, for a refused destination. */
  openAccessSettings() {
    void commands.downloadOpenAccessSettings().catch(() => undefined);
  }

  async perform(call: DownloadCall) {
    if (this.busy) return;
    const generation = this.generation;
    this.busy = true;
    this.error = null;
    try {
      const response = await commands.downloadCall(this.profile, call);
      if (generation !== this.generation) return;
      if (response.kind === "error") {
        if (response.error !== "cancelled") this.error = response.error;
      } else if (response.kind === "preferences") {
        this.preferences = response.preferences;
        this.siteDownloadsRequireConfirmation = response.site_downloads_require_confirmation;
        this.supported = response.supported;
      } else if (call.kind === "retry_cleanup") {
        await this.refresh();
      } else if (call.kind === "clear") {
        this.entries = this.entries.filter((entry) => !finished(entry));
        await this.reload();
      } else if (call.kind === "forget" || call.kind === "cancel" || call.kind === "resume") {
        await this.reload();
      }
    } catch {
      if (generation === this.generation) this.error = "unavailable";
    } finally {
      if (generation === this.generation) this.busy = false;
    }
  }
}
