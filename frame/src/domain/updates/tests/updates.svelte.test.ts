import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const native = vi.hoisted(() => ({
  status: vi.fn(),
  check: vi.fn(),
  relaunch: vi.fn(),
  autoCheck: "true",
}));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    updateStatus: native.status,
    updateCheck: native.check,
    updateRelaunch: native.relaunch,
  });
});
vi.mock("$domain/preferences", () => ({
  preferences: { value: () => native.autoCheck },
}));
vi.mock("$shared/ipc/native-events", () => ({
  events: { uiCommand: { listen: async () => () => {} } },
}));

const HOUR = 60 * 60 * 1000;
let page: EventTarget & { visibilityState: DocumentVisibilityState };

beforeEach(() => {
  vi.resetModules();
  vi.resetAllMocks();
  vi.useFakeTimers();
  native.autoCheck = "true";
  page = Object.assign(new EventTarget(), { visibilityState: "visible" as const });
  vi.stubGlobal("document", page);
});
afterEach(() => {
  vi.useRealTimers();
});

const load = () => import("../updates.svelte");

describe("update schedule", () => {
  it("checks thirty seconds after launch and then every six hours", async () => {
    native.status.mockResolvedValue({ state: "idle" });
    native.check.mockResolvedValue({ state: "upToDate" });
    const updates = await load();
    await updates.init();
    expect(updates.status()).toEqual({ state: "idle" });

    await vi.advanceTimersByTimeAsync(29_000);
    expect(native.check).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(1_000);
    expect(native.check).toHaveBeenCalledOnce();
    expect(updates.status()).toEqual({ state: "upToDate" });

    await vi.advanceTimersByTimeAsync(6 * HOUR - 1);
    expect(native.check).toHaveBeenCalledOnce();
    await vi.advanceTimersByTimeAsync(1);
    expect(native.check).toHaveBeenCalledTimes(2);
    updates.dispose();
  });

  it("never checks a build that cannot update", async () => {
    native.status.mockResolvedValue({ state: "unavailable" });
    const updates = await load();
    await updates.init();
    await vi.advanceTimersByTimeAsync(13 * HOUR);
    expect(native.check).not.toHaveBeenCalled();
    expect(updates.available()).toBe(false);
    updates.dispose();
  });

  it("skips scheduled checks while automatic checking is off", async () => {
    native.autoCheck = "false";
    native.status.mockResolvedValue({ state: "idle" });
    const updates = await load();
    await updates.init();
    await vi.advanceTimersByTimeAsync(7 * HOUR);
    expect(native.check).not.toHaveBeenCalled();

    native.autoCheck = "true";
    native.check.mockResolvedValue({ state: "upToDate" });
    await vi.advanceTimersByTimeAsync(6 * HOUR);
    expect(native.check).toHaveBeenCalledOnce();
    updates.dispose();
  });

  it("checks on return when the machine slept through a check", async () => {
    native.status.mockResolvedValue({ state: "idle" });
    native.check.mockResolvedValue({ state: "upToDate" });
    const updates = await load();
    await updates.init();
    await vi.advanceTimersByTimeAsync(30_000);
    expect(native.check).toHaveBeenCalledOnce();

    page.dispatchEvent(new Event("visibilitychange"));
    await vi.advanceTimersByTimeAsync(0);
    expect(native.check).toHaveBeenCalledOnce();

    vi.setSystemTime(Date.now() + 7 * HOUR);
    page.dispatchEvent(new Event("visibilitychange"));
    await vi.advanceTimersByTimeAsync(0);
    expect(native.check).toHaveBeenCalledTimes(2);
    updates.dispose();
  });

  it("stops checking once disposed", async () => {
    native.status.mockResolvedValue({ state: "idle" });
    const updates = await load();
    await updates.init();
    updates.dispose();
    await vi.advanceTimersByTimeAsync(7 * HOUR);
    expect(native.check).not.toHaveBeenCalled();
    expect(updates.status()).toEqual({ state: "unavailable" });
  });
});

describe("update check", () => {
  it("shows checking at once, then the progress and result native reports", async () => {
    native.status.mockResolvedValue({ state: "idle" });
    let finish!: (value: unknown) => void;
    native.check.mockReturnValueOnce(new Promise((resolve) => (finish = resolve)));
    const updates = await load();
    await updates.init();

    const first = updates.check();
    const second = updates.check();
    expect(updates.status()).toEqual({ state: "checking" });
    expect(native.check).toHaveBeenCalledOnce();

    native.status.mockResolvedValue({ state: "downloading" });
    await vi.advanceTimersByTimeAsync(1_500);
    expect(updates.status()).toEqual({ state: "downloading" });

    finish({ state: "ready", version: "1.0.1" });
    await Promise.all([first, second]);
    expect(updates.status()).toEqual({ state: "ready", version: "1.0.1" });
    await vi.advanceTimersByTimeAsync(3_000);
    expect(updates.status()).toEqual({ state: "ready", version: "1.0.1" });
    updates.dispose();
  });

  it("reports a failed check rather than a stale checking state", async () => {
    native.status.mockResolvedValue({ state: "upToDate" });
    native.check.mockRejectedValueOnce(new Error("offline"));
    const updates = await load();
    await updates.init();
    await updates.check();
    expect(updates.status()).toEqual({ state: "failed" });
    updates.dispose();
  });

  it("relaunches only a waiting update and shows what native did", async () => {
    native.status.mockResolvedValueOnce({ state: "upToDate" });
    const updates = await load();
    await updates.init();
    expect(await updates.relaunch()).toBe(false);
    expect(native.relaunch).not.toHaveBeenCalled();

    native.status.mockResolvedValueOnce({ state: "ready", version: "1.0.1" });
    await updates.refresh();
    native.relaunch.mockResolvedValueOnce(true);
    native.status.mockResolvedValueOnce({ state: "installing" });
    const relaunching = updates.relaunch();
    expect(updates.pendingRelaunch()).toBe(true);
    expect(await relaunching).toBe(true);
    expect(updates.status()).toEqual({ state: "installing" });
    expect(updates.pendingRelaunch()).toBe(false);
    updates.dispose();
  });
});

it("observes an asynchronous installation failure and stops polling", async () => {
  native.status.mockResolvedValueOnce({ state: "ready", version: "1.0.1" });
  const updates = await load();
  await updates.init();
  native.relaunch.mockResolvedValueOnce(true);
  native.status.mockResolvedValueOnce({ state: "installing" });
  await updates.relaunch();
  native.status.mockResolvedValueOnce({ state: "failed" });
  await vi.advanceTimersByTimeAsync(1500);
  expect(updates.status()).toEqual({ state: "failed" });
  const calls = native.status.mock.calls.length;
  await vi.advanceTimersByTimeAsync(3000);
  expect(native.status).toHaveBeenCalledTimes(calls);
  updates.dispose();
});

it("disposal cancels installation status polling", async () => {
  native.status.mockResolvedValueOnce({ state: "ready", version: "1.0.1" });
  const updates = await load();
  await updates.init();
  native.relaunch.mockResolvedValueOnce(true);
  native.status.mockResolvedValue({ state: "installing" });
  await updates.relaunch();
  updates.dispose();
  const calls = native.status.mock.calls.length;
  await vi.advanceTimersByTimeAsync(3000);
  expect(native.status).toHaveBeenCalledTimes(calls);
});
