import { afterEach, expect, test, vi } from "vitest";
import { page, userEvent } from "vitest/browser";
import { render } from "vitest-browser-svelte";
import type { HistoryCall, HistoryResponse, HistoryVisitView } from "$shared/ipc/bindings";
import primitives from "../../../styles/tokens/primitive.css?raw";
import tokens from "../../../styles/tokens.css?raw";
import browserCSS from "../../../styles/browser.css?raw";
import HistoryHost from "./HistoryHost.svelte";

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

const style = document.createElement("style");
style.textContent = primitives + tokens.replace("@theme static", ":root") + browserCSS;
afterEach(() => {
  style.remove();
  vi.clearAllMocks();
});

function visits(count: number): HistoryVisitView[] {
  const now = Math.floor(Date.now() / 1000);
  return Array.from({ length: count }, (_, index) => ({
    id: String(1_000_000 - index),
    url: `https://example-${index}.com/page`,
    title: `Visited page number ${index}`,
    visited_at: String(now - index * 900),
    icon: null,
  }));
}

test("history fills the sidebar tool host and scrolls inside it", async () => {
  await page.viewport(1000, 800);
  document.head.append(style);
  const all = visits(60);
  native.history.mockImplementation(async (_profile: string, call: HistoryCall) => {
    if (call.kind !== "page") return { kind: "removed", count: 0 } satisfies HistoryResponse;
    return { kind: "page", visits: all, next: null } satisfies HistoryResponse;
  });

  const screen = await render(HistoryHost, { profile: "profile" });
  await expect.element(page.getByText("Visited page number 0")).toBeVisible();

  const frame = screen.container.querySelector<HTMLElement>(".shared-tool")!;
  const content = screen.container.querySelector<HTMLElement>(".shared-tool-content")!;
  const scroller = screen.container.querySelector<HTMLElement>("[role='listbox']")!;

  // The frame must hand its height to the list rather than nesting a second
  // scroller around it, which collapsed the list to its content height.
  expect(content.scrollHeight).toBe(content.clientHeight);
  expect(scroller.clientHeight).toBeGreaterThan(200);
  expect(scroller.scrollHeight).toBeGreaterThan(scroller.clientHeight);

  const bounds = frame.getBoundingClientRect();
  const box = scroller.getBoundingClientRect();
  expect(box.left).toBeGreaterThanOrEqual(bounds.left - 1);
  expect(box.right).toBeLessThanOrEqual(bounds.right + 1);
  expect(box.bottom).toBeLessThanOrEqual(bounds.bottom + 1);

  // Rows stay inside the panel rather than spilling past its edge.
  for (const row of scroller.querySelectorAll<HTMLElement>("[role='option']")) {
    const rect = row.getBoundingClientRect();
    if (!row.getClientRects().length) continue;
    expect(rect.right).toBeLessThanOrEqual(box.right + 1);
    expect(rect.left).toBeGreaterThanOrEqual(box.left - 1);
  }
});

test("the history search parks an idle caret and takes the next key", async () => {
  vi.spyOn(document, "hasFocus").mockReturnValue(true);
  document.head.append(style);
  native.history.mockImplementation(async () => ({ kind: "page", visits: visits(3), next: null }));
  const screen = await render(HistoryHost, { profile: "profile" });
  await expect
    .poll(() => screen.container.querySelector(".shared-tool-search input"))
    .not.toBeNull();
  const search = screen.container.querySelector<HTMLInputElement>(".shared-tool-search input")!;
  await expect.poll(() => document.activeElement).toBe(search);
  await expect.poll(() => search.dataset.caretParked, { timeout: 6000 }).toBe("");
  expect(document.activeElement).not.toBe(search);
  await userEvent.keyboard("e");
  expect(document.activeElement).toBe(search);
  expect(search.value).toBe("e");
  vi.mocked(document.hasFocus).mockRestore();
});
