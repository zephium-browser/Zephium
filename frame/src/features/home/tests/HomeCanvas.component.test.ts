import { afterEach, expect, test, vi } from "vitest";
import { page, userEvent } from "vitest/browser";
import { cleanup, render } from "vitest-browser-svelte";
import "$styles/global.css";
import HomeStage from "./HomeStage.svelte";
import * as fixture from "./fixtures";
import type { HomeIntent, HomeModel } from "$features/home";
import type { Inside } from "../lib/model";

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

async function mount(home: HomeModel) {
  await page.viewport(1440, 900);
  const intents: HomeIntent[] = [];
  let enter: (() => void) | undefined;
  const screen = await render(HomeStage, {
    home,
    onintent: (intent: HomeIntent) => intents.push(intent),
    onready: (ready: () => void) => (enter = ready),
  });
  await expect.poll(() => nodes().length, { timeout: 10_000 }).toBeGreaterThan(2);
  return { screen, intents, enter: () => enter?.() };
}

const settle = () => new Promise((done) => setTimeout(done, 600));

test("every click is an intent naming ids, and nothing opens by itself", async () => {
  const { intents } = await mount(fixture.home([fixture.large(), fixture.grinders]));
  expect(intents).toEqual([]);
  stop("p:q-stripe/stripe.com/pricing")!.click();
  stop("p:q-stripe/stripe.com")!.click();
  stop("s:q-stripe")!.click();
  stop("a:q-stripe/a2")!.click();
  stop("e:q-stripe/stripe.com/github")!.click();
  stop("p:q-stripe/stripe.com/action:act1")!.click();
  stop("i:q-stripe/documentation/docs3")!.click();
  stop("i:q-stripe/documentation/more")!.click();
  document.querySelector<HTMLElement>('[aria-label="Show what is in Payments"]')!.click();
  document.querySelector<HTMLElement>('[aria-label="Hide what is in Documentation"]')!.click();
  expect(intents).toEqual([
    { kind: "row", search: "q-stripe", row: "pricing" },
    { kind: "site", search: "q-stripe", place: "stripe.com" },
    { kind: "results", search: "q-stripe" },
    { kind: "result", search: "q-stripe", result: "a2" },
    { kind: "social", search: "q-stripe", place: "stripe.com", network: "github" },
    { kind: "row", search: "q-stripe", row: "act1" },
    { kind: "row", search: "q-stripe", row: "docs3" },
    { kind: "row", search: "q-stripe", row: "documentation" },
    { kind: "inside", search: "q-stripe", row: "payments" },
    { kind: "fold", search: "q-stripe", row: "documentation" },
  ]);
  expect(JSON.stringify(intents)).not.toMatch(/https?:|\.com\//u);
});

test("the known sections show without asking for anything", async () => {
  const { intents } = await mount(fixture.home([fixture.large(null)]));
  expect(stop("p:q-stripe/stripe.com/more:m0")).toBeNull();
  stop("p:q-stripe/stripe.com/more")!.click();
  await expect.poll(() => stop("p:q-stripe/stripe.com/more:m19")).not.toBeNull();
  expect(intents).toEqual([]);
  stop("p:q-stripe/stripe.com/more:m5")!.click();
  expect(intents).toEqual([{ kind: "row", search: "q-stripe", row: "m5" }]);
});

test("resting on a row shows only what is held, and asks for nothing", async () => {
  const { intents } = await mount(fixture.home([fixture.large(null)]));
  stop("p:q-stripe/stripe.com/pricing")!.dispatchEvent(new PointerEvent("pointerenter"));
  await expect
    .poll(() => document.querySelector(".hover")?.textContent ?? "")
    .toContain("no hidden fees");
  expect(document.querySelector(".hover")!.textContent).toContain("Standard");
  stop("p:q-stripe/stripe.com/pricing")!.dispatchEvent(new PointerEvent("pointerleave"));
  await expect.poll(() => document.querySelector(".hover")).toBeNull();
  // A row with nothing more to say shows nothing.
  stop("p:q-stripe/stripe.com/radar")!.dispatchEvent(new PointerEvent("pointerenter"));
  await settle();
  expect(document.querySelector(".hover")).toBeNull();
  expect(intents).toEqual([]);
});

test("a row being read says so, then shows its inside or why it could not", async () => {
  const opening: Inside = { state: "opening" };
  const { screen } = await mount(fixture.home([fixture.large(opening)]));
  const inside = () => document.querySelector('[aria-label="Documentation"]');
  await expect.poll(() => inside()?.querySelector('[role="status"]')).not.toBeNull();
  expect(stop("i:q-stripe/documentation/docs0")).toBeNull();
  expect(document.querySelector(".lines .opening")).not.toBeNull();

  await screen.rerender({
    home: fixture.home([fixture.large({ state: "refused", reason: "blocked" })]),
  });
  await expect.poll(() => inside()?.textContent ?? "").toContain("does not allow");
  expect(inside()!.querySelector('[role="status"]')).toBeNull();
  expect(document.querySelector(".lines .refused")).not.toBeNull();
  expect(stop("i:q-stripe/documentation/docs0")).toBeNull();

  await screen.rerender({ home: fixture.home([fixture.large()]) });
  await expect.poll(() => stop("i:q-stripe/documentation/docs11")).not.toBeNull();
  expect(document.querySelector(".lines .opening, .lines .refused")).toBeNull();
});

test("the keyboard starts on the asked row and walks rows, insides and cards", async () => {
  const { intents, enter } = await mount(fixture.home([fixture.large(), fixture.raycast]));
  expect(stop("p:q-stripe/stripe.com/pricing")!.tabIndex).toBe(0);
  enter();
  await expect.poll(() => document.activeElement).toBe(stop("p:q-stripe/stripe.com/pricing"));

  await userEvent.keyboard("{ArrowRight}");
  expect(intents.at(-1)).toEqual({ kind: "inside", search: "q-stripe", row: "pricing" });

  await userEvent.keyboard("{ArrowDown}");
  await expect.poll(() => document.activeElement).toBe(stop("p:q-stripe/stripe.com/documentation"));
  await userEvent.keyboard("{ArrowRight}");
  await expect.poll(() => document.activeElement).toBe(stop("i:q-stripe/documentation/docs0"));
  await userEvent.keyboard("{ArrowDown}");
  await expect.poll(() => document.activeElement).toBe(stop("i:q-stripe/documentation/docs1"));
  await userEvent.keyboard("{ArrowLeft}");
  await expect.poll(() => document.activeElement).toBe(stop("p:q-stripe/stripe.com/documentation"));
  await userEvent.keyboard("{ArrowLeft}");
  expect(intents.at(-1)).toEqual({ kind: "fold", search: "q-stripe", row: "documentation" });

  await userEvent.keyboard("{Enter}");
  expect(intents.at(-1)).toEqual({ kind: "row", search: "q-stripe", row: "documentation" });
  await userEvent.keyboard("{Escape}");
  expect(intents.at(-1)).toEqual({ kind: "field" });
});

test("rows that come in after the card is drawn arrive; the rest are simply there", async () => {
  const { screen } = await mount(fixture.home([fixture.arriving(2)]));
  expect(document.querySelector('[role="status"]')?.textContent).toContain("vercel.com");
  expect(document.querySelectorAll(".row.arriving")).toHaveLength(0);
  await screen.rerender({ home: fixture.home([fixture.arriving(5)]) });
  await expect.poll(() => document.querySelectorAll(".row.arriving").length).toBe(3);
  expect(stop("p:q-vercel/vercel.com/v-products")!.closest(".row")!.classList).not.toContain(
    "arriving",
  );
  const done = {
    ...fixture.arriving(7),
    place: { ...fixture.arriving(7).place, state: "settled" as const },
  };
  await screen.rerender({ home: fixture.home([done]) });
  await expect.poll(() => document.querySelector('[role="status"]')).toBeNull();
  await expect
    .poll(
      () => document.getAnimations().filter((motion) => motion.playState === "running").length,
      {
        timeout: 4000,
      },
    )
    .toBe(0);
});

test("a settled Home runs no timer and no animation", async () => {
  await mount(fixture.today());
  await settle();
  await expect
    .poll(
      () => document.getAnimations().filter((motion) => motion.playState === "running").length,
      {
        timeout: 4000,
      },
    )
    .toBe(0);
  const timers = vi.spyOn(window, "setTimeout");
  const intervals = vi.spyOn(window, "setInterval");
  const frames = vi.spyOn(window, "requestAnimationFrame");
  await new Promise((done) => setTimeout(done, 1500));
  // The spy saw this test's own wait, and nothing else.
  expect(timers).toHaveBeenCalledTimes(1);
  expect(intervals).not.toHaveBeenCalled();
  expect(frames).not.toHaveBeenCalled();
  expect(document.getAnimations().filter((motion) => motion.playState === "running")).toHaveLength(
    0,
  );
});

test("only what is near the view is in the page, and an earlier day is asked for as it is reached", async () => {
  const { intents } = await mount(fixture.busy());
  const total = 30 + 6 + 4;
  expect(
    document.querySelectorAll('[data-canvas-stop^="s:"], [data-canvas-stop^="t:"]').length,
  ).toBeLessThan(total / 3);
  expect(intents.filter((intent) => intent.kind === "reach")).toEqual([]);
  const pane = document.querySelector(".svelte-flow__pane")!;
  for (let step = 0; step < 40; step++)
    pane.dispatchEvent(new WheelEvent("wheel", { deltaY: 400, bubbles: true, cancelable: true }));
  await expect
    .poll(() => intents.filter((intent) => intent.kind === "reach"))
    .toEqual([{ kind: "reach", day: "2026-11-20" }]);
  for (let step = 0; step < 4; step++)
    pane.dispatchEvent(new WheelEvent("wheel", { deltaY: -200, bubbles: true, cancelable: true }));
  await settle();
  expect(intents.filter((intent) => intent.kind === "reach")).toHaveLength(1);
});

test("page text is shown as text, never as markup", async () => {
  const hostile = fixture.raycast;
  await mount(
    fixture.home([
      {
        ...hostile,
        text: '<img src=x onerror="alert(1)">',
        place: {
          ...hostile.place,
          title: "<script>alert(1)</script>",
          rows: [
            fixture.row("x", '<a href="https://evil.example">Click</a>‮epyt', {
              note: "<b>bold</b>",
            }),
          ],
        },
      },
    ]),
  );
  const canvas = document.querySelector(".home-canvas")!;
  expect(canvas.querySelector("img, script, a[href]")).toBeNull();
  expect(canvas.textContent).toContain("<script>alert(1)</script>");
  expect(canvas.textContent).not.toContain("‮");
});

test("one search is open at a time; another comes to the front when its line is chosen", async () => {
  const { intents } = await mount(fixture.today());
  expect(stop("p:q-payments/stripe.com")).not.toBeNull();
  expect(stop("p:q-raycast/raycast.com")).toBeNull();
  expect(stop("s:q-raycast")!.getAttribute("aria-expanded")).toBe("false");
  expect(stop("s:q-raycast")!.textContent).toContain("Store, Pro, Teams");

  stop("s:q-raycast")!.click();
  await expect.poll(() => stop("p:q-raycast/raycast.com")).not.toBeNull();
  expect(stop("p:q-payments/stripe.com")).toBeNull();
  expect(stop("s:q-payments")!.getAttribute("aria-expanded")).toBe("false");
  // Bringing a search to the front opens nothing and asks for nothing.
  expect(intents).toEqual([]);
  stop("p:q-raycast/raycast.com/action:act0")!.click();
  expect(intents).toEqual([{ kind: "row", search: "q-raycast", row: "act0" }]);
});

test("the keyboard walks through the day's searches, each opening as it is reached", async () => {
  await mount(fixture.home([fixture.large(null), fixture.grinders, fixture.raycast]));
  stop("s:q-grinders")!.focus();
  await expect.poll(() => stop("o:q-grinders/g1")).not.toBeNull();
  await expect.poll(() => document.activeElement).toBe(stop("s:q-grinders"));
  expect(stop("p:q-payments/stripe.com")).toBeNull();

  // Down goes through what the search in front holds, then on to the next line.
  const at = () => document.activeElement?.getAttribute("data-canvas-stop");
  const walked: (string | null | undefined)[] = [];
  for (let step = 0; step < 4 && !stop("p:q-raycast/raycast.com"); step++) {
    await userEvent.keyboard("{ArrowDown}");
    await settle();
    walked.push(at());
  }
  expect(walked).toEqual(["o:q-grinders/g2", "o:q-grinders/g3", "s:q-raycast"]);
  await expect.poll(() => stop("p:q-raycast/raycast.com")).not.toBeNull();
  await expect.poll(() => document.activeElement).toBe(stop("s:q-raycast"));
  expect(stop("o:q-grinders/g1")).toBeNull();

  await userEvent.keyboard("{ArrowUp}");
  await expect.poll(() => stop("o:q-grinders/g1")).not.toBeNull();
  await expect.poll(() => document.activeElement).toBe(stop("s:q-grinders"));
});

test("with reduced motion a search opens and closes without moving", async () => {
  document.documentElement.dataset.reduceMotion = "true";
  try {
    await mount(fixture.today());
    stop("s:q-raycast")!.click();
    await expect.poll(() => stop("p:q-raycast/raycast.com")).not.toBeNull();
    expect(document.getAnimations().filter((motion) => motion.playState === "running")).toEqual([]);
  } finally {
    delete document.documentElement.dataset.reduceMotion;
  }
});

test("a place too small for a card is a line whose links are intents", async () => {
  const { intents } = await mount(fixture.thinPlaces());
  expect(document.querySelector('[aria-label="Ana Clay Studio"]')!.classList).toContain("thin");
  stop("p:q-studio/anaclay.example/s-visit")!.click();
  stop("p:q-studio/anaclay.example/action:act0")!.click();
  stop("p:q-studio/anaclay.example")!.click();
  expect(intents).toEqual([
    { kind: "row", search: "q-studio", row: "s-visit" },
    { kind: "row", search: "q-studio", row: "act0" },
    { kind: "site", search: "q-studio", place: "anaclay.example" },
  ]);
});

test("a menu row shows what the site already holds under it, asking for nothing", async () => {
  const { intents } = await mount(fixture.today());
  const developers = stop("p:q-payments/stripe.com/developers")!;
  expect(stop("i:q-payments/developers/developers0")).toBeNull();
  const row = developers.closest(".row")!;
  expect(row.querySelector(".host, .shot")).toBeNull();
  expect(row.querySelector('[aria-label="Show what is in Developers"]')).not.toBeNull();
  expect(developers.getAttribute("aria-expanded")).toBe("false");

  developers.click();
  await expect.poll(() => stop("i:q-payments/developers/developers7")).not.toBeNull();
  expect(document.querySelector('[aria-label="Developers"] [role="status"]')).toBeNull();
  expect(document.querySelector(".lines .opening")).toBeNull();
  expect(developers.getAttribute("aria-expanded")).toBe("true");
  expect(intents).toEqual([]);

  stop("i:q-payments/developers/developers1")!.click();
  stop("i:q-payments/developers/more")!.click();
  stop("p:q-payments/stripe.com/pricing")!.click();
  expect(intents).toEqual([
    { kind: "row", search: "q-payments", row: "developers1" },
    { kind: "site", search: "q-payments", place: "stripe.com" },
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
  expect(intents).toHaveLength(3);
});
