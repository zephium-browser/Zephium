import "$styles/global.css";
import { afterEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { page, userEvent } from "vitest/browser";
import { emitNativeEvent } from "$shared/testing/native-events";
import { tabFixture, revision, spaceFixture } from "$shared/testing/fixtures";
import { tabs } from "$domain/tabs";
import { surface } from "$domain/surface";
import Shell from "../Shell.svelte";
import { preferences } from "$domain/preferences";
import * as sidebar from "$session/sidebar-mode.svelte";
import { flushSync } from "svelte";
import { IS_MAC } from "$shared/platform";

vi.mock("../WorkWorkspace.svelte", async () => await import("./StageStub.svelte"));

vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    pagePermissionCardHost: async () => {},
    tabsBootstrap: async () => {},
    sidebarSetHidden: async () => {},
    sidebarSetWidth: async () => {},
    sidebarPeek: async () => {},
    sidebarResize: async () => false,
    sidebarResizeGuide: async () => true,
    settingGet: async () => null,
    settingSet: async () => ({ accepted: true, operation_id: null }),
  });
});
afterEach(() => {
  surface.dispose();
  tabs.dispose();
  preferences.dispose();
  sidebar.setMode("default");
  delete document.documentElement.dataset.reduceMotion;
});
test("Settings stands beside the tab rail and returns to the same web tab", async () => {
  await surface.init();
  await tabs.init();
  const tab = tabFixture();
  const items = {
    projection_revision: revision(10),
    profile: null,
    spaces: [],
    active_space_id: null,
    nodes: [],
    tabs: [tab],
    active: tab.id,
    split_group: null,
  };
  emitNativeEvent("itemsChanged", items);
  const screen = await render(Shell);
  emitNativeEvent("uiCommand", "browser.settings");
  expect(screen.container.querySelector('[data-zephium-surface="settings"]')).not.toBeNull();
  expect(screen.container.querySelector(".settings-sidebar-navigation")).not.toBeNull();
  // The tab stays one click away in the rail beside the settings panel.
  expect(screen.container.querySelector(`[data-zephium-tab-id="${tab.id}"]`)).not.toBeNull();
  emitNativeEvent("browserReturn", { ...items, projection_revision: revision(11) });
  expect(screen.container.querySelector('[data-zephium-surface="browse"]')).not.toBeNull();
  const rows = screen.container.querySelectorAll(`[data-zephium-tab-id="${tab.id}"]`);
  expect(rows).toHaveLength(1);
  expect(rows[0]?.querySelector("[data-zephium-tab-label]")?.textContent).toBe(tab.title);
  expect(screen.container.querySelector<HTMLInputElement>("[data-zephium-address]")?.value).toBe(
    "example.com",
  );
});

test("Work keeps the column as the rail of tabs, and a tab chosen there opens over the canvas", async () => {
  await surface.init();
  await tabs.init();
  const tab = tabFixture();
  emitNativeEvent("itemsChanged", {
    projection_revision: revision(20),
    profile: { id: "profile", name: "Personal", kind: "default" },
    spaces: [spaceFixture("space", "Home")],
    active_space_id: "space",
    nodes: [],
    tabs: [tab],
    active: tab.id,
    split_group: null,
  });
  const screen = await render(Shell);
  const column = screen.container.querySelector("aside")!;
  expect(screen.container.querySelector(".shelf")).not.toBeNull();

  emitNativeEvent("uiCommand", "browser.work");
  await expect.poll(() => column.style.width).toBe("56px");
  await expect.poll(() => screen.container.querySelector("[data-work-stage-stub]")).not.toBeNull();
  // The account closes the rail, where Browse keeps its tools.
  await expect.element(screen.getByRole("button", { name: "Account" })).toBeVisible();
  await page.viewport(900, 640);
  const rail = page.elementLocator(column);
  await rail.screenshot({ path: "../../../../../target/work-shell/rail-work.png" });
  await screen.getByRole("button", { name: "Work", exact: true }).click();
  await expect.element(screen.getByRole("button", { name: "Browse", exact: true })).toBeVisible();
  await new Promise((done) => setTimeout(done, 400));
  await rail.screenshot({ path: "../../../../../target/work-shell/rail-work-open.png" });
  await userEvent.keyboard("{Escape}");
  // The same column, not a new one: the rail, its tabs, no tool case.
  expect(screen.container.querySelector("aside")).toBe(column);
  expect(screen.container.querySelector(".shelf")).toBeNull();
  const row = screen.container.querySelector<HTMLElement>(
    `[data-zephium-tab-id="${tab.id}"] button`,
  )!;
  expect(row).not.toBeNull();
  const { tabRequest } = await import("$session/work-tab.svelte");
  row.click();
  await expect.poll(() => tabRequest()?.tab).toBe(tab.id);
  expect(surface.currentPage()).toBe("work");

  emitNativeEvent("uiCommand", "browser.return");
  await expect.poll(() => screen.container.querySelector(".shelf")).not.toBeNull();
  expect(screen.container.querySelector("aside")).toBe(column);
  await expect.poll(() => screen.container.querySelector(".account-trigger")).toBeNull();
});

test("leaving Work or Settings gives back the column the window had", async () => {
  await surface.init();
  await tabs.init();
  const tab = tabFixture();
  const items = {
    projection_revision: revision(30),
    profile: { id: "profile", name: "Personal", kind: "default" as const },
    spaces: [spaceFixture("space", "Home")],
    active_space_id: "space",
    nodes: [],
    tabs: [tab],
    active: tab.id,
    split_group: null,
  };
  emitNativeEvent("itemsChanged", items);
  const screen = await render(Shell);
  const column = screen.container.querySelector("aside")!;
  const expanded = column.style.width;
  const shown = () => ({
    width: column.style.width,
    resize: !!screen.container.querySelector('[aria-label="Resize sidebar"]'),
    toggle: !!screen.container.querySelector('[aria-label="Toggle sidebar"]'),
    modes: !!screen.container.querySelector('[aria-label="Switch between Browse and Work"]'),
  });
  const browse = { width: expanded, resize: true, toggle: true, modes: true };
  expect(shown()).toEqual(browse);

  for (let round = 0; round < 2; round++) {
    emitNativeEvent("uiCommand", "browser.work");
    await expect.poll(() => column.style.width).toBe("56px");
    emitNativeEvent("uiCommand", "browser.return");
    await expect.poll(shown).toEqual(browse);
  }

  emitNativeEvent("uiCommand", "browser.settings");
  await expect
    .poll(() => screen.container.querySelector(".settings-sidebar-navigation"))
    .not.toBeNull();
  emitNativeEvent("browserReturn", { ...items, projection_revision: revision(31) });
  await expect.poll(shown).toEqual(browse);
  expect(screen.container.querySelector(".settings-sidebar-navigation")).toBeNull();
});

test("changing edges keeps Settings inward and moves the hidden reveal to the selected edge", async () => {
  await page.viewport(900, 640);
  document.documentElement.dataset.reduceMotion = "true";
  await preferences.init();
  await surface.init();
  await tabs.init();
  const tab = tabFixture();
  const items = {
    projection_revision: revision(40),
    profile: null,
    spaces: [],
    active_space_id: null,
    nodes: [],
    tabs: [tab],
    active: tab.id,
    split_group: null,
  };
  emitNativeEvent("itemsChanged", items);
  const screen = await render(Shell);
  const column = screen.container.querySelector<HTMLElement>("aside")!;
  expect(column.getBoundingClientRect().left).toBe(8);
  emitNativeEvent("uiCommand", "preference.sidebar.position=right");
  flushSync();
  expect(column.getBoundingClientRect().right).toBe(892);

  emitNativeEvent("uiCommand", "browser.settings");
  await expect.poll(() => column.querySelector(".sidebar-tool-host")).not.toBeNull();
  const rail = column.querySelector<HTMLElement>(".sidebar-tool-rail")!;
  const panel = column.querySelector<HTMLElement>(".sidebar-tool-host")!;
  expect(panel.getBoundingClientRect().right).toBeLessThanOrEqual(
    rail.getBoundingClientRect().left,
  );
  const settingsPage = screen.container.querySelector<HTMLElement>(".internal-stage")!;
  expect(settingsPage.getBoundingClientRect().right).toBeLessThanOrEqual(
    column.getBoundingClientRect().left,
  );
  emitNativeEvent("uiCommand", "preference.sidebar.position=left");
  flushSync();
  expect(panel.getBoundingClientRect().left).toBeGreaterThanOrEqual(
    rail.getBoundingClientRect().right,
  );
  expect(column.querySelector(".sidebar-tool-host")).toBe(panel);

  emitNativeEvent("browserReturn", { ...items, projection_revision: revision(41) });
  sidebar.setMode("hidden");
  flushSync();
  await expect.poll(() => column.dataset.hidden).toBe("true");
  emitNativeEvent("uiCommand", "preference.sidebar.position=right");
  flushSync();
  expect(column.getBoundingClientRect().left).toBeGreaterThanOrEqual(900);
  window.dispatchEvent(new CustomEvent("zephium:sidebar-edge", { detail: { near: true } }));
  await expect.poll(() => column.dataset.peek).toBe("true");
  await expect.poll(() => column.getBoundingClientRect().right).toBe(892);
  emitNativeEvent("uiCommand", "preference.sidebar.position=left");
  flushSync();
  expect(column.dataset.peek).toBeUndefined();
  expect(column.getBoundingClientRect().right).toBeLessThanOrEqual(0);
});

test.each(["new-tab", "navigation-failure"] as const)(
  "%s uses the native page gap on either edge and fills it while hidden",
  async (kind) => {
    await page.viewport(900, 640);
    document.documentElement.dataset.reduceMotion = "true";
    await preferences.init();
    await surface.init();
    await tabs.init();
    const projection = kind === "new-tab" ? 50 : 60;
    const tab = tabFixture({
      id: kind,
      projection_revision: revision(projection),
      url: null,
      failure:
        kind === "navigation-failure"
          ? { url: "https://example.com/", reason: "unreachable", covers_page: true }
          : null,
    });
    emitNativeEvent("itemsChanged", {
      projection_revision: revision(projection),
      profile: null,
      spaces: [],
      active_space_id: null,
      nodes: [],
      tabs: [tab],
      active: tab.id,
      split_group: null,
    });
    const screen = await render(Shell);
    const column = screen.container.querySelector<HTMLElement>("aside")!;
    const pane = screen.container.querySelector<HTMLElement>(
      ".stage-column > main > .content-pane",
    )!;
    expect(pane.getBoundingClientRect().left).toBe(column.getBoundingClientRect().right + 8);
    expect(pane.getBoundingClientRect().right).toBe(892);
    emitNativeEvent("uiCommand", "preference.sidebar.position=right");
    flushSync();
    expect(pane.getBoundingClientRect().left).toBe(8);
    expect(pane.getBoundingClientRect().right).toBe(column.getBoundingClientRect().left - 8);
    expect(column.getBoundingClientRect().top).toBe(8);
    expect(pane.getBoundingClientRect().top).toBe(IS_MAC ? 40 : 8);
    sidebar.setMode("hidden");
    flushSync();
    await expect.poll(() => column.dataset.hidden).toBe("true");
    expect(pane.getBoundingClientRect().left).toBe(8);
    expect(pane.getBoundingClientRect().right).toBe(892);
    expect(pane.getBoundingClientRect().top).toBe(IS_MAC ? 40 : 8);
    if (IS_MAC) {
      window.dispatchEvent(new CustomEvent("zephium:fullscreen", { detail: true }));
      flushSync();
      expect(pane.getBoundingClientRect().top).toBe(8);
      window.dispatchEvent(new CustomEvent("zephium:fullscreen", { detail: false }));
      flushSync();
      expect(pane.getBoundingClientRect().top).toBe(40);
    }
  },
);
