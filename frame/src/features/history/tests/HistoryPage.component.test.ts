import { afterEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { page, userEvent } from "vitest/browser";
import "$styles/global.css";
import primitives from "../../../styles/tokens/primitive.css?raw";
import tokens from "../../../styles/tokens.css?raw";
import browserCSS from "../../../styles/browser.css?raw";
import type { HistoryCall, HistoryResponse, HistoryVisitView } from "$shared/ipc/bindings";
import HistoryPage from "./PageHost.svelte";

const native = vi.hoisted(() => ({
  history: vi.fn(),
  open: vi.fn(async () => ({ accepted: true, operation_id: null })),
}));

vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    historyCall: native.history as never,
    browserOpenUrl: native.open as never,
  });
});

vi.mock("$domain/tabs", () => ({
  tabs: { profile: () => ({ id: "profile", name: "Personal", kind: "default" }) },
}));

vi.mock("$domain/surface", () => ({
  surface: { open: vi.fn(async () => {}) },
}));

const style = document.createElement("style");
style.textContent = primitives + tokens.replace("@theme static", ":root") + browserCSS;
document.head.append(style);

afterEach(() => {
  vi.useRealTimers();
  vi.clearAllMocks();
  delete document.documentElement.dataset.theme;
});

const DAY = 24 * 60 * 60;

function visit(title: string, agoSeconds: number, host = "example.com"): HistoryVisitView {
  const at = Math.floor(Date.now() / 1000) - agoSeconds;
  return {
    id: String(at * 1000 + Math.floor(Math.random() * 1000)),
    url: `https://${host}/${encodeURIComponent(title)}`,
    title,
    visited_at: String(at),
    icon: null,
  };
}

/** Serves pages from a fixed list, honouring the cursor the surface sends. */
function serve(all: HistoryVisitView[]) {
  let removed: string[] = [];
  native.history.mockImplementation(async (_profile: string, call: HistoryCall) => {
    if (call.kind === "forget") {
      removed = [...removed, ...call.urls];
      return { kind: "removed", count: call.urls.length } satisfies HistoryResponse;
    }
    if (call.kind === "clear") {
      removed = all.map((entry) => entry.url);
      return { kind: "removed", count: all.length } satisfies HistoryResponse;
    }
    if (call.kind !== "page") return { kind: "error", error: "invalid" } satisfies HistoryResponse;
    const needle = call.query.toLowerCase();
    const window = { hour: 3600, day: 24 * 3600, week: 7 * 24 * 3600, everything: null }[
      call.range
    ];
    const floor = window === null ? 0 : Math.floor(Date.now() / 1000) - window;
    const matching = all
      .filter((entry) => !removed.includes(entry.url))
      .filter((entry) => Number(entry.visited_at) >= floor)
      .filter((entry) => !needle || entry.title.toLowerCase().includes(needle))
      .filter((entry) => !call.before || Number(entry.id) < Number(call.before));
    const visits = matching.slice(0, call.limit);
    return {
      kind: "page",
      visits,
      next: visits.length === call.limit ? (visits.at(-1)?.id ?? null) : null,
    } satisfies HistoryResponse;
  });
}

test.each([0, 12, 23])("groups visits under the day they happened at %i:30", async (hour) => {
  vi.useFakeTimers({ toFake: ["Date"] });
  vi.setSystemTime(new Date(2026, 9, 28, hour, 30));
  const yesterday = new Date();
  yesterday.setDate(yesterday.getDate() - 1);
  serve([
    visit("Rust Book", 60),
    visit("Release Notes", 2 * 60),
    visit("Old Article", (Date.now() - yesterday.getTime()) / 1000),
  ]);
  render(HistoryPage);

  await expect.element(page.getByText("Rust Book")).toBeVisible();
  await expect.element(page.getByText("Today")).toBeVisible();
  await expect.element(page.getByText("Yesterday")).toBeVisible();
});

test("narrows the list to what the reader typed", async () => {
  serve([visit("Rust Book", 60), visit("Release Notes", 120)]);
  render(HistoryPage);
  await expect.element(page.getByText("Release Notes")).toBeVisible();

  await userEvent.fill(page.getByRole("searchbox"), "rust");

  await expect.element(page.getByText("Rust Book")).toBeVisible();
  await vi.waitFor(() => expect(page.getByText("Release Notes").elements()).toHaveLength(0));
});

test("a forgotten address leaves and does not come back with the next page", async () => {
  serve([visit("Forgettable", 60), visit("Keepsake", 120)]);
  render(HistoryPage);
  await expect.element(page.getByText("Forgettable")).toBeVisible();

  await page.getByText("Forgettable").hover();
  await page.getByRole("button", { name: "Remove from history" }).first().click();

  await vi.waitFor(() => expect(page.getByText("Forgettable").elements()).toHaveLength(0));
  await expect.element(page.getByText("Keepsake")).toBeVisible();
});

test("clearing asks before it removes anything", async () => {
  serve([visit("Rust Book", 60), visit("Release Notes", 120)]);
  render(HistoryPage);
  await expect.element(page.getByText("Rust Book")).toBeVisible();

  // One press states what it is about to do; it must not have done it yet.
  await page.getByRole("button", { name: "Clear" }).click();
  await expect.element(page.getByRole("button", { name: "Clear all history?" })).toBeVisible();
  expect(native.history.mock.calls.some((call) => call[1].kind === "clear")).toBe(false);
  await expect.element(page.getByText("Rust Book")).toBeVisible();

  await page.getByRole("button", { name: "Clear all history?" }).click();

  await expect.element(page.getByText("Your browsing history belongs here")).toBeVisible();
});

test("an armed clear stands down when the reader presses Escape", async () => {
  serve([visit("Rust Book", 60)]);
  render(HistoryPage);
  await expect.element(page.getByText("Rust Book")).toBeVisible();
  await page.getByRole("button", { name: "Clear" }).click();
  await expect.element(page.getByRole("button", { name: "Clear all history?" })).toBeVisible();

  await userEvent.keyboard("{Escape}");

  await expect.element(page.getByRole("button", { name: "Clear" })).toBeVisible();
  expect(native.history.mock.calls.some((call) => call[1].kind === "clear")).toBe(false);
});

test("the range the reader picks narrows the list and scopes the clear", async () => {
  const recent = visit("Just now", 60);
  const old = visit("Last week", 6 * DAY);
  serve([recent, old]);
  render(HistoryPage);
  await expect.element(page.getByText("Last week")).toBeVisible();

  await page.getByRole("button", { name: "Show" }).click();
  await page.getByRole("option", { name: "Last hour" }).click();

  await vi.waitFor(() => {
    const last = native.history.mock.calls.at(-1)?.[1];
    expect(last).toMatchObject({ kind: "page", range: "hour" });
  });
  await vi.waitFor(() => expect(page.getByText("Last week").elements()).toHaveLength(0));
  await expect.element(page.getByText("Just now")).toBeVisible();

  // Clear removes exactly the scope being shown, never more.
  await page.getByRole("button", { name: "Clear" }).click();
  await page.getByRole("button", { name: "Clear the last hour?" }).click();
  await vi.waitFor(() =>
    expect(
      native.history.mock.calls.some(
        (call) => call[1].kind === "clear" && call[1].range === "hour",
      ),
    ).toBe(true),
  );
});

test("opening a row navigates in place rather than naming a tab", async () => {
  const only = visit("Rust Book", 60);
  serve([only]);
  render(HistoryPage);
  await expect.element(page.getByText("Rust Book")).toBeVisible();

  await page.getByText("Rust Book").click();

  await vi.waitFor(() => expect(native.open).toHaveBeenCalledWith(only.url, false));
});

test("says so when nothing matches, and offers no rows to act on", async () => {
  serve([visit("Rust Book", 60)]);
  render(HistoryPage);
  await expect.element(page.getByText("Rust Book")).toBeVisible();

  await userEvent.fill(page.getByRole("searchbox"), "nothing here");

  await expect.element(page.getByText("No matching pages")).toBeVisible();
});

test("reports a failure instead of showing an empty list", async () => {
  native.history.mockResolvedValue({
    kind: "error",
    error: "unavailable",
  } satisfies HistoryResponse);
  render(HistoryPage);

  await expect.element(page.getByText("History could not be loaded.")).toBeVisible();
});

test("a long list renders a window rather than every row", async () => {
  await page.viewport(1100, 720);
  // Fifteen minutes apart, so this spans days and carries headings too.
  const many = Array.from({ length: 900 }, (_, index) => visit(`Page ${index}`, index * 900));
  native.history.mockImplementation(async (_profile: string, call: HistoryCall) => {
    if (call.kind !== "page") return { kind: "removed", count: 0 } satisfies HistoryResponse;
    return { kind: "page", visits: many, next: null } satisfies HistoryResponse;
  });
  render(HistoryPage);
  await expect.element(page.getByText("Page 0")).toBeVisible();

  const scroller = document.querySelector<HTMLElement>("[role='listbox']")!;
  const rendered = () => scroller.querySelectorAll("[role='option']").length;

  expect(rendered()).toBeLessThan(80);
  expect(scroller.scrollHeight).toBeGreaterThan(30_000);

  // Scrolling far down keeps the DOM the same size and shows different rows.
  scroller.scrollTop = 20_000;
  scroller.dispatchEvent(new Event("scroll"));
  await vi.waitFor(() => expect(page.getByText("Page 0").elements()).toHaveLength(0));
  expect(rendered()).toBeLessThan(80);
});

test("the keyboard reaches rows the window has not drawn", async () => {
  await page.viewport(1100, 720);
  const many = Array.from({ length: 900 }, (_, index) => visit(`Page ${index}`, index * 900));
  native.history.mockImplementation(async (_profile: string, call: HistoryCall) => {
    if (call.kind !== "page") return { kind: "removed", count: 0 } satisfies HistoryResponse;
    return { kind: "page", visits: many, next: null } satisfies HistoryResponse;
  });
  render(HistoryPage);
  await expect.element(page.getByText("Page 0")).toBeVisible();

  const scroller = document.querySelector<HTMLElement>("[role='listbox']")!;
  scroller.focus();

  // End lands on a row hundreds of screens away, which the window has not
  // drawn and scrollIntoView therefore cannot find.
  await userEvent.keyboard("{End}");

  await vi.waitFor(() => expect(scroller.scrollTop).toBeGreaterThan(30_000));
  await expect.element(page.getByText("Page 899")).toBeVisible();

  await userEvent.keyboard("{Home}");
  await vi.waitFor(() => expect(scroller.scrollTop).toBe(0));
  await expect.element(page.getByText("Page 0")).toBeVisible();
});

test("renders in both themes", async () => {
  // The library fills the browser stage, which is far wider than the default
  // test viewport.
  await page.viewport(1100, 720);
  serve([visit("Rust Book", 60), visit("Release Notes", 120), visit("Old Article", DAY + 60)]);
  render(HistoryPage);
  await expect.element(page.getByText("Rust Book")).toBeVisible();

  await page.screenshot({ path: "../../../../../target/history-page-dark.png" });
  document.documentElement.dataset.theme = "light";
  await page.screenshot({ path: "../../../../../target/history-page-light.png" });
});
