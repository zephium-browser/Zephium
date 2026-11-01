import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { parkCaret } from "../parked-caret";

vi.mock("$shared/platform", () => ({ IS_MAC: true }));

let field: HTMLFormElement;
let input: HTMLInputElement;
let stop: () => void;

beforeEach(() => {
  vi.useFakeTimers();
  vi.spyOn(document, "hasFocus").mockReturnValue(true);
  field = document.createElement("form");
  input = document.createElement("input");
  field.append(input);
  document.body.append(field);
  input.focus();
  stop = parkCaret(input);
});

afterEach(() => {
  stop();
  field.remove();
  vi.useRealTimers();
  vi.restoreAllMocks();
});

const caret = () => field.querySelector("span[aria-hidden='true']");

test("an idle empty field hands its caret to a drawn one", () => {
  vi.advanceTimersByTime(3900);
  expect(document.activeElement).toBe(input);
  vi.advanceTimersByTime(200);
  expect(document.activeElement).not.toBe(input);
  expect(input.dataset.caretParked).toBe("");
  expect(caret()).not.toBeNull();
});

test("the next typed key brings focus back, and shortcuts don't", () => {
  vi.advanceTimersByTime(4100);
  window.dispatchEvent(new KeyboardEvent("keydown", { key: "t", metaKey: true }));
  expect(document.activeElement).not.toBe(input);
  window.dispatchEvent(new KeyboardEvent("keydown", { key: "a" }));
  expect(document.activeElement).toBe(input);
  expect(input.dataset.caretParked).toBeUndefined();
  expect(caret()).toBeNull();
});

test("dead keys and AltGr bring focus back, plain Control doesn't", () => {
  vi.advanceTimersByTime(4100);
  window.dispatchEvent(new KeyboardEvent("keydown", { key: "k", ctrlKey: true }));
  expect(document.activeElement).not.toBe(input);
  window.dispatchEvent(new KeyboardEvent("keydown", { key: "Dead" }));
  expect(document.activeElement).toBe(input);
});

test("a field with text keeps its caret, and clicking elsewhere lets go", () => {
  input.value = "zephium";
  input.dispatchEvent(new Event("input"));
  vi.advanceTimersByTime(10_000);
  expect(document.activeElement).toBe(input);
  input.value = "";
  input.dispatchEvent(new Event("input"));
  vi.advanceTimersByTime(4100);
  document.body.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true }));
  expect(caret()).toBeNull();
  expect(document.activeElement).not.toBe(input);
});
