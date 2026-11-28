import "$styles/global.css";
import { afterEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { page } from "vitest/browser";
import type { GlanceLayout } from "$shared/ipc/bindings";
import { emitNativeEvent } from "$shared/testing/native-events";
import { tabFixture, revision, spaceFixture } from "$shared/testing/fixtures";
import * as m from "$shared/i18n/messages";
import { layout } from "$domain/layout";
import { surface } from "$domain/surface";
import { tabs } from "$domain/tabs";
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
  layout.dispose();
  surface.dispose();
  tabs.dispose();
});

const glance: GlanceLayout = {
  tab: "child",
  parent: "opener",
  region: { x: 0, y: 0, width: 400, height: 500 },
  page: { x: 40, y: 0, width: 320, height: 500 },
  backdrop: null,
};

const deliver = (value: GlanceLayout | null) =>
  emitNativeEvent("layoutChanged", { dividers: [], work_pane: null, glance: value });

test("the Glance overlay mounts only while native layout carries one", async () => {
  await surface.init();
  await tabs.init();
  await layout.init();
  const list = [tabFixture({ id: "opener", projection_revision: revision(1) })];
  emitNativeEvent("itemsChanged", {
    projection_revision: revision(10),
    profile: { id: "profile", name: "Personal", kind: "default" as const },
    spaces: [spaceFixture("space", "Home")],
    active_space_id: "space",
    nodes: [],
    tabs: list,
    active: "opener",
    split_group: null,
  });
  const screen = await render(Shell);
  const overlay = () => screen.container.ownerDocument.querySelector("[data-glance]");

  expect(overlay()).toBeNull();

  deliver(glance);
  await expect.element(page.getByRole("button", { name: m.glance_expand() })).toBeVisible();
  expect(overlay()).not.toBeNull();

  deliver(null);
  await expect.poll(overlay).toBeNull();

  deliver({ ...glance, page: { ...glance.page, width: null } });
  await new Promise((resolve) => setTimeout(resolve, 50));
  expect(overlay()).toBeNull();
});
