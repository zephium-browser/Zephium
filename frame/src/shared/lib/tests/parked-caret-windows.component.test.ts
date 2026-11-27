import { afterEach, expect, test, vi } from "vitest";
import { parkCaret } from "../parked-caret";

vi.mock("$shared/platform", () => ({ IS_MAC: false, IS_WINDOWS: true }));

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

test("an idle empty field parks a caret that stands still", () => {
  vi.useFakeTimers();
  vi.spyOn(document, "hasFocus").mockReturnValue(true);
  const field = document.createElement("form");
  const input = document.createElement("input");
  field.append(input);
  document.body.append(field);
  input.focus();
  const stop = parkCaret(input);
  vi.advanceTimersByTime(4100);
  expect(document.activeElement).not.toBe(input);
  const caret = field.querySelector("span[aria-hidden='true']");
  expect(caret).not.toBeNull();
  expect(caret?.getAnimations()).toHaveLength(0);
  stop();
  field.remove();
});
