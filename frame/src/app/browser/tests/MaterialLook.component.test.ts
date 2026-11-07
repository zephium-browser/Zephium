import { afterEach, test, vi } from "vitest";
import { page } from "vitest/browser";
import { render } from "vitest-browser-svelte";
import "$styles/global.css";
import { emitNativeEvent } from "$shared/testing/native-events";
import { favicons } from "$domain/favicons";
import { surface } from "$domain/surface";
import { tabs } from "$domain/tabs";
import { preferences } from "$domain/preferences";
import { keymap } from "$domain/keymap";
import * as sidebar from "$session/sidebar-mode.svelte";
import Shell from "../Shell.svelte";
import { paint, settle, shot, wallpaper } from "./look-bench";

const native = vi.hoisted(() => ({ settings: { appearance: "light" } as Record<string, string> }));
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
    keymapEntries: async () =>
      [
        ["tool.notes", "Cmd+Shift+E"],
        ["tool.tasks", "Cmd+Shift+K"],
        ["tool.time", "Cmd+Shift+U"],
        ["tool.history", "Cmd+Y"],
        ["tool.downloads", "Cmd+Shift+J"],
        ["tool.bookmarks", "Cmd+Shift+B"],
        ["browser.settings", "Cmd+,"],
      ].map(([id, accelerator]) => ({
        id: id!,
        title: id!,
        group: "window" as const,
        accelerator: accelerator!,
        default_accelerator: accelerator!,
        customizable: true,
        customized: false,
      })),
  });
});

afterEach(() => {
  sidebar.adoptMode("default");
  favicons.dispose();
  surface.dispose();
  tabs.dispose();
  preferences.dispose();
  document.getElementById("wallpaper")?.remove();
});

for (const theme of ["light", "dark"] as const) {
  test(`chrome over glass, ${theme}`, async () => {
    await page.viewport(1100, 760);
    native.settings = { appearance: theme };
    document.documentElement.dataset.theme = theme;
    document.documentElement.dataset.material = "liquid_glass";
    document.body.style.background = "transparent";
    await paint();
    await keymap.init();
    const screen = await render(Shell);
    for (const kind of ["mono", "violet"] as const) {
      wallpaper(kind, theme, theme === "light" ? "#ffffff" : "#0d1117");
      await settle();
      await shot(`${theme}-${kind}`);
    }
    await screen.getByRole("button", { name: "Tools", exact: true }).click();
    await settle();
    await shot(`${theme}-tools`);
    await page.getByRole("button", { name: "Tools", exact: true }).click();

    emitNativeEvent("uiCommand", "browser.settings");
    await page.getByRole("button", { name: "Appearance", exact: true }).click();
    await settle();
    await settle();
    await shot(`${theme}-settings-appearance`);
    await page.getByRole("button", { name: "General", exact: true }).click();
    await settle();
    await shot(`${theme}-settings-general`);
  });
}

test("chrome opaque, light", async () => {
  await page.viewport(1100, 760);
  native.settings = { appearance: "light" };
  document.documentElement.dataset.theme = "light";
  document.documentElement.dataset.material = "none";
  await paint();
  await render(Shell);
  await settle();
  await shot("light-opaque");
});
