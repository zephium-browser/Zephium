import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
const native = vi.hoisted(() => ({
  get: vi.fn(),
  shortcuts: vi.fn(),
  set: vi.fn(),
  settle: vi.fn(),
  stop: vi.fn(),
  listener: null as null | ((event: { payload: string }) => void),
  listen: vi.fn(),
}));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    settingGet: native.get,
    settingSet: native.set,
    searchShortcutDefaults: native.shortcuts,
  });
});
vi.mock("$domain/operations", () => ({ settle: native.settle }));
vi.mock("$shared/ipc/native-events", () => ({ events: { uiCommand: { listen: native.listen } } }));
beforeEach(() => {
  vi.resetModules();
  vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "Date", "performance"] });
  vi.resetAllMocks();
  native.listener = null;
  native.listen.mockImplementation((listener: typeof native.listener) => {
    native.listener = listener;
    return Promise.resolve(native.stop);
  });
  native.get.mockResolvedValue(null);
  native.shortcuts.mockResolvedValue([]);
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
    await vi.advanceTimersByTimeAsync(0);
    native.listener?.({ payload: "preference.ui.accent=sage" });
    native.listener?.({ payload: "preference.privacy.smartscreen=false" });
    finish("sky");
    await first;
    expect(native.listen).toHaveBeenCalledOnce();
    expect(preferences.value("ui.accent")).toBe("sage");
    expect(preferences.value("privacy.smartscreen")).toBe("false");
    preferences.dispose();
    expect(native.stop).toHaveBeenCalledOnce();
  });
  it("limits native startup reads and reuses a free slot while another read is held", async () => {
    const preferences = await import("../preferences.svelte");
    const releases: (() => void)[] = [];
    let outstanding = 0;
    let peak = 0;
    function hold<T>(value: T) {
      outstanding++;
      peak = Math.max(peak, outstanding);
      return new Promise<T>((resolve) => {
        releases.push(() => {
          outstanding--;
          resolve(value);
        });
      });
    }
    native.shortcuts.mockImplementation(() => hold([]));
    native.get.mockImplementation(() => hold(null));
    const startup = preferences.init();
    await vi.advanceTimersByTimeAsync(0);
    expect(outstanding).toBe(8);
    expect(native.get).toHaveBeenCalledTimes(7);
    releases.splice(1, 1)[0]!();
    await vi.advanceTimersByTimeAsync(0);
    expect(native.get).toHaveBeenCalledTimes(8);
    expect(outstanding).toBe(8);
    expect(peak).toBe(8);

    native.get.mockImplementation(async (key: string) =>
      key === "privacy.smartscreen" ? "false" : null,
    );
    for (const release of releases) release();
    await startup;
    expect(preferences.value("privacy.smartscreen")).toBe("false");
    preferences.dispose();
  });
  it("shares a deadline across reads started at different times", async () => {
    const preferences = await import("../preferences.svelte");
    const pending = new Map<string, (value: string) => void>();
    native.get.mockImplementation(
      (key: string) => new Promise<string>((resolve) => pending.set(key, resolve)),
    );
    const startup = preferences.init();
    const finished = vi.fn();
    void startup.then(finished);
    await vi.advanceTimersByTimeAsync(1000);
    expect(native.get).toHaveBeenCalledTimes(8);
    pending.get("appearance")!("dark");
    await vi.advanceTimersByTimeAsync(0);
    expect(native.get).toHaveBeenCalledTimes(9);
    expect(finished).not.toHaveBeenCalled();

    await vi.advanceTimersByTimeAsync(1000);
    expect(finished).toHaveBeenCalledOnce();
    expect(vi.getTimerCount()).toBe(0);
    expect(preferences.value("appearance")).toBe("dark");
    pending.get("ui.accent")!("sky");
    await vi.advanceTimersByTimeAsync(0);
    expect(native.get).toHaveBeenCalledTimes(9);
    expect(preferences.value("ui.accent")).toBe("graphite");
    preferences.dispose();
  });
  it("stops startup reads on disposal and ignores replies from that generation", async () => {
    const preferences = await import("../preferences.svelte");
    const pending = new Map<string, (value: string) => void>();
    native.get.mockImplementation(
      (key: string) => new Promise<string>((resolve) => pending.set(key, resolve)),
    );
    const first = preferences.init();
    await vi.advanceTimersByTimeAsync(0);
    expect(native.get).toHaveBeenCalledTimes(8);
    preferences.dispose();
    await first;
    expect(native.stop).toHaveBeenCalledOnce();
    expect(vi.getTimerCount()).toBe(0);
    expect(native.get).toHaveBeenCalledTimes(8);

    native.get.mockImplementation(async (key: string) => (key === "ui.accent" ? "sage" : null));
    await preferences.init();
    const calls = native.get.mock.calls.length;
    for (const reply of pending.values()) reply("sky");
    await vi.advanceTimersByTimeAsync(0);
    expect(native.get).toHaveBeenCalledTimes(calls);
    expect(preferences.value("ui.accent")).toBe("sage");
    preferences.dispose();
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

it("persists shortcut settings and preserves equals signs in live URL templates", async () => {
  const preferences = await import("../preferences.svelte");
  await preferences.init();
  const stored = JSON.stringify([
    {
      id: "custom:example",
      name: "Example",
      keywords: ["ex"],
      domain: "example.com",
      search_url: "https://example.com/?q={query}&mode=all",
      home_url: "https://example.com/",
      enabled: true,
      suggestions: null,
    },
  ]);
  native.get.mockImplementation(async (key: string) =>
    key === "search.shortcuts" ? stored : null,
  );
  await preferences.set("search.shortcuts", stored);
  expect(preferences.saveFailed()).toBe(false);
  expect(preferences.searchShortcuts()[0]?.search_url).toBe(
    "https://example.com/?q={query}&mode=all",
  );
  native.listener?.({
    payload: `preference.search.shortcuts=${stored.replace('"enabled":true', '"enabled":false')}`,
  });
  expect(preferences.searchShortcuts()[0]?.enabled).toBe(false);
  expect(preferences.searchShortcuts()[0]?.search_url).toContain("&mode=all");
  preferences.dispose();
  await preferences.init();
  expect(preferences.searchShortcuts()[0]?.id).toBe("custom:example");
  preferences.dispose();
});
