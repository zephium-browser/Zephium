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

vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    pagePermissionCardHost: async () => {},
    tabsBootstrap: async () => {},
    sidebarSetHidden: async () => {},
    sidebarPeek: async () => {},
    sidebarSetWidth: async () => {},
    sidebarResize: async () => true,
    sidebarResizeGuide: async () => true,
    settingSet: async () => ({ accepted: true, operation_id: null }),
  });
});
afterEach(() => {
  sidebar.adoptMode("default");
  surface.dispose();
  tabs.dispose();
});
const frame = () => new Promise((r) => requestAnimationFrame(() => r(null)));

test("going to the rail and back carries the kept sites both ways", async () => {
  await page.viewport(268, 700);
  launch.dispose();
  await surface.init();
  await tabs.init();
  const all = ["a", "b", "k1", "k2", "k3"].map((id) => tabFixture({ id, title: `Tab ${id}` }));
  const nodes = all.map((tab) => ({
    id: tab.id,
    parent_id: null,
    section: tab.id.startsWith("k") ? "favorites" : "today",
    kind: { type: "tab", tab_id: tab.id },
  })) as SidebarNodeView[];
  emitNativeEvent("itemsChanged", {
    projection_revision: revision(Date.now()),
    profile: null,
    spaces: [],
    active_space_id: null,
    nodes,
    tabs: all,
    active: "a",
    split_group: null,
  });
  flushSync();
  render(Shell);
  await new Promise((resolve) => setTimeout(resolve, 400));
  const kept = () =>
    document
      .querySelector(".sidebar-columns > .sidebar-browser-column:not([aria-hidden])")!
      .querySelector<HTMLElement>('[data-motion-key="tab:k3"]')!;

  // The rail is the shape Work, Tasks and the tools give the column.
  sidebar.setMode("compact");
  await frame();
  await frame();
  expect(kept().getAnimations().length).toBeGreaterThan(0);
  // Native narrows the view after the change; the flights keep going.
  await page.viewport(64, 700);
  await new Promise((resolve) => setTimeout(resolve, 600));

  sidebar.setMode("default");
  await frame();
  await frame();
  expect(kept().getAnimations().length).toBeGreaterThan(0);
  await page.viewport(268, 700);
  await frame();
  await frame();
  expect(kept().getAnimations()[0]?.playState).toBe("running");
});
