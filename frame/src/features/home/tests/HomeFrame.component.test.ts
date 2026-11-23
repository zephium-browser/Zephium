import { afterEach, expect, test, vi } from "vitest";
import { page, userEvent } from "vitest/browser";
import { cleanup, render } from "vitest-browser-svelte";
import "$styles/global.css";
import { loadHome } from "$features/home";
import HomeFrame from "../components/HomeFrame.svelte";
import HomeStage from "./HomeStage.svelte";
import * as fixture from "./fixtures";

vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({ workReleaseMemory: async () => true });
});

afterEach(() => cleanup());

test("Home paints its field before asking for any canvas code", async () => {
  await page.viewport(1440, 900);
  const seen: { field: boolean; painted: boolean }[] = [];
  let painted = false;
  requestAnimationFrame(() => requestAnimationFrame(() => (painted = true)));
  const load = vi.fn(async () => {
    seen.push({ field: !!document.querySelector("input[aria-label]"), painted });
    return import("../components/HomeCanvas.svelte");
  });
  await render(HomeStage, { home: fixture.today(), load });
  expect(load).not.toHaveBeenCalled();
  expect(document.querySelector("input[aria-label]")).not.toBeNull();
  await expect.poll(() => load.mock.calls.length).toBe(1);
  expect(seen).toEqual([{ field: true, painted: true }]);
  await expect
    .poll(() => document.querySelectorAll(".svelte-flow__node").length, { timeout: 10_000 })
    .toBeGreaterThan(2);
});

test("before the day's first search Home shows its name and time, and loads no canvas", async () => {
  await page.viewport(1440, 900);
  const load = vi.fn(async () => import("../components/HomeCanvas.svelte"));
  await render(HomeStage, {
    home: fixture.home([], [{ key: "2026-11-22", count: 1, entries: [fixture.compare] }]),
    load,
  });
  await new Promise((done) => setTimeout(done, 400));
  expect(document.querySelector('svg[aria-label="Zephium"]')).not.toBeNull();
  expect(load).not.toHaveBeenCalled();
  expect(document.querySelector(".svelte-flow")).toBeNull();
});

test("Home's public entry is lazy and leads to its frame", async () => {
  expect((await loadHome()).default).toBe(HomeFrame);
});

test("when the canvas cannot load Home says so quietly, the field still works, and it can try again", async () => {
  await page.viewport(1440, 900);
  let fail = true;
  const load = vi.fn(async () => {
    if (fail) throw new Error("gone");
    return import("../components/HomeCanvas.svelte");
  });
  await render(HomeStage, { home: fixture.today(), load });
  await expect
    .poll(() => document.querySelector('[role="status"]')?.textContent ?? "")
    .toContain("could not be shown");
  const field = document.querySelector<HTMLInputElement>("input[aria-label]")!;
  await userEvent.type(field, "still here");
  expect(field.value).toBe("still here");
  fail = false;
  document.querySelector<HTMLElement>(".failed button")!.click();
  await expect
    .poll(() => document.querySelectorAll(".svelte-flow__node").length, { timeout: 10_000 })
    .toBeGreaterThan(2);
  expect(load).toHaveBeenCalledTimes(2);
  expect(document.querySelector(".failed")).toBeNull();
});

const yesterday = () =>
  fixture.home([], [{ key: "2026-11-22", count: 2, entries: [fixture.compare, fixture.raycast] }]);

test("before the first search, scrolling past the welcome brings the days before", async () => {
  await page.viewport(1440, 900);
  const load = vi.fn(async () => import("../components/HomeCanvas.svelte"));
  await render(HomeStage, { home: yesterday(), load });
  expect(document.querySelector(".earlier")!.textContent).toContain("Yesterday");
  await new Promise((done) => setTimeout(done, 300));
  expect(load).not.toHaveBeenCalled();
  document
    .querySelector(".home")!
    .dispatchEvent(new WheelEvent("wheel", { deltaY: 120, bubbles: true }));
  await expect
    .poll(() => document.querySelector('[data-canvas-stop="s:q-compare"]'), { timeout: 10_000 })
    .not.toBeNull();
  expect(document.querySelector(".svelte-flow")!.textContent).toContain("Yesterday");
  expect(document.querySelector(".welcome")!.classList).toContain("gone");
  expect(document.querySelector(".earlier")).toBeNull();
});

test("before the first search, ArrowDown goes past the welcome onto the days before", async () => {
  await page.viewport(1440, 900);
  let enter: (() => void) | undefined;
  await render(HomeStage, { home: yesterday(), onready: (ready: () => void) => (enter = ready) });
  expect(document.querySelector(".svelte-flow")).toBeNull();
  enter!();
  await expect
    .poll(() => document.activeElement?.getAttribute("data-canvas-stop"), { timeout: 10_000 })
    .toBe("s:q-compare");
});
