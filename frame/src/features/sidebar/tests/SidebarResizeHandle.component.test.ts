import { expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import * as sidebar from "$session/sidebar-mode.svelte";
import SidebarResizeHandle from "../components/SidebarResizeHandle.svelte";

const native = vi.hoisted(() => ({
  width: vi.fn(async (_width: number | null, _travel: boolean, _revision: number | null) => {}),
  resize: vi.fn(
    async (_width: number | null, enabled: boolean, _revision: number | null) => enabled,
  ),
}));

vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    sidebarResize: native.resize,
    sidebarResizeGuide: async () => true,
    sidebarSetHidden: async () => {},
    sidebarPeek: async () => {},
    sidebarSetWidth: native.width,
    settingSet: async () => ({ accepted: true, operation_id: null }),
    settingGet: async () => null,
  });
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

// Without a native capture, as on Windows, the page's own handle takes the drag.
function domDrag(handle: HTMLElement) {
  handle.setPointerCapture = () => {};
  handle.hasPointerCapture = () => false;
  const pointer = (type: string, clientX: number) =>
    handle.dispatchEvent(
      new PointerEvent(type, { bubbles: true, button: 0, pointerId: 1, clientX }),
    );
  return pointer;
}

test("without a native capture the column follows the pointer and settles on release", async () => {
  native.resize.mockImplementation(async () => false);
  const screen = await render(SidebarResizeHandle, { width: sidebar.expanded() });
  await expect.poll(() => native.resize.mock.calls.length).toBeGreaterThan(0);
  const start = sidebar.expanded();
  const pointer = domDrag(screen.getByRole("separator").element() as HTMLElement);
  pointer("pointerdown", 500);
  pointer("pointermove", 540);
  expect(sidebar.expanded()).toBe(start + 40);
  expect(native.width.mock.calls.at(-1)?.slice(0, 2)).toEqual([start + 40, false]);
  pointer("pointermove", 0);
  expect(sidebar.expanded()).toBe(sidebar.MIN_EXPANDED_WIDTH);
  pointer("pointermove", 540);
  pointer("pointerup", 540);
  expect(sidebar.sidebarResizeActive()).toBe(false);
  expect(sidebar.expanded()).toBe(start + 40);
  expect(native.width.mock.calls.at(-1)?.slice(0, 2)).toEqual([start + 40, false]);
  screen.unmount();
  native.resize.mockImplementation(async (_width, enabled) => enabled);
});

test("cancelling a drag without a native capture gives back the starting width", async () => {
  native.resize.mockImplementation(async () => false);
  const screen = await render(SidebarResizeHandle, { width: sidebar.expanded() });
  await expect.poll(() => native.resize.mock.calls.length).toBeGreaterThan(0);
  const start = sidebar.expanded();
  const handle = screen.getByRole("separator").element() as HTMLElement;
  const pointer = domDrag(handle);
  pointer("pointerdown", 500);
  pointer("pointermove", 460);
  expect(sidebar.expanded()).toBe(start - 40);
  handle.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
  expect(sidebar.expanded()).toBe(start);
  screen.unmount();
  native.resize.mockImplementation(async (_width, enabled) => enabled);
});
