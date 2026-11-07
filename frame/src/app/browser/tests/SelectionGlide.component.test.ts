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
import Shell from "../Shell.svelte";

vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    pagePermissionCardHost: async () => {},
    tabsBootstrap: async () => {},
    sidebarSetHidden: async () => {},
    sidebarPeek: async () => {},
    sidebarSetWidth: async () => {},
  });
});

afterEach(() => {
  surface.dispose();
  tabs.dispose();
});

let projection = 1_000;
const open = ["a", "b", "c", "d"].map((id) => tabFixture({ id, title: `Tab ${id}` }));
const node = (id: string): SidebarNodeView => ({
  id,
  parent_id: null,
  section: "today",
  kind: { type: "tab", tab_id: id },
});
const select = (active: string, space: string | null = null, only = open) => {
  emitNativeEvent("itemsChanged", {
    projection_revision: revision((projection += 1)),
    profile: null,
    spaces: [],
    active_space_id: space,
    nodes: only.map((tab) => node(tab.id)),
    tabs: only,
    active,
    split_group: null,
  });
  flushSync();
};
const row = (id: string) =>
  document.querySelector<HTMLElement>(`.browse-tab[data-zephium-tab-id="${id}"]`)!;

test("the current tab's plate travels to the next one and hands itself back", async () => {
  await page.viewport(900, 700);
  await surface.init();
  await tabs.init();
  select("a");
  await render(Shell);

  select("d");
  const glide = document.querySelector<HTMLElement>(".selection-glide");
  expect(glide).not.toBeNull();
  expect(glide!.getAnimations().length).toBeGreaterThan(0);
  // The destination does not paint a second plate under the travelling one.
  expect(row("d").hasAttribute("data-gliding")).toBe(true);
  expect(getComputedStyle(row("d")).backgroundColor).toBe("rgba(0, 0, 0, 0)");

  await expect.poll(() => glide!.isConnected, { timeout: 2000 }).toBe(false);
  expect(row("d").hasAttribute("data-gliding")).toBe(false);
  expect(getComputedStyle(row("d")).backgroundColor).not.toBe("rgba(0, 0, 0, 0)");
});

test("a second change mid-flight carries on from where the plate has got to", async () => {
  await page.viewport(900, 700);
  await surface.init();
  await tabs.init();
  select("a");
  await render(Shell);

  select("d");
  select("b");
  expect(document.querySelectorAll(".selection-glide")).toHaveLength(1);
  expect(row("d").hasAttribute("data-gliding")).toBe(false);
  expect(row("b").hasAttribute("data-gliding")).toBe(true);
});

test("the current tab's plate never crosses from one space's list to another's", async () => {
  await page.viewport(900, 700);
  await surface.init();
  await tabs.init();
  select("a", "home", open.slice(0, 2));
  await render(Shell);

  // Another space is another sheet: its current tab is simply lit there.
  select("d", "studio", open.slice(2));
  expect(document.querySelector(".selection-glide")).toBeNull();
  expect(row("d").hasAttribute("data-gliding")).toBe(false);
  expect(row("d").dataset.selected).toBe("true");

  // Within a space it travels as ever.
  select("c", "studio", open.slice(2));
  expect(document.querySelector(".selection-glide")).not.toBeNull();
});
