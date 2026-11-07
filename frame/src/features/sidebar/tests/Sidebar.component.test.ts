import { afterEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { userEvent } from "vitest/browser";
import * as sidebar from "$session/sidebar-mode.svelte";
import * as attention from "$session/attention.svelte";
import SidebarHost from "./SidebarHost.svelte";

vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    settingGet: async (key) => (key === "sidebar.mode" ? "hidden" : null),
    sidebarPeek: async () => {},
    sidebarSetWidth: async () => {},
    sidebarSetHidden: async () => {},
    sidebarResize: async () => false,
  });
});

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

async function peek(panel = "") {
  await sidebar.init();
  await render(SidebarHost, { panel });
  const aside = document.querySelector<HTMLElement>(".browser-sidebar")!;
  window.dispatchEvent(new CustomEvent("zephium:sidebar-edge", { detail: { near: true } }));
  await expect.poll(() => aside.dataset.peek).toBe("true");
  vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
  return aside;
}

async function leave(aside: HTMLElement) {
  window.dispatchEvent(new CustomEvent("zephium:sidebar-edge", { detail: { near: false } }));
  aside.dispatchEvent(new PointerEvent("pointerleave"));
  await vi.advanceTimersByTimeAsync(1000);
}

test("a live Essential without a panel does not hold the card open", async () => {
  const aside = await peek();
  expect(aside.querySelector('.essential[data-open="true"]')).not.toBeNull();
  await userEvent.click(aside.querySelector<HTMLButtonElement>(".essential button")!);
  await leave(aside);
  expect(aside.dataset.peek).toBeUndefined();
});

test.each(["ui-disclosure-panel", "capture"])("an open %s holds the card", async (panel) => {
  const aside = await peek(panel);
  await leave(aside);
  expect(aside.dataset.peek).toBe("true");
});

test("attention still holds the card until released", async () => {
  const aside = await peek();
  const release = attention.hold();
  try {
    await leave(aside);
    expect(aside.dataset.peek).toBe("true");
  } finally {
    release();
  }
  await vi.advanceTimersByTimeAsync(1000);
  expect(aside.dataset.peek).toBeUndefined();
});

test("a stale input in unfocused chrome does not hold the card", async () => {
  const aside = await peek();
  const input = aside.querySelector("input")!;
  input.focus();
  vi.spyOn(document, "hasFocus").mockReturnValue(false);
  window.dispatchEvent(new Event("blur"));
  await leave(aside);
  expect(document.activeElement).toBe(input);
  expect(aside.dataset.peek).toBeUndefined();
});

test("keyboard focus between sidebar controls keeps the card visible", async () => {
  const aside = await peek();
  vi.spyOn(document, "hasFocus").mockReturnValue(true);
  const input = aside.querySelector("input")!;
  input.focus();
  await userEvent.keyboard("{Tab}");
  const control = aside.querySelector("input + button")!;
  expect(document.activeElement).toBe(control);
  expect(control.matches(":focus-visible")).toBe(true);
  await leave(aside);
  expect(aside.dataset.peek).toBe("true");
  (control as HTMLElement).blur();
  await vi.advanceTimersByTimeAsync(1000);
  expect(aside.dataset.peek).toBeUndefined();
});
