import { afterEach, expect, test, vi } from "vitest";
import * as attention from "$session/attention.svelte";
import { holdWhilePopupOpen } from "../lib/popup-hold";

afterEach(() => vi.useRealTimers());

test("the column stays out while the popup has the keyboard", () => {
  vi.useFakeTimers();
  holdWhilePopupOpen();
  window.dispatchEvent(new Event("blur"));
  vi.advanceTimersByTime(10_000);
  expect(attention.wanted()).toBe(true);
  window.dispatchEvent(new Event("focus"));
  expect(attention.wanted()).toBe(false);
});

test("a hold whose keyboard never comes back to the window still ends", () => {
  vi.useFakeTimers();
  holdWhilePopupOpen();
  window.dispatchEvent(new Event("blur"));
  vi.advanceTimersByTime(45_000);
  expect(attention.wanted()).toBe(false);
});

test("a popup that never takes the keyboard lets go by itself", () => {
  vi.useFakeTimers();
  holdWhilePopupOpen();
  expect(attention.wanted()).toBe(true);
  vi.advanceTimersByTime(1500);
  expect(attention.wanted()).toBe(false);
});

test("the popup closing by a click on the page lets the column go", () => {
  vi.useFakeTimers();
  holdWhilePopupOpen();
  window.dispatchEvent(new Event("blur"));
  window.dispatchEvent(new CustomEvent("zephium:popover-closed"));
  expect(attention.wanted()).toBe(false);
});
