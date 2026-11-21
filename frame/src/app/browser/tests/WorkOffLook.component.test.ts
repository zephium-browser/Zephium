import { afterEach, expect, test, vi } from "vitest";
import { page } from "vitest/browser";
import { render } from "vitest-browser-svelte";
import "$styles/global.css";
import { emitNativeEvent } from "$shared/testing/native-events";
import { favicons } from "$domain/favicons";
import { surface } from "$domain/surface";
import { tabs } from "$domain/tabs";
import { preferences } from "$domain/preferences";
import * as sidebar from "$session/sidebar-mode.svelte";
import Shell from "../Shell.svelte";
import { paint, settle, shot, wallpaper } from "./look-bench";

const native = vi.hoisted(() => ({ settings: {} as Record<string, string> }));
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
    settingGet: async (key: string) => native.settings[key] ?? null,
    settingSet: async () => ({ accepted: true, operation_id: null }),
    keymapEntries: async () => [],
    bookmarkCall: async () => ({
      kind: "listing" as const,
      folder: null,
      path: [],
      items: [
        {
          id: "bookmark",
          title: "Example bookmark",
          url: "https://example.com/",
          parent: null,
          children: 0,
          icon: null,
        },
      ],
    }),
  });
});

afterEach(() => {
  sidebar.setMode("default");
  favicons.dispose();
  surface.dispose();
  tabs.dispose();
  preferences.dispose();
  document.getElementById("wallpaper")?.remove();
});

const switchShown = () => document.querySelector(".modes, .mode-switch") !== null;

test("visibility comparisons for expanded Browse, compact Browse, Bookmarks and Work", async () => {
  native.settings = { appearance: "dark" };
  await page.viewport(900, 620);
  document.documentElement.dataset.theme = "dark";
  await paint();
  await render(Shell);
  await settle();
  const tools = await import("$session/tools.svelte");
  async function pair(name: string) {
    for (const [state, shown] of [
      ["before", true],
      ["after", false],
    ] as const) {
      emitNativeEvent("uiCommand", `preference.ui.mode-switch=${shown}`);
      await settle();
      expect(switchShown()).toBe(shown);
      expect(preferences.enabled("work")).toBe(true);
      await page
        .elementLocator(document.querySelector<HTMLElement>("aside.browser-sidebar")!)
        .screenshot({ path: `../../../../../target/look/visibility/${name}-${state}.png` });
    }
  }
  await pair("browse");
  sidebar.setMode("compact");
  await pair("compact");
  sidebar.setMode("default");
  tools.open("bookmarks");
  await expect.element(page.getByText("Example bookmark", { exact: true })).toBeVisible();
  await pair("bookmarks");
  await page.viewport(740, 600);
  await pair("bookmarks-narrow");
  tools.close();
  await page.viewport(900, 620);
  emitNativeEvent("uiCommand", "browser.work");
  await pair("work");
  emitNativeEvent("uiCommand", "browser.return");
});

for (const theme of ["light", "dark"] as const) {
  test(`the column without Work, every shape, ${theme}`, async () => {
    await page.viewport(1100, 760);
    native.settings = { appearance: theme, "work.enabled": "false" };
    document.documentElement.dataset.theme = theme;
    document.documentElement.dataset.material = "liquid_glass";
    document.body.style.background = "transparent";
    await paint();
    await render(Shell);
    wallpaper("mono", theme, theme === "light" ? "#ffffff" : "#0d1117");
    await settle();
    expect(switchShown()).toBe(false);
    await shot(`work-off-${theme}`);

    sidebar.setMode("compact");
    await settle();
    expect(switchShown()).toBe(false);
    await shot(`work-off-${theme}-rail`);

    emitNativeEvent("uiCommand", "browser.settings");
    await settle();
    await settle();
    expect(switchShown()).toBe(false);
    await shot(`work-off-${theme}-settings`);
  });
}

test("hiding the switch covers Browse, Bookmarks and Settings without disabling Work", async () => {
  native.settings = { "ui.mode-switch": "false" };
  await page.viewport(1100, 760);
  document.documentElement.dataset.theme = "dark";
  await paint();
  await render(Shell);
  await settle();
  expect(switchShown()).toBe(false);
  expect(document.querySelector(".sidebar-head")).toBeNull();
  expect(preferences.enabled("work")).toBe(true);
  await shot("mode-switch-hidden-browse");

  const tools = await import("$session/tools.svelte");
  tools.open("bookmarks");
  await settle();
  await settle();
  expect(tools.activeTool()).toBe("bookmarks");
  expect(switchShown()).toBe(false);
  await expect.element(page.getByText("Example bookmark", { exact: true })).toBeVisible();
  await shot("mode-switch-hidden-bookmarks");
  await page.viewport(740, 600);
  await settle();
  expect(switchShown()).toBe(false);
  await shot("mode-switch-hidden-bookmarks-narrow");
  tools.close();

  emitNativeEvent("uiCommand", "browser.settings");
  await settle();
  expect(switchShown()).toBe(false);
  emitNativeEvent("uiCommand", "browser.return");
  emitNativeEvent("uiCommand", "preference.ui.mode-switch=true");
  await settle();
  expect(switchShown()).toBe(true);
});
