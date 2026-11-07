import { expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import * as sidebar from "$session/sidebar-mode.svelte";
import SidebarResizeHandle from "../components/SidebarResizeHandle.svelte";

const native = vi.hoisted(() => ({
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
    sidebarSetWidth: async () => {},
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
