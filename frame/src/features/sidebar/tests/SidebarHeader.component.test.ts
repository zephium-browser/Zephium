import { afterEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { preferences } from "$domain/preferences";
import { surface } from "$domain/surface";
import { tabs } from "$domain/tabs";
import { revision, tabFixture } from "$shared/testing/fixtures";
import { emitNativeEvent } from "$shared/testing/native-events";
import SidebarHeader from "../components/SidebarHeader.svelte";

const native = vi.hoisted(() => ({
  menu: vi.fn(async (..._args: unknown[]) => true),
  kind: "default" as "default" | "incognito",
}));

vi.mock("$domain/blocker", async (original) => ({
  ...(await original<typeof import("$domain/blocker")>()),
  blocker: { status: () => ({ protection: "unavailable", applied_enabled: null, site: null }) },
}));
vi.mock("$domain/webext", () => ({ webext: { isAvailable: () => false } }));
vi.mock("$domain/extensions", () => ({ extensions: { activeActions: () => [] } }));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    tabsBootstrap: async () => {},
    runCommand: async () => ({ accepted: true, operation_id: null }),
    settingGet: async () => null,
    sidebarMenuPopup: native.menu,
  });
});

let clock = Date.now();
const stamp = () => revision((clock += 1));

afterEach(() => {
  tabs.dispose();
  surface.dispose();
  preferences.dispose();
  native.menu.mockClear();
  native.kind = "default";
});

async function rail(over: Parameters<typeof tabFixture>[0]) {
  await tabs.init();
  const tab = tabFixture({ projection_revision: stamp(), ...over });
  emitNativeEvent("itemsChanged", {
    projection_revision: stamp(),
    profile: { id: "p1", name: "Personal", kind: native.kind },
    spaces: [],
    active_space_id: null,
    nodes: [],
    tabs: [tab],
    active: tab.id,
    split_group: null,
  });
  const screen = await render(SidebarHeader, { compact: true, ontoggle: () => {} });
  await screen.getByRole("button", { name: "Sidebar options" }).click();
  return native.menu.mock.calls.at(-1)!;
}

// x, y, siteProtected, canHide, canClearData, extensions, manageExtensions
const offersClearData = (call: unknown[]) => call[4];

test("the rail's menu offers to clear the data of a web page's site", async () => {
  expect(offersClearData(await rail({ url: "https://example.com/a" }))).toBe(true);
});

test("a new tab and a blank page have no site, so no data to clear", async () => {
  expect(offersClearData(await rail({ url: null }))).toBe(false);
});

test("an internal page has no site to clear either", async () => {
  expect(offersClearData(await rail({ url: "https://example.com/", content: "settings" }))).toBe(
    false,
  );
});

test("a private window keeps nothing, so offers nothing to clear", async () => {
  native.kind = "incognito";
  expect(offersClearData(await rail({ url: "https://example.com/" }))).toBe(false);
});
