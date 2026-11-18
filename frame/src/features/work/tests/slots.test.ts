import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { Component } from "svelte";
import { loadView, loadedView } from "../components/run/slots";

beforeEach(() => vi.useFakeTimers());
afterEach(() => vi.useRealTimers());

const view = (() => null) as unknown as Component<Record<string, unknown>>;

it("a view whose load fails is retried while its row stays mounted", async () => {
  let calls = 0;
  const loader = vi.fn(async () => {
    calls++;
    if (calls === 1) throw new Error("offline");
    return { default: view };
  });
  loadView(loader);
  await vi.advanceTimersByTimeAsync(0);
  expect(loadedView(loader)).toBeNull();
  await vi.advanceTimersByTimeAsync(500);
  expect(loadedView(loader)).toBe(view);
  expect(loader).toHaveBeenCalledTimes(2);
});

it("a load that keeps failing gives up after its retries and can be started again", async () => {
  const errors: unknown[] = [];
  const onError = (error: unknown) => errors.push(error);
  process.on("unhandledRejection", onError);
  try {
    const loader = vi.fn(async (): Promise<{ default: typeof view }> => {
      throw new Error("gone");
    });
    loadView(loader);
    await vi.advanceTimersByTimeAsync(3000);
    expect(loader).toHaveBeenCalledTimes(3);
    expect(loadedView(loader)).toBeNull();
    loadView(loader);
    await vi.advanceTimersByTimeAsync(0);
    expect(loader).toHaveBeenCalledTimes(4);
    await vi.advanceTimersByTimeAsync(3000);
  } finally {
    await Promise.resolve();
    process.off("unhandledRejection", onError);
  }
  expect(errors.length).toBeGreaterThan(0);
});
