import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
const native = vi.hoisted(() => ({
  get: vi.fn(),
  set: vi.fn(),
  settle: vi.fn(),
  stop: vi.fn(),
  listener: null as null | ((event: { payload: string }) => void),
  listen: vi.fn(),
}));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({ settingGet: native.get, settingSet: native.set });
});
vi.mock("$domain/operations", () => ({ settle: native.settle }));
vi.mock("$shared/ipc/native-events", () => ({ events: { uiCommand: { listen: native.listen } } }));
beforeEach(() => {
  vi.resetModules();
  vi.useFakeTimers();
  vi.resetAllMocks();
  native.listener = null;
  native.listen.mockImplementation((listener: typeof native.listener) => {
    native.listener = listener;
    return Promise.resolve(native.stop);
  });
  native.get.mockResolvedValue(null);
  native.set.mockResolvedValue({ accepted: true, operation_id: "0000000000000001" });
  native.settle.mockResolvedValue({ outcome: "deferred", disposition: null });
});
afterEach(() => vi.useRealTimers());
describe("preference observation lifecycle", () => {
  it.each([null, "continue", "pages", "new-tab"])(
    "defaults startup to a new tab while preserving the stored choice %s",
    async (choice) => {
      native.get.mockImplementation(async (key: string) =>
        key === "tabs.startup" ? choice : null,
      );
      const preferences = await import("../preferences.svelte");
      await preferences.init();
      expect(preferences.value("tabs.startup")).toBe(choice ?? "new-tab");
      expect(native.set).not.toHaveBeenCalled();
      preferences.dispose();
    },
  );
  it("initializes once and keeps a live update ahead of an older startup read", async () => {
    const preferences = await import("../preferences.svelte");
    let finish!: (value: string) => void;
    native.get.mockImplementation((key: string) =>
      key === "ui.accent"
        ? new Promise((resolve) => {
            finish = resolve;
          })
        : Promise.resolve(null),
    );
    const first = preferences.init();
    expect(preferences.init()).toBe(first);
    await Promise.resolve();
    native.listener?.({ payload: "preference.ui.accent=sage" });
    finish("sky");
    await first;
    expect(native.listen).toHaveBeenCalledOnce();
    expect(preferences.value("ui.accent")).toBe("sage");
    preferences.dispose();
    expect(native.stop).toHaveBeenCalledOnce();
  });
  it("cannot leave a hung readback pending forever", async () => {
    const preferences = await import("../preferences.svelte");
    await preferences.init();
    native.get.mockImplementation(() => new Promise(() => {}));
    const pending = preferences.set("ui.accent", "sky");
    await vi.advanceTimersByTimeAsync(1500);
    await pending;
    expect(preferences.saving()).toBe(false);
    expect(preferences.saveFailed()).toBe(true);
    expect(preferences.value("ui.accent")).toBe("graphite");
    preferences.dispose();
    expect(vi.getTimerCount()).toBe(0);
  });
  it("cancels readback observation on disposal without overwriting the next generation", async () => {
    const preferences = await import("../preferences.svelte");
    await preferences.init();
    let finish!: (value: string) => void;
    native.get.mockImplementation(
      () =>
        new Promise((resolve) => {
          finish = resolve;
        }),
    );
    const pending = preferences.set("ui.accent", "sky");
    await Promise.resolve();
    preferences.dispose();
    await pending;
    native.get.mockResolvedValue(null);
    await preferences.init();
    finish("sky");
    await Promise.resolve();
    expect(preferences.value("ui.accent")).toBe("graphite");
    expect(preferences.saveFailed()).toBe(false);
    preferences.dispose();
  });
  it("uses confirmed readback when the event was missed, without replacing a newer event", async () => {
    const preferences = await import("../preferences.svelte");
    await preferences.init();
    native.get.mockResolvedValue("sky");
    await preferences.set("ui.accent", "sky");
    expect(preferences.value("ui.accent")).toBe("sky");
    native.get.mockImplementation(async () => {
      native.listener?.({ payload: "preference.ui.accent=rose" });
      return "sage";
    });
    await preferences.set("ui.accent", "sage");
    expect(preferences.value("ui.accent")).toBe("rose");
    preferences.dispose();
  });
});
