import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import type { BrowserCommand } from "vitest/node";
import { ensurePhotos } from "./scripts/canvas-bench/photos";
import type { WebKitFootprint } from "./src/shared/testing/webkit-footprint";

/** Photo-sized JPEGs for the canvas bench, served at /node_modules/.canvas-bench/photos/. */
export const benchPhotos: BrowserCommand<[count: number], void> = (_context, count) =>
  ensurePhotos(
    fileURLToPath(new URL("./node_modules/.canvas-bench/photos", import.meta.url)),
    count,
  );

type Row = { pid: number; ppid: number; started: number; command: string };

function table(): Row[] {
  return execFileSync("ps", ["-axww", "-o", "pid=,ppid=,lstart=,command="])
    .toString()
    .trim()
    .split("\n")
    .flatMap((line) => {
      const match = /^\s*(\d+)\s+(\d+)\s+(\w{3} \w{3}\s+\d+ [\d:]+ \d{4})\s+(.*)$/u.exec(line);
      return match
        ? [
            {
              pid: Number(match[1]),
              ppid: Number(match[2]),
              started: Date.parse(match[3]!),
              command: match[4]!,
            },
          ]
        : [];
    });
}

/**
 * The memory macOS charges the WebKit processes of this test run (what Activity
 * Monitor shows), so a component bench reads real WebKit memory, not a
 * JavaScript estimate. WebKit's content, GPU and network processes are XPC
 * services launchd starts, not children of the browser: they are this run's
 * when they belong to Playwright's WebKit and started after this run's browser.
 * Darwin only; elsewhere it reports nothing.
 */
export const webkitFootprint: BrowserCommand<[], WebKitFootprint> = () => {
  if (process.platform !== "darwin") return { total: 0, processes: [] };
  const rows = table();
  const children = new Map<number, Row[]>();
  for (const row of rows) children.set(row.ppid, [...(children.get(row.ppid) ?? []), row]);
  const ours: Row[] = [];
  const queue = [process.pid];
  while (queue.length)
    for (const child of children.get(queue.shift()!) ?? []) {
      ours.push(child);
      queue.push(child.pid);
    }
  const browser = ours.filter((row) => row.command.includes("ms-playwright/webkit"));
  if (!browser.length) return { total: 0, processes: [] };
  const since = Math.min(...browser.map((row) => row.started));
  const processes = rows.flatMap((row) => {
    const name = /ms-playwright\/webkit.*WebKit\.(WebContent|GPU|Networking)/u.exec(
      row.command,
    )?.[1];
    if (!name || row.started < since) return [];
    try {
      const out = execFileSync("footprint", ["-f", "bytes", String(row.pid)]).toString();
      const bytes = Number(/phys_footprint:\s+(\d+)/u.exec(out)?.[1] ?? 0);
      const cpu = execFileSync("ps", ["-o", "time=", "-p", String(row.pid)])
        .toString()
        .trim();
      return [{ pid: row.pid, name, mb: Math.round(bytes / 1048576), cpu }];
    } catch {
      return [];
    }
  });
  return { total: processes.reduce((sum, entry) => sum + entry.mb, 0), processes };
};
