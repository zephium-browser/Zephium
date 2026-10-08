import { afterEach, expect, test, vi } from "vitest";
import { page, userEvent } from "vitest/browser";
import { render } from "vitest-browser-svelte";
import "$styles/global.css";
import { emitNativeEvent } from "$shared/testing/native-events";
import { revision, tabFixture } from "$shared/testing/fixtures";
import { favicons } from "$domain/favicons";
import { surface } from "$domain/surface";
import { tabs } from "$domain/tabs";
import * as sidebar from "$session/sidebar-mode.svelte";
import * as launch from "$session/motion.svelte";
import Shell from "../Shell.svelte";

vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    tabsBootstrap: async () => {},
    sidebarSetWidth: async () => {},
    sidebarResize: async () => false,
    sidebarResizeGuide: async () => true,
    settingSet: async () => ({ accepted: true, operation_id: null }),
  });
});

afterEach(() => {
  sidebar.adoptMode("default");
  favicons.dispose();
  surface.dispose();
  tabs.dispose();
});

const settle = () => new Promise((resolve) => setTimeout(resolve, 450));

// While a page is shown the chrome WebView is exactly the column's width. A
// document wider than that view can be scrolled sideways by WebKit's focus
// reveal, which slid the whole column left when the address field took focus.
test("the column's document cannot scroll sideways in a column-wide view", async () => {
  await page.viewport(240, 720);
  document.documentElement.dataset.theme = "dark";
  launch.dispose();
  await surface.init();
  await tabs.init();
  await favicons.init();
  const open = tabFixture({
    id: "open",
    title: "Example",
    url: "https://www.example.com/a/long/path",
  });
  emitNativeEvent("itemsChanged", {
    projection_revision: revision(20),
    profile: null,
    spaces: [],
    active_space_id: null,
    nodes: [
      {
        id: open.id,
        parent_id: null,
        section: "today" as const,
        kind: { type: "tab" as const, tab_id: open.id },
      },
    ],
    tabs: [open],
    active: "open",
    split_group: null,
  });
  render(Shell);
  await settle();
  await userEvent.click(
    document.querySelector<HTMLInputElement>("aside input[data-zephium-address]")!,
  );
  await settle();

  const root = document.scrollingElement!;
  expect(root.scrollWidth).toBe(root.clientWidth);
  root.scrollLeft = 20;
  const shell = document.querySelector<HTMLElement>(".shell")!;
  shell.scrollLeft = 20;
  expect(root.scrollLeft).toBe(0);
  expect(shell.scrollLeft).toBe(0);
});
