import "$styles/global.css";
import { afterEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import * as sidebar from "$session/sidebar-mode.svelte";
import SidebarResizeHandle from "../components/SidebarResizeHandle.svelte";
import { preferences } from "$domain/preferences";
import { emitNativeEvent } from "$shared/testing/native-events";
import { flushSync } from "svelte";

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
    settingGet: async () => null,
  });
});

afterEach(() => {
  preferences.dispose();
  sidebar.cancelSidebarResize();
  sidebar.applyDragWidth(240);
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

test.each(["left", "right"] as const)(
  "the %s handle follows the physical arrow direction",
  async (position) => {
    await preferences.init();
    emitNativeEvent("uiCommand", `preference.sidebar.position=${position}`);
    sidebar.applyDragWidth(240);
    const screen = await render(SidebarResizeHandle, { width: 240 });
    const handle = screen.container.querySelector<HTMLElement>('[role="separator"]')!;
    handle.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowLeft", bubbles: true }));
    expect(sidebar.expanded()).toBe(position === "right" ? 248 : 232);
    handle.dispatchEvent(
      new KeyboardEvent("keydown", { key: "ArrowRight", shiftKey: true, bubbles: true }),
    );
    expect(sidebar.expanded()).toBe(position === "right" ? 216 : 264);
    handle.dispatchEvent(new KeyboardEvent("keydown", { key: "Home", bubbles: true }));
    expect(sidebar.expanded()).toBe(sidebar.MIN_EXPANDED_WIDTH);
    handle.dispatchEvent(new KeyboardEvent("keydown", { key: "End", bubbles: true }));
    expect(sidebar.expanded()).toBe(sidebar.MAX_EXPANDED_WIDTH);
  },
);

test.each(["left", "right"] as const)(
  "the %s fallback previews inward dragging without changing layout until release",
  async (position) => {
    native.resize.mockResolvedValue(false);
    await preferences.init();
    emitNativeEvent("uiCommand", `preference.sidebar.position=${position}`);
    sidebar.applyDragWidth(240);
    const screen = await render(SidebarResizeHandle, { width: 240 });
    const handle = screen.container.querySelector<HTMLElement>('[role="separator"]')!;
    vi.spyOn(handle, "setPointerCapture").mockImplementation(() => {});
    vi.spyOn(handle, "hasPointerCapture").mockReturnValue(false);
    const init = { bubbles: true, pointerId: 1, button: 0 };
    handle.dispatchEvent(new PointerEvent("pointerdown", { ...init, clientX: 300 }));
    handle.dispatchEvent(
      new PointerEvent("pointermove", { ...init, clientX: position === "right" ? 260 : 340 }),
    );
    flushSync();
    expect(sidebar.expanded()).toBe(240);
    const guide = screen.container.querySelector<HTMLElement>("span[style]")!;
    expect(position === "right" ? guide.style.right : guide.style.left).toBe("238px");
    handle.dispatchEvent(new PointerEvent("pointerup", init));
    expect(sidebar.expanded()).toBe(280);
    expect(sidebar.sidebarResizeActive()).toBe(false);
    native.resize.mockImplementation(async (_width, enabled) => enabled);
  },
);
