import { afterEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { page } from "vitest/browser";
import { preferences } from "$domain/preferences";
import AppearancePage from "../components/sections/AppearancePage.svelte";

const native = vi.hoisted(() => ({
  settings: {} as Record<string, string>,
}));
vi.mock("$shared/platform", async (original) => ({
  ...(await original<typeof import("$shared/platform")>()),
  IS_WINDOWS: true,
  IS_MAC: false,
}));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    settingGet: async (key) => native.settings[key] ?? null,
    settingSet: async (key, value) => {
      native.settings[key] = value;
      return { accepted: true, operation_id: null };
    },
  });
});
afterEach(() => preferences.dispose());

test("Windows offers all five materials and saves the selected backdrop", async () => {
  await preferences.init();
  const screen = await render(AppearancePage);
  await screen.getByRole("button", { name: "Window material" }).click();
  expect(
    page
      .getByRole("option")
      .elements()
      .map((option) => option.textContent?.trim()),
  ).toEqual(["Automatic", "Mica", "Mica Alt", "Acrylic", "None"]);
  await page.getByRole("option", { name: "Mica Alt", exact: true }).click();
  await expect.poll(() => native.settings["ui.material"]).toBe("mica_alt");
});
