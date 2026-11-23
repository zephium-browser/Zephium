import { afterEach, expect, test, vi } from "vitest";
import { page } from "vitest/browser";
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
