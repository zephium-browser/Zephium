import type { DownloadView } from "$shared/ipc/bindings";
import { downloadProgress, formatDownloadBytes } from "$domain/downloads";
import * as m from "$shared/i18n/messages";

/** A smoothed rate per download, so time left settles instead of jumping
 *  with every progress report. */
export class TransferRate {
  #samples = new Map<string, { at: number; bytes: number; rate: number | null }>();

  /** Bytes a second, or null until there are two readings apart in time. */
  observe(id: string, bytes: number, now = performance.now()): number | null {
    const last = this.#samples.get(id);
    if (!last || bytes < last.bytes) {
      this.#samples.set(id, { at: now, bytes, rate: null });
      return null;
    }
    const elapsed = (now - last.at) / 1000;
    if (elapsed < 0.25) return last.rate;
    const instant = (bytes - last.bytes) / elapsed;
    const rate = last.rate === null ? instant : last.rate * 0.7 + instant * 0.3;
    this.#samples.set(id, { at: now, bytes, rate });
    return rate;
  }

  /** Pauses and interruptions end a sample window; their idle time is not transfer time. */
  retain(entries: Iterable<Pick<DownloadView, "id" | "state">>) {
    const keep = new Set(
      [...entries].filter((entry) => entry.state === "receiving").map((entry) => entry.id),
    );
    for (const id of this.#samples.keys()) if (!keep.has(id)) this.#samples.delete(id);
  }
}

const durations = {
  second: new Intl.NumberFormat(undefined, { style: "unit", unit: "second", unitDisplay: "short" }),
  minute: new Intl.NumberFormat(undefined, { style: "unit", unit: "minute", unitDisplay: "short" }),
  hour: new Intl.NumberFormat(undefined, {
    style: "unit",
    unit: "hour",
    unitDisplay: "short",
    maximumFractionDigits: 1,
  }),
};

export function formatTimeLeft(seconds: number): string {
  if (seconds < 60) return durations.second.format(Math.max(1, Math.ceil(seconds)));
  if (seconds < 3600) return durations.minute.format(Math.ceil(seconds / 60));
  return durations.hour.format(seconds / 3600);
}

/** The line under a download's name: how far along, and how long to go. */
export function transferLine(entry: DownloadView, rate: number | null): string {
  const received = formatDownloadBytes(entry.received);
  if (entry.state !== "receiving") return "";
  const progress = downloadProgress(entry);
  if (entry.total === null || progress === undefined) return received;
  const sizes = m.download_of({ received, total: formatDownloadBytes(entry.total) });
  if (rate === null || rate <= 0) return sizes;
  const left = (Number(entry.total) - Number(entry.received)) / rate;
  return `${sizes} · ${m.download_time_left({ time: formatTimeLeft(left) })}`;
}
