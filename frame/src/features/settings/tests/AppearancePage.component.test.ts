import { expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { preferences } from "$domain/preferences";
import AppearancePage from "../components/sections/AppearancePage.svelte";

vi.mock("$domain/preferences", () => ({
  preferences: {
    value: (key: string) =>
      ({
        appearance: "system",
        "ui.accent": "graphite",
        "sidebar.mode": "default",
        "ui.reduce-motion": "false",
        "ui.density": "comfortable",
      })[key],
    saving: () => false,
    saveFailed: () => false,
    set: vi.fn().mockResolvedValue(undefined),
  },
}));

test("clicking a theme preview selects that appearance", async () => {
  const screen = await render(AppearancePage);
  await expect
    .element(screen.getByRole("button", { name: "Dark" }))
    .toHaveAttribute("aria-pressed", "false");
  await screen.getByRole("button", { name: "Dark" }).click();
  expect(preferences.set).toHaveBeenCalledWith("appearance", "dark");
});
