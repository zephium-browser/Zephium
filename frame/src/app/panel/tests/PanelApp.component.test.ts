import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { cleanup, render } from "vitest-browser-svelte";
import { emitNativeEvent } from "$shared/testing/native-events";
import type { PanelState } from "$shared/ipc/bindings";
import PanelApp from "../PanelApp.svelte";

const native = vi.hoisted(() => ({
  load: Promise.resolve(),
  loading: vi.fn(),
  loaded: vi.fn(),
  read: vi.fn(async (key: string) => (key === "ui.reduce-motion" ? "false" : null)),
  shortcuts: vi.fn(async () => []),
  uiInfo: vi.fn(async () => ({ material: "none" as const })),
  ready: vi.fn<() => Promise<PanelState>>(),
}));

vi.mock("$features/search", async (original) => {
  const search = await original<typeof import("$features/search")>();
  return {
    ...search,
    loadLauncherPanel: async () => {
      native.loading();
      await native.load;
      const module = await search.loadLauncherPanel();
      native.loaded();
      return module;
    },
  };
});
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    settingGet: native.read,
    searchShortcutDefaults: native.shortcuts,
    uiInfo: native.uiInfo,
    panelReady: native.ready,
    panelLayout: async () => {},
    panelIntent: async () => true,
  });
});

let finishLoading: () => void;
beforeEach(() => {
  vi.clearAllMocks();
  native.load = new Promise<void>((resolve) => (finishLoading = resolve));
});

afterEach(async () => {
  await cleanup();
  finishLoading();
  delete document.documentElement.dataset.theme;
  delete document.documentElement.dataset.material;
  delete document.documentElement.dataset.reduceMotion;
});

test("loads the launcher before startup IPC and preserves a live setting during the import", async () => {
  const screen = await render(PanelApp);
  let inputAtReady: HTMLInputElement | null = null;
  native.ready.mockImplementation(async () => {
    inputAtReady = screen.container.querySelector(".launcher input");
    return {
      switcher: null,
      window_id: null,
      revision: "0000000000000001",
      session_id: "0000000000000001",
      visible: false,
      profile_id: null,
      profile_name: null,
      space_id: null,
      error: false,
      corner_radius: 20,
      seed: null,
      seed_tab: null,
    };
  });
  await expect.poll(() => native.loading.mock.calls.length).toBe(1);
  emitNativeEvent("uiCommand", "preference.ui.reduce-motion=true");
  expect(document.documentElement.dataset.reduceMotion).toBe("true");
  expect(native.uiInfo).not.toHaveBeenCalled();
  expect(native.read).not.toHaveBeenCalled();
  expect(native.shortcuts).not.toHaveBeenCalled();
  expect(native.ready).not.toHaveBeenCalled();

  finishLoading();
  await expect.poll(() => native.ready.mock.calls.length).toBe(1);
  for (const command of [native.uiInfo, native.read, native.shortcuts]) {
    expect(command.mock.invocationCallOrder[0]).toBeGreaterThan(
      native.loaded.mock.invocationCallOrder[0]!,
    );
  }
  expect(inputAtReady).toBeInstanceOf(HTMLInputElement);
  expect(document.documentElement.dataset.reduceMotion).toBe("true");
});

test("unmount during launcher import never starts bootstrap IPC", async () => {
  await render(PanelApp);
  await expect.poll(() => native.loading.mock.calls.length).toBe(1);
  await cleanup();
  finishLoading();
  await expect.poll(() => native.loaded.mock.calls.length).toBe(1);
  expect(native.uiInfo).not.toHaveBeenCalled();
  expect(native.read).not.toHaveBeenCalled();
  expect(native.shortcuts).not.toHaveBeenCalled();
  expect(native.ready).not.toHaveBeenCalled();
});
