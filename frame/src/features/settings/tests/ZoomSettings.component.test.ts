import { beforeEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { page } from "vitest/browser";
import type { ZoomCall, ZoomLevelView } from "$shared/ipc/bindings";
import { preferences } from "$domain/preferences";
import PreferenceSelect from "../components/PreferenceSelect.svelte";
import ZoomLevels from "../components/ZoomLevels.svelte";

const native = vi.hoisted(() => ({
  levels: [] as ZoomLevelView[],
  calls: [] as ZoomCall[],
  failing: false,
}));

vi.mock("$domain/tabs", () => ({
  tabs: { profile: () => ({ id: "01J00000000000000000000000", kind: "default" }) },
}));
vi.mock("$domain/favicons", () => ({ favicons: { forPage: () => null } }));
vi.mock("$domain/preferences", () => ({
  preferences: {
    value: (key: string) => (key === "zoom.default" ? "125" : undefined),
    saving: () => false,
    saveFailed: () => false,
    set: vi.fn().mockResolvedValue(undefined),
  },
}));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    faviconProbe: async () => true,
    zoomCall: async (_profile: string, call: ZoomCall) => {
      native.calls.push(call);
      if (native.failing) return { kind: "error", error: "unavailable" };
      if (call.kind === "forget") native.levels = native.levels.filter((l) => l.site !== call.site);
      if (call.kind === "forget_all") native.levels = [];
      return { kind: "levels", levels: native.levels };
    },
  });
});

beforeEach(() => {
  native.levels = [];
  native.calls = [];
  native.failing = false;
});

test("the default zoom offers Chrome's levels and writes the one chosen", async () => {
  const screen = await render(PreferenceSelect, {
    id: "appearance.zoom",
    preference: "zoom.default",
  });
  const trigger = screen.getByRole("button", { name: "Default zoom" });
  await expect.element(trigger).toHaveTextContent("125%");
  await trigger.click();
  await expect.element(page.getByRole("option", { name: "50%", exact: true })).toBeVisible();
  expect(
    page
      .getByRole("option")
      .elements()
      .map((option) => option.textContent?.trim()),
  ).toEqual(["50%", "67%", "75%", "80%", "90%", "100%", "110%", "125%", "150%", "175%", "200%"]);
  await page.getByRole("option", { name: "90%", exact: true }).click();
  expect(preferences.set).toHaveBeenCalledWith("zoom.default", "90");
});

test("zoomed sites are listed with their level and reset one at a time", async () => {
  native.levels = [
    { site: "github.com", percent: 80 },
    { site: "nytimes.com", percent: 125 },
  ];
  const screen = await render(ZoomLevels);
  await expect.element(screen.getByText("github.com")).toBeVisible();
  await expect.element(screen.getByText("80%")).toBeVisible();
  await expect.element(screen.getByText("nytimes.com")).toBeVisible();

  await screen.getByRole("button", { name: "Reset zoom for github.com" }).click();
  await expect.poll(() => native.calls).toContainEqual({ kind: "forget", site: "github.com" });
  await expect.element(screen.getByText("github.com")).not.toBeInTheDocument();
  await expect.element(screen.getByText("nytimes.com")).toBeVisible();
});

test("an empty list says so, and reset all clears a full one in one call", async () => {
  const empty = await render(ZoomLevels);
  await expect.element(empty.getByText("No site has its own zoom level.")).toBeVisible();
  await expect.element(empty.getByRole("button", { name: "Reset all" })).toBeDisabled();
  await empty.unmount();

  native.levels = [{ site: "github.com", percent: 80 }];
  const screen = await render(ZoomLevels);
  await expect.element(screen.getByText("github.com")).toBeVisible();
  await screen.getByRole("button", { name: "Reset all" }).click();
  await expect.poll(() => native.calls.at(-1)).toEqual({ kind: "forget_all" });
  await expect.element(screen.getByText("No site has its own zoom level.")).toBeVisible();
});

test("a list that cannot be read says so instead of looking empty", async () => {
  native.failing = true;
  const screen = await render(ZoomLevels);
  await expect
    .element(screen.getByRole("alert"))
    .toHaveTextContent("Zoom levels could not be loaded.");
  await expect.element(screen.getByText("No site has its own zoom level.")).not.toBeInTheDocument();
});
