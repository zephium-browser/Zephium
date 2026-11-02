import { afterEach, expect, test, vi } from "vitest";
import * as attention from "../attention.svelte";

afterEach(() => vi.useRealTimers());

test("the column is wanted while anything holds it, and each release counts once", () => {
  const first = attention.hold();
  const second = attention.hold();
  first();
  first();
  expect(attention.wanted()).toBe(true);
  second();
  expect(attention.wanted()).toBe(false);
});

test("a pulse lets go by itself", () => {
  vi.useFakeTimers();
  attention.pulse(1000);
  expect(attention.wanted()).toBe(true);
  vi.advanceTimersByTime(1000);
  expect(attention.wanted()).toBe(false);
});
