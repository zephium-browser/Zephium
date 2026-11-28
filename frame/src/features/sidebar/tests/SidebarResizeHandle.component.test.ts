import { afterEach, expect, test, vi } from "vitest";
import { cleanup, render } from "vitest-browser-svelte";
import * as sidebar from "$session/sidebar-mode.svelte";
import { IS_MAC } from "$shared/platform";
import SidebarResizeHandle from "../components/SidebarResizeHandle.svelte";

const native = vi.hoisted(() => ({
  resize: vi.fn(
    async (_width: number | null, enabled: boolean, _revision: number | null) => enabled,
  ),
  guide: vi.fn(async (_width: number | null) => false),
  width: vi.fn(async (_width: number | null, _travel: boolean, _revision: number | null) => {}),
  set: vi.fn(async (_key: string, _value: string) => ({ accepted: true, operation_id: null })),
}));

vi.mock("$shared/platform", () => ({
  IS_MAC: import.meta.env.VITE_SIDEBAR_WINDOWS !== "true",
}));

vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    sidebarResize: native.resize,
    sidebarResizeGuide: native.guide,
    sidebarSetHidden: async () => {},
    sidebarPeek: async () => {},
    sidebarSetWidth: native.width,
    settingSet: native.set,
    settingGet: async () => null,
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

async function domDrag() {
  native.resize.mockResolvedValueOnce(false);
  const width = sidebar.expanded();
  const screen = await render(SidebarResizeHandle, { width });
  await expect.poll(() => native.resize.mock.calls.length).toBeGreaterThan(0);
  const handle = screen.getByRole("separator", { name: "Resize sidebar" }).element();
  vi.spyOn(handle, "setPointerCapture").mockImplementation(() => {});
  const pointer = { bubbles: true, pointerId: 1, button: 0 };
  const send = (type: string, clientX: number) =>
    handle.dispatchEvent(new PointerEvent(type, { ...pointer, clientX }));
  send("pointerdown", width);
  return { handle, send, width };
}

test("the DOM resize guide hides below the threshold without replacing the expanded width", async () => {
  const { send, width } = await domDrag();
  send("pointermove", sidebar.SNAP_THRESHOLD - 1);
  await expect.poll(() => native.guide.mock.calls.at(-1)).toEqual([sidebar.COMPACT_WIDTH]);
  expect(sidebar.sidebarMode()).toBe("default");
  expect(sidebar.expanded()).toBe(IS_MAC ? sidebar.MIN_EXPANDED_WIDTH : width);
  expect(native.set).not.toHaveBeenCalled();

  send("pointerup", sidebar.SNAP_THRESHOLD - 1);
  expect(sidebar.sidebarMode()).toBe("hidden");
  expect(sidebar.expanded()).toBe(width);
  expect(native.guide.mock.calls.at(-1)).toEqual([null]);
});

test("the expanded fallback moves live on macOS and waits for release on Windows", async () => {
  const { send, width } = await domDrag();
  const next = width + 20;
  native.width.mockClear();
  send("pointermove", next);
  expect(sidebar.expanded()).toBe(IS_MAC ? next : width);
  expect(native.width.mock.calls).toEqual(IS_MAC ? [[next, false, expect.any(Number)]] : []);
  expect(native.set).not.toHaveBeenCalled();
  if (IS_MAC) window.dispatchEvent(new Event("resize"));
  send("pointerup", next);
  expect(sidebar.expanded()).toBe(next);
  expect(native.width).toHaveBeenLastCalledWith(next, !IS_MAC, expect.any(Number));
  expect(native.set).toHaveBeenCalledExactlyOnceWith("sidebar.width", String(next));
  sidebar.applyDragWidth(width);
});

test.skipIf(!IS_MAC).each(["Escape", "blur"])(
  "%s restores the starting width of an expanded drag without saving",
  async (reason) => {
    const { handle, send, width } = await domDrag();
    send("pointermove", width + 40);
    expect(sidebar.expanded()).toBe(width + 40);
    if (reason === "blur") window.dispatchEvent(new Event("blur"));
    else handle.dispatchEvent(new KeyboardEvent("keydown", { key: reason, bubbles: true }));
    send("pointerup", width + 40);
    expect(sidebar.sidebarResizeActive()).toBe(false);
    expect(sidebar.expanded()).toBe(width);
    expect(native.width).toHaveBeenLastCalledWith(width, false, expect.any(Number));
    expect(native.set).not.toHaveBeenCalled();
  },
);
