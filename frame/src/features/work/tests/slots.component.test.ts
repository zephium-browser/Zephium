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

/** A view whose first import fails on its own stylesheet, as Vite's preload does. */
function failingOnce(href: string): {
  loader: () => Promise<{ default: typeof view }>;
  sheet: () => HTMLLinkElement | undefined;
} {
  let calls = 0;
  let sheet: HTMLLinkElement | undefined;
  return {
    loader: async () => {
      calls++;
      if (calls === 1) {
        sheet = failedSheet(href);
        throw new Error(`Unable to preload CSS for ${href}`);
      }
      return { default: view };
    },
    sheet: () => sheet,
  };
}

function rejections(): { reported: unknown[]; stop: () => void } {
  const reported: unknown[] = [];
  const onRejection = (event: PromiseRejectionEvent) => {
    reported.push(event.reason);
    event.preventDefault();
  };
  window.addEventListener("unhandledrejection", onRejection);
  return { reported, stop: () => window.removeEventListener("unhandledrejection", onRejection) };
}

it("a retried view waits for the stylesheet that failed to load", async () => {
  const { loader, sheet } = failingOnce("data:text/css,.slot-sheet{}");
  loadView(loader);
  await vi.waitFor(() => expect(loadedView(loader)).toBe(view), { timeout: 3000 });
  expect(sheet()?.isConnected).toBe(false);
  const again = [...document.head.querySelectorAll("link[data-test-sheet]")];
  expect(again).toHaveLength(1);
  expect((again[0] as HTMLLinkElement).sheet).not.toBeNull();
});

it("a retried view is not kept while its stylesheet still fails", async () => {
  const { reported, stop } = rejections();
  try {
    const { loader } = failingOnce("/__missing__/slot-sheet.css");
    loadView(loader);
    await vi.waitFor(() => expect(reported).toHaveLength(1), { timeout: 5000 });
    expect(String(reported[0])).toContain("slot-sheet.css");
    expect(loadedView(loader)).toBeNull();
  } finally {
    stop();
  }
});

it("another view's failing stylesheet does not hold a view back", async () => {
  const { reported, stop } = rejections();
  try {
    const broken = failingOnce("/__missing__/other-sheet.css");
    loadView(broken.loader);
    const fine = failingOnce("data:text/css,.fine-sheet{}");
    loadView(fine.loader);
    await vi.waitFor(() => expect(loadedView(fine.loader)).toBe(view), { timeout: 3000 });
    await vi.waitFor(() => expect(reported).toHaveLength(1), { timeout: 5000 });
    expect(String(reported[0])).toContain("other-sheet.css");
    expect(loadedView(broken.loader)).toBeNull();
  } finally {
    stop();
  }
});
