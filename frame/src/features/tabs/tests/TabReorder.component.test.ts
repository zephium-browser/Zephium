import { afterEach, expect, test, vi } from "vitest";
import { page } from "vitest/browser";
import { render } from "vitest-browser-svelte";
import { flushSync } from "svelte";
import "$styles/global.css";
import type { SidebarNodeView } from "$shared/ipc/bindings";
import { emitNativeEvent } from "$shared/testing/native-events";
import { revision, tabFixture } from "$shared/testing/fixtures";
import { tabs } from "$domain/tabs";
import TabList from "../components/TabList.svelte";
import { sidebarTree } from "../lib/sidebar-model";
import { TabSelection } from "../lib/tab-selection.svelte";

const native = vi.hoisted(() => ({
  leaveSplit: vi.fn(async () => ({ accepted: true, operation_id: null })),
  setEssential: vi.fn(async () => ({ accepted: true, operation_id: null })),
  split: vi.fn(async () => ({ accepted: true, operation_id: null })),
  dragOver: vi.fn(async () => undefined),
}));

vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    tabsBootstrap: async () => {},
    tabsLeaveSplit: native.leaveSplit,
    tabsSetEssential: native.setEssential,
    tabsSplit: native.split,
    tabDragOver: native.dragOver,
    tabDrop: async () => ({ accepted: true, operation_id: null }),
  });
});

vi.mock("$domain/operations", () => ({
  settle: async () => ({ outcome: "succeeded" }),
}));

afterEach(() => {
  tabs.dispose();
  native.leaveSplit.mockClear();
  native.setEssential.mockClear();
  native.dragOver.mockClear();
});

const open = ["a", "b", "c", "d"].map((id) => tabFixture({ id, title: `Tab ${id}` }));

async function list(
  section: "today" | "pinned" = "today",
  onSelect: (id: string) => void = () => {},
  split: string[] = [],
  selection?: TabSelection,
) {
  await page.viewport(400, 400);
  await tabs.init();
  const nodes: SidebarNodeView[] = open.map((tab) => ({
    id: tab.id,
    parent_id: null,
    section,
    kind: { type: "tab", tab_id: tab.id },
  }));
  emitNativeEvent("itemsChanged", {
    projection_revision: revision(Date.now()),
    profile: null,
    spaces: [],
    active_space_id: null,
    nodes,
    tabs: open,
    active: "a",
    split_group: split.length > 0 ? { members: split } : null,
  });
  flushSync();
  const tree = sidebarTree(nodes, open);
  const screen = await render(TabList, {
    entries: tree[section],
    section,
    label: "Open tabs",
    splitting: false,
    onSelect,
    selection,
  });
  const row = (id: string) =>
    screen.container.querySelector<HTMLElement>(`.browse-tab[data-zephium-tab-id="${id}"]`)!;
  // The sidebar's scroller, whose room past the last row is still the list's.
  screen.container.setAttribute("data-tabs-drop", "");
  screen.container.style.height = "380px";
  return { row, screen };
}

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
  return { init, handle };
}

const frame = () => new Promise((resolve) => requestAnimationFrame(() => resolve(undefined)));

test("dragging a row down its list opens a slot and drops it there", async () => {
  const { row } = await list();
  const handle = row("a").querySelector<HTMLElement>(".tab-open")!;
  const target = row("c").getBoundingClientRect();
  const { init } = drag(handle, { x: target.left + 20, y: target.bottom - 4 });
  await frame();

  // The row waits in the slot its drop would take, a card of it is with the
  // pointer, and the rows it passed have moved up to make room.
  expect(row("a").hasAttribute("data-lifted")).toBe(true);
  expect(row("a").style.transform).toContain("translateY(");
  expect(row("b").style.transform).toContain("translateY(-");
  expect(row("c").style.transform).toContain("translateY(-");
  expect(row("d").style.transform).toBe("");
  expect(document.querySelector(".drag-card")).not.toBeNull();

  handle.dispatchEvent(
    new PointerEvent("pointerup", {
      ...init,
      clientX: target.left + 20,
      clientY: target.bottom - 4,
    }),
  );
  await expect.poll(() => native.setEssential.mock.calls.length).toBe(1);
  expect(native.setEssential).toHaveBeenCalledWith("a", false, "d");
  expect(native.split).not.toHaveBeenCalled();
});

test("letting go where it started moves nothing and puts every row back", async () => {
  const { row } = await list();
  const handle = row("b").querySelector<HTMLElement>(".tab-open")!;
  const home = row("b").getBoundingClientRect();
  const { init } = drag(handle, { x: home.left + 20, y: home.top + home.height / 2 + 4 });
  await frame();
  handle.dispatchEvent(
    new PointerEvent("pointerup", { ...init, clientX: home.left + 20, clientY: home.top + 20 }),
  );
  expect(native.setEssential).not.toHaveBeenCalled();
  await expect.poll(() => row("b").hasAttribute("data-lifted")).toBe(false);
  await expect.poll(() => row("b").style.transform).toBe("");
});

test("letting go in the room below the last row drops the tab at the end", async () => {
  const { row, screen } = await list();
  const handle = row("a").querySelector<HTMLElement>(".tab-open")!;
  const room = screen.container.getBoundingClientRect();
  const to = { x: room.left + 40, y: room.bottom - 8 };
  const { init } = drag(handle, to);
  await frame();
  expect(row("b").style.transform).toContain("translateY(-");
  expect(row("d").style.transform).toContain("translateY(-");

  handle.dispatchEvent(new PointerEvent("pointerup", { ...init, clientX: to.x, clientY: to.y }));
  await expect.poll(() => native.setEssential.mock.calls.length).toBe(1);
  expect(native.setEssential).toHaveBeenCalledWith("a", false, null);
});

test("dropping a split member below the list detaches it and moves it to the end", async () => {
  const { row, screen } = await list("today", undefined, ["a", "b"]);
  const handle = row("a").querySelector<HTMLElement>(".tab-open")!;
  const room = screen.container.getBoundingClientRect();
  const to = { x: room.left + 40, y: room.bottom - 8 };
  const { init } = drag(handle, to);
  await frame();

  handle.dispatchEvent(new PointerEvent("pointerup", { ...init, clientX: to.x, clientY: to.y }));
  expect(native.leaveSplit).toHaveBeenCalledExactlyOnceWith("a");
  await expect.poll(() => native.setEssential.mock.calls.length).toBe(1);
  expect(native.setEssential).toHaveBeenCalledWith("a", false, null);
});

test("a tab carried out of its list shows a card with its title, and the far half of the last row means after it", async () => {
  const { row } = await list("pinned");
  const handle = row("a").querySelector<HTMLElement>(".tab-open")!;
  const last = row("d").getBoundingClientRect();
  const upper = { x: last.left + 40, y: last.top + 4 };
  const { init } = drag(handle, upper);
  await frame();
  const card = document.querySelector<HTMLElement>(".drag-card");
  expect(card?.textContent).toContain("Tab a");
  const held = card!.getBoundingClientRect();
  expect(held.left).toBeGreaterThanOrEqual(0);
  expect(held.top).toBeGreaterThanOrEqual(0);

  handle.dispatchEvent(
    new PointerEvent("pointerup", { ...init, clientX: upper.x, clientY: upper.y }),
  );
  await expect.poll(() => native.setEssential.mock.calls.length).toBe(1);
  expect(native.setEssential).toHaveBeenLastCalledWith("a", false, "d");

  const lower = { x: last.left + 40, y: last.bottom - 4 };
  drag(handle, lower);
  await frame();
  handle.dispatchEvent(
    new PointerEvent("pointerup", { ...init, clientX: lower.x, clientY: lower.y }),
  );
  await expect.poll(() => native.setEssential.mock.calls.length).toBe(2);
  expect(native.setEssential).toHaveBeenLastCalledWith("a", false, null);
});

test("window blur, Escape, and lost capture clear the drag card and native overlay", async () => {
  const { row } = await list();
  const handle = row("a").querySelector<HTMLElement>(".tab-open")!;
  const target = row("c").getBoundingClientRect();
  drag(handle, { x: target.left + 20, y: target.bottom - 4 });
  await frame();
  expect(document.querySelector(".drag-card")).not.toBeNull();

  window.dispatchEvent(new Event("blur"));
  await expect.poll(() => document.querySelector(".drag-card")).toBeNull();
  expect(document.body.style.cursor).toBe("");
  expect(row("a").hasAttribute("data-lifted")).toBe(false);
  expect(native.setEssential).not.toHaveBeenCalled();
  expect(native.dragOver).toHaveBeenLastCalledWith(null, null);

  native.dragOver.mockClear();
  drag(handle, { x: target.left + 20, y: target.bottom - 4 });
  await frame();
  expect(document.querySelector(".drag-card")).not.toBeNull();

  window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
  await expect.poll(() => document.querySelector(".drag-card")).toBeNull();
  expect(native.dragOver).toHaveBeenLastCalledWith(null, null);

  native.dragOver.mockClear();
  drag(handle, { x: target.left + 20, y: target.bottom - 4 });
  await frame();
  expect(document.querySelector(".drag-card")).not.toBeNull();

  handle.dispatchEvent(new PointerEvent("lostpointercapture", { pointerId: 1 }));
  await expect.poll(() => document.querySelector(".drag-card")).toBeNull();
  expect(native.dragOver).toHaveBeenLastCalledWith(null, null);
});

test("releasing outside the row before crossing the drag threshold cancels the press", async () => {
  const { row } = await list();
  const handle = row("a").querySelector<HTMLElement>(".tab-open")!;
  const from = handle.getBoundingClientRect();
  const init = { bubbles: true, pointerId: 1, button: 0, isPrimary: true };
  handle.dispatchEvent(
    new PointerEvent("pointerdown", { ...init, clientX: from.left + 20, clientY: from.top + 10 }),
  );
  window.dispatchEvent(
    new PointerEvent("pointerup", { ...init, clientX: from.right + 40, clientY: from.top + 10 }),
  );
  handle.dispatchEvent(
    new PointerEvent("pointermove", { ...init, clientX: from.left + 20, clientY: from.top + 40 }),
  );
  await frame();
  expect(document.querySelector(".drag-card")).toBeNull();
});

test("unmounting the tab list while dragging cleans up the floating card and cursor", async () => {
  const { row, screen } = await list();
  const handle = row("a").querySelector<HTMLElement>(".tab-open")!;
  const target = row("c").getBoundingClientRect();
  drag(handle, { x: target.left + 20, y: target.bottom - 4 });
  await frame();
  expect(document.querySelector(".drag-card")).not.toBeNull();

  await screen.unmount();
  expect(document.querySelector(".drag-card")).toBeNull();
  expect(document.body.style.cursor).toBe("");
  expect(native.dragOver).toHaveBeenLastCalledWith(null, null);
});

test("Escape-cancelled drag suppresses the trailing click without consuming a later press", async () => {
  const onSelect = vi.fn();
  const { row } = await list("today", onSelect);
  const handle = row("b").querySelector<HTMLElement>(".tab-open")!;
  const target = row("c").getBoundingClientRect();
  const { init } = drag(handle, { x: target.left + 20, y: target.bottom - 4 });
  await frame();

  window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
  await expect.poll(() => document.querySelector(".drag-card")).toBeNull();

  handle.dispatchEvent(new PointerEvent("pointerup", { ...init, clientX: 20, clientY: 20 }));
  handle.click();
  expect(onSelect).not.toHaveBeenCalled();

  const home = handle.getBoundingClientRect();
  handle.dispatchEvent(
    new PointerEvent("pointerdown", { ...init, clientX: home.left + 10, clientY: home.top + 10 }),
  );
  handle.dispatchEvent(
    new PointerEvent("pointerup", { ...init, clientX: home.left + 10, clientY: home.top + 10 }),
  );
  handle.click();
  expect(onSelect).toHaveBeenCalledExactlyOnceWith("b");
});

test("movement from another pointer does not hijack or cancel an active drag", async () => {
  const { row } = await list();
  const handle = row("a").querySelector<HTMLElement>(".tab-open")!;
  const target = row("c").getBoundingClientRect();
  const { init } = drag(handle, { x: target.left + 20, y: target.bottom - 4 });
  await frame();
  expect(document.querySelector(".drag-card")).not.toBeNull();

  handle.dispatchEvent(
    new PointerEvent("pointermove", {
      ...init,
      pointerId: 2,
      clientX: target.left + 20,
      clientY: target.top,
    }),
  );
  await frame();
  expect(document.querySelector(".drag-card")).not.toBeNull();

  handle.dispatchEvent(
    new PointerEvent("pointerup", {
      ...init,
      clientX: target.left + 20,
      clientY: target.bottom - 4,
    }),
  );
});

test.each([
  { type: "pointerdown", shiftKey: false },
  { type: "pointerdown", shiftKey: true },
  { type: "pointerup", shiftKey: false },
  { type: "pointercancel", shiftKey: false },
])(
  "$type from another pointer leaves the original drag in control (Shift: $shiftKey)",
  async ({ type, shiftKey }) => {
    const { row } = await list("today", undefined, [], new TabSelection());
    const handle = row("a").querySelector<HTMLElement>(".tab-open")!;
    const target = row("c").getBoundingClientRect();
    const point = { clientX: target.left + 20, clientY: target.bottom - 4 };
    const { init } = drag(handle, { x: point.clientX, y: point.clientY });
    await frame();
    expect(document.querySelector(".drag-card")).not.toBeNull();

    handle.dispatchEvent(new PointerEvent(type, { ...init, ...point, pointerId: 2, shiftKey }));
    await frame();
    expect(document.querySelector(".drag-card")).not.toBeNull();
    expect(native.setEssential).not.toHaveBeenCalled();

    handle.dispatchEvent(new PointerEvent("pointerup", { ...init, ...point }));
    await expect.poll(() => document.querySelector(".drag-card")).toBeNull();
    expect(native.setEssential).toHaveBeenCalledExactlyOnceWith("a", false, "d");
  },
);

test("unmounting an idle list does not clear another list's active drag", async () => {
  const active = await list();
  const idle = await list("pinned");
  const handle = active.row("a").querySelector<HTMLElement>(".tab-open")!;
  const target = active.row("c").getBoundingClientRect();
  drag(handle, { x: target.left + 20, y: target.bottom - 4 });
  await frame();
  expect(document.querySelector(".drag-card")).not.toBeNull();
  native.dragOver.mockClear();

  await idle.screen.unmount();
  expect(document.querySelector(".drag-card")).not.toBeNull();
  expect(document.body.style.cursor).toBe("grabbing");
  expect(native.dragOver).not.toHaveBeenCalledWith(null, null);

  await active.screen.unmount();
  expect(document.querySelector(".drag-card")).toBeNull();
  expect(document.body.style.cursor).toBe("");
});
