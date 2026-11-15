import { expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import * as m from "$shared/i18n/messages";
import NewTabPage from "../components/sections/NewTabPage.svelte";

vi.mock("$shared/platform", () => ({ SUPPORTS_BLOCKER_STATISTICS: false }));
vi.mock("$domain/preferences", () => ({
  preferences: {
    value: (key: string) => (key === "ui.newtab-background" ? "sky" : "true"),
    enabled: () => true,
    saving: () => false,
  },
}));

test("a host without a blocker counter omits the statistic setting", async () => {
  const screen = await render(NewTabPage);
  expect(screen.getByRole("switch", { name: m.ntp_trackers_blocked() }).query()).toBeNull();
  await expect.element(screen.getByRole("switch", { name: m.ntp_focused() })).toBeVisible();
});
