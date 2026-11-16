import "$styles/global.css";
import { test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { commands, page } from "vitest/browser";
import WorkCanvas from "../components/WorkCanvas.svelte";
import { wakeupCounter } from "$shared/testing/idle";
import { drawn, drive, scaleCanvas, settle, sweep, type ScaleMix } from "./scale-fixtures";

/**
 * A seeded canvas of N mixed items measured in Playwright's WebKit: what
 * opening it costs, how steady its frames are while it pans and pinches, what
 * stays drawn, what wakes at rest, and the WebKit processes' footprint before,
 * after opening, after a sweep over all of it and back. The numbers are the
 * report. scripts/canvas-bench runs the same canvas on the system WebKit.
 *
 *   VITE_SCALE=50,200,500 [VITE_SCALE_ZOOM=1,0.35] [VITE_SCALE_MIX=media]
 *   [VITE_SCALE_CSS='…'] pnpm vitest run --project component WorkCanvasScale
 */
declare module "vitest/browser" {
  interface BrowserCommands {
    benchPhotos: (count: number) => Promise<void>;
    webkitFootprint: () => Promise<import("$shared/testing/webkit-footprint").WebKitFootprint>;
  }
}

const counter = wakeupCounter();
let pictures = new Map<string, string>();
vi.mock("$domain/resources", async (original) => ({
  ...(await original<typeof import("$domain/resources")>()),
  mediaUrl: (_profile: string, digest: string) => pictures.get(digest) ?? null,
}));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({ faviconProbe: async () => true, workReleaseMemory: async () => true });
});

const env = import.meta.env;
const counts = String(env.VITE_SCALE ?? "")
  .split(",")
  .map(Number)
  .filter((count) => count > 0);
const zooms = String(env.VITE_SCALE_ZOOM ?? "1,0.35")
  .split(",")
  .map(Number);
const cases = counts.flatMap((count) => zooms.map((zoom) => [count, zoom] as const));

// Without VITE_SCALE one skipped case stands in, so the file still holds a test.
test.skipIf(!cases.length).each(cases.length ? cases : [[50, 1] as const])(
  "a canvas of %i items at %sx",
  async (count, zoom) => {
    await page.viewport(1440, 900);
    const canvas = scaleCanvas(
      count,
      (index) => `/node_modules/.canvas-bench/photos/photo-${index}.jpg`,
      (env.VITE_SCALE_MIX as ScaleMix | undefined) ?? "mixed",
    );
    await commands.benchPhotos(canvas.photos);
    pictures = canvas.pictures;
    const before = await commands.webkitFootprint();
    let center: (id: string) => void = () => {};
    const opened = performance.now();
    const screen = await render(WorkCanvas, {
      items: canvas.items,
      links: [],
      authoritative: new Set<string>(),
      initialView: { positions: canvas.positions, viewport: { x: 40, y: 40, zoom } },
      oninspect: () => {},
      expose: (api: { center: (id: string) => void }) => (center = api.center),
    });
    screen.container.style.width = "1440px";
    screen.container.style.height = "900px";
    const root = screen.container as HTMLElement;
    await settle(root);
    const open = Math.round(performance.now() - opened);
    const atOpen = drawn(root);
    const openFootprint = await commands.webkitFootprint();
    const rest = await counter.rest(2000);
    if (env.VITE_SCALE_CSS)
      document.head.insertAdjacentHTML(
        "beforeend",
        `<style id="scale-css">${env.VITE_SCALE_CSS}</style>`,
      );
    const pane = root.querySelector(".svelte-flow__pane")!;
    const pan = await drive(pane, 180, false);
    const pinch = await drive(pane, 120, true);
    document.getElementById("scale-css")?.remove();
    await sweep(center, count);
    await settle(root);
    const swept = drawn(root);
    const sweptFootprint = await commands.webkitFootprint();
    center("item-0");
    await settle(root);
    const back = drawn(root);
    const backFootprint = await commands.webkitFootprint();
    console.warn(
      "work-canvas-scale",
      JSON.stringify({
        count,
        zoom,
        openMs: open,
        atOpen,
        idle: rest,
        pan,
        pinch,
        swept,
        back,
        footprintMB: {
          before: before.total,
          open: openFootprint.total,
          swept: sweptFootprint.total,
          back: backFootprint.total,
          processes: backFootprint.processes,
        },
      }),
    );
    await screen.unmount();
  },
  300_000,
);
