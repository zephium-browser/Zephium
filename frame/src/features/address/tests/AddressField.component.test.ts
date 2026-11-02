import { afterEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { emitNativeEvent } from "$shared/testing/native-events";
import { revision, tabFixture } from "$shared/testing/fixtures";
import { tabs } from "$domain/tabs";
import { preferences } from "$domain/preferences";
import AddressFieldHost from "./AddressFieldHost.svelte";

const native = vi.hoisted(() => ({
  defaultZoom: "100",
  page: null as string | null,
  edit: vi.fn(async () => true),
}));

vi.mock("$domain/surface", () => ({ surface: { currentPage: () => native.page } }));
vi.mock("$domain/permissions", () => ({
  sitePermissions: { list: async () => null, set: async () => true },
}));
vi.mock("$domain/blocker", async (original) => ({
  ...(await original<typeof import("$domain/blocker")>()),
  blocker: {
    status: () => ({ protection: "unavailable", applied_enabled: null, site: null }),
    refresh: async () => {},
  },
}));

vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    tabsBootstrap: async () => {},
    runCommand: async () => ({ accepted: true, operation_id: null }),
    launcherEdit: native.edit,
    settingGet: async (key: string) => (key === "zoom.default" ? native.defaultZoom : null),
  });
});

// The store keeps the newest projection it admitted across tests, so a stamp
// that follows the clock can land behind the last test's when tests run fast.
let clock = Date.now();
const stamp = () => revision((clock += 1));

afterEach(() => {
  tabs.dispose();
  preferences.dispose();
  native.defaultZoom = "100";
  native.page = null;
  native.edit.mockClear();
});

async function field(props: { compact?: boolean } = {}) {
  await tabs.init();
  const tab = tabFixture({
    projection_revision: stamp(),
    url: "https://www.wikipedia.org/wiki/Premium",
  });
  emitNativeEvent("itemsChanged", {
    projection_revision: stamp(),
    profile: null,
    spaces: [],
    active_space_id: null,
    nodes: [],
    tabs: [tab],
    active: tab.id,
    split_group: null,
  });
  const screen = await render(AddressFieldHost, props);
  const input = screen.container.querySelector<HTMLInputElement>("[data-zephium-address]")!;
  return { screen, input };
}

test("a click on a page's address edits it here, and only the shortcut opens the launcher", async () => {
  const { screen, input } = await field();
  await screen.getByRole("textbox", { name: /address/iu }).click();
  expect(document.activeElement).toBe(input);
  await expect.poll(() => input.value).toBe("https://www.wikipedia.org/wiki/Premium");
  expect(native.edit).not.toHaveBeenCalled();
});

test("editing shows the whole address and rests on the host", async () => {
  const { input } = await field();
  expect(input.value).toBe("www.wikipedia.org");
  input.focus();
  await expect.poll(() => input.value).toBe("https://www.wikipedia.org/wiki/Premium");
  input.blur();
  await expect.poll(() => input.value).toBe("www.wikipedia.org");
});

test("a press anywhere else in the chrome ends editing", async () => {
  const { input } = await field();
  const elsewhere = document.createElement("div");
  elsewhere.style.cssText = "width:40px;height:40px";
  document.body.append(elsewhere);

  input.focus();
  expect(document.activeElement).toBe(input);
  elsewhere.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true }));
  expect(document.activeElement).not.toBe(input);
  await expect.poll(() => input.value).toBe("www.wikipedia.org");
  elsewhere.remove();
});

test("the page taking focus from the chrome ends editing", async () => {
  const { input } = await field();
  input.focus();
  window.dispatchEvent(new Event("blur"));
  expect(document.activeElement).not.toBe(input);
});

test("a native blocked-popup projection is visible without changing the address", async () => {
  const { screen, input } = await field();
  const tab = tabs.activeTab()!;
  emitNativeEvent("tabChanged", {
    ...tab,
    projection_revision: stamp(),
    page_request: { kind: "popup", host: "example.com", site: null },
  });
  const card = screen.getByRole("alertdialog");
  await expect.element(card).toHaveTextContent("Pop-up blocked");
  await expect.element(card).toHaveTextContent("example.com");
  await expect.element(card.getByRole("button", { name: "Open" })).toBeVisible();
  expect(input.value).toBe("www.wikipedia.org");
});

test("a page's link for another app asks before it opens", async () => {
  const { screen } = await field();
  const tab = tabs.activeTab()!;
  emitNativeEvent("tabChanged", {
    ...tab,
    projection_revision: stamp(),
    page_request: { kind: "external_app", site: "zoom.us", scheme: "zoommtg", app: "zoom.us" },
  });
  const card = screen.getByRole("alertdialog");
  await expect.element(card).toHaveTextContent("Open zoom.us?");
  await expect.element(card.getByRole("checkbox")).not.toBeChecked();
  await expect.element(card.getByRole("button", { name: "Cancel" })).toBeVisible();
});

test("a page that stopped responding offers to wait or reload", async () => {
  const { screen } = await field();
  const tab = tabs.activeTab()!;
  emitNativeEvent("tabChanged", {
    ...tab,
    projection_revision: stamp(),
    page_request: { kind: "unresponsive" },
  });
  const card = screen.getByRole("alertdialog");
  await expect.element(card).toHaveTextContent("This page isn't responding");
  await expect.element(card.getByRole("button", { name: "Wait" })).toBeVisible();
  await expect.element(card.getByRole("button", { name: "Reload" })).toBeVisible();
  emitNativeEvent("tabChanged", {
    ...tab,
    projection_revision: stamp(),
    page_request: null,
  });
  await expect.element(screen.getByRole("alertdialog")).not.toBeInTheDocument();
});

const told = (screen: { container: Element }) =>
  JSON.parse(screen.container.querySelector("[data-trailing]")?.textContent ?? "undefined");

async function trailed(change: Record<string, unknown> = {}) {
  const { screen, input } = await field();
  if (Object.keys(change).length > 0) {
    emitNativeEvent("tabChanged", {
      ...tabs.activeTab()!,
      projection_revision: stamp(),
      ...change,
    });
  }
  return { screen, input };
}

test("the trailing controls are told about a secure web page, with nothing wrong with it", async () => {
  const { screen } = await trailed();
  expect(told(screen)).toEqual({
    url: "https://www.wikipedia.org/wiki/Premium",
    security: "secure",
    warning: null,
    capture: null,
    privateWindow: false,
    zoom: null,
    standardZoom: 100,
  });
});

test("an unencrypted page, and one past a certificate warning, are told what is wrong", async () => {
  const { screen } = await trailed({ url: "http://example.com/" });
  await expect.poll(() => told(screen).security).toBe("insecure");
  expect(told(screen).warning).toBe("Connection is not encrypted");

  emitNativeEvent("tabChanged", {
    ...tabs.activeTab()!,
    projection_revision: stamp(),
    url: "https://self-signed.example/",
    certificate_exception: true,
  });
  await expect.poll(() => told(screen).security).toBe("untrusted");
  expect(told(screen).warning).toBe("Certificate not trusted. You chose to continue to this site.");
});

test("the page's zoom is passed on with what pages show at by default", async () => {
  native.defaultZoom = "110";
  await preferences.init();
  const { screen } = await trailed({ zoom: 125 });
  await expect.poll(() => told(screen).zoom).toBe(125);
  expect(told(screen).standardZoom).toBe(110);
});

test("a camera in use is told too", async () => {
  const { screen } = await trailed({ capture: { camera: "active", microphone: "none" } });
  await expect.poll(() => told(screen).capture).toEqual({ camera: "active", microphone: "none" });
});

test("a new tab, an internal page and a blank page have no site to be told of", async () => {
  const blank = await pageWithout({ url: null });
  await expect.poll(() => told(blank)).toBeNull();
  blank.unmount();

  const internal = await pageWithout({ url: null, content: "settings" });
  await expect.poll(() => told(internal)).toBeNull();
  internal.unmount();

  const about = await pageWithout({ url: "about:blank" });
  await expect.poll(() => told(about)).toBeNull();
});

test("a page behind Settings has none either, and the rail draws no trailing controls", async () => {
  native.page = "settings";
  const behind = await field();
  await expect.poll(() => told(behind.screen)).toBeNull();
  behind.screen.unmount();

  native.page = null;
  const rail = await field({ compact: true });
  expect(rail.screen.container.querySelector("[data-trailing]")).toBeNull();
});

test("the field itself carries no site glyph and no zoom badge", async () => {
  const { screen } = await trailed({ url: "http://example.com/", zoom: 90 });
  await expect.poll(() => told(screen).zoom).toBe(90);
  await expect
    .element(screen.getByRole("button", { name: /Site information|Page zoom/u }))
    .not.toBeInTheDocument();
});

async function pageWithout(change: Record<string, unknown>) {
  const { screen, input } = await field();
  emitNativeEvent("tabChanged", {
    ...tabs.activeTab()!,
    projection_revision: stamp(),
    ...change,
  });
  await expect.poll(() => input.value).toBe("");
  return screen;
}
