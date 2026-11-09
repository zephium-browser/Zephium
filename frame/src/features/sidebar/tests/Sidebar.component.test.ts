import { afterEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { userEvent } from "vitest/browser";
import { tick } from "svelte";
import * as sidebar from "$session/sidebar-mode.svelte";
import * as attention from "$session/attention.svelte";
import * as tools from "$session/tools.svelte";
import SidebarHost from "./SidebarHost.svelte";

const native = vi.hoisted(() => ({
  set: vi.fn(async () => ({ accepted: true, operation_id: null })),
}));

vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    settingSet: native.set,
    settingGet: async (key) => (key === "sidebar.mode" ? "hidden" : null),
    sidebarPeek: async () => {},
    sidebarSetWidth: async () => {},
    sidebarSetHidden: async () => {},
    sidebarResize: async () => false,
  });
});

afterEach(() => {
  tools.close();
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

test("the card stays while native has the pointer over it, and goes when it leaves", async () => {
  const aside = await peek();
  window.dispatchEvent(
    new CustomEvent("zephium:sidebar-edge", { detail: { near: false, over: true } }),
  );
  aside.dispatchEvent(new PointerEvent("pointerleave"));
  await vi.advanceTimersByTimeAsync(1000);
  expect(aside.dataset.peek).toBe("true");
  window.dispatchEvent(
    new CustomEvent("zephium:sidebar-edge", { detail: { near: false, over: false } }),
  );
  await vi.advanceTimersByTimeAsync(1000);
  expect(aside.dataset.peek).toBeUndefined();
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

test("the hidden card resizes live without becoming a window drag region or snapping", async () => {
  const aside = await peek();
  const handle = aside.querySelector<HTMLElement>('[aria-label="Resize sidebar"]')!;
  expect(handle.getAttribute("aria-disabled")).toBe("false");
  expect(handle.dataset.tauriDragRegion).toBe("false");
  expect(handle.classList.contains("pointer-events-none")).toBe(false);
  // Synthetic pointer events have no browser pointer to capture.
  const capture = vi.spyOn(handle, "setPointerCapture").mockImplementation(() => {});
  handle.dispatchEvent(
    new PointerEvent("pointerdown", { bubbles: true, button: 0, clientX: 240, pointerId: 1 }),
  );
  expect(capture).toHaveBeenCalledWith(1);
  expect(sidebar.sidebarResizeActive()).toBe(true);
  handle.dispatchEvent(
    new PointerEvent("pointermove", { bubbles: true, clientX: 340, pointerId: 1 }),
  );
  await tick();
  expect(aside.style.width).toBe("340px");
  expect(sidebar.sidebarResizeActive()).toBe(true);
  await leave(aside);
  expect(aside.dataset.peek).toBe("true");
  handle.dispatchEvent(
    new PointerEvent("pointermove", { bubbles: true, clientX: 56, pointerId: 1 }),
  );
  await tick();
  expect(aside.style.width).toBe("180px");
  handle.dispatchEvent(new PointerEvent("pointerup", { bubbles: true, clientX: 56, pointerId: 1 }));
  expect(sidebar.sidebarResizeActive()).toBe(false);
  expect(sidebar.sidebarMode()).toBe("hidden");
  expect(native.set).toHaveBeenCalledWith("sidebar.width", "180");
});

test("Escape closes an open tool, after first leaving a field typed in", async () => {
  await sidebar.init();
  const screen = await render(SidebarHost);
  const aside = document.querySelector<HTMLElement>(".browser-sidebar")!;
  tools.open("notes");
  await expect.element(screen.getByRole("textbox", { name: "Tool field" })).toBeVisible();
  const field = aside.querySelector<HTMLInputElement>('input[aria-label="Tool field"]')!;
  field.focus();
  await userEvent.keyboard("{Escape}");
  expect(document.activeElement).not.toBe(field);
  expect(tools.activeTool()).toBe("notes");
  await userEvent.keyboard("{Escape}");
  expect(tools.activeTool()).toBeNull();
  await expect.element(screen.getByRole("textbox", { name: "Sidebar field" })).toBeVisible();
});
