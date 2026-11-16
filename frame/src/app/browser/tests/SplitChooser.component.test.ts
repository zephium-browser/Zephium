import "$styles/global.css";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { cleanup, render } from "vitest-browser-svelte";
import { page, userEvent } from "vitest/browser";
import { flushSync } from "svelte";
import type { ItemsChanged, SidebarNodeView } from "$shared/ipc/bindings";
import { emitNativeEvent } from "$shared/testing/native-events";
import { revision, spaceFixture, tabFixture } from "$shared/testing/fixtures";
import { tabs } from "$domain/tabs";
import { surface } from "$domain/surface";
import { uiCommands } from "$domain/ui-commands";
import * as sidebar from "$session/sidebar-mode.svelte";
import * as tools from "$session/tools.svelte";
import Shell from "../Shell.svelte";

const native = vi.hoisted(() => ({
  split: vi.fn(async (_id: string) => ({ accepted: true, operation_id: null })),
  run: vi.fn(async (_id: string) => ({ accepted: true, operation_id: null })),
  settings: vi.fn(async () => ({ accepted: true, operation_id: null })),
  width: vi.fn(async (_width: number | null, _travel: boolean, _revision: number | null) => {}),
  hidden: vi.fn(async (_hidden: boolean) => {}),
}));

vi.mock("../WorkWorkspace.svelte", async () => await import("./StageStub.svelte"));
vi.mock("$features/tools", () => ({ loadToolSlot: () => import("./SplitToolStub.svelte") }));
vi.mock("$features/notes", () => ({ loadNotesPage: () => import("./StageStub.svelte") }));
vi.mock("$shared/lib/motion", async (original) => ({
  ...(await original<typeof import("$shared/lib/motion")>()),
  reducedMotion: () => true,
}));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    pagePermissionCardHost: async () => {},
    tabsBootstrap: async () => {},
    tabsSplit: native.split,
    runCommand: native.run,
    settingGet: async () => null,
    settingSet: native.settings,
    sidebarSetHidden: native.hidden,
    sidebarSetWidth: native.width,
    sidebarPeek: async () => {},
    sidebarResize: async () => false,
    sidebarResizeGuide: async () => true,
  });
});

let projection = 100;
function project(overrides: Partial<ItemsChanged> = {}) {
  const all = ["a", "b"].map((id) => tabFixture({ id, title: `Tab ${id}` }));
  const items: ItemsChanged = {
    projection_revision: revision(++projection),
    profile: { id: "profile", name: "Personal", kind: "default" },
    spaces: [spaceFixture("home", "Home"), spaceFixture("other", "Other")],
    active_space_id: "home",
    nodes: all.map((tab) => ({
      id: tab.id,
      parent_id: null,
      section: "today",
      kind: { type: "tab", tab_id: tab.id },
    })) as SidebarNodeView[],
    tabs: all,
    active: "a",
    split_group: null,
    ...overrides,
  };
  emitNativeEvent("itemsChanged", items);
  return items;
}

beforeEach(async () => {
  await page.viewport(900, 700);
  vi.spyOn(document, "hasFocus").mockReturnValue(true);
  await surface.init();
  await tabs.init();
  await uiCommands.init();
  emitNativeEvent("uiCommand", "test.reset");
  sidebar.setMode("default");
  project();
});

afterEach(async () => {
  await cleanup();
  tools.close();
  sidebar.holdShown(false);
  sidebar.setMode("default");
  surface.dispose();
  tabs.dispose();
  uiCommands.dispose();
  vi.clearAllMocks();
});

for (const destination of ["work", "settings", "notes"] as const) {
  test(`${destination} returns to authoritative Browse before showing the chooser`, async () => {
    emitNativeEvent("uiCommand", `browser.${destination}`);
    const screen = await render(Shell);
    emitNativeEvent("uiCommand", "split.choose");
    await expect.poll(() => native.run.mock.calls).toEqual([["browser.return"]]);
    expect(sidebar.fullColumnShown()).toBe(false);
    expect(surface.currentPage()).toBe(destination);
    emitNativeEvent("uiCommand", "split.choose");
    flushSync();
    expect(native.run).toHaveBeenCalledTimes(1);
    emitNativeEvent("uiCommand", "browser.return");
    await expect.element(screen.getByText("Choose a second tab for split view")).toBeVisible();
    expect(sidebar.fullColumnShown()).toBe(true);
    await userEvent.keyboard("{Escape}");
    expect(surface.currentPage()).toBeNull();
  });
}

test("a refused return clears the pending chooser request", async () => {
  emitNativeEvent("uiCommand", "browser.work");
  const screen = await render(Shell);
  native.run.mockResolvedValueOnce({ accepted: false, operation_id: null });
  emitNativeEvent("uiCommand", "split.choose");
  await expect.poll(surface.navigationFailed).toBe(true);
  emitNativeEvent("uiCommand", "browser.return");
  flushSync();
  expect(sidebar.fullColumnShown()).toBe(false);
  expect(screen.container.querySelector('[data-split-candidate="true"]')).toBeNull();
});

test("a target change clears a chooser waiting for Browse", async () => {
  emitNativeEvent("uiCommand", "browser.work");
  const screen = await render(Shell);
  emitNativeEvent("uiCommand", "split.choose");
  await expect.poll(() => native.run.mock.calls.length).toBe(1);
  project({ active: "b" });
  flushSync();
  emitNativeEvent("uiCommand", "browser.return");
  flushSync();
  expect(sidebar.fullColumnShown()).toBe(false);
  expect(screen.container.querySelector('[data-split-candidate="true"]')).toBeNull();
});

test("Escape cancels a pending chooser before a delayed native return", async () => {
  emitNativeEvent("uiCommand", "browser.work");
  const items = project();
  const screen = await render(Shell);
  emitNativeEvent("uiCommand", "split.choose");
  await expect.poll(() => native.run.mock.calls.length).toBe(1);
  await userEvent.keyboard("{Escape}");
  emitNativeEvent("browserReturn", { ...items, projection_revision: revision(++projection) });
  flushSync();
  expect(surface.currentPage()).toBeNull();
  expect(sidebar.fullColumnShown()).toBe(false);
  expect(screen.container.querySelector('[data-split-candidate="true"]')).toBeNull();
});

async function choose(screen: Awaited<ReturnType<typeof render>>) {
  emitNativeEvent("uiCommand", "split.choose");
  await expect.element(screen.getByText("Choose a second tab for split view")).toBeVisible();
  await expect.element(screen.getByRole("button", { name: "Cancel", exact: true })).toBeVisible();
  await expect
    .poll(() =>
      screen.container
        .querySelector('[data-zephium-tab-id="b"]')
        ?.getAttribute("data-split-candidate"),
    )
    .toBe("true");
}

for (const mode of ["compact", "hidden"] as const) {
  test(`${mode} opens the full chooser without saving preferences and Cancel restores it`, async () => {
    sidebar.setMode(mode);
    const screen = await render(Shell);
    const aside = screen.container.querySelector("aside")!;
    native.settings.mockClear();
    await choose(screen);
    expect(aside.style.width).toBe(`${sidebar.expanded()}px`);
    expect(aside.dataset.hidden).toBeUndefined();
    expect(sidebar.sidebarMode()).toBe(mode);
    expect(sidebar.isHidden()).toBe(false);
    expect(native.hidden).toHaveBeenLastCalledWith(false);
    aside.dispatchEvent(new PointerEvent("pointerleave"));
    window.dispatchEvent(new CustomEvent("zephium:sidebar-edge", { detail: { near: false } }));
    flushSync();
    await expect.element(screen.getByText("Choose a second tab for split view")).toBeVisible();
    await screen.getByRole("button", { name: "Cancel", exact: true }).click();
    await expect.poll(sidebar.fullColumnShown).toBe(false);
    expect(sidebar.sidebarMode()).toBe(mode);
    expect(sidebar.isHidden()).toBe(mode === "hidden");
    expect(aside.style.width).toBe(
      `${mode === "compact" ? sidebar.COMPACT_WIDTH : sidebar.expanded()}px`,
    );
    expect(native.settings).not.toHaveBeenCalled();
    expect(native.split).not.toHaveBeenCalled();
  });

  test(`${mode} supports keyboard completion and restores the original shape`, async () => {
    sidebar.setMode(mode);
    const screen = await render(Shell);
    await choose(screen);
    expect(document.activeElement).toBe(
      screen.getByRole("button", { name: "Cancel", exact: true }).element(),
    );
    await screen.getByRole("button", { name: "Tab a", exact: true }).click();
    expect(native.split).not.toHaveBeenCalled();
    expect(sidebar.fullColumnShown()).toBe(true);
    const candidate = screen.getByRole("button", { name: "Tab b", exact: true });
    (candidate.element() as HTMLElement).focus();
    await userEvent.keyboard("{Enter}");
    expect(native.split).toHaveBeenCalledExactlyOnceWith("b");
    await expect.poll(sidebar.fullColumnShown).toBe(false);
    expect(sidebar.sidebarMode()).toBe(mode);
    await expect
      .element(screen.getByText("Choose a second tab for split view"))
      .not.toBeInTheDocument();
  });
}

test("Escape restores the mounted tool, its draft and the invoking control", async () => {
  tools.open("notes");
  const screen = await render(Shell);
  const draft = screen.getByRole("textbox", { name: "Tool draft" });
  await expect.element(draft).toBeVisible();
  const input = draft.element() as HTMLInputElement;
  input.value = "Unfinished note";
  input.focus();
  const panel = input.closest<HTMLElement>(".sidebar-tool-panel")!;
  await choose(screen);
  expect(panel.hidden).toBe(true);
  expect(panel.inert).toBe(true);
  expect(input.isConnected).toBe(true);
  expect(tools.activeTool()).toBe("notes");
  expect(screen.container.querySelector("aside")!.style.width).toBe(`${sidebar.expanded()}px`);
  const composing = new KeyboardEvent("keydown", {
    key: "Escape",
    isComposing: true,
    bubbles: true,
    cancelable: true,
  });
  window.dispatchEvent(composing);
  expect(composing.defaultPrevented).toBe(false);
  expect(sidebar.fullColumnShown()).toBe(true);
  await userEvent.keyboard("{Escape}");
  await expect.element(draft).toBeVisible();
  expect(draft.element()).toBe(input);
  expect(input.value).toBe("Unfinished note");
  expect(panel.inert).toBe(false);
  expect(tools.activeTool()).toBe("notes");
  expect(screen.container.querySelector("aside")!.style.width).toBe(
    `${sidebar.COMPACT_WIDTH + sidebar.PANEL_GAP + 336}px`,
  );
  await expect.poll(() => document.activeElement).toBe(input);
  expect(native.split).not.toHaveBeenCalled();
});

test("a native page holding focus is not pulled into the chooser or back to a chrome control", async () => {
  const screen = await render(Shell);
  const origin = screen
    .getByRole("button", { name: "Reload", exact: true })
    .element() as HTMLElement;
  origin.focus();
  vi.spyOn(document, "hasFocus").mockReturnValue(false);
  const focus = vi.spyOn(HTMLElement.prototype, "focus");
  await choose(screen);
  expect(focus).not.toHaveBeenCalled();
  window.dispatchEvent(
    new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }),
  );
  await expect.poll(sidebar.fullColumnShown).toBe(false);
  expect(focus).not.toHaveBeenCalled();
});

for (const context of ["tab", "space", "profile", "page", "tool", "unmount"] as const) {
  test(`${context} changes release the chooser without splitting`, async () => {
    sidebar.setMode("hidden");
    const screen = await render(Shell);
    await choose(screen);
    if (context === "tab") project({ active: "b" });
    if (context === "space") project({ active_space_id: "other" });
    if (context === "profile")
      project({ profile: { id: "other", name: "Other", kind: "default" } });
    if (context === "page") emitNativeEvent("uiCommand", "browser.work");
    if (context === "tool") tools.open("notes");
    if (context === "unmount") await cleanup();
    await expect.poll(sidebar.fullColumnShown).toBe(false);
    expect(sidebar.sidebarMode()).toBe("hidden");
    expect(native.split).not.toHaveBeenCalled();
    expect(screen.container.querySelector('[data-split-candidate="true"]')).toBeNull();
  });
}
