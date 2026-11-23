import { afterEach, expect, test, vi } from "vitest";
import { page, userEvent } from "vitest/browser";
import { cleanup, render } from "vitest-browser-svelte";
import "$styles/global.css";
import type { HomeModel } from "$domain/home";
import {
  ended,
  entry,
  first,
  mapEntries,
  placed,
  wirePlace,
  wireSearch,
} from "$shared/testing/home-wire";
import CanvasStage from "./CanvasStage.svelte";
import * as fixture from "./fixtures";
import type { HomeIntent } from "../lib/intent";

vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({ workReleaseMemory: async () => true });
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

const nodes = () => document.querySelectorAll(".svelte-flow__node");
const stop = (id: string) =>
  document.querySelector<HTMLElement>(`[data-canvas-stop="${CSS.escape(id)}"]`);

async function mount(home: HomeModel, props: Record<string, unknown> = {}) {
  await page.viewport(1440, 900);
  const intents: HomeIntent[] = [];
  let enter: (() => void) | undefined;
  const screen = await render(CanvasStage, {
    home,
    onintent: (intent: HomeIntent) => intents.push(intent),
    onready: (ready: () => void) => (enter = ready),
    ...props,
  });
  await expect.poll(() => nodes().length, { timeout: 10_000 }).toBeGreaterThan(2);
  return { screen, intents, enter: () => enter?.() };
}

const settle = () => new Promise((done) => setTimeout(done, 600));
const running = () =>
  document.getAnimations().filter((motion) => motion.playState === "running").length;
const docs = new Set(["q-stripe/documentation"]);

test("every click is an intent naming ids, and nothing opens by itself", async () => {
  const { intents } = await mount(
    fixture.home([fixture.large(), fixture.grinders()], { unfolded: docs }),
  );
  expect(intents).toEqual([]);
  stop("p:q-stripe/0/pricing")!.click();
  stop("p:q-stripe/0")!.click();
  stop("s:q-stripe")!.click();
  stop("a:q-stripe/1")!.click();
  stop("e:q-stripe/0/git_hub")!.click();
  stop("p:q-stripe/0/action:act1")!.click();
  stop("i:q-stripe/documentation/docs3")!.click();
  stop("i:q-stripe/documentation/more")!.click();
  document.querySelector<HTMLElement>('[aria-label="Show what is in Payments"]')!.click();
  document.querySelector<HTMLElement>('[aria-label="Hide what is in Documentation"]')!.click();
  expect(intents).toEqual([
    { kind: "row", search: "q-stripe", row: "pricing" },
    { kind: "site", search: "q-stripe", place: "0" },
    { kind: "results", search: "q-stripe" },
    { kind: "result", search: "q-stripe", result: "1" },
    { kind: "social", search: "q-stripe", place: "0", network: "git_hub" },
    { kind: "row", search: "q-stripe", row: "act1" },
    { kind: "row", search: "q-stripe", row: "docs3" },
    { kind: "row", search: "q-stripe", row: "documentation" },
    { kind: "inside", search: "q-stripe", row: "payments" },
    { kind: "fold", search: "q-stripe", row: "documentation" },
  ]);
  expect(JSON.stringify(intents)).not.toMatch(/https?:|\.com/u);
});

test("the known sections show without asking for anything", async () => {
  const { intents } = await mount(fixture.home([fixture.large()]));
  expect(stop("p:q-stripe/0/more:m0")).toBeNull();
  stop("p:q-stripe/0/more")!.click();
  await expect.poll(() => stop("p:q-stripe/0/more:m19")).not.toBeNull();
  expect(intents).toEqual([]);
  stop("p:q-stripe/0/more:m5")!.click();
  expect(intents).toEqual([{ kind: "row", search: "q-stripe", row: "m5" }]);
});

test("other results show four, and the rest only when asked", async () => {
  const { intents } = await mount(fixture.today());
  expect(stop("a:q-payments/3")).not.toBeNull();
  expect(stop("a:q-payments/4")).toBeNull();
  expect(stop("a:q-payments/more")!.textContent).toContain("4 more");
  stop("a:q-payments/more")!.click();
  await expect.poll(() => stop("a:q-payments/7")).not.toBeNull();
  expect(stop("a:q-payments/more")).toBeNull();
  expect(intents).toEqual([]);
});

test("resting on a row shows only what is held, and asks for nothing", async () => {
  const { intents } = await mount(fixture.home([fixture.large()]));
  stop("p:q-stripe/0/pricing")!.dispatchEvent(new PointerEvent("pointerenter"));
  await expect
    .poll(() => document.querySelector(".hover")?.textContent ?? "")
    .toContain("Standard");
  stop("p:q-stripe/0/pricing")!.dispatchEvent(new PointerEvent("pointerleave"));
  await expect.poll(() => document.querySelector(".hover")).toBeNull();
  // A row with nothing more to say shows nothing.
  stop("p:q-stripe/0/radar")!.dispatchEvent(new PointerEvent("pointerenter"));
  await settle();
  expect(document.querySelector(".hover")).toBeNull();
  expect(intents).toEqual([]);
});

test("a row being read says so, then shows its inside or why it could not", async () => {
  const unread = mapEntries(fixture.large(), (one) =>
    one.id === "documentation" ? { ...one, read: null } : one,
  );
  const { screen } = await mount(fixture.home([unread], { unfolded: docs, waiting: docs }));
  const inside = () => document.querySelector('[aria-label="Documentation"]');
  await expect.poll(() => inside()?.querySelector('[role="status"]')).not.toBeNull();
  expect(stop("i:q-stripe/documentation/docs0")).toBeNull();
  expect(document.querySelector(".lines .opening")).not.toBeNull();

  await screen.rerender({
    home: fixture.home([unread], {
      unfolded: docs,
      refused: new Map([["q-stripe/documentation", "blocked"]]),
    }),
  });
  await expect.poll(() => inside()?.textContent ?? "").toContain("does not allow");
  expect(inside()!.querySelector('[role="status"]')).toBeNull();
  expect(document.querySelector(".lines .refused")).not.toBeNull();
  expect(stop("i:q-stripe/documentation/docs0")).toBeNull();

  await screen.rerender({ home: fixture.largeOpen() });
  await expect.poll(() => stop("i:q-stripe/documentation/docs11")).not.toBeNull();
  expect(document.querySelector(".lines .opening, .lines .refused")).toBeNull();
});

test("the keyboard starts on the asked row and walks rows, insides and cards", async () => {
  const { intents, enter } = await mount(
    fixture.home([fixture.large(), fixture.raycast()], { unfolded: docs }),
  );
  expect(stop("p:q-stripe/0/pricing")!.tabIndex).toBe(0);
  enter();
  await expect.poll(() => document.activeElement).toBe(stop("p:q-stripe/0/pricing"));

  await userEvent.keyboard("{ArrowRight}");
  expect(intents.at(-1)).toEqual({ kind: "inside", search: "q-stripe", row: "pricing" });

  await userEvent.keyboard("{ArrowDown}");
  await expect.poll(() => document.activeElement).toBe(stop("p:q-stripe/0/documentation"));
  await userEvent.keyboard("{ArrowRight}");
  await expect.poll(() => document.activeElement).toBe(stop("i:q-stripe/documentation/docs0"));
  await userEvent.keyboard("{ArrowDown}");
  await expect.poll(() => document.activeElement).toBe(stop("i:q-stripe/documentation/docs1"));
  await userEvent.keyboard("{ArrowLeft}");
  await expect.poll(() => document.activeElement).toBe(stop("p:q-stripe/0/documentation"));
  await userEvent.keyboard("{ArrowLeft}");
  expect(intents.at(-1)).toEqual({ kind: "fold", search: "q-stripe", row: "documentation" });

  await userEvent.keyboard("{Enter}");
  expect(intents.at(-1)).toEqual({ kind: "row", search: "q-stripe", row: "documentation" });
  await userEvent.keyboard("{Escape}");
  expect(intents.at(-1)).toEqual({ kind: "field" });
});

test("returning to a search marks the row its page was opened from", async () => {
  const { enter } = await mount(fixture.today(), {
    marked: { search: "q-payments", entry: "developers" },
  });
  const developers = stop("p:q-payments/0/developers")!;
  expect(developers.getAttribute("aria-current")).toBe("true");
  // The asked row steps aside for the one the page came from.
  expect(stop("p:q-payments/0/pricing")!.getAttribute("aria-current")).toBeNull();
  enter();
  await expect.poll(() => document.activeElement).toBe(developers);
});

test("a search native shows comes to the front without a click", async () => {
  const { screen, intents } = await mount(fixture.today());
  expect(stop("p:q-raycast/0")).toBeNull();
  await screen.rerender({ show: "q-raycast" });
  await expect.poll(() => stop("p:q-raycast/0")).not.toBeNull();
  expect(intents).toEqual([]);
});

const vercelDone = () =>
  wireSearch({
    id: "q-vercel",
    text: "vercel",
    at: fixture.NOW - 60_000,
    changes: fixture.vercelStream(),
  });

test("rows that come in after the card is drawn arrive; the rest are simply there", async () => {
  const { screen } = await mount(fixture.home([fixture.arriving(2)]));
  expect(document.querySelector('[role="status"]')?.textContent).toContain("vercel.com");
  expect(document.querySelectorAll(".row.arriving")).toHaveLength(0);
  await screen.rerender({ home: fixture.home([fixture.arriving(5)]) });
  await expect.poll(() => document.querySelectorAll(".row.arriving").length).toBe(3);
  expect(stop("p:q-vercel/0/v-products")!.closest(".row")!.classList).not.toContain("arriving");
  await screen.rerender({ home: fixture.home([vercelDone()]) });
  await expect.poll(() => document.querySelector('[role="status"]')).toBeNull();
  await expect.poll(running, { timeout: 4000 }).toBe(0);
});

test("a settled Home runs no timer and no animation", async () => {
  await mount(fixture.today());
  await settle();
  await expect.poll(running, { timeout: 4000 }).toBe(0);
  const timers = vi.spyOn(window, "setTimeout");
  const intervals = vi.spyOn(window, "setInterval");
  const frames = vi.spyOn(window, "requestAnimationFrame");
  await new Promise((done) => setTimeout(done, 1500));
  // The spy saw this test's own wait, and nothing else.
  expect(timers).toHaveBeenCalledTimes(1);
  expect(intervals).not.toHaveBeenCalled();
  expect(frames).not.toHaveBeenCalled();
  expect(running()).toBe(0);
});

test("only what is near the view is in the page, and earlier days come in as they are reached", async () => {
  await mount(fixture.busy());
  const total = 32;
  await expect
    .poll(
      () => document.querySelectorAll('[data-canvas-stop^="s:"], [data-canvas-stop^="t:"]').length,
    )
    .toBeLessThan(total / 3);
  expect(stop("s:s-3")).toBeNull();
  const pane = document.querySelector(".svelte-flow__pane")!;
  for (let step = 0; step < 12 && !stop("s:s-3"); step++) {
    pane.dispatchEvent(new WheelEvent("wheel", { deltaY: 200, bubbles: true, cancelable: true }));
    await new Promise((done) => setTimeout(done, 120));
  }
  await expect.poll(() => stop("s:s-3")).not.toBeNull();
  expect(stop("s:t-2")).toBeNull();
});

test("page text is shown as text, never as markup", async () => {
  const hostile = wireSearch({
    id: "q-hostile",
    text: '<img src=x onerror="alert(1)">',
    at: fixture.NOW - 60_000,
    changes: [
      first(1, { title: "<script>alert(1)</script>", host: "raycast.com" }),
      placed(
        2,
        wirePlace({
          title: "<script>alert(1)</script>",
          host: "raycast.com",
          rows: [entry("x", '<a href="https://evil.example">Click</a>‮epyt')],
        }),
      ),
      ended(3),
    ],
  });
  await mount(fixture.home([hostile]));
  const canvas = document.querySelector(".home-canvas")!;
  expect(canvas.querySelector("img:not(.engine-mark), script, a[href]")).toBeNull();
  expect(canvas.textContent).toContain("<script>alert(1)</script>");
  expect(canvas.textContent).not.toContain("‮");
});

test("one search is open at a time; another comes to the front when its line is chosen", async () => {
  const { intents } = await mount(fixture.today());
  expect(stop("p:q-payments/0")).not.toBeNull();
  expect(stop("p:q-raycast/0")).toBeNull();
  expect(stop("s:q-raycast")!.getAttribute("aria-expanded")).toBe("false");
  expect(stop("s:q-raycast")!.textContent).toContain("Store, Pro, Teams");

  stop("s:q-raycast")!.click();
  await expect.poll(() => stop("p:q-raycast/0")).not.toBeNull();
  expect(stop("p:q-payments/0")).toBeNull();
  expect(stop("s:q-payments")!.getAttribute("aria-expanded")).toBe("false");
  // Bringing a search to the front opens nothing and asks for nothing.
  expect(intents).toEqual([]);
  stop("p:q-raycast/0/action:act0")!.click();
  expect(intents).toEqual([{ kind: "row", search: "q-raycast", row: "act0" }]);
});

test("a search opened by hand stays open when a newer one arrives", async () => {
  const { screen } = await mount(fixture.today());
  stop("s:q-raycast")!.click();
  await expect.poll(() => stop("p:q-raycast/0")).not.toBeNull();
  await screen.rerender({ home: fixture.home([fixture.studio(), ...fixture.todayWire()]) });
  await expect.poll(() => stop("p:q-studio/0")).not.toBeNull();
  expect(stop("p:q-raycast/0")).not.toBeNull();
  expect(stop("p:q-payments/0")).toBeNull();
});

test("a line is removed from its quiet button or with Delete, and nothing else is asked", async () => {
  const { intents } = await mount(fixture.today());
  const remove = document.querySelector<HTMLElement>(
    '[aria-label="Remove raycast pro from Home"]',
  )!;
  expect(getComputedStyle(remove).opacity).toBe("0");
  expect(remove.tabIndex).toBe(-1);
  remove.click();
  expect(intents).toEqual([{ kind: "forget", search: "q-raycast" }]);

  stop("s:q-grinders")!.focus();
  await expect
    .poll(() => document.activeElement?.getAttribute("data-canvas-stop"))
    .toBe("s:q-grinders");
  await userEvent.keyboard("{Delete}");
  expect(intents.at(-1)).toEqual({ kind: "forget", search: "q-grinders" });
  stop("t:t-gh")!.focus();
  await userEvent.keyboard("{Backspace}");
  expect(intents.at(-1)).toEqual({ kind: "forget", search: "t-gh" });
  expect(intents).toHaveLength(3);
});

test("the keyboard walks through the day's searches, each opening as it is reached", async () => {
  await mount(fixture.home([fixture.large(), fixture.grinders(), fixture.raycast()]));
  stop("s:q-grinders")!.focus();
  await expect.poll(() => stop("o:q-grinders/0")).not.toBeNull();
  await expect.poll(() => document.activeElement).toBe(stop("s:q-grinders"));
  expect(stop("p:q-stripe/0")).toBeNull();

  // Down goes through what the search in front holds, then on to the next line.
  const at = () => document.activeElement?.getAttribute("data-canvas-stop");
  const walked: (string | null | undefined)[] = [];
  for (let step = 0; step < 4 && !stop("p:q-raycast/0"); step++) {
    await userEvent.keyboard("{ArrowDown}");
    await settle();
    walked.push(at());
  }
  expect(walked).toEqual(["o:q-grinders/1", "o:q-grinders/2", "s:q-raycast"]);
  await expect.poll(() => stop("p:q-raycast/0")).not.toBeNull();
  await expect.poll(() => document.activeElement).toBe(stop("s:q-raycast"));
  expect(stop("o:q-grinders/0")).toBeNull();

  await userEvent.keyboard("{ArrowUp}");
  await expect.poll(() => stop("o:q-grinders/0")).not.toBeNull();
  await expect.poll(() => document.activeElement).toBe(stop("s:q-grinders"));
});

test("with reduced motion a search opens and closes without moving", async () => {
  document.documentElement.dataset.reduceMotion = "true";
  try {
    await mount(fixture.today());
    stop("s:q-raycast")!.click();
    await expect.poll(() => stop("p:q-raycast/0")).not.toBeNull();
    expect(running()).toBe(0);
  } finally {
    delete document.documentElement.dataset.reduceMotion;
  }
});

test("a place too small for a card is a line whose links are intents", async () => {
  const { intents } = await mount(fixture.thinPlaces());
  expect(document.querySelector('[aria-label="Ana Clay Studio"]')!.classList).toContain("thin");
  stop("p:q-studio/0/s-visit")!.click();
  stop("p:q-studio/0/action:act0")!.click();
  stop("p:q-studio/0")!.click();
  expect(intents).toEqual([
    { kind: "row", search: "q-studio", row: "s-visit" },
    { kind: "row", search: "q-studio", row: "act0" },
    { kind: "site", search: "q-studio", place: "0" },
  ]);
});

test("a place with no tab open is drawn by its letter, never by asking its address", async () => {
  await mount(fixture.thinPlaces(), { mark: () => null });
  const studio = document.querySelector('[aria-label="Ana Clay Studio"]')!;
  expect(studio.querySelector(".letter")?.textContent).toBe("A");
});

test("a menu row shows what the site already holds under it, asking for nothing", async () => {
  const { intents } = await mount(fixture.today());
  const developers = stop("p:q-payments/0/developers")!;
  expect(stop("i:q-payments/developers/developers0")).toBeNull();
  const row = developers.closest(".row")!;
  expect(row.querySelector(".host")).toBeNull();
  expect(row.querySelector('[aria-label="Show what is in Developers"]')).not.toBeNull();
  expect(developers.getAttribute("aria-expanded")).toBe("false");

  developers.click();
  await expect.poll(() => stop("i:q-payments/developers/developers6")).not.toBeNull();
  expect(stop("i:q-payments/developers/developers7")).toBeNull();
  expect(document.querySelector('[aria-label="Developers"] [role="status"]')).toBeNull();
  expect(document.querySelector(".lines .opening")).toBeNull();
  expect(developers.getAttribute("aria-expanded")).toBe("true");
  expect(intents).toEqual([]);

  stop("i:q-payments/developers/developers1")!.click();
  stop("i:q-payments/developers/more")!.click();
  await expect.poll(() => stop("i:q-payments/developers/developers7")).not.toBeNull();
  stop("p:q-payments/0/pricing")!.click();
  expect(intents).toEqual([
    { kind: "row", search: "q-payments", row: "developers1" },
    { kind: "row", search: "q-payments", row: "pricing" },
  ]);

  developers.focus();
  await userEvent.keyboard("{ArrowRight}");
  await expect.poll(() => document.activeElement).toBe(stop("i:q-payments/developers/developers0"));
  await userEvent.keyboard("{ArrowLeft}");
  await expect.poll(() => document.activeElement).toBe(developers);
  await userEvent.keyboard("{ArrowLeft}");
  await expect.poll(() => stop("i:q-payments/developers/developers0")).toBeNull();
  await userEvent.keyboard("{Enter}");
  await expect.poll(() => stop("i:q-payments/developers/developers0")).not.toBeNull();
  expect(intents).toHaveLength(2);
});
