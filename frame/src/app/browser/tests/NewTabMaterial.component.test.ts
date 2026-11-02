import "$styles/global.css";
import { afterEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { emitNativeEvent } from "$shared/testing/native-events";
import { tabFixture, revision, spaceFixture } from "$shared/testing/fixtures";
import { tabs } from "$domain/tabs";
import { surface } from "$domain/surface";
import { preferences } from "$domain/preferences";
import { adoptMode } from "$session/sidebar-mode.svelte";
import { retryImage } from "$session/newtab-background.svelte";
import Shell from "../Shell.svelte";

vi.mock("../WorkWorkspace.svelte", async () => await import("./StageStub.svelte"));
vi.mock("$domain/resources", async (original) => ({
  ...(await original<Record<string, unknown>>()),
  mediaUrl: () => "/src/shared/sky/assets/sky-day.avif",
}));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    pagePermissionCardHost: async () => {},
    tabsBootstrap: async () => {},
    settingGet: async () => null,
    sidebarSetWidth: async () => {},
    sidebarSetHidden: async () => {},
    blockerStats: async () => ({ status: "ok", data: { today: 0, last7Days: 0, days: [] } }),
    timeCall: async () => ({ kind: "focus_days", days: [] }),
  });
});
afterEach(() => {
  adoptMode("default");
  surface.dispose();
  tabs.dispose();
  preferences.dispose();
  retryImage();
  delete document.documentElement.dataset.material;
});

test("custom images remove sky light and hidden-card frost, and failure restores both", async () => {
  await preferences.init();
  await surface.init();
  await tabs.init();
  const fresh = tabFixture({ id: "tab", url: null, title: "", projection_revision: revision(20) });
  emitNativeEvent("itemsChanged", {
    profile: { id: "00000000000000000000000901", name: "Personal", kind: "default" },
    spaces: [spaceFixture("space", "Home")],
    active_space_id: "space",
    nodes: [],
    split_group: null,
    projection_revision: revision(20),
    tabs: [fresh],
    active: fresh.id,
  });
  emitNativeEvent("uiCommand", "preference.ui.newtab-tasks=false");
  adoptMode("hidden");
  document.documentElement.dataset.material = "mica";
  const screen = await render(Shell);
  const sidebar = () => screen.container.querySelector<HTMLElement>(".browser-sidebar")!;
  await expect.poll(() => sidebar()?.style.getPropertyValue("--sidebar-frost")).toContain("frost-");
  expect(screen.container.querySelector(".sky-light")).not.toBeNull();
  emitNativeEvent("uiCommand", `preference.ui.newtab-image=${"a".repeat(64)}`);
  emitNativeEvent("uiCommand", "preference.ui.newtab-background=image");
  await expect.poll(() => screen.container.querySelector(".sky-light")).toBeNull();
  expect(sidebar().style.getPropertyValue("--sidebar-frost")).toBe("");
  expect(document.documentElement.dataset.material).toBe("mica");
  await expect.poll(() => screen.container.querySelector(".newtab img")).not.toBeNull();
  screen.container.querySelector(".newtab img")!.dispatchEvent(new Event("error"));
  await expect.poll(() => screen.container.querySelector(".sky-light")).not.toBeNull();
  expect(sidebar().style.getPropertyValue("--sidebar-frost")).toContain("frost-");
  expect(document.documentElement.dataset.material).toBe("mica");
});
