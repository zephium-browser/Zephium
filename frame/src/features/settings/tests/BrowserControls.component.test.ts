import "$styles/global.css";
import { beforeEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { page } from "vitest/browser";
import { preferences } from "$domain/preferences";
import BrowserControls from "../components/BrowserControls.svelte";

const state = vi.hoisted(() => ({ shown: true }));
vi.mock("$domain/preferences", () => ({
  preferences: {
    showSignInPrompts: () => state.shown,
    value: () => String(state.shown),
    saving: () => false,
    set: vi.fn().mockResolvedValue(undefined),
  },
}));
beforeEach(() => {
  state.shown = true;
  vi.mocked(preferences.set).mockClear();
});

test("the settings submit persistent visibility choices", async () => {
  const screen = await render(BrowserControls);
  await screen.getByRole("switch", { name: "Show sign-in prompts" }).click();
  expect(preferences.set).toHaveBeenLastCalledWith("ui.sign-in-prompts", "false");
  await screen.getByRole("switch", { name: "Show Browse / Work switch" }).click();
  expect(preferences.set).toHaveBeenLastCalledWith("ui.mode-switch", "false");
});

test.each([true, false])("visibility settings at normal and narrow widths (%s)", async (shown) => {
  state.shown = shown;
  const screen = await render(BrowserControls);
  screen.container.style.cssText =
    "padding:24px;max-inline-size:820px;background:var(--color-canvas)";
  for (const width of [900, 440]) {
    await page.viewport(width, 520);
    for (const theme of ["light", "dark"]) {
      document.documentElement.dataset.theme = theme;
      await page.screenshot({
        path: `../../../../../target/look/visibility/controls-${shown}-${width}-${theme}.png`,
      });
    }
  }
});
