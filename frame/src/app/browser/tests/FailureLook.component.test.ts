import { afterEach, expect, test, vi } from "vitest";
import { page } from "vitest/browser";
import { render } from "vitest-browser-svelte";
import "$styles/global.css";
import { emitNativeEvent } from "$shared/testing/native-events";
import { revision, tabFixture } from "$shared/testing/fixtures";
import { favicons } from "$domain/favicons";
import { surface } from "$domain/surface";
import { tabs } from "$domain/tabs";
import * as sidebar from "$session/sidebar-mode.svelte";
import * as launch from "$session/motion.svelte";
import Shell from "../Shell.svelte";

vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    tabsBootstrap: async () => {},
    sidebarSetWidth: async () => {},
    sidebarResize: async () => false,
    sidebarResizeGuide: async () => true,
    settingSet: async () => ({ accepted: true, operation_id: null }),
  });
});

afterEach(() => {
  sidebar.adoptMode("default");
  favicons.dispose();
  surface.dispose();
  tabs.dispose();
});

const shot = (name: string) => page.screenshot({ path: `../../../../../target/look/${name}.png` });
const settle = () => new Promise((resolve) => setTimeout(resolve, 450));

test("a first load that failed explains itself beside the sidebar", async () => {
  await page.viewport(1100, 720);
  document.documentElement.dataset.theme = "dark";
  document.body.style.background = "#232326";
  launch.dispose();
  await surface.init();
  await tabs.init();
  await favicons.init();
  const open = tabFixture({ id: "open", title: "Example", url: "https://example.com/" });
  const failed = tabFixture({
    id: "failed",
    title: "New Tab",
    url: null,
    failure: { url: "http://localhost:3000/", reason: "unreachable" },
  });
  emitNativeEvent("itemsChanged", {
    projection_revision: revision(20),
    profile: null,
    spaces: [],
    active_space_id: null,
    nodes: [open, failed].map((entry) => ({
      id: entry.id,
      parent_id: null,
      section: "today" as const,
      kind: { type: "tab" as const, tab_id: entry.id },
    })),
    tabs: [open, failed],
    active: "failed",
    split_group: null,
  });
  const screen = await render(Shell);
  await settle();
  await shot("failure-first-load");
  // The card stands in the content pane; the sidebar stays usable beside it.
  const card = document.querySelector("[role=alert] h1")!.closest("[role=alert]")!;
  const sidebarEdge = document
    .querySelector("aside.browser-sidebar")!
    .getBoundingClientRect().right;
  expect(card.getBoundingClientRect().left).toBeGreaterThanOrEqual(sidebarEdge);
  screen.unmount();
  emitNativeEvent("tabChanged", { ...failed, projection_revision: revision(21), failure: null });
  await render(Shell);
  await settle();
  await shot("failure-new-tab");
});

test("a new tab opened from settings shows the new tab", async () => {
  await page.viewport(1100, 720);
  document.documentElement.dataset.theme = "dark";
  document.body.style.background = "#232326";
  launch.dispose();
  await surface.init();
  await tabs.init();
  await favicons.init();
  const site = tabFixture({ id: "site", title: "Example", url: "https://example.com/" });
  const items = (list: ReturnType<typeof tabFixture>[], active: string, at: number) => ({
    projection_revision: revision(at),
    profile: null,
    spaces: [],
    active_space_id: null,
    nodes: list.map((entry) => ({
      id: entry.id,
      parent_id: null,
      section: "today" as const,
      kind: { type: "tab" as const, tab_id: entry.id },
    })),
    tabs: list,
    active,
    split_group: null,
  });
  emitNativeEvent("itemsChanged", items([site], "site", 30));
  await render(Shell);
  emitNativeEvent("uiCommand", "browser.settings");
  await settle();
  emitNativeEvent("browserReturn", items([site], "site", 31));
  const fresh = tabFixture({ id: "fresh", title: "New Tab", url: null });
  emitNativeEvent("itemsChanged", items([site, fresh], "fresh", 32));
  emitNativeEvent("uiCommand", "browser.return");
  await settle();
  await shot("settings-then-new-tab");
  expect(document.querySelector("[data-zephium-new-tab]")).not.toBeNull();
});

test("a private window names itself at the foot of the column", async () => {
  await page.viewport(1100, 720);
  document.documentElement.dataset.theme = "dark";
  document.body.style.background = "#232326";
  launch.dispose();
  await surface.init();
  await tabs.init();
  await favicons.init();
  const fresh = tabFixture({ id: "private-tab", title: "New Tab", url: null });
  emitNativeEvent("itemsChanged", {
    projection_revision: revision(40),
    profile: { id: "private", name: "Private", kind: "incognito" },
    spaces: [],
    active_space_id: null,
    nodes: [
      { id: fresh.id, parent_id: null, section: "today", kind: { type: "tab", tab_id: fresh.id } },
    ],
    tabs: [fresh],
    active: fresh.id,
    split_group: null,
  });
  await render(Shell);
  await settle();
  await shot("private-window");
  expect(document.querySelector("[data-essentials-drop]")).toBeNull();
  expect(document.querySelector("[role=group][aria-label=Private]")).not.toBeNull();
});
