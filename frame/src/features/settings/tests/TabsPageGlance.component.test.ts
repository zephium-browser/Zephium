import { expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import * as m from "$shared/i18n/messages";
import TabsPage from "../components/sections/TabsPage.svelte";

vi.mock("$shared/platform", async (original) => ({
  ...(await original<typeof import("$shared/platform")>()),
  IS_WINDOWS: true,
}));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings();
});

test("the Glance group and its setting show on Windows", async () => {
  const screen = await render(TabsPage);

  await expect
    .element(screen.getByRole("heading", { name: m.tabs_group_archive(), exact: true }))
    .toBeVisible();
  const heading = screen.getByRole("heading", { name: m.tabs_group_glance(), exact: true }).query();
  const setting = screen.container.querySelector("[data-setting='tabs.glance']");
  expect(heading).not.toBeNull();
  expect(setting).not.toBeNull();
});
