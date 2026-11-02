import { afterAll, expect, test, vi } from "vitest";
import { commands, page } from "vitest/browser";
import { render } from "vitest-browser-svelte";
import { createRawSnippet } from "svelte";
import "$styles/global.css";
import { preferences } from "$domain/preferences";
import { wakeupCounter } from "$shared/testing/idle";
import { loadNewTab } from "$features/newtab";

/**
 * What the sky costs: the WebKit processes' footprint at rest, frames through
 * a live resize, and what wakes the page at rest. Run once with the sky and
 * once without it, each in a fresh browser, and compare the files it writes
 * to node_modules/.cache:
 *
 *   VITE_NEWTAB_COST=sky pnpm vitest run --project component NewTabCost
 *   VITE_NEWTAB_COST=bare pnpm vitest run --project component NewTabCost
 */
declare module "vitest/browser" {
  interface BrowserCommands {
    webkitFootprint: () => Promise<import("../../../../vitest.bench-commands").WebKitFootprint>;
  }
}

const mode = String(import.meta.env.VITE_NEWTAB_COST ?? "");
vi.mock("$shared/sky", async (original) => {
  const real = await original<Record<string, unknown>>();
  if (import.meta.env.VITE_NEWTAB_COST !== "bare") return real;
  return { ...real, SCENES: { day: "", night: "" }, decode: () => new Promise<void>(() => {}) };
});
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    settingGet: async () => null,
    timeCall: async () => ({ kind: "focus_days" as const, days: [] }),
    blockerStats: async () => ({
      status: "ok" as const,
      data: { today: 1284, last7Days: 9120, days: [1100, 1250, 1400, 1310, 1380, 1396, 1284] },
    }),
  });
});
vi.mock("$domain/tabs", () => ({
  tabs: { profile: () => ({ id: "00000000000000000000000901", name: "", kind: "default" }) },
}));

const counter = wakeupCounter();
afterAll(() => counter.restore());
const frame = () => new Promise<number>((done) => requestAnimationFrame(done));
const rest = (ms: number) => new Promise((done) => setTimeout(done, ms));

test.skipIf(!mode)(
  `what the new tab costs (${mode || "off"})`,
  async () => {
    await preferences.init();
    document.documentElement.dataset.theme = "light";
    await page.viewport(1512, 945);
    const before = await commands.webkitFootprint();
    const { default: NewTab } = await loadNewTab();
    const screen = await render(NewTab, {
      search: createRawSnippet(() => ({ render: () => '<div class="dock"></div>' })),
      oncustomize: () => {},
    });
    screen.container.style.cssText = "inline-size:1512px;block-size:945px";
    if (mode === "sky")
      await expect
        .poll(() => screen.container.querySelector('.newtab[data-ready="true"]'), {
          timeout: 8000,
        })
        .toBeTruthy();
    await rest(1500);

    // A live resize: a frame for every step of the pane's width.
    const frames: number[] = [];
    let last = await frame();
    for (let step = 0; step < 60; step++) {
      screen.container.style.inlineSize = `${1512 - step * 8}px`;
      const now = await frame();
      frames.push(now - last);
      last = now;
    }
    frames.sort((a, b) => a - b);
    screen.container.style.inlineSize = "1512px";
    await rest(1000);

    const idle = await counter.rest(3000);
    await rest(2000);
    const after = await commands.webkitFootprint();

    // Warm new-tab opens: committed DOM and the next paint opportunity.
    await screen.unmount();
    const opening: number[] = [];
    for (let attempt = 0; attempt < 30; attempt++) {
      const started = performance.now();
      const opened = await render(NewTab, {
        search: createRawSnippet(() => ({ render: () => '<div class="dock"></div>' })),
        oncustomize: () => {},
      });
      opened.container.style.cssText = "inline-size:1512px;block-size:945px";
      await frame();
      opening.push(performance.now() - started);
      await opened.unmount();
    }
    opening.sort((a, b) => a - b);

    await commands.writeFile(
      `./node_modules/.cache/newtab-cost-${mode}.json`,
      JSON.stringify(
        {
          mode,
          before: before.processes,
          after: after.processes,
          resizeFrameMs: { median: frames[30], p90: frames[54], max: frames[59] },
          idle,
          openingMs: { median: opening[15], p90: opening[27], max: opening[29] },
        },
        null,
        1,
      ),
    );
  },
  90_000,
);
