import { beforeEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import MiniApp from "../MiniApp.svelte";

const native = vi.hoisted(() => ({
  view: vi.fn<() => Promise<() => void>>(),
  presentation: vi.fn<() => Promise<() => void>>(),
  command: vi.fn<() => Promise<() => void>>(),
  initTheme: vi.fn(async () => {}),
  ready: vi.fn(async () => true),
}));

vi.mock("$shared/ipc/native-events", () => ({
  events: {
    mini: { listen: native.view },
    miniPresentation: { listen: native.presentation },
    uiCommand: { listen: native.command },
  },
}));
vi.mock("$domain/appearance", () => ({
  theme: {
    init: native.initTheme,
    dispose: vi.fn(),
    customizationKeys: [],
    setReducedMotion: vi.fn(),
  },
}));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({ settingGet: async () => null, miniReady: native.ready });
});

beforeEach(() => vi.clearAllMocks());

test("closing during subscription setup releases all listeners and skips startup", async () => {
  const stopView = vi.fn();
  const stopPresentation = vi.fn();
  const stopCommand = vi.fn();
  let finishView!: (stop: () => void) => void;
  native.view.mockReturnValueOnce(new Promise((resolve) => (finishView = resolve)));
  native.presentation.mockResolvedValueOnce(stopPresentation);
  native.command.mockResolvedValueOnce(stopCommand);

  const screen = await render(MiniApp, { label: "mini-2" });
  expect(native.view).toHaveBeenCalledOnce();
  await screen.unmount();
  finishView(stopView);

  await expect.poll(() => stopView.mock.calls.length, { timeout: 1000 }).toBe(1);
  expect(stopPresentation).toHaveBeenCalledOnce();
  expect(stopCommand).toHaveBeenCalledOnce();
  expect(native.initTheme).not.toHaveBeenCalled();
  expect(native.ready).not.toHaveBeenCalled();
});

test("a refused subscription releases the other listeners without showing the window", async () => {
  const stopPresentation = vi.fn();
  const stopCommand = vi.fn();
  native.view.mockRejectedValueOnce(new Error("subscription refused"));
  native.presentation.mockResolvedValueOnce(stopPresentation);
  native.command.mockResolvedValueOnce(stopCommand);

  const screen = await render(MiniApp, { label: "mini-2" });
  await expect.poll(() => stopCommand.mock.calls.length).toBe(1);
  expect(stopPresentation).toHaveBeenCalledOnce();
  expect(native.initTheme).not.toHaveBeenCalled();
  expect(native.ready).not.toHaveBeenCalled();
  await screen.unmount();
  expect(stopCommand).toHaveBeenCalledOnce();
  expect(stopPresentation).toHaveBeenCalledOnce();
});
