import { afterEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { flushSync } from "svelte";
import "$styles/global.css";
import { tabs } from "$domain/tabs";
import { emitNativeEvent } from "$shared/testing/native-events";
import { revision, tabFixture } from "$shared/testing/fixtures";
import SidebarBody from "../components/SidebarBody.svelte";
import { sidebarTree } from "../lib/sidebar-model";

const native = vi.hoisted(() => ({
  activate: vi.fn(async (_id: string) => ({ accepted: true, operation_id: null })),
}));

vi.mock("$shared/platform", () => ({ IS_MAC: true, IS_WINDOWS: false }));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    tabsBootstrap: async () => {},
    tabsActivate: native.activate,
    swipeRegion: async () => false,
  });
});

afterEach(() => {
  tabs.dispose();
  vi.clearAllMocks();
});

test("Cmd-click selects on macOS while Ctrl-click retains ordinary activation", async () => {
  await tabs.init();
  const all = ["a", "b", "c"].map((id) => tabFixture({ id, title: `Tab ${id}` }));
  const tree = sidebarTree([], all);
  flushSync(() =>
    emitNativeEvent("itemsChanged", {
      projection_revision: revision(1000),
      profile: null,
      spaces: [],
      active_space_id: null,
      nodes: [],
      tabs: all,
      active: "a",
      split_group: null,
    }),
  );
  const screen = await render(SidebarBody, {
    pinned: tree.pinned,
    today: tree.today,
    splitting: false,
    onSelect: tabs.activate,
  });
  const button = (id: string) =>
    screen.container.querySelector<HTMLButtonElement>(`[data-zephium-tab-id="${id}"] .tab-open`)!;
  const click = (modifiers: MouseEventInit) => {
    button("b").dispatchEvent(new MouseEvent("click", { bubbles: true, ...modifiers }));
    flushSync();
  };

  click({ ctrlKey: true });
  expect(screen.container.querySelector('[data-multi-selected="true"]')).toBeNull();
  expect(screen.container.querySelector(".tab-open[aria-pressed]")).toBeNull();
  expect(native.activate).toHaveBeenCalledExactlyOnceWith("b");
  native.activate.mockClear();

  click({ metaKey: true });
  expect(button("a").getAttribute("aria-pressed")).toBe("true");
  expect(button("b").getAttribute("aria-pressed")).toBe("true");
  expect(button("c").getAttribute("aria-pressed")).toBe("false");
  expect(screen.container.textContent).toContain("2 tabs selected");
  click({ metaKey: true });
  expect(button("b").closest("[data-multi-selected]")).toBeNull();
  expect(screen.container.querySelector(".tab-open[aria-pressed]")).toBeNull();
  expect(screen.container.querySelector(".selection-bar")).toBeNull();
  expect(native.activate).not.toHaveBeenCalled();
});
