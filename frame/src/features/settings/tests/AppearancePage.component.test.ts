import { beforeEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { userEvent } from "vitest/browser";
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

beforeEach(() => vi.mocked(preferences.set).mockClear());

test("the previews are the color scheme choice", async () => {
  const screen = await render(AppearancePage);
  const group = screen.getByRole("radiogroup", { name: "Appearance" });
  await expect.element(group.getByRole("radio", { name: "System" })).toBeChecked();
  await expect.element(group.getByRole("radio", { name: "Dark" })).not.toBeChecked();
  await group.getByRole("radio", { name: "Dark" }).click();
  expect(preferences.set).toHaveBeenCalledExactlyOnceWith("appearance", "dark");
  // The row that offered the same three choices again is gone.
  expect(screen.container.querySelectorAll('[role="radiogroup"], [role="tablist"]')).toHaveLength(
    1,
  );
});

test("arrow keys move through the schemes and select them", async () => {
  const screen = await render(AppearancePage);
  const system = screen.getByRole("radio", { name: "System" });
  await expect.element(system).toHaveAttribute("tabindex", "0");
  await expect
    .element(screen.getByRole("radio", { name: "Light" }))
    .toHaveAttribute("tabindex", "-1");
  (system.element() as HTMLElement).focus();
  await userEvent.keyboard("{ArrowLeft}");
  expect(preferences.set).toHaveBeenLastCalledWith("appearance", "dark");
  await expect.element(screen.getByRole("radio", { name: "Dark" })).toHaveFocus();
  await userEvent.keyboard("{ArrowRight}{ArrowRight}");
  expect(preferences.set).toHaveBeenLastCalledWith("appearance", "light");
});
