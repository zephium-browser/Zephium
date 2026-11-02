import { beforeEach, expect, test, vi } from "vitest";

const host = vi.hoisted(() => ({
  stored: new Map<string, string>(),
  set: vi.fn(),
  width: vi.fn(),
}));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    settingGet: async (key) => host.stored.get(key) ?? null,
    settingSet: host.set,
    sidebarSetWidth: host.width,
    sidebarSetHidden: vi.fn(),
  });
});

beforeEach(() => {
  vi.resetModules();
  host.stored.clear();
  host.width.mockReset();
  host.set.mockReset().mockImplementation(async (key: string, value: string) => {
    host.stored.set(key, value);
    return { operation_id: null, accepted: true };
  });
  vi.stubGlobal("window", new EventTarget());
});

test("completed and keyboard resizes survive a new sidebar session", async () => {
  const sidebar = await import("../sidebar-mode.svelte");
  await sidebar.init();
  sidebar.beginSidebarResize();
  sidebar.applyDragWidth(330);
  expect(host.set).not.toHaveBeenCalled();
  sidebar.finishSidebarResize(330);
  expect(host.set).toHaveBeenCalledWith("sidebar.width", "330");
  sidebar.applyDragWidth(350);
  expect(host.stored.get("sidebar.width")).toBe("350");
  vi.resetModules();
  const reopened = await import("../sidebar-mode.svelte");
  await reopened.init();
  expect(reopened.expanded()).toBe(350);
  expect(host.width).toHaveBeenLastCalledWith(350, false, expect.any(Number));
});

test("hidden mode keeps its expanded width and temporary panels do not replace it", async () => {
  host.stored.set("sidebar.width", "360");
  host.stored.set("sidebar.mode", "hidden");
  const sidebar = await import("../sidebar-mode.svelte");
  await sidebar.init();
  expect(sidebar.sidebarMode()).toBe("hidden");
  expect(sidebar.expanded()).toBe(360);
  sidebar.setPanelExtent(200);
  sidebar.setPanelExtent(0);
  sidebar.beginSidebarResize();
  sidebar.cancelSidebarResize();
  sidebar.adoptResizeWidth(56);
  expect(host.stored.get("sidebar.width")).toBe("360");
  expect(host.set).not.toHaveBeenCalledWith("sidebar.width", expect.anything());
});

test("only a current, finite native resize saves a rounded width", async () => {
  const sidebar = await import("../sidebar-mode.svelte");
  await sidebar.init();
  const revision = sidebar.sidebarResizeRevision();
  sidebar.adoptResizeWidth(320, revision - 1);
  sidebar.adoptResizeWidth(Number.NaN, revision);
  expect(host.set).not.toHaveBeenCalled();
  sidebar.adoptResizeWidth(389.7, revision);
  expect(host.set).toHaveBeenCalledExactlyOnceWith("sidebar.width", "390");
});

test.each([null, "56", "NaN", "461", "240.5", " 300"])(
  "an absent or invalid saved width %s retains the default",
  async (width) => {
    if (width !== null) host.stored.set("sidebar.width", width);
    const sidebar = await import("../sidebar-mode.svelte");
    await sidebar.init();
    expect(sidebar.expanded()).toBe(240);
    expect(host.width).toHaveBeenLastCalledWith(240, false, expect.any(Number));
  },
);
