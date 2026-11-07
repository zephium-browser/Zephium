import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ItemsState, TabView } from "$shared/ipc/bindings";
import { spaceFixture } from "$shared/testing/fixtures";

const harness = vi.hoisted(() => {
  const order: string[] = [];
  const listeners: {
    restore?: (event: { payload: ItemsState }) => void;
    items?: (event: { payload: ItemsState }) => void;
    tab?: (event: { payload: TabView }) => void;
    presentation?: (event: { payload: { tab: TabView; active: string | null } }) => void;
  } = {};
  const stops = [vi.fn(), vi.fn(), vi.fn(), vi.fn()];

  return {
    order,
    listeners,
    stops,
    flushSync: vi.fn((callback: () => void) => callback()),
    tabsBootstrap: vi.fn(async () => {
      order.push("bootstrap");
    }),
  };
});

vi.mock("svelte", () => ({ flushSync: harness.flushSync }));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    tabsBootstrap: harness.tabsBootstrap,
    tabsOpen: vi.fn(),
    tabsActivate: vi.fn(),
    tabsClose: vi.fn(),
    tabsNavigate: vi.fn(),
    tabsReload: vi.fn(),
    tabsBack: vi.fn(),
    tabsForward: vi.fn(),
    tabsSplit: vi.fn(),
    tabsUnsplit: vi.fn(),
    sidebarSetHidden: async () => {},
    sidebarPeek: async () => {},
    sidebarSetWidth: vi.fn(),
    tabDragOver: vi.fn(),
    tabDrop: vi.fn(),
    tabMenuPopup: vi.fn(),
  });
});
vi.mock("$shared/ipc/native-events", () => ({
  events: {
    itemsChanged: {
      listen: vi.fn((listener: (event: { payload: ItemsState }) => void) => {
        harness.order.push("items");
        harness.listeners.items = listener;
        return Promise.resolve(harness.stops[0]);
      }),
    },
    tabChanged: {
      listen: vi.fn((listener: (event: { payload: TabView }) => void) => {
        harness.order.push("tab");
        harness.listeners.tab = listener;
        return Promise.resolve(harness.stops[1]);
      }),
    },
    browserReturn: {
      listen: vi.fn((listener: (event: { payload: ItemsState }) => void) => {
        harness.order.push("restore");
        harness.listeners.restore = listener;
        return Promise.resolve(harness.stops[3]);
      }),
    },
    presentationTab: {
      listen: vi.fn(
        (listener: (event: { payload: { tab: TabView; active: string | null } }) => void) => {
          harness.order.push("presentation");
          harness.listeners.presentation = listener;
          return Promise.resolve(harness.stops[2]);
        },
      ),
    },
  },
}));

function revision(value: number): string {
  return value.toString(16).padStart(32, "0");
}

function tab(id: string, value: number, title = `Tab ${id}`): TabView {
  return {
    id,
    projection_revision: revision(value),
    title,
    url: `https://${id}.example/`,
    loading: false,
    can_go_back: false,
    can_go_forward: false,
    icon: null,
  };
}

describe("Svelte tab state lifecycle", () => {
  beforeEach(() => {
    vi.resetModules();
    harness.order.length = 0;
    delete harness.listeners.items;
    delete harness.listeners.tab;
    delete harness.listeners.presentation;
    harness.flushSync.mockClear();
    harness.tabsBootstrap.mockClear();
    for (const stop of harness.stops) stop.mockClear();
  });

  it("installs every scoped projection listener before bootstrap and only once", async () => {
    const state = await import("../tabs.svelte");
    const first = state.init();
    const second = state.init();

    expect(first).toBe(second);
    expect(harness.order).toEqual(["items", "tab", "restore", "presentation"]);

    await first;
    expect(harness.order).toEqual(["items", "tab", "restore", "presentation", "bootstrap"]);

    state.dispose();
    state.dispose();
    expect(harness.stops.map((stop) => stop.mock.calls.length)).toEqual([1, 1, 1, 1]);
  });

  it("offers no close on the window's one empty new tab", async () => {
    const state = await import("../tabs.svelte");
    await state.init();
    const snapshot = (value: number, tabs: TabView[]): ItemsState => ({
      projection_revision: revision(value),
      profile: { id: "profile-a", name: "Personal", kind: "default" },
      spaces: [spaceFixture("space-a", "Main")],
      active_space_id: "space-a",
      nodes: [],
      tabs,
      active: tabs[0]?.id ?? null,
      split_group: null,
    });
    const empty = { ...tab("fresh", 1), url: null };
    harness.listeners.items?.({ payload: snapshot(2, [empty]) });
    expect(state.loneNewTab("fresh")).toBe(true);
    harness.listeners.items?.({ payload: snapshot(3, [tab("fresh", 3)]) });
    expect(state.loneNewTab("fresh")).toBe(false);
    harness.listeners.items?.({ payload: snapshot(4, [empty, tab("page", 4)]) });
    expect(state.loneNewTab("fresh")).toBe(false);
    state.dispose();
  });

  it("hands the address of the tab a menu was opened on to its command once", async () => {
    const state = await import("../tabs.svelte");
    await state.init();
    expect(state.takeMenuTargetUrl()).toBeNull();

    harness.listeners.items?.({
      payload: {
        projection_revision: revision(2),
        profile: { id: "profile-a", name: "Personal", kind: "default" },
        spaces: [spaceFixture("space-a", "Main")],
        active_space_id: "space-a",
        nodes: [],
        tabs: [tab("a", 2), tab("b", 2)],
        active: "a",
        split_group: null,
      },
    });
    state.openTabMenu("b", 10, 20);

    expect(state.takeMenuTargetUrl()).toBe("https://b.example/");
    expect(state.takeMenuTargetUrl()).toBeNull();
    state.dispose();
  });

  it("publishes presentation state inside the synchronous Svelte flush", async () => {
    const state = await import("../tabs.svelte");
    await state.init();

    const first = tab("a", 1);
    harness.listeners.items?.({
      payload: {
        projection_revision: revision(2),
        profile: {
          id: "profile-a",
          name: "Personal",
          kind: "default",
        },
        spaces: [spaceFixture("space-a", "Main")],
        active_space_id: "space-a",
        nodes: [
          {
            id: "a",
            parent_id: null,
            section: "today",
            kind: { type: "tab", tab_id: "a" },
          },
        ],
        tabs: [first],
        active: "a",
        split_group: null,
      },
    });

    const presented = tab("a", 3, "Committed");
    harness.listeners.presentation?.({ payload: { tab: presented, active: null } });

    expect(harness.flushSync).toHaveBeenCalledOnce();
    expect(state.tabs()[0]).toBe(presented);
    expect(state.activeId()).toBeNull();

    state.dispose();
  });
});
