import "$styles/global.css";
import { afterEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { page, userEvent } from "vitest/browser";
import { emitNativeEvent } from "$shared/testing/native-events";
import { tabFixture, revision, spaceFixture } from "$shared/testing/fixtures";
import { tabs } from "$domain/tabs";
import { surface } from "$domain/surface";
import Shell from "../Shell.svelte";

vi.mock("../WorkWorkspace.svelte", async () => await import("./StageStub.svelte"));

vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    pagePermissionCardHost: async () => {},
    tabsBootstrap: async () => {},
    sidebarSetHidden: async () => {},
  });
});
afterEach(() => {
  surface.dispose();
  tabs.dispose();
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
