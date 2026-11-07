import "$styles/global.css";
import { afterEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { surface } from "$domain/surface";
import { tabs } from "$domain/tabs";
import * as tools from "$session/tools.svelte";
import Shell from "../Shell.svelte";

vi.mock("$shared/platform", () => ({ IS_MAC: false, IS_WINDOWS: true }));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    pagePermissionCardHost: async () => {},
    tabsBootstrap: async () => {},
    sidebarSetHidden: async () => {},
    sidebarPeek: async () => {},
    sidebarSetWidth: async () => {},
  });
});

afterEach(() => {
  tools.close();
  surface.dispose();
  tabs.dispose();
  document.documentElement.removeAttribute("data-material");
});

test("acrylic retains its color wash through the transparent material reset", () => {
  const root = document.documentElement;
  root.setAttribute("data-material", "acrylic");
  const acrylic = getComputedStyle(document.body).backgroundColor;
  expect(acrylic).not.toBe("rgba(0, 0, 0, 0)");
  expect(acrylic).toMatch(/(?:0\.28|28%)/);
  root.setAttribute("data-material", "mica");
  expect(getComputedStyle(document.body).backgroundColor).toMatch(/(?:0\.2\b|20%)/);
  root.setAttribute("data-material", "vibrancy");
  expect(getComputedStyle(document.body).backgroundColor).toBe("rgba(0, 0, 0, 0)");
  root.setAttribute("data-material", "none");
  expect(getComputedStyle(document.body).backgroundColor).not.toBe("rgba(0, 0, 0, 0)");
});

test("the Windows tool rail puts mode above search without reserving a page title band", async () => {
  await surface.init();
  await tabs.init();
  tools.open("downloads");
  const screen = await render(Shell);
  const browse = screen.getByRole("button", { name: "Browse", exact: true });
  const search = screen.getByRole("button", { name: "Search or enter an address", exact: true });
  await expect.element(browse).toBeVisible();
  await expect.element(search).toBeVisible();
  const modeRect = browse.element().getBoundingClientRect();
  const searchRect = search.element().getBoundingClientRect();
  expect(modeRect.bottom).toBeLessThanOrEqual(searchRect.top);
  expect(screen.container.querySelector(".title-band")).toBeNull();
  const stage = screen.container.querySelector(".stage-column")!;
  expect(stage.getBoundingClientRect().top).toBe(8);
});
