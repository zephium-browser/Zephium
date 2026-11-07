import { afterEach, expect, test, vi } from "vitest";
import { page } from "vitest/browser";
import { render } from "vitest-browser-svelte";
import "$styles/global.css";
import { emitNativeEvent } from "$shared/testing/native-events";
import { favicons } from "$domain/favicons";
import { surface } from "$domain/surface";
import { tabs } from "$domain/tabs";
import { preferences } from "$domain/preferences";
import { theme } from "$domain/appearance";
import * as sidebar from "$session/sidebar-mode.svelte";
import Shell from "../Shell.svelte";
import { paint, settle, themed, wallpaper } from "./look-bench";

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
  });
});

afterEach(() => {
  sidebar.adoptMode("default");
  favicons.dispose();
  surface.dispose();
  tabs.dispose();
  preferences.dispose();
  themed("zephium");
  document.getElementById("wallpaper")?.remove();
  document.body.style.background = "";
});

const shoot = (name: string) =>
  page.screenshot({ path: `../../../../../target/look/themes/${name}.png` });

// The chrome in each theme the brief names, in both modes, over the glass and
// solid: the column and the new tab, the tools menu, and Settings' cards.
for (const id of ["zephium", "paper", "nord", "dracula", "catppuccin"])
  for (const mode of ["light", "dark"] as const)
    for (const material of ["liquid_glass", "none"] as const)
      test(`${id} ${mode} over ${material}`, async () => {
        await page.viewport(1100, 720);
        native.settings = { appearance: mode, "ui.theme": id, "ui.material": material };
        document.documentElement.dataset.theme = mode;
        document.documentElement.dataset.material = material;
        themed(id);
        await paint();
        const screen = await render(Shell);
        if (material === "liquid_glass") {
          document.body.style.background = "transparent";
          wallpaper("violet", mode, "transparent");
        }
        const name = `${id}-${mode}-${material === "none" ? "solid" : "glass"}`;
        await settle();
        await shoot(`${name}-newtab`);
        await screen.getByRole("button", { name: "Tools", exact: true }).click();
        await settle();
        await shoot(`${name}-menu`);
        await page.getByRole("button", { name: "Tools", exact: true }).click();
        emitNativeEvent("uiCommand", "browser.settings");
        await page.getByRole("button", { name: "General", exact: true }).click();
        await settle();
        await settle();
        await shoot(`${name}-settings`);
      });

test("a larger serif interface keeps every row's height", async () => {
  await page.viewport(1100, 720);
  native.settings = { appearance: "light", "ui.theme": "paper" };
  document.documentElement.dataset.theme = "light";
  document.documentElement.dataset.material = "none";
  theme.applyCustomization({
    theme: "paper",
    custom: "",
    level: "50",
    contrast: false,
    font: "serif",
    fontSize: "15",
    codeFont: "system",
  });
  await paint();
  const screen = await render(Shell);
  await settle();
  await shoot("paper-light-serif-15-sidebar");
  const row = screen.container.querySelector<HTMLElement>(".browse-tab")!;
  expect(row.getBoundingClientRect().height).toBe(36);
  emitNativeEvent("uiCommand", "browser.settings");
  await page.getByRole("button", { name: "General", exact: true }).click();
  await settle();
  await settle();
  await shoot("paper-light-serif-15-settings");
});
