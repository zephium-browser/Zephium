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
import { preferences } from "$domain/preferences";

vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    pagePermissionCardHost: async () => {},
    tabsBootstrap: async () => {},
    sidebarSetHidden: async () => {},
    sidebarPeek: async () => {},
    sidebarSetWidth: async () => {},
    sidebarResize: async () => false,
    sidebarResizeGuide: async () => true,
    settingSet: async () => ({ accepted: true, operation_id: null }),
    settingGet: async () => null,
  });
});

afterEach(() => {
  sidebar.setMode("default");
  surface.dispose();
  tabs.dispose();
  preferences.dispose();
});

const open = ["a", "b", "c"].map((id) => tabFixture({ id, title: `Tab ${id}` }));

async function shell(position: "left" | "right" = "left") {
  await page.viewport(900, 700);
  launch.dispose();
  await surface.init();
  await preferences.init();
  emitNativeEvent("uiCommand", `preference.sidebar.position=${position}`);
  await tabs.init();
  const nodes: SidebarNodeView[] = open.map((tab) => ({
    id: tab.id,
    parent_id: null,
    section: "today",
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
    split_group: null,
  });
  flushSync();
  return render(Shell);
}

test.each(["left", "right"] as const)(
  "collapsing at the %s edge carries every mark into the rail and lets the list go as a ghost",
  async (position) => {
    await shell(position);
    sidebar.setMode("compact");
    flushSync();

    // The column's width travels rather than jumps, so the page area beside it
    // moves with the change instead of re-centring in one frame.
    const aside = document.querySelector<HTMLElement>(".browser-sidebar")!;
    expect(aside.dataset.reshaping).toBe("true");
    expect(
      aside
        .getAnimations()
        .some((animation) => (animation as CSSTransition).transitionProperty === "width"),
    ).toBe(true);

    const ghosts = document.querySelectorAll<HTMLElement>(
      ".sidebar-columns > .sidebar-browser-column[aria-hidden='true']",
    );
    expect(ghosts).toHaveLength(1);
    const ghost = ghosts[0]!;
    expect(ghost.inert).toBe(true);
    // Native resolves the address and every tab through these; the ghost must
    // be invisible to it.
    expect(ghost.querySelector("[data-zephium-address], [data-zephium-tab-id]")).toBeNull();
    expect(document.querySelectorAll("[data-zephium-address]")).toHaveLength(1);

    const rail = document.querySelector<HTMLElement>(
      ".sidebar-columns > .sidebar-browser-column:not([aria-hidden])",
    )!;
    for (const id of ["a", "b", "c"]) {
      const item = rail.querySelector<HTMLElement>(`[data-motion-key="tab:${id}"]`)!;
      expect(item.getAnimations().length).toBeGreaterThan(0);
    }
    await expect.poll(() => ghost.isConnected, { timeout: 2000 }).toBe(false);
  },
);

test.each(["left", "right"] as const)(
  "expanding at the %s edge carries the rail back out into the list",
  async (position) => {
    await shell(position);
    sidebar.setMode("compact");
    flushSync();
    await new Promise((resolve) => setTimeout(resolve, 500));

    sidebar.setMode("default");
    flushSync();
    const list = document.querySelector<HTMLElement>(
      ".sidebar-columns > .sidebar-browser-column:not([aria-hidden])",
    )!;
    const row = list.querySelector<HTMLElement>(`[data-motion-key="tab:b"]`)!;
    expect(row.getAnimations().length).toBeGreaterThan(0);
    // The name is new to this shape and follows its mark in.
    expect(row.querySelector(".tab-label")!.getAnimations().length).toBeGreaterThan(0);
  },
);

test("nothing in the new shape moves again when the change of shape ends", async () => {
  await shell();
  sidebar.setMode("compact");
  flushSync();
  const rail = document.querySelector<HTMLElement>(
    ".sidebar-columns > .sidebar-browser-column:not([aria-hidden])",
  )!;
  // Layout positions: offsets ignore the transforms a flight is drawn with.
  const top = (element: HTMLElement) => {
    let y = 0;
    for (let node: HTMLElement | null = element; node; node = node.offsetParent as HTMLElement) {
      y += node.offsetTop;
    }
    return y;
  };
  const resting = () =>
    [...rail.querySelectorAll<HTMLElement>("[data-motion-key], footer.dock")].map(top);
  const during = resting();
  const dock = rail.querySelector<HTMLElement>("footer.dock")!.getBoundingClientRect();
  const column = rail.getBoundingClientRect();
  // The dock keeps to the foot of the column throughout.
  expect(Math.abs(dock.bottom - column.bottom)).toBeLessThan(2);

  await expect
    .poll(() => document.querySelector(".browser-sidebar")!.getAttribute("data-reshaping"), {
      timeout: 2000,
    })
    .toBe("false");
  expect(resting()).toEqual(during);
});
