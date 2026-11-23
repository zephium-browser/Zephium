import { afterAll, expect, test, vi } from "vitest";
import { commands, page } from "vitest/browser";
import { render } from "vitest-browser-svelte";
import "$styles/global.css";
import { preferences } from "$domain/preferences";
import { wakeupCounter } from "$shared/testing/idle";
import { todayWire } from "$shared/testing/home-scenes";
import HomeCostStage from "./HomeCostStage.svelte";

/**
 * What Home costs over the plain new tab: the WebKit processes' footprint
 * with a day of searches drawn, frames through a live resize, what wakes the
 * page at rest, and how soon a new tab opens. Run once with Home and once
 * without it, each in a fresh browser, and compare the files it writes to
 * node_modules/.cache:
 *
 *   VITE_HOME_COST=home pnpm vitest run --project component HomeCost
 *   VITE_HOME_COST=bare pnpm vitest run --project component HomeCost
 */
declare module "vitest/browser" {
  interface BrowserCommands {
    webkitFootprint: () => Promise<import("$shared/testing/webkit-footprint").WebKitFootprint>;
  }
}

const mode = String(import.meta.env.VITE_HOME_COST ?? "");
const held = vi.hoisted(() => ({ searches: [] as unknown[] }));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  const { snapshot } = await import("$shared/testing/home-wire");
  return mockBindings({
    settingGet: async (key) => (key === "ui.newtab-background" ? "plain" : null),
    timeCall: async () => ({ kind: "focus_days" as const, days: [] }),
    blockerStats: async () => ({
      status: "ok" as const,
      data: { today: 1284, last7Days: 9120, days: [1100, 1250, 1400, 1310, 1380, 1396, 1284] },
    }),
    homeSnapshot: async () => snapshot(held.searches as never) as never,
    workReleaseMemory: async () => true,
  });
});
vi.mock("$domain/tabs", () => ({
  tabs: {
    profile: () => ({ id: "profile-a", name: "", kind: "default" }),
    activeSpaceId: () => "space-a",
    tabs: () => [],
    homeOn: () => true,
  },
}));

held.searches = todayWire();
const counter = wakeupCounter();
afterAll(() => counter.restore());
const frame = () => new Promise<number>((done) => requestAnimationFrame(done));
const rest = (ms: number) => new Promise((done) => setTimeout(done, ms));

test.skipIf(!mode)(
  `what Home costs (${mode || "off"})`,
  async () => {
    const home = mode === "home";
    await preferences.init();
    document.documentElement.dataset.theme = "light";
    await page.viewport(1512, 945);
    const before = await commands.webkitFootprint();
    const screen = await render(HomeCostStage, { home });
    screen.container.style.cssText = "position:relative;inline-size:1512px;block-size:945px";
    if (home)
      await expect
        .poll(() => screen.container.querySelectorAll(".svelte-flow__node").length, {
          timeout: 10_000,
        })
        .toBeGreaterThan(2);
    await rest(1500);
    // The bare page never asks for the canvas at all.
    expect(screen.container.querySelector(".svelte-flow") !== null).toBe(home);

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

    // Warm opens: the field is in the page and painted, before any canvas code is asked for.
    await screen.unmount();
    const opening: number[] = [];
    for (let attempt = 0; attempt < 30; attempt++) {
      const started = performance.now();
      const opened = await render(HomeCostStage, { home });
      await expect.poll(() => opened.container.querySelector(".dock input")).not.toBeNull();
      await frame();
      opening.push(performance.now() - started);
      await opened.unmount();
    }
    opening.sort((a, b) => a - b);

    await commands.writeFile(
      `./node_modules/.cache/home-cost-${mode}.json`,
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
  120_000,
);
