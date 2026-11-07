import { afterEach, expect, test, vi } from "vitest";
import * as find from "../lib/find.svelte";
import type { FindResultView } from "$shared/ipc/bindings";

const native = vi.hoisted(() => ({
  find: vi.fn(async () => true),
  result: null as ((event: { payload: FindResultView }) => void) | null,
}));
vi.mock("$shared/ipc/bindings", () => ({ commands: { pageFind: native.find } }));
vi.mock("$shared/ipc/native-events", () => ({
  events: {
    find: {
      listen: async (listener: typeof native.result) => {
        native.result = listener;
        return () => {};
      },
    },
  },
}));
vi.mock("$session/sidebar-mode.svelte", () => ({ expandBriefly: () => () => {} }));

afterEach(() => {
  find.dispose();
  vi.clearAllMocks();
  vi.useRealTimers();
});

test.each(["refused", "failed"])("a %s find settles with no results", async (failure) => {
  vi.useFakeTimers();
  if (failure === "refused") native.find.mockResolvedValueOnce(false);
  else native.find.mockRejectedValueOnce(new Error("unavailable"));
  find.show("tab");
  find.setText("needle");
  await vi.runAllTimersAsync();
  expect(find.count()).toBe(0);
  expect(find.position()).toBeNull();
});

test("a stale refusal cannot settle a newer search or a reopened field", async () => {
  vi.useFakeTimers();
  let refuse!: (accepted: boolean) => void;
  native.find.mockReturnValueOnce(new Promise((resolve) => (refuse = resolve)));
  find.show("tab");
  find.setText("needle");
  await vi.runAllTimersAsync();
  find.hide();
  find.show("other-tab");
  find.setText("needle");
  await vi.runAllTimersAsync();
  refuse(false);
  await Promise.resolve();
  expect(find.count()).toBeNull();
});

test.each(["refused", "failed"])(
  "a %s step preserves matches and can be retried",
  async (failure) => {
    vi.useFakeTimers();
    find.show("tab");
    find.setText("needle");
    await vi.runAllTimersAsync();
    native.result?.({ payload: { query: "needle", matches: 4, active: 2 } });
    if (failure === "refused") native.find.mockResolvedValueOnce(false);
    else native.find.mockRejectedValueOnce(new Error("unavailable"));
    find.step(true, "tab");
    await vi.runAllTimersAsync();
    expect(find.count()).toBe(4);
    expect(find.position()).toBe(2);
    expect(find.failed()).toBe(true);
    find.step(false, "tab");
    await vi.runAllTimersAsync();
    native.result?.({ payload: { query: "needle", matches: 4, active: 1 } });
    expect(find.failed()).toBe(false);
    expect(find.position()).toBe(1);
  },
);
