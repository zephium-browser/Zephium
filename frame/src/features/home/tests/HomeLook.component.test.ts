import { afterEach, expect, test, vi } from "vitest";
import { page } from "vitest/browser";
import { cleanup, render } from "vitest-browser-svelte";
import "$styles/global.css";
import type { HomeModel } from "$domain/home";
import CanvasStage from "./CanvasStage.svelte";
import * as fixture from "./fixtures";

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

const SCENES: Record<string, { home: () => HomeModel; wide?: boolean }> = {
  today: { home: fixture.today },
  compare: { home: fixture.comparing },
  states: { home: fixture.states },
  edges: { home: fixture.edges },
  day: { home: fixture.busy, wide: true },
  large: { home: () => fixture.home([fixture.large(), fixture.raycast()]), wide: true },
  open: { home: fixture.largeOpen, wide: true },
  thin: { home: fixture.thinPlaces, wide: true },
  arriving: { home: () => fixture.home([fixture.arriving(), fixture.figma()]), wide: true },
  failed: { home: () => fixture.home([fixture.failed(), fixture.raycast()]), wide: true },
  rtl: { home: fixture.rtlFirst, wide: true },
};

async function stage(home: HomeModel, theme: string, width: number, height: number) {
  await page.viewport(width, height);
  document.documentElement.dataset.theme = theme;
  document.documentElement.dataset.material = "none";
  const screen = await render(CanvasStage, { home });
  await expect
    .poll(() => screen.container.querySelectorAll(".svelte-flow__node").length, {
      timeout: 10_000,
    })
    .toBeGreaterThan(2);
  await new Promise((done) => setTimeout(done, 700));
  return screen;
}

const at = (root: HTMLElement, id: string) =>
  root.querySelector<HTMLElement>(`[data-canvas-stop="${CSS.escape(id)}"]`);

for (const [scene, { home, wide }] of Object.entries(SCENES))
  for (const theme of ["light", "dark"])
    for (const [width, height] of wide ? SIZES.slice(0, 1) : SIZES)
      test(`Home ${scene}, ${theme}, ${width}×${height}`, async () => {
        await stage(home(), theme, width, height);
        await page.screenshot({ path: `${shots}/${scene}-${theme}-${width}.png` });
      });

for (const theme of ["light", "dark"]) {
  test(`Home with a row's known links on hover, ${theme}`, async () => {
    const screen = await stage(
      fixture.home([fixture.large(), fixture.raycast()]),
      theme,
      1440,
      900,
    );
    at(screen.container, "p:q-stripe/0/pricing")!.dispatchEvent(new PointerEvent("pointerenter"));
    await expect.poll(() => document.querySelector(".hover")).not.toBeNull();
    await new Promise((done) => setTimeout(done, 300));
    await page.screenshot({ path: `${shots}/hover-${theme}-1440.png` });
  });

  test(`Home with every known section shown, ${theme}`, async () => {
    const screen = await stage(
      fixture.home([fixture.large(), fixture.raycast()]),
      theme,
      1440,
      900,
    );
    at(screen.container, "p:q-stripe/0/more")!.click();
    await new Promise((done) => setTimeout(done, 500));
    await page.screenshot({ path: `${shots}/sections-${theme}-1440.png` });
  });

  test(`Home with a menu row's children and every other result shown, ${theme}`, async () => {
    const screen = await stage(fixture.today(), theme, 1440, 900);
    at(screen.container, "p:q-payments/0/developers")!.click();
    at(screen.container, "a:q-payments/more")!.click();
    await new Promise((done) => setTimeout(done, 500));
    await page.screenshot({ path: `${shots}/menu-${theme}-1440.png` });
  });

  test(`Home with an older search brought to the front, ${theme}`, async () => {
    const screen = await stage(fixture.today(), theme, 1440, 900);
    at(screen.container, "s:q-raycast")!.click();
    await expect.poll(() => at(screen.container, "p:q-raycast/0")).not.toBeNull();
    await new Promise((done) => setTimeout(done, 700));
    await page.screenshot({ path: `${shots}/opened-${theme}-1440.png` });
  });

  test(`Home returned to from a closed page, ${theme}`, async () => {
    const screen = await render(CanvasStage, {
      home: fixture.today(),
      marked: { search: "q-payments", entry: "developers" },
    });
    await page.viewport(1440, 900);
    document.documentElement.dataset.theme = theme;
    document.documentElement.dataset.material = "none";
    await expect.poll(() => at(screen.container, "p:q-payments/0/developers")).not.toBeNull();
    await new Promise((done) => setTimeout(done, 700));
    await page.screenshot({ path: `${shots}/returned-${theme}-1440.png` });
  });

  test(`Home a day back, ${theme}`, async () => {
    const screen = await stage(fixture.busy(), theme, 1440, 900);
    const pane = screen.container.querySelector(".svelte-flow__pane")!;
    for (let step = 0; step < 6; step++)
      pane.dispatchEvent(new WheelEvent("wheel", { deltaY: 200, bubbles: true, cancelable: true }));
    await new Promise((done) => setTimeout(done, 700));
    await page.screenshot({ path: `${shots}/busy-${theme}-1440.png` });
  });
}
