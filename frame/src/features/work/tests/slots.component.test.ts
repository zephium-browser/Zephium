import { afterEach, expect, it, vi } from "vitest";
import type { Component } from "svelte";
import { loadView, loadedView } from "../components/run/slots";

const view = (() => null) as unknown as Component<Record<string, unknown>>;

afterEach(() => {
  for (const link of document.head.querySelectorAll("link[data-test-sheet]")) link.remove();
});

function failedSheet(href: string): HTMLLinkElement {
  const link = document.createElement("link");
  link.rel = "stylesheet";
  link.href = href;
  link.dataset.testSheet = "";
  document.head.append(link);
  link.dispatchEvent(new Event("error"));
  return link;
}

function failingOnce(): () => Promise<{ default: typeof view }> {
  let calls = 0;
  return async () => {
    calls++;
    if (calls === 1) throw new Error("offline");
    return { default: view };
  };
}

it("a retried view waits for the stylesheet that failed to load", async () => {
  const loader = failingOnce();
  loadView(loader);
  await Promise.resolve();
  const failed = failedSheet("data:text/css,.slot-sheet{}");
  await vi.waitFor(() => expect(loadedView(loader)).toBe(view), { timeout: 3000 });
  expect(failed.isConnected).toBe(false);
  const again = [...document.head.querySelectorAll("link[data-test-sheet]")];
  expect(again).toHaveLength(1);
  expect((again[0] as HTMLLinkElement).sheet).not.toBeNull();
});

it("a retried view is not kept while its stylesheet still fails", async () => {
  const reported: unknown[] = [];
  const onRejection = (event: PromiseRejectionEvent) => {
    reported.push(event.reason);
    event.preventDefault();
  };
  window.addEventListener("unhandledrejection", onRejection);
  try {
    const loader = failingOnce();
    loadView(loader);
    await Promise.resolve();
    failedSheet("/__missing__/slot-sheet.css");
    await vi.waitFor(() => expect(reported).toHaveLength(1), { timeout: 5000 });
    expect(String(reported[0])).toContain("slot-sheet.css");
    expect(loadedView(loader)).toBeNull();
  } finally {
    window.removeEventListener("unhandledrejection", onRejection);
  }
});
