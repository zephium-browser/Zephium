import { expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { preferences } from "$domain/preferences";
import GeneralPage from "../components/sections/GeneralPage.svelte";

const state = vi.hoisted(() => ({
  values: {
    "tabs.startup": "new-tab",
    "tabs.restore-session": "true",
  } as Record<string, string>,
}));

vi.mock("$domain/preferences", () => ({
  preferences: {
    value: (key: string) => state.values[key] ?? "",
    saving: () => false,
    saveFailed: () => false,
    set: vi.fn().mockResolvedValue(undefined),
  },
}));
vi.mock("$domain/default-browser", () => ({
  defaultBrowser: {
    current: () => undefined,
    pending: () => false,
    refresh: () => {},
    request: () => {},
  },
}));

test("the session-restore switch only appears for new-tab startup", async () => {
  const screen = await render(GeneralPage);
  const label = "Reopen tabs from last session";
  const toggle = screen.getByRole("switch", { name: label });
  await expect.element(toggle).toBeVisible();
  await toggle.click();
  expect(preferences.set).toHaveBeenCalledWith("tabs.restore-session", "false");

  await screen.unmount();
  state.values["tabs.startup"] = "continue";
  const other = await render(GeneralPage);
  await expect.element(other.getByRole("switch", { name: label }).query()).toBeNull();
});
