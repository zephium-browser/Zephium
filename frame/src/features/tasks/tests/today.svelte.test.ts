import { afterEach, expect, test, vi } from "vitest";

const interval = vi.hoisted(() => ({ start: vi.fn(), stop: vi.fn() }));
vi.mock("$shared/lib/visible-interval", () => ({
  everyWhileVisible: interval.start,
}));

afterEach(() => {
  vi.useRealTimers();
  vi.resetModules();
  vi.clearAllMocks();
});

test("reopening tasks after midnight refreshes today before the first timer tick", async () => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date(2026, 9, 26, 23, 59));
  interval.start.mockReturnValue(interval.stop);
  const { today, watchToday } = await import("../lib/today.svelte");
  const close = watchToday();
  expect(today()).toBe("2026-10-26");
  close();

  vi.setSystemTime(new Date(2026, 9, 27, 0, 1));
  const closeAgain = watchToday();
  try {
    expect(today()).toBe("2026-10-27");
    expect(interval.start).toHaveBeenCalledTimes(2);
  } finally {
    closeAgain();
  }
});

test("task surfaces share one timer and stop it only after the last closes", async () => {
  interval.start.mockReturnValue(interval.stop);
  const { watchToday } = await import("../lib/today.svelte");
  const closeFirst = watchToday();
  const closeSecond = watchToday();
  expect(interval.start).toHaveBeenCalledTimes(1);
  closeFirst();
  expect(interval.stop).not.toHaveBeenCalled();
  closeSecond();
  expect(interval.stop).toHaveBeenCalledTimes(1);
});
