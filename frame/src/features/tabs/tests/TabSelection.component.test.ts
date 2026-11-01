import { afterEach, expect, test, vi } from "vitest";
import { page } from "vitest/browser";
import { render } from "vitest-browser-svelte";
import { flushSync } from "svelte";
import "$styles/global.css";
import type { ItemsState, SidebarNodeView, TabView } from "$shared/ipc/bindings";
import { emitNativeEvent } from "$shared/testing/native-events";
import { revision, spaceFixture, tabFixture } from "$shared/testing/fixtures";
import { tabs } from "$domain/tabs";
import TabList from "../components/TabList.svelte";
import SidebarBody from "../components/SidebarBody.svelte";
import { sidebarTree } from "../lib/sidebar-model";

const native = vi.hoisted(() => ({
  activate: vi.fn(async (_id: string) => ({ accepted: true, operation_id: null })),
  close: vi.fn(async (_id: string) => ({ accepted: true, operation_id: null })),
  essential: vi.fn(async (_id: string, _essential: boolean, _before: string | null) => ({
    accepted: true,
    operation_id: null,
  })),
  mute: vi.fn(async (_id: string, _muted: boolean) => ({ accepted: true, operation_id: null })),
  move: vi.fn(async (_id: string, _space: string) => ({ accepted: true, operation_id: null })),
}));

vi.mock("$shared/platform", () => ({ IS_MAC: false, IS_WINDOWS: true }));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    tabsBootstrap: async () => {},
    tabsActivate: native.activate,
    tabsClose: native.close,
    tabsSetEssential: native.essential,
    tabSetMuted: native.mute,
    spacesMoveTab: native.move,
    tabDragOver: async () => {},
    swipeRegion: async () => false,
  });
});

afterEach(() => {
  tabs.dispose();
  vi.clearAllMocks();
});

let sequence = 1000;
const open = ["a", "b", "c", "d"].map((id) =>
  tabFixture({
    id,
    title: `Tab ${id}`,
    ...(id === "a" || id === "b" ? { audio: { muted: false, can_mute: true } } : {}),
  }),
);

async function list(
  options: {
    section?: "today" | "pinned";
    nodes?: SidebarNodeView[];
    all?: TabView[];
    split?: string[];
    splitting?: boolean;
    standalone?: boolean;
  } = {},
) {
  await page.viewport(300, 500);
  await tabs.init();
  const section = options.section ?? "today";
  const all = (options.all ?? open).map((tab) => ({
    ...tab,
    projection_revision: revision(++sequence),
  }));
  const nodes: SidebarNodeView[] =
    options.nodes ??
    all.map((tab) => ({
      id: tab.id,
      parent_id: null,
      section,
      kind: { type: "tab", tab_id: tab.id },
    }));
  const snapshot: ItemsState = {
    projection_revision: revision(++sequence),
    profile: null,
    spaces: [spaceFixture("home", "Home"), spaceFixture("work", "Work")],
    active_space_id: "home",
    nodes,
    tabs: all,
    active: "a",
    split_group: options.split ? { members: options.split } : null,
  };
  flushSync(() => emitNativeEvent("itemsChanged", snapshot));
  const props = {
    entries: sidebarTree(nodes, all)[section],
    section,
    label: "Open tabs",
    splitting: options.splitting ?? false,
    onSelect: tabs.activate,
  };
  const screen = options.standalone
    ? await render(TabList, props)
    : await render(SidebarBody, {
        pinned: section === "pinned" ? props.entries : [],
        today: section === "today" ? props.entries : [],
        splitting: props.splitting,
        onSelect: props.onSelect,
      });
  const row = (id: string) =>
    screen.container.querySelector<HTMLElement>(`[data-zephium-tab-id="${id}"]`)!;
  const button = (id: string) => row(id).querySelector<HTMLButtonElement>(".tab-open")!;
  const click = (id: string, modifiers: MouseEventInit = {}) => {
    button(id).dispatchEvent(new MouseEvent("click", { bubbles: true, ...modifiers }));
    flushSync();
  };
  const selected = () =>
    [...screen.container.querySelectorAll<HTMLElement>('[data-multi-selected="true"]')].map(
      (element) => element.dataset.zephiumTabId,
    );
  const publish = (overrides: Partial<ItemsState>) => {
    const changed = overrides.tabs?.map((tab) => ({
      ...tab,
      projection_revision: revision(++sequence),
    }));
    flushSync(() =>
      emitNativeEvent("itemsChanged", {
        ...snapshot,
        ...overrides,
        ...(changed ? { tabs: changed } : {}),
        projection_revision: revision(++sequence),
      }),
    );
  };
  return { screen, row, button, click, selected, publish, snapshot };
}

test("Ctrl-click includes the active tab, toggles, and never activates on Windows", async () => {
  const { click, selected, screen, button } = await list();
  expect(screen.container.querySelector(".selection-bar")).toBeNull();
  expect(screen.container.querySelector(".tab-open[aria-pressed]")).toBeNull();
  click("b", { ctrlKey: true });
  expect(selected()).toEqual(["a", "b"]);
  expect(button("a").getAttribute("aria-current")).toBe("page");
  expect(button("a").getAttribute("aria-pressed")).toBe("true");
  expect(button("b").getAttribute("aria-pressed")).toBe("true");
  expect(button("c").getAttribute("aria-pressed")).toBe("false");
  expect(screen.container.textContent).toContain("2 tabs selected");
  click("b", { ctrlKey: true });
  expect(selected()).toEqual(["a"]);
  expect(screen.container.querySelector(".tab-open[aria-pressed]")).toBeNull();
  expect(screen.container.querySelector(".selection-bar")).toBeNull();
  expect(native.activate).not.toHaveBeenCalled();
});

test("Meta-click does not toggle on Windows", async () => {
  const { click, selected } = await list();
  click("b", { metaKey: true });
  expect(selected()).toEqual([]);
  expect(native.activate).toHaveBeenCalledExactlyOnceWith("b");
});

test("lists without selection retain activation for modifier clicks", async () => {
  const { click, selected, screen } = await list({ standalone: true });
  click("b", { ctrlKey: true });
  click("c", { shiftKey: true });
  expect(selected()).toEqual([]);
  expect(screen.container.querySelector(".tab-open[aria-pressed]")).toBeNull();
  expect(screen.container.querySelector(".selection-bar")).toBeNull();
  expect(native.activate.mock.calls).toEqual([["b"], ["c"]]);
});

test("Shift ranges use the last plain or toggled anchor; a plain click clears and activates", async () => {
  const { click, selected, screen } = await list();
  click("b");
  click("d", { shiftKey: true });
  expect(selected()).toEqual(["b", "c", "d"]);
  expect(native.activate).toHaveBeenCalledExactlyOnceWith("b");
  click("c", { shiftKey: true });
  expect(selected()).toEqual(["b", "c"]);
  click("a");
  expect(selected()).toEqual([]);
  expect(screen.container.querySelector(".selection-bar")).toBeNull();
  expect(native.activate).toHaveBeenLastCalledWith("a");
});

test("Escape clears selection even when the sidebar has lost focus", async () => {
  const { click, selected } = await list();
  click("b", { ctrlKey: true });
  window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
  flushSync();
  expect(selected()).toEqual([]);
});

test("Close settles a command for every selected ID", async () => {
  const { click, screen } = await list();
  click("c", { shiftKey: true });
  await screen.getByRole("button", { name: "Close selected tabs" }).click();
  await expect.poll(() => native.close.mock.calls).toEqual([["a"], ["b"], ["c"]]);
  expect(native.activate).not.toHaveBeenCalled();
});

test("Essentials and Mute use only eligible targets and Unmute follows the projection", async () => {
  const { click, screen, publish, snapshot } = await list();
  click("c", { shiftKey: true });
  await screen.getByRole("button", { name: "Add to Essentials" }).click();
  await expect
    .poll(() => native.essential.mock.calls)
    .toEqual([
      ["a", true, null],
      ["b", true, null],
      ["c", true, null],
    ]);
  await screen.getByRole("button", { name: "Mute selected tabs" }).click();
  await expect
    .poll(() => native.mute.mock.calls)
    .toEqual([
      ["a", true],
      ["b", true],
    ]);
  publish({
    tabs: snapshot.tabs.map((tab) =>
      tab.audio ? { ...tab, audio: { ...tab.audio, muted: true } } : tab,
    ),
  });
  await screen.getByRole("button", { name: "Unmute selected tabs" }).click();
  await expect
    .poll(() => native.mute.mock.calls.slice(2))
    .toEqual([
      ["a", false],
      ["b", false],
    ]);
});

test("Move to space sends every selected ID to the chosen space", async () => {
  const { click, screen } = await list();
  click("b", { ctrlKey: true });
  const picker = screen
    .getByRole("combobox", { name: "Move to Space" })
    .element() as HTMLSelectElement;
  expect([...picker.options].map((option) => option.value)).toEqual(["", "work"]);
  picker.value = "work";
  picker.dispatchEvent(new Event("change", { bubbles: true }));
  await expect
    .poll(() => native.move.mock.calls)
    .toEqual([
      ["a", "work"],
      ["b", "work"],
    ]);
});

test.each([true, false])(
  "bulk mute keeps muted=%s through synchronous projections",
  async (muted) => {
    const { click, screen, publish, snapshot } = await list();
    click("b", { ctrlKey: true });
    const all = snapshot.tabs.map((tab) =>
      tab.audio ? { ...tab, audio: { ...tab.audio, muted: tab.id === "a" ? !muted : true } } : tab,
    );
    publish({ tabs: all });
    native.mute.mockImplementationOnce(async (id, value) => {
      publish({
        tabs: all.map((tab) =>
          tab.id === id && tab.audio ? { ...tab, audio: { ...tab.audio, muted: value } } : tab,
        ),
      });
      return { accepted: true, operation_id: null };
    });
    await screen
      .getByRole("button", { name: muted ? "Mute selected tabs" : "Unmute selected tabs" })
      .click();
    expect(native.mute.mock.calls).toEqual([
      ["a", muted],
      ["b", muted],
    ]);
  },
);

test.each([true, false])(
  "bulk Essentials keeps essential=%s through synchronous projections",
  async (essential) => {
    const { click, screen, publish, snapshot } = await list();
    click("b", { ctrlKey: true });
    const nodes = snapshot.nodes.map((node) => ({
      ...node,
      section: node.id === "a" && essential ? ("today" as const) : ("favorites" as const),
    }));
    publish({ nodes });
    native.essential.mockImplementationOnce(async (id, value) => {
      publish({
        nodes: nodes.map((node) =>
          node.kind.type === "tab" && node.kind.tab_id === id
            ? { ...node, section: value ? "favorites" : "today" }
            : node,
        ),
      });
      return { accepted: true, operation_id: null };
    });
    await screen
      .getByRole("button", { name: essential ? "Add to Essentials" : "Remove from Essentials" })
      .click();
    expect(native.essential.mock.calls).toEqual([
      ["a", essential, null],
      ["b", essential, null],
    ]);
  },
);

test("projection removal prunes selection, and switching space clears it even with shared IDs", async () => {
  const { click, selected, screen, publish, snapshot } = await list();
  click("c", { shiftKey: true });
  publish({ tabs: snapshot.tabs.filter((tab) => tab.id !== "b") });
  expect(screen.container.textContent).toContain("2 tabs selected");
  publish({ tabs: snapshot.tabs.filter((tab) => tab.id === "a") });
  expect(screen.container.querySelector(".selection-bar")).toBeNull();
  publish({ active_space_id: "work" });
  expect(selected()).toEqual([]);
});

test("pinned rows and split members support selection in displayed order", async () => {
  const { click, selected, button } = await list({ section: "pinned", split: ["c", "a"] });
  expect(button("c").hasAttribute("aria-pressed")).toBe(false);
  click("c", { shiftKey: true });
  expect(selected()).toEqual(["c", "a"]);
  expect(button("c").getAttribute("aria-pressed")).toBe("true");
  expect(button("a").getAttribute("aria-pressed")).toBe("true");
  expect(button("b").getAttribute("aria-pressed")).toBe("false");
  expect(native.activate).not.toHaveBeenCalled();
});

test("collapsed folder children are skipped by ranges", async () => {
  const nodes: SidebarNodeView[] = [
    { id: "a", parent_id: null, section: "today", kind: { type: "tab", tab_id: "a" } },
    { id: "folder", parent_id: null, section: "today", kind: { type: "folder", name: "Folder" } },
    { id: "b", parent_id: "folder", section: "today", kind: { type: "tab", tab_id: "b" } },
    { id: "c", parent_id: null, section: "today", kind: { type: "tab", tab_id: "c" } },
  ];
  const { click, selected, screen } = await list({ nodes, all: open.slice(0, 3) });
  click("c", { shiftKey: true });
  expect(selected()).toEqual(["a", "b", "c"]);
  await screen.getByRole("button", { name: "Folder" }).click();
  click("c", { shiftKey: true });
  expect(selected()).toEqual(["a", "c"]);
});

test("a modified pointer gesture cannot start a reorder or swallow selection", async () => {
  const { click, button, selected, screen } = await list();
  const handle = button("b");
  const init = { bubbles: true, pointerId: 1, button: 0, isPrimary: true, ctrlKey: true };
  handle.dispatchEvent(new PointerEvent("pointerdown", { ...init, clientX: 20, clientY: 20 }));
  handle.dispatchEvent(new PointerEvent("pointermove", { ...init, clientX: 20, clientY: 80 }));
  handle.dispatchEvent(new PointerEvent("pointerup", { ...init, clientX: 20, clientY: 80 }));
  click("b", { ctrlKey: true });
  expect(selected()).toEqual(["a", "b"]);
  expect(screen.container.querySelector("[data-lifted]")).toBeNull();
  expect(native.essential).not.toHaveBeenCalled();
});

test("split candidate mode retains activation instead of selecting", async () => {
  const { click, selected } = await list({ splitting: true });
  click("b", { ctrlKey: true });
  expect(selected()).toEqual([]);
  expect(native.activate).toHaveBeenCalledExactlyOnceWith("b");
});

test("Remove from Essentials follows the authoritative section for every selected tab", async () => {
  const { click, screen, publish, snapshot } = await list();
  click("b", { ctrlKey: true });
  publish({ nodes: snapshot.nodes.map((node) => ({ ...node, section: "favorites" })) });
  await screen.getByRole("button", { name: "Remove from Essentials" }).click();
  await expect
    .poll(() => native.essential.mock.calls)
    .toEqual([
      ["a", false, null],
      ["b", false, null],
    ]);
});

test("a rejected close does not prevent closing the other selected tabs", async () => {
  const { notice } = await import("$session/notice.svelte");
  const { click, screen } = await list();
  native.close.mockRejectedValueOnce(new Error("transport"));
  click("b", { ctrlKey: true });
  await screen.getByRole("button", { name: "Close selected tabs" }).click();
  await expect.poll(() => native.close.mock.calls).toEqual([["a"], ["b"]]);
  await expect.poll(() => notice()?.text).toContain("Could not apply an action");
});
