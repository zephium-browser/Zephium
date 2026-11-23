import { afterEach, beforeAll, beforeEach, expect, test, vi } from "vitest";
import { page } from "vitest/browser";
import { cleanup, render } from "vitest-browser-svelte";
import "$styles/global.css";
import { emitNativeEvent } from "$shared/testing/native-events";
import { wireSearch } from "$shared/testing/home-wire";
import { init as showing } from "$session/home-show.svelte";
import { loadHome } from "$features/home";
import HomeFrame from "../components/HomeFrame.svelte";
import * as fixture from "./fixtures";

const harness = vi.hoisted(() => ({
  searches: [] as unknown[],
  settled: { outcome: "applied", disposition: null } as unknown,
  inside: { accepted: true, refusal: null } as unknown,
  calls: [] as unknown[][],
}));

vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  const { snapshot } = await import("$shared/testing/home-wire");
  const call =
    (name: string, answer: unknown = { accepted: true }) =>
    async (...args: unknown[]) => {
      harness.calls.push([name, ...args]);
      return answer as never;
    };
  return mockBindings({
    workReleaseMemory: async () => true,
    homeSnapshot: async () => snapshot(harness.searches as never) as never,
    homeOpenTarget: call("open"),
    homeOpenInside: async (...args: unknown[]) => {
      harness.calls.push(["inside", ...args]);
      return harness.inside as never;
    },
    homeOpenResults: call("results"),
    homeFocusSearch: call("focus"),
    homeForgetSearch: call("forget"),
  });
});
vi.mock("$domain/operations", async (actual) => ({
  ...(await actual<object>()),
  settle: vi.fn(async (pending: Promise<unknown>) => {
    await pending;
    return harness.settled;
  }),
}));
vi.mock("$domain/tabs", () => ({
  tabs: {
    profile: () => ({ id: "profile-a" }),
    activeSpaceId: () => "space-a",
    tabs: () => [],
    activate: (id: string) => harness.calls.push(["activate", id]),
  },
}));

beforeAll(async () => {
  await showing();
});
beforeEach(() => {
  harness.searches = [];
  harness.settled = { outcome: "applied", disposition: null };
  harness.inside = { accepted: true, refusal: null };
  harness.calls = [];
});
afterEach(() => cleanup());

const nodes = () => document.querySelectorAll(".svelte-flow__node").length;
const stop = (id: string) =>
  document.querySelector<HTMLElement>(`[data-canvas-stop="${CSS.escape(id)}"]`);
const drawn = () => expect.poll(nodes, { timeout: 10_000 }).toBeGreaterThan(2);
const named = (name: string) => harness.calls.filter((call) => call[0] === name);
const changed = () => emitNativeEvent("homeChanged", { profile: "profile-a", space: "space-a" });

async function mount(props: Record<string, unknown> = {}) {
  await page.viewport(1440, 900);
  const said: boolean[] = [];
  const asked: string[] = [];
  const screen = await render(HomeFrame, {
    onsearching: (searching: boolean) => said.push(searching),
    onask: (text: string) => asked.push(text),
    ...props,
  });
  return { screen, said, asked };
}

test("Home's canvas code is asked for after the first paint, once there is something to draw", async () => {
  harness.searches = fixture.todayWire();
  let painted = false;
  requestAnimationFrame(() => requestAnimationFrame(() => (painted = true)));
  const seen: boolean[] = [];
  const load = vi.fn(async () => {
    seen.push(painted);
    return import("../components/HomeCanvas.svelte");
  });
  const { said } = await mount({ load });
  expect(load).not.toHaveBeenCalled();
  await expect.poll(() => load.mock.calls.length).toBe(1);
  expect(seen).toEqual([true]);
  await drawn();
  expect(said.at(-1)).toBe(true);
});

test("with no searches Home loads no canvas and says there is nothing to draw", async () => {
  const load = vi.fn(async () => import("../components/HomeCanvas.svelte"));
  const { said } = await mount({ load });
  await expect.poll(() => said.at(-1)).toBe(false);
  expect(said).not.toContain(true);
  await new Promise((done) => setTimeout(done, 300));
  expect(load).not.toHaveBeenCalled();
  expect(document.querySelector(".svelte-flow")).toBeNull();
});

test("Home's public entry is lazy and leads to its frame", async () => {
  expect((await loadHome()).default).toBe(HomeFrame);
});

test("when the canvas cannot load Home says so quietly and can try again", async () => {
  harness.searches = fixture.todayWire();
  let fail = true;
  const load = vi.fn(async () => {
    if (fail) throw new Error("gone");
    return import("../components/HomeCanvas.svelte");
  });
  await mount({ load });
  await expect
    .poll(() => document.querySelector('[role="status"]')?.textContent ?? "")
    .toContain("could not be shown");
  fail = false;
  document.querySelector<HTMLElement>(".status button")!.click();
  await drawn();
  expect(load).toHaveBeenCalledTimes(2);
  expect(document.querySelector(".status")).toBeNull();
});

test("a map that arrives in pieces is drawn as each piece comes", async () => {
  harness.searches = [wireSearch({ id: "q-vercel", text: "vercel", at: fixture.NOW, live: true })];
  await mount();
  await drawn();
  const stream = fixture.vercelStream();
  for (const change of stream.slice(0, 3))
    emitNativeEvent("homeMapChanged", { map: "map-q-vercel", search: "q-vercel", change });
  await expect.poll(() => stop("p:q-vercel/0/v-docs")).not.toBeNull();
  expect(stop("p:q-vercel/0/v-pricing")).toBeNull();
  for (const change of stream.slice(3))
    emitNativeEvent("homeMapChanged", { map: "map-q-vercel", search: "q-vercel", change });
  await expect.poll(() => stop("p:q-vercel/0/v-enterprise")).not.toBeNull();
  await expect.poll(() => document.querySelector(".reading")).toBeNull();
});

test("native showing a search brings it to the front on the row its page came from", async () => {
  harness.searches = fixture.todayWire();
  await mount();
  await drawn();
  expect(stop("p:q-raycast/0")).toBeNull();
  emitNativeEvent("homeShow", { search: "q-raycast", slot: 0, entry: "teams" });
  await expect.poll(() => stop("p:q-raycast/0/teams")?.getAttribute("aria-current")).toBe("true");
  expect(stop("p:q-payments/0")).not.toBeNull();
  // A search Home does not hold is not shown.
  emitNativeEvent("homeShow", { search: "q-nowhere", slot: null, entry: null });
  await new Promise((done) => setTimeout(done, 200));
  expect(stop("p:q-raycast/0/teams")!.getAttribute("aria-current")).toBe("true");
});

test("clicks go to native by ids, and a refusal is said quietly", async () => {
  harness.searches = fixture.todayWire();
  await mount();
  await drawn();
  stop("p:q-payments/0/pricing")!.click();
  stop("a:q-payments/1")!.click();
  stop("e:q-payments/0/git_hub")!.click();
  stop("s:q-payments")!.click();
  await expect.poll(() => harness.calls.length).toBe(4);
  expect(harness.calls).toEqual([
    ["open", "q-payments", 0, { kind: "entry", id: "pricing" }],
    ["open", "q-payments", 0, { kind: "also", index: 1 }],
    ["open", "q-payments", 0, { kind: "social", network: "git_hub" }],
    ["results", "q-payments"],
  ]);
  expect(JSON.stringify(harness.calls)).not.toMatch(/https?:|\.com/u);
  harness.settled = { outcome: "rejected", disposition: { reason: "home_search_unknown" } };
  stop("p:q-payments/0")!.click();
  await expect
    .poll(() => document.querySelector('[role="status"]')?.textContent ?? "")
    .toContain("This search has ended.");
});

test("a row whose map has gone offers to search again with the same words", async () => {
  harness.searches = [fixture.large()];
  harness.inside = { accepted: false, refusal: "map_gone" };
  const { asked } = await mount();
  await drawn();
  document.querySelector<HTMLElement>('[aria-label="Show what is in Payments"]')!.click();
  await expect.poll(() => document.body.textContent ?? "").toContain("Search again");
  stop("i:q-stripe/payments")!.click();
  expect(asked).toEqual(["stripe pricing"]);
});

test("a search nothing could be drawn for opens its results page once, untouched", async () => {
  harness.searches = [fixture.raycast()];
  await mount();
  await drawn();
  harness.searches = [fixture.failed(), fixture.raycast()];
  changed();
  await expect.poll(() => named("results")).toEqual([["results", "q-failed"]]);
  changed();
  await new Promise((done) => setTimeout(done, 300));
  expect(named("results")).toHaveLength(1);
});

test("a search that fails after the person has acted leaves the results page to them", async () => {
  harness.searches = [fixture.raycast()];
  await mount();
  await drawn();
  harness.searches = [fixture.arriving(), fixture.raycast()];
  changed();
  await expect.poll(() => stop("s:q-vercel")).not.toBeNull();
  document.dispatchEvent(new KeyboardEvent("keydown", { key: "Shift", bubbles: true }));
  harness.searches = [{ ...fixture.failed(), id: "q-vercel" }, fixture.raycast()];
  changed();
  await expect.poll(() => document.body.textContent ?? "").toContain("Northwind");
  await new Promise((done) => setTimeout(done, 300));
  expect(named("results")).toEqual([]);
});

test("forgetting the last search brings the welcome back", async () => {
  harness.searches = [fixture.raycast(), fixture.grinders()];
  const { said } = await mount();
  await drawn();
  document
    .querySelector<HTMLElement>(
      '[aria-label="Remove best espresso grinder for beginners from Home"]',
    )!
    .click();
  await expect.poll(() => named("forget")).toEqual([["forget", "q-grinders"]]);
  harness.searches = [];
  changed();
  await expect.poll(() => said.at(-1)).toBe(false);
  expect(document.querySelector(".svelte-flow")).toBeNull();
});
