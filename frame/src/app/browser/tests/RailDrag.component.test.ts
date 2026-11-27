import { afterEach, expect, test, vi } from "vitest";
import { page } from "vitest/browser";
import { render } from "vitest-browser-svelte";
import { flushSync } from "svelte";
import "$styles/global.css";
import type { SidebarNodeView } from "$shared/ipc/bindings";
import { emitNativeEvent } from "$shared/testing/native-events";
import { revision, tabFixture } from "$shared/testing/fixtures";
import { surface } from "$domain/surface";
import { tabs } from "$domain/tabs";
import * as launch from "$session/motion.svelte";
import * as sidebar from "$session/sidebar-mode.svelte";
import Shell from "../Shell.svelte";

const native = vi.hoisted(() => ({
  setEssential: vi.fn(async () => ({ accepted: true, operation_id: null })),
}));

vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    pagePermissionCardHost: async () => {},
    tabsBootstrap: async () => {},
    sidebarSetHidden: async () => {},
    sidebarPeek: async () => {},
    sidebarSetWidth: async () => {},
    sidebarResize: async () => false,
    sidebarResizeGuide: async () => true,
    settingSet: async () => ({ accepted: true, operation_id: null }),
    tabsSetEssential: native.setEssential,
    tabDragOver: async () => undefined,
    tabDrop: async () => ({ accepted: true, operation_id: null }),
  });
});

vi.mock("$domain/operations", async (original) => ({
  ...(await original<Record<string, unknown>>()),
  settle: async () => ({ outcome: "succeeded" }),
}));

afterEach(() => {
  sidebar.setMode("default");
  surface.dispose();
  tabs.dispose();
  native.setEssential.mockClear();
});

async function rail() {
  await page.viewport(900, 700);
  launch.dispose();
  await surface.init();
  await tabs.init();
  const kept = [tabFixture({ id: "k", title: "Kept" })];
  const open = ["a", "b", "c"].map((id) => tabFixture({ id, title: `Tab ${id}` }));
  const node = (id: string, section: "favorites" | "today"): SidebarNodeView => ({
    id,
    parent_id: null,
    section,
    kind: { type: "tab", tab_id: id },
  });
  emitNativeEvent("itemsChanged", {
    projection_revision: revision(Date.now()),
    profile: null,
    spaces: [],
    active_space_id: null,
    nodes: [node("k", "favorites"), ...open.map((tab) => node(tab.id, "today"))],
    tabs: [...kept, ...open],
    active: "a",
    split_group: null,
  });
  // The rail a Mac now keeps for Work and the tools, set directly.
  sidebar.setMode("compact");
  flushSync();
  await render(Shell);
  const item = (id: string) =>
    document.querySelector<HTMLElement>(`.rail-item[aria-label="${id}"]`) ??
    document.querySelector<HTMLElement>(`[data-motion-key="tab:${id}"] button`)!;
  return { item };
}

const frame = () => new Promise((resolve) => requestAnimationFrame(() => resolve(undefined)));

function press(handle: HTMLElement, to: { x: number; y: number }) {
  const from = handle.getBoundingClientRect();
  const start = { x: from.left + from.width / 2, y: from.top + from.height / 2 };
  const init = { bubbles: true, pointerId: 1, button: 0, isPrimary: true };
  handle.dispatchEvent(
    new PointerEvent("pointerdown", { ...init, clientX: start.x, clientY: start.y }),
  );
  handle.dispatchEvent(
    new PointerEvent("pointermove", { ...init, clientX: start.x, clientY: start.y + 8 }),
  );
  handle.dispatchEvent(new PointerEvent("pointermove", { ...init, clientX: to.x, clientY: to.y }));
  return () =>
    handle.dispatchEvent(new PointerEvent("pointerup", { ...init, clientX: to.x, clientY: to.y }));
}

test("the rail's tabs reorder in place", async () => {
  const { item } = await rail();
  const target = item("c").getBoundingClientRect();
  const release = press(item("a"), {
    x: target.left + target.width / 2,
    y: target.bottom - 4,
  });
  await frame();
  release();
  await expect.poll(() => native.setEssential.mock.calls.length).toBe(1);
  expect(native.setEssential).toHaveBeenCalledWith("a", false, null);
});

test("a rail tab dropped on the kept sites is kept", async () => {
  const { item } = await rail();
  const kept = item("k").getBoundingClientRect();
  const release = press(item("b"), { x: kept.left + kept.width / 2, y: kept.top + 6 });
  await frame();
  release();
  await expect.poll(() => native.setEssential.mock.calls.length).toBe(1);
  expect(native.setEssential).toHaveBeenCalledWith("b", true, "k");
});
