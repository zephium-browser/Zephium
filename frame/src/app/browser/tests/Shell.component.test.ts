import "$styles/global.css";
import { afterEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { page, userEvent } from "vitest/browser";
import { flushSync } from "svelte";
import type { SidebarNodeView } from "$shared/ipc/bindings";
import { emitNativeEvent } from "$shared/testing/native-events";
import { tabFixture, revision, spaceFixture } from "$shared/testing/fixtures";
import { tabs } from "$domain/tabs";
import { surface } from "$domain/surface";
import Shell from "../Shell.svelte";

const native = vi.hoisted(() => ({
  activate: vi.fn(async (_id: string) => ({ accepted: true, operation_id: null })),
}));

vi.mock("../WorkWorkspace.svelte", async () => await import("./StageStub.svelte"));

vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    pagePermissionCardHost: async () => {},
    runCommand: async (id) => {
      emitNativeEvent("uiCommand", id);
      return { accepted: true, operation_id: null };
    },
    tabsBootstrap: async () => {},
    tabsActivate: native.activate,
    sidebarSetHidden: async () => {},
  });
});
afterEach(() => {
  surface.dispose();
  tabs.dispose();
  vi.clearAllMocks();
});

test("activating either Essentials list clears the sidebar selection before native activation", async () => {
  await page.viewport(900, 640);
  await surface.init();
  await tabs.init();
  const all = ["a", "b", "e1", "e2", "e3", "e4", "e5", "e6"].map((id) =>
    tabFixture({ id, title: `Tab ${id}` }),
  );
  const nodes: SidebarNodeView[] = all.map((tab) => ({
    id: tab.id,
    parent_id: null,
    section: tab.id.startsWith("e") ? "favorites" : "today",
    kind: { type: "tab", tab_id: tab.id },
  }));
  emitNativeEvent("itemsChanged", {
    projection_revision: revision(40),
    profile: null,
    spaces: [],
    active_space_id: null,
    nodes,
    tabs: all,
    active: "a",
    split_group: null,
  });
  const screen = await render(Shell);
  const selected = () => screen.container.querySelectorAll('[data-multi-selected="true"]');
  for (const layout of ["dock-row", "dock-stack"]) {
    screen.container
      .querySelector<HTMLButtonElement>('[data-zephium-tab-id="b"] .tab-open')!
      .dispatchEvent(new MouseEvent("click", { bubbles: true, shiftKey: true }));
    flushSync();
    expect(selected()).toHaveLength(2);
    let selectedAtActivation = -1;
    let barAtActivation = true;
    native.activate.mockImplementationOnce(async () => {
      flushSync();
      selectedAtActivation = selected().length;
      barAtActivation = screen.container.querySelector(".selection-bar") !== null;
      return { accepted: true, operation_id: null };
    });
    const tile = screen.container.querySelector<HTMLButtonElement>(
      `.${layout} .essential > button[data-plate]`,
    )!;
    expect(tile).not.toBeNull();
    tile.click();
    await expect
      .poll(() => native.activate.mock.calls.at(-1))
      .toEqual([tile.closest<HTMLElement>("[data-zephium-tab-id]")!.dataset.zephiumTabId]);
    expect(selectedAtActivation).toBe(0);
    expect(barAtActivation).toBe(false);
  }
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

test("the new tab settings button opens General after another section was selected", async () => {
  await surface.init();
  await tabs.init();
  const { handleNativeSection } = await import("$features/settings");
  handleNativeSection("settings.section.newtab");
  const tab = tabFixture({ url: null });
  emitNativeEvent("itemsChanged", {
    projection_revision: revision(60),
    profile: null,
    spaces: [],
    active_space_id: null,
    nodes: [],
    tabs: [tab],
    active: tab.id,
    split_group: null,
  });
  const screen = await render(Shell);
  await screen.getByRole("button", { name: "Customize New Tab" }).click();
  await expect.poll(() => surface.currentPage()).toBe("settings");
  await expect.element(screen.getByRole("heading", { name: "General", exact: true })).toBeVisible();
});
