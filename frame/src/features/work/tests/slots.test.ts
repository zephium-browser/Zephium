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
  const reported: unknown[] = [];
  const onError = (error: unknown) => reported.push(error);
  process.on("unhandledRejection", onError);
  try {
    const thrown: Error[] = [];
    const loader = vi.fn(async (): Promise<{ default: typeof view }> => {
      const error = new Error(`gone ${thrown.length}`);
      thrown.push(error);
      throw error;
    });
    loadView(loader);
    await vi.advanceTimersByTimeAsync(3000);
    await vi.waitFor(() => expect(reported).toEqual([thrown[2]]));
    expect(loader).toHaveBeenCalledTimes(3);
    expect(loadedView(loader)).toBeNull();

    loadView(loader);
    await vi.advanceTimersByTimeAsync(3000);
    await vi.waitFor(() => expect(reported).toEqual([thrown[2], thrown[5]]));
    expect(loader).toHaveBeenCalledTimes(6);
  } finally {
    process.off("unhandledRejection", onError);
  }
});
