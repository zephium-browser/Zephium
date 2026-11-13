import { beforeEach, expect, test, vi } from "vitest";

// What the page asked for, in order: leaving the manager, then the options tab.
const ui = vi.hoisted(() => ({
  page: null as string | null,
  calls: [] as string[],
}));

vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    webExtensionOpenOptions: (id: string) => {
      ui.calls.push(`options:${id}`);
      return Promise.resolve({ status: "ok", data: null });
    },
  });
});

vi.mock("$shared/ipc/native-events", () => ({
  events: { webExtensionAccessRequested: { listen: vi.fn() } },
}));

vi.mock("$domain/surface", () => ({
  surface: {
    currentPage: () => ui.page,
    open: (next: string | null) => {
      ui.calls.push(`open:${next}`);
      ui.page = next;
      return Promise.resolve();
    },
  },
}));

beforeEach(() => {
  vi.resetModules();
  ui.calls = [];
  ui.page = null;
});

test("leaves the manager before asking for an options tab", async () => {
  const { openOptions } = await import("../webext.svelte");
  ui.page = "extensions";
  await openOptions("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
  expect(ui.calls).toEqual([
    "open:null",
    "options:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
  ]);
});

test("asks directly when a page already owns the window", async () => {
  const { openOptions } = await import("../webext.svelte");
  await openOptions("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb");
  expect(ui.calls).toEqual(["options:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"]);
});
