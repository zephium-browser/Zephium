import { expect, test, vi } from "vitest";

const native = vi.hoisted(() => ({ export: vi.fn() }));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({ exportBrowserData: native.export as never });
});

import { exportBrowserData } from "..";

test("asks native for the kind and returns what it answered", async () => {
  native.export.mockResolvedValueOnce("saved");
  expect(await exportBrowserData("history")).toBe("saved");
  expect(native.export).toHaveBeenCalledExactlyOnceWith("history");
});

test("a call that throws reads as a failed export", async () => {
  native.export.mockRejectedValueOnce(new Error("ipc"));
  expect(await exportBrowserData("bookmarks")).toBe("failed");
});
