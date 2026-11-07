import { afterEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { page } from "vitest/browser";
import "$styles/global.css";
import type {
  ClosedTabView,
  HistoryCall,
  HistoryResponse,
  HistoryVisitView,
} from "$shared/ipc/bindings";
import HistoryPanel from "../components/HistoryPanel.svelte";

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

afterEach(() => {
  vi.clearAllMocks();
  delete document.documentElement.dataset.theme;
});

function visit(title: string, agoSeconds: number): HistoryVisitView {
  const at = Math.floor(Date.now() / 1000) - agoSeconds;
  return {
    id: String(at * 1000 + Math.floor(Math.random() * 1000)),
    url: `https://example.com/${encodeURIComponent(title)}`,
    title,
    visited_at: String(at),
    icon: null,
  };
}

function closedTab(title: string): ClosedTabView {
  return { id: `closed-${title}`, url: `https://closed.example/${title}`, title, icon: null };
}

function serve(all: HistoryVisitView[], closed: ClosedTabView[] = []) {
  native.history.mockImplementation(async (_profile: string, call: HistoryCall) => {
    if (call.kind === "closed_tabs") return { kind: "closed_tabs", tabs: closed };
    if (call.kind === "reopen_closed") return { kind: "reopened" } satisfies HistoryResponse;
    if (call.kind !== "page") return { kind: "removed", count: 0 } satisfies HistoryResponse;
    const needle = call.query.toLowerCase();
    const visits = all.filter((entry) => !needle || entry.title.toLowerCase().includes(needle));
    return { kind: "page", visits, next: null } satisfies HistoryResponse;
  });
}

test("lists recent pages in the sidebar", async () => {
  serve([visit("Rust Book", 60), visit("Release Notes", 120)]);

  render(HistoryPanel, { profile: "profile", query: "" });

  await expect.element(page.getByText("Rust Book")).toBeVisible();
  await expect.element(page.getByText("Today")).toBeVisible();
});

test("highlights the original title after a lowercase expansion", async () => {
  serve([visit("İstanbul News", 60)]);
  const screen = await render(HistoryPanel, { profile: "unicode-highlight", query: "news" });
  await expect.poll(() => screen.container.querySelector("mark")?.textContent).toBe("News");
});

test("takes its query from the frame that owns the field", async () => {
  serve([visit("Rust Book", 60), visit("Release Notes", 120)]);

  // ToolFrame owns the search input and persists its value per tool; the panel
  // only mirrors it into the session.
  render(HistoryPanel, { profile: "profile", query: "rust" });

  await expect.element(page.getByText("Rust Book")).toBeVisible();
  // The first request already carries the query, so the unfiltered list never
  // appears and then vanishes.
  expect(native.history.mock.calls[0]?.[1]).toMatchObject({ query: "rust" });
  expect(page.getByText("Release Notes").elements()).toHaveLength(0);
});

test("opens a page in the window the panel belongs to, not a tab it names", async () => {
  const only = visit("Rust Book", 60);
  serve([only]);
  render(HistoryPanel, { profile: "profile", query: "" });
  await expect.element(page.getByText("Rust Book")).toBeVisible();

  await page.getByText("Rust Book").click();

  await vi.waitFor(() => expect(native.open).toHaveBeenCalledWith(only.url, false));
});

test("says so when a profile has nothing recorded", async () => {
  serve([]);

  render(HistoryPanel, { profile: "empty", query: "" });

  await expect.element(page.getByText("Your browsing history belongs here")).toBeVisible();
});

test("lists the tabs closed lately above the history and reopens one by name", async () => {
  serve([visit("Rust Book", 60)], [closedTab("Docs"), closedTab("Mail")]);

  render(HistoryPanel, { profile: "profile", query: "" });

  await expect.element(page.getByText("Recently closed")).toBeVisible();
  await expect.element(page.getByText("Rust Book")).toBeVisible();
  await page.getByRole("button", { name: "Mail" }).click();

  await vi.waitFor(() =>
    expect(native.history).toHaveBeenCalledWith("profile", {
      kind: "reopen_closed",
      id: "closed-Mail",
    }),
  );
  expect(native.open).not.toHaveBeenCalled();
});

test("keeps the closed tabs out of the way of a search and of an empty list", async () => {
  serve([visit("Rust Book", 60)], [closedTab("Docs")]);
  render(HistoryPanel, { profile: "profile", query: "rust" });
  await expect.element(page.getByText("Rust Book")).toBeVisible();
  expect(page.getByText("Recently closed").elements()).toHaveLength(0);
  expect(native.history.mock.calls.some(([, call]) => call.kind === "closed_tabs")).toBe(false);
});

test("shows a few closed tabs and the rest on request", async () => {
  const many = ["A", "B", "C", "D", "E", "F", "G"].map(closedTab);
  serve([], many);

  render(HistoryPanel, { profile: "profile", query: "" });

  await expect.element(page.getByRole("button", { name: "E", exact: true })).toBeVisible();
  expect(page.getByRole("button", { name: "F", exact: true }).elements()).toHaveLength(0);
  await page.getByRole("button", { name: "Show more" }).click();
  await expect.element(page.getByRole("button", { name: "G", exact: true })).toBeVisible();
  await page.getByRole("button", { name: "Show fewer" }).click();
  expect(page.getByRole("button", { name: "G", exact: true }).elements()).toHaveLength(0);
});
