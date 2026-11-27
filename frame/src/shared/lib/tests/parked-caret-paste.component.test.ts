import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { parkCaret } from "../parked-caret";

vi.mock("$shared/platform", () => ({ IS_MAC: false }));

let input: HTMLInputElement;
let stop: () => void;

beforeEach(() => {
  vi.useFakeTimers();
  vi.spyOn(document, "hasFocus").mockReturnValue(true);
  input = document.createElement("input");
  document.body.append(input);
  input.focus();
  stop = parkCaret(input);
});

afterEach(() => {
  stop();
  input.remove();
  vi.useRealTimers();
  vi.restoreAllMocks();
});

test.each([
  ["Ctrl+V", { key: "v", ctrlKey: true }],
  ["Ctrl+Shift+V", { key: "V", ctrlKey: true, shiftKey: true }],
  ["Ctrl+V on a non-Latin layout", { key: "м", ctrlKey: true }],
])("%s wakes a parked field before the paste", (_, init) => {
  vi.advanceTimersByTime(4100);
  expect(input.dataset.caretParked).toBe("");
  const event = new KeyboardEvent("keydown", { ...init, code: "KeyV", cancelable: true });
  window.dispatchEvent(event);
  expect(document.activeElement).toBe(input);
  expect(input.dataset.caretParked).toBeUndefined();
  expect(event.defaultPrevented).toBe(false);
});

test("other Ctrl shortcuts leave it parked", () => {
  vi.advanceTimersByTime(4100);
  window.dispatchEvent(new KeyboardEvent("keydown", { key: "k", code: "KeyK", ctrlKey: true }));
  expect(document.activeElement).not.toBe(input);
  expect(input.dataset.caretParked).toBe("");
});
