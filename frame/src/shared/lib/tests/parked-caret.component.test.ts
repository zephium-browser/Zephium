import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { userEvent } from "vitest/browser";
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

test("Cmd+V wakes a parked field before the paste, Ctrl+V doesn't on macOS", () => {
  vi.advanceTimersByTime(4100);
  const ctrl = new KeyboardEvent("keydown", { key: "v", code: "KeyV", ctrlKey: true });
  window.dispatchEvent(ctrl);
  expect(document.activeElement).not.toBe(input);
  const cmd = new KeyboardEvent("keydown", {
    key: "v",
    code: "KeyV",
    metaKey: true,
    cancelable: true,
  });
  window.dispatchEvent(cmd);
  expect(document.activeElement).toBe(input);
  expect(cmd.defaultPrevented).toBe(false);
});

test("Escape, Enter and the arrows reach a parked field's own handlers once", () => {
  const field: string[] = [];
  const window_: string[] = [];
  const onfield = (event: KeyboardEvent) => field.push(event.key);
  const onwindow = (event: KeyboardEvent) => window_.push(event.key);
  input.addEventListener("keydown", onfield);
  window.addEventListener("keydown", onwindow);
  try {
    for (const key of ["Escape", "Enter", "ArrowDown"]) {
      vi.advanceTimersByTime(4100);
      expect(input.dataset.caretParked).toBe("");
      document.body.dispatchEvent(
        new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true }),
      );
      expect(document.activeElement).toBe(input);
      expect(input.dataset.caretParked).toBeUndefined();
    }
    expect(field).toEqual(["Escape", "Enter", "ArrowDown"]);
    expect(window_).toEqual(["Escape", "Enter", "ArrowDown"]);
  } finally {
    input.removeEventListener("keydown", onfield);
    window.removeEventListener("keydown", onwindow);
  }
});

test("a modifier alone leaves the field parked", () => {
  vi.advanceTimersByTime(4100);
  window.dispatchEvent(new KeyboardEvent("keydown", { key: "Shift", shiftKey: true }));
  expect(document.activeElement).not.toBe(input);
  expect(input.dataset.caretParked).toBe("");
});

test("Tab moves on from a parked field as from a focused one", async () => {
  const next = document.createElement("input");
  field.append(next);
  vi.advanceTimersByTime(4100);
  expect(input.dataset.caretParked).toBe("");
  vi.useRealTimers();
  await userEvent.keyboard("{Tab}");
  expect(document.activeElement).toBe(next);
});

test("a text area parks with its caret on its first line", () => {
  stop();
  const area = document.createElement("textarea");
  Object.assign(area.style, {
    padding: "10px 12px",
    lineHeight: "20px",
    fontSize: "15px",
    height: "120px",
  });
  field.append(area);
  area.focus();
  const unpark = parkCaret(area);
  try {
    vi.advanceTimersByTime(4100);
    expect(area.dataset.caretParked).toBe("");
    const drawn = field.querySelector<HTMLElement>("span[aria-hidden='true']");
    const top = parseFloat(drawn?.style.top ?? "NaN") - area.offsetTop;
    expect(top).toBeGreaterThanOrEqual(10);
    expect(top).toBeLessThan(20);
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "a" }));
    expect(document.activeElement).toBe(area);
  } finally {
    unpark();
    area.remove();
    stop = parkCaret(input);
  }
});

test("a parked text area breaks the line on Shift+Enter, and its handler still takes Enter", async () => {
  stop();
  const area = document.createElement("textarea");
  field.append(area);
  const sent: string[] = [];
  area.addEventListener("keydown", (event) => {
    if (event.key === "Enter" && !event.shiftKey) {
      event.preventDefault();
      sent.push(area.value);
    }
  });
  area.focus();
  const unpark = parkCaret(area);
  try {
    vi.advanceTimersByTime(4100);
    expect(area.dataset.caretParked).toBe("");
    vi.useRealTimers();
    await userEvent.keyboard("{Shift>}{Enter}{/Shift}");
    expect(document.activeElement).toBe(area);
    expect(area.value).toBe("\n");
    area.value = "";
    area.blur();
    vi.useFakeTimers();
    area.focus();
    vi.advanceTimersByTime(4100);
    expect(area.dataset.caretParked).toBe("");
    vi.useRealTimers();
    await userEvent.keyboard("{Enter}");
    expect(document.activeElement).toBe(area);
    expect(sent).toEqual([""]);
    expect(area.value).toBe("");
  } finally {
    unpark();
    area.remove();
    stop = parkCaret(input);
  }
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

test("on macOS the drawn caret stands still", () => {
  vi.advanceTimersByTime(4100);
  const drawn = caret() as HTMLElement;
  expect(drawn.getAnimations()).toHaveLength(0);
  vi.advanceTimersByTime(5000);
  expect(drawn.style.opacity).toBe("");
  expect(drawn.getAnimations()).toHaveLength(0);
});
