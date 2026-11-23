import { afterEach, expect, test, vi } from "vitest";
import { page } from "vitest/browser";
import { cleanup, render } from "vitest-browser-svelte";
import "$styles/global.css";
import HomeStage from "./HomeStage.svelte";
import * as fixture from "./fixtures";
import { layout } from "../lib/layout";
import { admit, type HomeModel } from "../lib/model";

vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({ workReleaseMemory: async () => true });
});

const shots = "../../../../../target/home-look";
afterEach(() => {
  cleanup();
  document.documentElement.dataset.theme = "dark";
});

const SIZES = [
  [1440, 900],
  [1100, 700],
] as const;

const SCENES: Record<string, { home: () => HomeModel; backdrop?: boolean; wide?: boolean }> = {
  rest: {
    home: () => fixture.home([], [{ key: "2026-11-22", count: 1, entries: [fixture.compare] }]),
  },
  today: { home: fixture.today },
  compare: { home: fixture.comparing },
  states: { home: fixture.states },
  edges: { home: fixture.edges },
  picture: { home: fixture.today, backdrop: true },
  day: { home: fixture.busy, wide: true },
  large: { home: () => fixture.home([fixture.large(), fixture.raycast]), wide: true },
  thin: { home: fixture.thinPlaces, wide: true },
  arriving: { home: () => fixture.home([fixture.arriving(), fixture.figma()]), wide: true },
  failed: { home: () => fixture.home([fixture.failed, fixture.raycast]), wide: true },
  rtl: { home: fixture.rtlFirst, wide: true },
};

async function stage(
  home: HomeModel,
  theme: string,
  width: number,
  height: number,
  backdrop = false,
  load?: () => Promise<never>,
) {
  await page.viewport(width, height);
  document.documentElement.dataset.theme = theme;
  document.documentElement.dataset.material = "none";
  const screen = await render(HomeStage, { home, backdrop, ...(load ? { load } : {}) });
  if (home.today.entries?.length && !load)
    await expect
      .poll(() => screen.container.querySelectorAll(".svelte-flow__node").length, {
        timeout: 10_000,
      })
      .toBeGreaterThan(2);
  await new Promise((done) => setTimeout(done, 700));
  return screen;
}

for (const [scene, { home, backdrop, wide }] of Object.entries(SCENES))
  for (const theme of ["light", "dark"])
    for (const [width, height] of wide ? SIZES.slice(0, 1) : SIZES)
      test(`Home ${scene}, ${theme}, ${width}×${height}`, async () => {
        await stage(home(), theme, width, height, backdrop);
        await page.screenshot({ path: `${shots}/${scene}-${theme}-${width}.png` });
      });

for (const theme of ["light", "dark"]) {
  test(`Home with a row's known links on hover, ${theme}`, async () => {
    const screen = await stage(
      fixture.home([fixture.large(null), fixture.raycast]),
      theme,
      1440,
      900,
    );
    const pricing = screen.container.querySelector<HTMLElement>(
      '[data-canvas-stop="p:q-stripe/stripe.com/pricing"]',
    )!;
    pricing.dispatchEvent(new PointerEvent("pointerenter"));
    await expect.poll(() => document.querySelector(".hover")).not.toBeNull();
    await new Promise((done) => setTimeout(done, 300));
    await page.screenshot({ path: `${shots}/hover-${theme}-1440.png` });
  });

  test(`Home with every known section shown, ${theme}`, async () => {
    const screen = await stage(
      fixture.home([fixture.large(null), fixture.raycast]),
      theme,
      1440,
      900,
    );
    screen.container
      .querySelector<HTMLElement>('[data-canvas-stop="p:q-stripe/stripe.com/more"]')!
      .click();
    await new Promise((done) => setTimeout(done, 500));
    await page.screenshot({ path: `${shots}/sections-${theme}-1440.png` });
  });

  test(`Home with a menu row's children shown, ${theme}`, async () => {
    const screen = await stage(fixture.today(), theme, 1440, 900);
    screen.container
      .querySelector<HTMLElement>('[data-canvas-stop="p:q-payments/stripe.com/developers"]')!
      .click();
    await new Promise((done) => setTimeout(done, 500));
    await page.screenshot({ path: `${shots}/menu-${theme}-1440.png` });
  });

  test(`Home with an older search brought to the front, ${theme}`, async () => {
    const screen = await stage(fixture.today(), theme, 1440, 900);
    screen.container.querySelector<HTMLElement>('[data-canvas-stop="s:q-raycast"]')!.click();
    await expect
      .poll(() => screen.container.querySelector('[data-canvas-stop="p:q-raycast/raycast.com"]'))
      .not.toBeNull();
    await new Promise((done) => setTimeout(done, 700));
    await page.screenshot({ path: `${shots}/opened-${theme}-1440.png` });
  });

  test(`Home when its canvas could not load, ${theme}`, async () => {
    await stage(fixture.today(), theme, 1440, 900, false, () => Promise.reject(new Error("gone")));
    await expect.poll(() => document.querySelector(".failed button")).not.toBeNull();
    await page.screenshot({ path: `${shots}/unloaded-${theme}-1440.png` });
  });

  test(`Home a day back, ${theme}`, async () => {
    const screen = await stage(fixture.busy(), theme, 1440, 900);
    const home = admit(fixture.busy());
    const day = layout([home.today, ...home.earlier], home.today.key, {
      expanded: new Set(),
      front: "q-payments",
      shown: new Set(),
    }).nodes.find((node) => node.id === "d:2026-11-22")!;
    const pane = screen.container.querySelector(".svelte-flow__pane")!;
    for (let left = day.box.y - 80; left > 0; left -= 400)
      pane.dispatchEvent(
        new WheelEvent("wheel", { deltaY: Math.min(400, left), bubbles: true, cancelable: true }),
      );
    await new Promise((done) => setTimeout(done, 700));
    await page.screenshot({ path: `${shots}/busy-${theme}-1440.png` });
  });
}
