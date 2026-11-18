import { afterEach, expect, test, vi } from "vitest";
import { page, userEvent } from "vitest/browser";
import { render } from "vitest-browser-svelte";
import { flushSync } from "svelte";
import "$styles/global.css";
import type { FolderIntent, SidebarNodeView, TabView } from "$shared/ipc/bindings";
import { emitNativeEvent } from "$shared/testing/native-events";
import { revision, tabFixture } from "$shared/testing/fixtures";
import { tabs } from "$domain/tabs";
import TabList from "../components/TabList.svelte";
import { sidebarTree } from "../lib/sidebar-model";

const native = vi.hoisted(() => ({
  folder: vi.fn(async (_intent: FolderIntent) => ({ accepted: true, operation_id: null })),
  setEssential: vi.fn(async () => ({ accepted: true, operation_id: null })),
  menu: vi.fn(async () => true),
  close: vi.fn(async () => ({ accepted: true, operation_id: null })),
}));

vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    tabsBootstrap: async () => {},
    tabsFolder: native.folder,
    tabsSetEssential: native.setEssential,
    folderMenuPopup: native.menu,
    tabsClose: native.close,
    tabDragOver: async () => undefined,
    tabDrop: async () => ({ accepted: true, operation_id: null }),
  });
});

vi.mock("$domain/operations", async () => {
  const { commands } = await import("$shared/ipc/bindings");
  return {
    settle: async (admission: Promise<unknown>) => {
      await admission;
      return { outcome: "applied", disposition: null };
    },
    commands,
  };
});

afterEach(() => {
  tabs.dispose();
  for (const mock of Object.values(native)) mock.mockClear();
});

const folderNode = (id: string, collapsed = false, parent_id: string | null = null) =>
  ({
    id,
    parent_id,
    section: "today",
    kind: { type: "folder", name: `Folder ${id}`, color: "sky", collapsed },
  }) satisfies SidebarNodeView;

const tabNode = (id: string, parent_id: string | null = null): SidebarNodeView => ({
  id,
  parent_id,
  section: "today",
  kind: { type: "tab", tab_id: id },
});

async function list(collapsed = false, open: TabView[] = []) {
  await page.viewport(400, 500);
  await tabs.init();
  const all = open.length
    ? open
    : ["a", "b", "c", "d"].map((id) => tabFixture({ id, title: `Tab ${id}` }));
  const nodes: SidebarNodeView[] = [
    tabNode("a"),
    folderNode("F", collapsed),
    tabNode("b", "F"),
    tabNode("c", "F"),
    tabNode("d"),
  ];
  const project = (next: SidebarNodeView[]) => {
    emitNativeEvent("itemsChanged", {
      projection_revision: revision(Date.now()),
      profile: null,
      spaces: [],
      active_space_id: null,
      nodes: next,
      tabs: all,
      active: "a",
      split_group: null,
    });
    flushSync();
  };
  project(nodes);
  const screen = await render(TabList, {
    entries: sidebarTree(nodes, all).today,
    section: "today",
    label: "Open tabs",
    splitting: false,
    onSelect: () => {},
  });
  screen.container.setAttribute("data-tabs-drop", "");
  const row = (id: string) =>
    screen.container.querySelector<HTMLElement>(`[data-node-id="${id}"]`)!;
  return { row, screen, project, nodes, all };
}

const frame = () => new Promise((resolve) => requestAnimationFrame(() => resolve(undefined)));

test("a folder folds through native and, folded, shows what it holds", async () => {
  const { row, screen } = await list();
  const button = row("F").querySelector<HTMLButtonElement>("button")!;
  expect(button.getAttribute("aria-expanded")).toBe("true");
  expect(button.getAttribute("aria-label")).toBe("Folder F, folder, 2 tabs");
  expect(row("b").hasAttribute("data-spined")).toBe(true);

  button.click();
  await expect.poll(() => native.folder.mock.calls.length).toBe(1);
  expect(native.folder).toHaveBeenCalledWith({ type: "set_collapsed", id: "F", collapsed: true });
  // Drawn folded at once, until native's projection says the same.
  flushSync();
  expect(button.getAttribute("aria-expanded")).toBe("false");
  expect(row("b").classList.contains("hidden")).toBe(true);
  expect(row("b").getClientRects()).toHaveLength(0);
  expect(row("c").getClientRects()).toHaveLength(0);
  expect(screen.container.querySelectorAll(".folder-face")).toHaveLength(2);
});

test("a folded folder keeps the tab in front visible beneath it", async () => {
  const open = ["a", "b", "c", "d"].map((id) => tabFixture({ id, title: `Tab ${id}` }));
  const { row, project, nodes } = await list(true, open);
  expect(row("b").classList.contains("hidden")).toBe(true);
  emitNativeEvent("itemsChanged", {
    projection_revision: revision(Date.now()),
    profile: null,
    spaces: [],
    active_space_id: null,
    nodes,
    tabs: open,
    active: "c",
    split_group: null,
  });
  flushSync();
  expect(row("c").classList.contains("hidden")).toBe(false);
  expect(row("b").classList.contains("hidden")).toBe(true);
  void project;
});

test("arrow keys fold, unfold and walk out of a folder", async () => {
  const { row } = await list();
  const button = row("F").querySelector<HTMLButtonElement>("button")!;
  button.focus();
  await userEvent.keyboard("{ArrowLeft}");
  await expect.poll(() => native.folder.mock.calls.length).toBe(1);
  expect(native.folder).toHaveBeenLastCalledWith({
    type: "set_collapsed",
    id: "F",
    collapsed: true,
  });
  flushSync();
  await userEvent.keyboard("{ArrowRight}");
  await expect.poll(() => native.folder.mock.calls.length).toBe(2);
  expect(native.folder).toHaveBeenLastCalledWith({
    type: "set_collapsed",
    id: "F",
    collapsed: false,
  });
});

test("double-click renames in place; Escape keeps the name", async () => {
  const { row } = await list();
  const button = row("F").querySelector<HTMLButtonElement>("button")!;
  button.dispatchEvent(new MouseEvent("dblclick", { bubbles: true }));
  flushSync();
  const input = row("F").querySelector<HTMLInputElement>("input")!;
  expect(document.activeElement).toBe(input);
  expect(input.value).toBe("Folder F");
  await userEvent.keyboard("{Escape}");
  flushSync();
  expect(row("F").querySelector("input")).toBeNull();
  expect(native.folder).not.toHaveBeenCalled();

  row("F")
    .querySelector<HTMLButtonElement>("button")!
    .dispatchEvent(new MouseEvent("dblclick", { bubbles: true }));
  flushSync();
  await userEvent.fill(row("F").querySelector<HTMLInputElement>("input")!, "Reading");
  await userEvent.keyboard("{Enter}");
  await expect.poll(() => native.folder.mock.calls.length).toBe(1);
  expect(native.folder).toHaveBeenCalledWith({ type: "rename", id: "F", name: "Reading" });
});

test("the folder menu opens from the keyboard with what the folder can offer", async () => {
  const { row } = await list();
  const button = row("F").querySelector<HTMLButtonElement>("button")!;
  button.focus();
  await userEvent.keyboard("{Shift>}{F10}{/Shift}");
  await expect.poll(() => native.menu.mock.calls.length).toBe(1);
  expect(native.menu.mock.calls[0]).toEqual([
    "F",
    expect.any(Number),
    expect.any(Number),
    { color: "sky", can_nest: true, awake: 2 > 0 },
  ]);
});

test("a folder's tab offers to close its page while awake and to leave the folder asleep", async () => {
  const open = [
    tabFixture({ id: "a", title: "Tab a" }),
    tabFixture({ id: "b", title: "Tab b" }),
    tabFixture({ id: "c", title: "Tab c", asleep: true }),
    tabFixture({ id: "d", title: "Tab d" }),
  ];
  const { row } = await list(false, open);
  expect(row("b").querySelector(".tab-close")?.getAttribute("aria-label")).toBe(
    "Close Tab b and keep it in the folder",
  );
  expect(row("c").querySelector(".tab-close")?.getAttribute("aria-label")).toBe(
    "Remove Tab c from the folder",
  );
  expect(row("d").querySelector(".tab-close")?.getAttribute("aria-label")).toBe("Close Tab d");
});

function drag(handle: HTMLElement, to: { x: number; y: number }) {
  const from = handle.getBoundingClientRect();
  const start = { x: from.left + 20, y: from.top + from.height / 2 };
  const init = { bubbles: true, pointerId: 1, button: 0, isPrimary: true };
  handle.dispatchEvent(
    new PointerEvent("pointerdown", { ...init, clientX: start.x, clientY: start.y }),
  );
  handle.dispatchEvent(
    new PointerEvent("pointermove", { ...init, clientX: start.x, clientY: start.y + 8 }),
  );
  handle.dispatchEvent(new PointerEvent("pointermove", { ...init, clientX: to.x, clientY: to.y }));
  const up = () =>
    handle.dispatchEvent(new PointerEvent("pointerup", { ...init, clientX: to.x, clientY: to.y }));
  return { start, up };
}

test("a tab dragged to the end of a folder and set in goes into it", async () => {
  const { row } = await list();
  const handle = row("d").querySelector<HTMLElement>(".tab-open")!;
  const start = handle.getBoundingClientRect();
  // Up past nothing (it already follows the folder's last tab), set in by an indent.
  const { up } = drag(handle, { x: start.left + 20 + 16, y: start.top + start.height / 2 });
  await frame();
  expect(row("F").hasAttribute("data-drop-into")).toBe(true);
  expect(row("d").style.transform).toContain("translate(13px");
  up();
  await expect.poll(() => native.folder.mock.calls.length).toBe(1);
  expect(native.folder).toHaveBeenCalledWith({ type: "move", id: "d", parent: "F", before: null });
  expect(native.setEssential).not.toHaveBeenCalled();
});

test("a folder drags with what it holds and lands at the top level", async () => {
  const { row } = await list();
  const handle = row("F").querySelector<HTMLElement>("button")!;
  const above = row("a").getBoundingClientRect();
  const { up } = drag(handle, { x: above.left + 20, y: above.top + 2 });
  await frame();
  expect(row("F").style.transform).toContain("translateY(-");
  expect(row("b").style.transform).toContain("translateY(-");
  expect(row("c").style.transform).toContain("translateY(-");
  expect(row("a").style.transform).toContain("translateY(");
  up();
  await expect.poll(() => native.folder.mock.calls.length).toBe(1);
  expect(native.folder).toHaveBeenCalledWith({ type: "move", id: "F", parent: null, before: "a" });
});

test("a folder's tab dragged out to the top level goes as an open-tab reorder", async () => {
  const { row } = await list();
  const handle = row("b").querySelector<HTMLElement>(".tab-open")!;
  const top = row("a").getBoundingClientRect();
  const { up } = drag(handle, { x: top.left + 20 - 13, y: top.top + 2 });
  await frame();
  up();
  await expect.poll(() => native.setEssential.mock.calls.length).toBe(1);
  expect(native.setEssential).toHaveBeenCalledWith("b", false, "a");
  expect(native.folder).not.toHaveBeenCalled();
});
