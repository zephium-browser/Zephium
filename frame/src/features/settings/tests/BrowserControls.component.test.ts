import "$styles/global.css";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { page } from "vitest/browser";
import { preferences } from "$domain/preferences";
import { emitNativeEvent } from "$shared/testing/native-events";
import BrowserControls from "../components/BrowserControls.svelte";

const native = vi.hoisted(() => ({
  stored: {} as Record<string, string>,
  set: vi.fn(),
}));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    settingGet: async (key) => native.stored[key] ?? null,
    searchShortcutDefaults: async () => [],
    settingSet: native.set,
  });
});
beforeEach(() => {
  native.stored = {};
  native.set.mockReset().mockImplementation(async (key: string, value: string) => {
    native.stored[key] = value;
    return { accepted: true, operation_id: null };
  });
});
afterEach(() => preferences.dispose());

const controls = [
  { key: "ui.sign-in-prompts", label: "Show sign-in prompts" },
  { key: "ui.mode-switch", label: "Show Browse / Work switch" },
] as const;

test("the settings submit persistent visibility choices", async () => {
  await preferences.init();
  const screen = await render(BrowserControls);
  for (const { key, label } of controls) {
    const control = screen.getByRole("switch", { name: label });
    for (const value of [false, true]) {
      await control.click();
      await expect.element(control).toBeEnabled();
      expect(native.set).toHaveBeenLastCalledWith(key, String(value));
      expect(preferences.value(key)).toBe(String(value));
      await expect.element(control).toHaveAttribute("aria-checked", String(value));
    }
  }
});

test.each(controls)("$key follows live stored updates", async ({ key, label }) => {
  await preferences.init();
  const screen = await render(BrowserControls);
  for (const value of [false, true]) {
    emitNativeEvent("uiCommand", `preference.${key}=${value}`);
    await expect
      .element(screen.getByRole("switch", { name: label }))
      .toHaveAttribute("aria-checked", String(value));
  }
  expect(native.set).not.toHaveBeenCalled();
});

test.each(controls.flatMap((control) => [true, false].map((stored) => ({ ...control, stored }))))(
  "$key returns to $stored after a rejected write and can retry",
  async ({ key, label, stored }) => {
    native.stored[key] = String(stored);
    native.set.mockRejectedValueOnce(new Error("write rejected"));
    await preferences.init();
    const screen = await render(BrowserControls);
    const control = screen.getByRole("switch", { name: label });
    await control.click();
    await expect.element(control).toBeEnabled();
    expect(preferences.saveFailed()).toBe(true);
    expect(preferences.value(key)).toBe(String(stored));
    await expect.element(control).toHaveAttribute("aria-checked", String(stored));
    await control.click();
    await expect.element(control).toBeEnabled();
    expect(preferences.saveFailed()).toBe(false);
    expect(preferences.value(key)).toBe(String(!stored));
    await expect.element(control).toHaveAttribute("aria-checked", String(!stored));
  },
);

test.each(controls)(
  "$key disables both controls until the write settles",
  async ({ key, label }) => {
    let finish!: () => void;
    native.set.mockImplementationOnce(async () => {
      await new Promise<void>((resolve) => {
        finish = resolve;
      });
      native.stored[key] = "false";
      return { accepted: true, operation_id: null };
    });
    await preferences.init();
    const screen = await render(BrowserControls);
    await screen.getByRole("switch", { name: label }).click();
    for (const control of controls)
      await expect.element(screen.getByRole("switch", { name: control.label })).toBeDisabled();
    expect(preferences.value(key)).toBe("true");
    finish();
    await expect.element(screen.getByRole("switch", { name: label })).toBeEnabled();
    expect(preferences.value(key)).toBe("false");
  },
);

test.each([true, false])("visibility settings at normal and narrow widths (%s)", async (shown) => {
  for (const { key } of controls) native.stored[key] = String(shown);
  await preferences.init();
  const screen = await render(BrowserControls);
  for (const { label } of controls)
    await expect
      .element(screen.getByRole("switch", { name: label }))
      .toHaveAttribute("aria-checked", String(shown));
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
