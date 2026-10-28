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
import * as launch from "$session/motion.svelte";
import * as sidebar from "$session/sidebar-mode.svelte";
import Shell from "../Shell.svelte";

const native = vi.hoisted(() => ({
  overlay: vi.fn(async (_raised: boolean, _width: number | null): Promise<boolean | null> => true),
  run: vi.fn(async (_id: string) => ({ accepted: true, operation_id: null })),
}));
vi.mock("$shared/platform", () => ({ IS_MAC: true, IS_WINDOWS: false }));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    tabsBootstrap: async () => {},
    sidebarSetWidth: async () => {},
    sidebarSetHidden: async () => {},
    sidebarOverlay: native.overlay,
    sidebarResize: async () => true,
    sidebarResizeGuide: async () => true,
    settingSet: async () => ({ accepted: true, operation_id: null }),
    pagePermissionCardHost: async () => {},
    runCommand: native.run,
  });
});
afterEach(() => {
  if (sidebar.isHidden()) sidebar.toggleHidden();
  sidebar.adoptMode("default");
  surface.dispose();
  tabs.dispose();
});
const edge = (inside: boolean) =>
  window.dispatchEvent(new CustomEvent("zephium:sidebar-edge", { detail: { inside } }));
const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

async function hiddenShell(url: string | null = "https://example.com/", id = "a") {
  native.overlay.mockClear();
  await page.viewport(900, 700);
  launch.dispose();
  await surface.init();
  await tabs.init();
  const all = [tabFixture({ id, title: "A", url })];
  emitNativeEvent("itemsChanged", {
    projection_revision: revision(Date.now()),
    profile: null,
    spaces: [],
    active_space_id: null,
    nodes: all.map((tab) => ({
      id: tab.id,
      parent_id: null,
      section: "today",
      kind: { type: "tab", tab_id: tab.id },
    })) as SidebarNodeView[],
    tabs: all,
    active: id,
    split_group: null,
  });
  flushSync();
  render(Shell);
  await sleep(100);
  sidebar.toggleHidden();
}

// Raising answers where the pointer was when the column stood up; a
// departure native reports after that answer is newer, and must win.
test("a pointer that leaves while the column rises sends it away", async () => {
  await hiddenShell();
  let answer: (onSide: boolean | null) => void = () => {};
  native.overlay.mockReturnValueOnce(new Promise((resolve) => (answer = resolve)));
  edge(true);
  edge(false);
  answer(true);
  await expect.poll(() => native.overlay.mock.calls.at(-1)?.[0]).toBe(false);
  expect(sidebar.isPeeking()).toBe(false);
});

// The pointer leaves the edge strip on its way into the column, which native
// does not report; the column stays for the pointer that arrived on it.
test("a pointer that comes onto the rising column keeps it", async () => {
  await hiddenShell();
  let answer: (onSide: boolean | null) => void = () => {};
  native.overlay.mockReturnValueOnce(new Promise((resolve) => (answer = resolve)));
  edge(true);
  answer(true);
  await expect.poll(() => sidebar.isPeeking()).toBe(true);
  await sleep(300);
  expect(sidebar.isPeeking()).toBe(true);
  expect(native.overlay.mock.calls.some(([raised]) => !raised)).toBe(false);
});

// Open popups vanish with the column, so they hold it up; an expanded folder
// stays open, and holding for it would keep the column up for good.
test("an open popup holds the floating column, an expanded folder does not", async () => {
  await hiddenShell();
  const aside = document.querySelector<HTMLElement>(".browser-sidebar")!;
  const folder = document.createElement("button");
  folder.setAttribute("aria-expanded", "true");
  aside.append(folder);
  edge(true);
  await expect.poll(() => sidebar.isPeeking()).toBe(true);
  edge(false);
  await expect.poll(() => sidebar.isPeeking()).toBe(false);
  folder.remove();

  const popup = document.createElement("button");
  popup.setAttribute("aria-haspopup", "menu");
  popup.setAttribute("aria-expanded", "true");
  aside.append(popup);
  edge(true);
  await expect.poll(() => sidebar.isPeeking()).toBe(true);
  edge(false);
  await sleep(300);
  expect(sidebar.isPeeking()).toBe(true);
  popup.remove();
});

// Without a page to edit in the launcher, Open Location wants the field, which
// takes no focus while the column is away.
test("Open Location on a page-less tab floats the column up and focuses its field", async () => {
  await hiddenShell(null, "blank");
  const { uiCommands } = await import("$domain/ui-commands");
  await uiCommands.init();
  emitNativeEvent("uiCommand", "url.focus");
  await expect.poll(() => native.overlay.mock.calls.at(-1)?.[0]).toBe(true);
  await expect
    .poll(
      () =>
        document.activeElement?.closest(".browser-sidebar") !== null &&
        document.activeElement instanceof HTMLInputElement,
    )
    .toBe(true);
  expect(sidebar.isPeeking()).toBe(true);
});

// A rail draws no field, so a hidden rail answers with the launcher rather
// than floating up to focus a field nobody can see.
test("Open Location on a page-less tab opens the launcher from a hidden rail", async () => {
  await hiddenShell(null, "blank");
  sidebar.adoptMode("compact");
  native.run.mockClear();
  const { uiCommands } = await import("$domain/ui-commands");
  await uiCommands.init();
  emitNativeEvent("uiCommand", "url.focus");
  await expect.poll(() => native.run.mock.calls.at(-1)?.[0]).toBe("launcher.toggle");
  expect(native.overlay.mock.calls.some(([raised]) => raised)).toBe(false);
  expect(sidebar.isPeeking()).toBe(false);
});
