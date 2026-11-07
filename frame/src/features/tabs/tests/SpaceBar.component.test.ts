import { afterEach, expect, test, vi } from "vitest";
import { page, userEvent } from "vitest/browser";
import { render } from "vitest-browser-svelte";
import "$styles/global.css";
import type { ItemsState, SidebarNodeView } from "$shared/ipc/bindings";
import { emitNativeEvent } from "$shared/testing/native-events";
import { revision, spaceFixture, tabFixture } from "$shared/testing/fixtures";
import { tabs } from "$domain/tabs";
import { uiCommands } from "$domain/ui-commands";
import SidebarBody from "../components/SidebarBody.svelte";
import { sidebarTree } from "../lib/sidebar-model";

const native = vi.hoisted(() => ({
  open: vi.fn(),
  switchTo: vi.fn(),
  step: vi.fn(),
  create: vi.fn(),
  update: vi.fn(),
  remove: vi.fn(),
  reorder: vi.fn(),
}));

vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  const ok = async () => ({ accepted: true, operation_id: null });
  return mockBindings({
    tabsBootstrap: async () => {},
    tabsOpen: native.open.mockImplementation(ok),
    spacesSwitch: native.switchTo.mockImplementation(ok),
    spacesStep: native.step.mockImplementation(ok),
    spacesCreate: native.create.mockImplementation(ok),
    spacesUpdate: native.update.mockImplementation(ok),
    spacesDelete: native.remove.mockImplementation(ok),
    spacesReorder: native.reorder.mockImplementation(ok),
    keymapEntries: async () => [],
  });
});

const HOME = spaceFixture("home", "Home", {
  icon: { kind: "symbol", index: 2 },
  color: 1,
  tabs: 2,
});
const STUDIO = spaceFixture("studio", "Studio", {
  icon: { kind: "emoji", text: "🎨" },
  color: 4,
  theme: { kind: "preset", id: "nord" },
  tabs: 3,
});

let projection = 100;
function project(state: Partial<ItemsState>) {
  const open = [tabFixture({ id: "a", title: "Alpha" }), tabFixture({ id: "b", title: "Beta" })];
  const node = (id: string): SidebarNodeView => ({
    id,
    parent_id: null,
    section: "today",
    kind: { type: "tab", tab_id: id },
  });
  emitNativeEvent("itemsChanged", {
    projection_revision: revision((projection += 1)),
    profile: { id: "p", name: "Personal", kind: "default" },
    spaces: [HOME, STUDIO],
    active_space_id: "home",
    nodes: open.map((tab) => node(tab.id)),
    tabs: open,
    active: "a",
    split_group: null,
    ...state,
  });
}

async function mount(empty = false) {
  await page.viewport(420, 760);
  await tabs.init();
  await uiCommands.init();
  project(empty ? { nodes: [], tabs: [], active: null } : {});
  const tree = sidebarTree(tabs.sidebarNodes(), tabs.tabs());
  return render(SidebarBody, {
    pinned: tree.pinned,
    today: tree.today,
    splitting: false,
    onSelect: () => {},
  });
}

afterEach(() => {
  tabs.dispose();
  uiCommands.dispose();
  vi.clearAllMocks();
});

test("the row names the space and its plus opens a tab, with no pager beside them", async () => {
  const screen = await mount();
  await expect.element(screen.getByRole("button", { name: "Spaces — Home" })).toBeVisible();
  await screen.getByRole("button", { name: "New tab", exact: true }).click();
  expect(native.open).toHaveBeenCalledOnce();
  expect(screen.container.querySelector(".space-dots, .space-dot")).toBeNull();
  expect(screen.container.querySelectorAll(".space-bar button")).toHaveLength(2);
});

test("the switcher lists every space, checks the one shown and moves to another", async () => {
  const screen = await mount();
  await screen.getByRole("button", { name: "Spaces — Home" }).click();
  const rows = screen.getByRole("menuitemradio");
  await expect.element(rows.first()).toHaveAttribute("aria-checked", "true");
  await expect.element(rows.nth(1)).toHaveTextContent("Studio");
  await rows.nth(1).click();
  expect(native.switchTo).toHaveBeenCalledWith("studio");
});

test("a row is a name, with the icons of its first tabs and a count of the rest, and nothing for an empty space", async () => {
  const screen = await mount();
  project({
    spaces: [
      HOME,
      spaceFixture("busy", "Busy", {
        tabs: 5,
        previews: [null, { origin: "https://a.example", revision: "a" }, null],
      }),
      spaceFixture("blank", "Blank"),
    ],
  });
  await screen.getByRole("button", { name: "Spaces — Home" }).click();
  const rows = [...screen.container.querySelectorAll<HTMLElement>("[data-space-row]")];
  expect(rows).toHaveLength(3);
  expect(rows[1]!.querySelectorAll(".switch-fav")).toHaveLength(3);
  expect(rows[1]!.querySelector(".switch-more")?.textContent).toBe("+2");
  expect(rows[2]!.querySelector(".switch-stack")).toBeNull();
  // A space no one dressed gets no mark and no tile of its own.
  expect(rows[2]!.querySelector(".space-glyph")).toBeNull();
  expect(screen.container.querySelector(".switch-tile")).toBeNull();
});

test("holding Control names the keys on every row", async () => {
  const screen = await mount();
  await screen.getByRole("button", { name: "Spaces — Home" }).click();
  const list = screen.container.querySelector<HTMLElement>(".switcher")!;
  expect(list.dataset.keys).toBeUndefined();
  window.dispatchEvent(new KeyboardEvent("keydown", { key: "Control", ctrlKey: true }));
  await expect.poll(() => list.dataset.keys).toBeDefined();
  window.dispatchEvent(new KeyboardEvent("keyup", { key: "Control", ctrlKey: false }));
  await expect.poll(() => list.dataset.keys).toBeUndefined();
});

test("a right-click on a row opens its name to be typed over", async () => {
  const screen = await mount();
  await screen.getByRole("button", { name: "Spaces — Home" }).click();
  const row = screen.container.querySelectorAll<HTMLElement>("[data-space-row]")[1]!;
  row.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true }));
  const name = screen.getByRole("textbox", { name: "Name" });
  await expect.element(name).toHaveValue("Studio");
  await expect.element(name).toHaveFocus();
});

test("a space changes as it is edited, and Done keeps it", async () => {
  const screen = await mount();
  await screen.getByRole("button", { name: "Spaces — Home" }).click();
  await screen.getByRole("button", { name: "Edit Studio" }).click();
  const look = {
    icon: { kind: "emoji", text: "🎨" },
    color: 4,
    theme: { kind: "preset", id: "nord" },
  };
  await screen.getByRole("textbox", { name: "Name" }).fill("Atelier");
  await expect.poll(() => native.update.mock.calls.at(-1)).toEqual(["studio", "Atelier", look]);
  await screen.getByRole("radio", { name: "Symbol 4" }).click();
  expect(native.update).toHaveBeenLastCalledWith("studio", "Atelier", {
    ...look,
    icon: { kind: "symbol", index: 3 },
  });
  await screen.getByRole("radio", { name: "No icon" }).click();
  await screen.getByRole("radio", { name: "Color 7" }).click();
  await screen.getByRole("radio", { name: "None", exact: true }).click();
  await expect
    .poll(() => native.update.mock.calls.at(-1))
    .toEqual(["studio", "Atelier", { icon: null, color: 6, theme: null }]);
  await screen.getByRole("button", { name: "Done" }).click();
  await expect
    .element(screen.getByRole("button", { name: "Spaces — Home" }))
    .toHaveAttribute("aria-expanded", "false");
});

test("Escape puts the space back as it was", async () => {
  const screen = await mount();
  await screen.getByRole("button", { name: "Spaces — Home" }).click();
  await screen.getByRole("button", { name: "Edit Studio" }).click();
  await screen.getByRole("radio", { name: "Color 7" }).click();
  await screen.getByRole("radio", { name: "No icon" }).click();
  await userEvent.keyboard("{Escape}");
  await expect
    .poll(() => native.update.mock.calls.at(-1))
    .toEqual([
      "studio",
      "Studio",
      { icon: { kind: "emoji", text: "🎨" }, color: 4, theme: { kind: "preset", id: "nord" } },
    ]);
  // Opened from the list, so Escape steps back to it.
  await expect.element(screen.getByRole("menuitemradio").first()).toBeVisible();
});

test("deleting asks first and says how many tabs will close", async () => {
  const screen = await mount();
  await screen.getByRole("button", { name: "Spaces — Home" }).click();
  await screen.getByRole("button", { name: "Edit Studio" }).click();
  await screen.getByRole("button", { name: "Delete…" }).click();
  await expect.element(screen.getByText("Its 3 tabs will close.")).toBeVisible();
  expect(native.remove).not.toHaveBeenCalled();
  await screen.getByRole("button", { name: "Delete Space" }).click();
  expect(native.remove).toHaveBeenCalledWith("studio");
});

test("New Space makes one at once, with no icon, and opens it to be named", async () => {
  const screen = await mount();
  await screen.getByRole("button", { name: "Spaces — Home" }).click();
  await screen.getByRole("menuitem", { name: "New Space" }).click();
  expect(native.create).toHaveBeenCalledWith("", { icon: null, color: 0, theme: null });
  project({
    spaces: [HOME, STUDIO, spaceFixture("made", "Space 3", { tabs: 1 })],
    active_space_id: "made",
  });
  const name = screen.getByRole("textbox", { name: "Name" });
  await expect.element(name).toHaveValue("Space 3");
  await expect.element(name).toHaveFocus();
  await userEvent.keyboard("Reading{Enter}");
  await expect.poll(() => native.update.mock.calls.at(-1)?.[1]).toBe("Reading");

  emitNativeEvent("uiCommand", "space.edit");
  await expect.element(screen.getByRole("textbox", { name: "Name" })).toHaveValue("Space 3");
});

test("a space with nothing listed draws no empty state; the row's plus still opens a tab", async () => {
  const screen = await mount(true);
  await expect.element(screen.getByRole("button", { name: "Spaces — Home" })).toBeVisible();
  expect(screen.container.textContent).not.toContain("Nothing open");
  await screen.getByRole("button", { name: "New tab", exact: true }).click();
  expect(native.open).toHaveBeenCalledOnce();
});

const frame = () => new Promise((resolve) => requestAnimationFrame(() => resolve(undefined)));
const sheetOf = (screen: { container: HTMLElement }) =>
  screen.container.querySelector<HTMLElement>(".space-sheet")!;
const swipe = (target: HTMLElement, deltaX: number, count = 6, deltaY = 0, natural = true) => {
  for (let index = 0; index < count; index += 1) {
    const wheel = new WheelEvent("wheel", { deltaX, deltaY, bubbles: true });
    Object.defineProperty(wheel, "webkitDirectionInvertedFromDevice", { value: natural });
    target.dispatchEvent(wheel);
  }
};

test("a sideways swipe moves the spaces as one sheet and settles on the next one", async () => {
  const screen = await mount();
  const list = screen.container.querySelector<HTMLElement>(".tab-scroller")!;
  const sheet = sheetOf(screen);
  // Fingers to the right on the first space: nothing to go to, so the sheet
  // resists and comes back.
  swipe(list, -6, 4);
  await frame();
  expect(native.step).not.toHaveBeenCalled();
  expect(sheet.style.transform).toMatch(/^translate3d\(\d/);
  await expect.poll(() => sheet.style.transform, { timeout: 2000 }).toBe("");
  await expect.poll(() => screen.container.querySelector(".peek")).toBeNull();

  // Mostly vertical: not ours.
  swipe(list, 3, 6, 20);
  await frame();
  expect(sheet.style.transform).toBe("");
  await new Promise((resolve) => setTimeout(resolve, 200));

  // Fingers to the left: the next space is already there, arriving from the edge.
  swipe(list, 30);
  expect(native.step).toHaveBeenCalledExactlyOnceWith(1);
  await frame();
  await frame();
  expect(sheet.style.transform).toMatch(/^translate3d\(-/);
  const next = screen.container.querySelector<HTMLElement>('.peek[data-sheet="next"]');
  expect(next).not.toBeNull();
  // The one being left steps back; the one arriving is brighter than it.
  expect(Number.parseFloat(list.style.opacity)).toBeLessThan(1);

  // Native has moved: the sheet carries on from where it is and rests.
  project({ spaces: [HOME, STUDIO], active_space_id: "studio" });
  await expect.poll(() => sheet.style.transform, { timeout: 2000 }).toBe("");
  expect(list.style.opacity).toBe("");
  await expect.poll(() => screen.container.querySelector(".peek")).toBeNull();
});

test("a neighbour the window has shown is drawn as it was, its own tab lit", async () => {
  const screen = await mount();
  const list = screen.container.querySelector<HTMLElement>(".tab-scroller")!;
  const studioTabs = [
    tabFixture({ id: "s1", title: "Moodboard" }),
    tabFixture({ id: "s2", title: "Palette" }),
  ];
  const node = (id: string): SidebarNodeView => ({
    id,
    parent_id: null,
    section: "today",
    kind: { type: "tab", tab_id: id },
  });
  project({
    active_space_id: "studio",
    nodes: studioTabs.map((tab) => node(tab.id)),
    tabs: studioTabs,
    active: "s2",
  });
  await frame();
  project({});
  await frame();
  swipe(list, 30);
  await frame();
  await frame();
  const next = screen.container.querySelector<HTMLElement>('.peek[data-sheet="next"]')!;
  expect(next.textContent).toContain("Moodboard");
  expect(next.querySelector('[data-selected="true"]')?.textContent).toContain("Palette");
  // A drawn copy is never a tab to native: it carries none of its markers.
  expect(next.querySelector("[data-zephium-tab-id]")).toBeNull();
  expect(next.inert).toBe(true);
});

test("a neighbour holding no tab peeks as empty until the window opens a tab in it", async () => {
  const screen = await mount();
  const list = screen.container.querySelector<HTMLElement>(".tab-scroller")!;
  project({ spaces: [HOME, spaceFixture("quiet", "Quiet", { tabs: 0 })] });
  await frame();
  swipe(list, 30);
  await frame();
  await frame();
  const next = screen.container.querySelector<HTMLElement>('.peek[data-sheet="next"]')!;
  expect(next.textContent).toContain("Nothing open in Quiet");
  expect(next.querySelector("button")).toBeNull();
});

test("a swipe the native side refuses brings the sheet back", async () => {
  native.step.mockImplementationOnce(async () => ({ accepted: false, operation_id: null }));
  const screen = await mount();
  const list = screen.container.querySelector<HTMLElement>(".tab-scroller")!;
  swipe(list, 30);
  await frame();
  await expect.poll(() => sheetOf(screen).style.transform, { timeout: 2000 }).toBe("");
});

test("a space something else switched to slides in without a sheet", async () => {
  const screen = await mount();
  project({ active_space_id: "studio" });
  await frame();
  expect(sheetOf(screen).style.transform).toMatch(/^translate3d\(/);
  expect(screen.container.querySelector(".peek")).toBeNull();
  await expect.poll(() => sheetOf(screen).style.transform, { timeout: 2000 }).toBe("");
});

test("the fingers decide the direction whichever way the wheel is set to run", async () => {
  const screen = await mount();
  const list = screen.container.querySelector<HTMLElement>(".tab-scroller")!;
  swipe(list, -30, 6, 0, false);
  expect(native.step).toHaveBeenCalledExactlyOnceWith(1);
});

test("a private window has one space and no switcher", async () => {
  const screen = await mount();
  project({ profile: { id: "x", name: "Private", kind: "incognito" }, spaces: [HOME] });
  await expect.element(screen.getByRole("button", { name: "New tab" })).toBeVisible();
  expect(screen.container.querySelector(".space-bar")).toBeNull();
});
