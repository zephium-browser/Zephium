import { afterEach, expect, test, vi } from "vitest";
import { cleanup, render } from "vitest-browser-svelte";
import * as sidebar from "$session/sidebar-mode.svelte";
import SidebarResizeHandle from "../components/SidebarResizeHandle.svelte";

const native = vi.hoisted(() => ({
  resize: vi.fn(
    async (_width: number | null, enabled: boolean, _revision: number | null) => enabled,
  ),
  guide: vi.fn(async (_width: number | null) => false),
}));

vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    sidebarResize: native.resize,
    sidebarResizeGuide: native.guide,
    sidebarSetHidden: async () => {},
    sidebarPeek: async () => {},
    sidebarSetWidth: async () => {},
    settingSet: async () => ({ accepted: true, operation_id: null }),
  });
});

afterEach(async () => {
  await cleanup();
  sidebar.setPanelExtent(0);
  sidebar.setMode("default");
  vi.clearAllMocks();
  vi.restoreAllMocks();
});

test("leaving for a tool switches the native guide off at the newest revision", async () => {
  const screen = await render(SidebarResizeHandle, { width: 240 });
  await expect.poll(() => native.resize.mock.calls.length).toBeGreaterThan(0);
  // A tool opening advances the revision in the same turn the handle leaves.
  sidebar.setPanelExtent(336);
  screen.unmount();
  await expect.poll(() => native.resize.mock.calls.at(-1)?.[1]).toBe(false);
  expect(native.resize.mock.calls.at(-1)?.[2]).toBe(sidebar.sidebarResizeRevision());
  sidebar.setPanelExtent(0);
});

test("the DOM resize guide hides below the threshold without replacing the expanded width", async () => {
  native.resize.mockResolvedValueOnce(false);
  const width = sidebar.expanded();
  const screen = await render(SidebarResizeHandle, { width });
  await expect.poll(() => native.resize.mock.calls.length).toBeGreaterThan(0);
  const handle = screen.getByRole("separator", { name: "Resize sidebar" }).element();
  vi.spyOn(handle, "setPointerCapture").mockImplementation(() => {});
  const pointer = { bubbles: true, pointerId: 1, button: 0 };
  handle.dispatchEvent(new PointerEvent("pointerdown", { ...pointer, clientX: width }));
  handle.dispatchEvent(
    new PointerEvent("pointermove", { ...pointer, clientX: sidebar.SNAP_THRESHOLD - 1 }),
  );
  await expect.poll(() => native.guide.mock.calls.at(-1)).toEqual([sidebar.COMPACT_WIDTH]);
  expect(sidebar.sidebarMode()).toBe("default");
  expect(sidebar.expanded()).toBe(width);

  handle.dispatchEvent(
    new PointerEvent("pointerup", { ...pointer, clientX: sidebar.SNAP_THRESHOLD - 1 }),
  );
  expect(sidebar.sidebarMode()).toBe("hidden");
  expect(sidebar.expanded()).toBe(width);
  expect(native.guide.mock.calls.at(-1)).toEqual([null]);
});
