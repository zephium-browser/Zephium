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
