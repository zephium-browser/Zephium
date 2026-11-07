import "$styles/global.css";
import { afterEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { emitNativeEvent } from "$shared/testing/native-events";
import { tabFixture, revision, spaceFixture } from "$shared/testing/fixtures";
import { tabs } from "$domain/tabs";
import { surface } from "$domain/surface";
import Shell from "../Shell.svelte";

vi.mock("../WorkWorkspace.svelte", async () => await import("./StageStub.svelte"));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    pagePermissionCardHost: async () => {},
    tabsBootstrap: async () => {},
  });
});
afterEach(() => {
  surface.dispose();
  tabs.dispose();
});

test("a new tab opened from Settings over a page shows the new tab", async () => {
  await surface.init();
  await tabs.init();
  const page = tabFixture();
  const base = {
    profile: { id: "profile", name: "Personal", kind: "default" as const },
    spaces: [spaceFixture("space", "Home")],
    active_space_id: "space",
    nodes: [],
    split_group: null,
  };
  emitNativeEvent("itemsChanged", {
    ...base,
    projection_revision: revision(10),
    tabs: [page],
    active: page.id,
  });
  const screen = await render(Shell);
  emitNativeEvent("uiCommand", "browser.settings");
  await expect
    .poll(() => screen.container.querySelector(".settings-sidebar-navigation"))
    .not.toBeNull();
  // Native restores Browse from its snapshot, then opens the tab.
  emitNativeEvent("browserReturn", {
    ...base,
    projection_revision: revision(11),
    tabs: [page],
    active: page.id,
  });
  const fresh = tabFixture({
    id: "tab-2",
    url: null,
    title: "",
    projection_revision: revision(12),
  });
  emitNativeEvent("itemsChanged", {
    ...base,
    projection_revision: revision(12),
    tabs: [page, fresh],
    active: fresh.id,
  });
  await expect.poll(() => screen.container.querySelector("[data-zephium-new-tab]")).not.toBeNull();
});
