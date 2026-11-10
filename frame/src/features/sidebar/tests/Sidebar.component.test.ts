import { afterEach, describe, expect, test, vi } from "vitest";
import { cleanup, render } from "vitest-browser-svelte";
import { userEvent } from "vitest/browser";
import * as sidebar from "$session/sidebar-mode.svelte";
import * as attention from "$session/attention.svelte";
import { commands } from "$shared/ipc/bindings";
import { IS_WINDOWS } from "$shared/platform";
import { flushSync } from "svelte";
import SidebarHost from "./SidebarHost.svelte";

// Browser-mode module exports snapshot getters. Select the platform before
// importing Sidebar; run Windows coverage with VITE_SIDEBAR_WINDOWS=true.
vi.mock("$shared/platform", () => {
  const windows = import.meta.env.VITE_SIDEBAR_WINDOWS === "true";
  return { IS_WINDOWS: windows, IS_MAC: !windows };
});
vi.mock("$shared/lib/motion", async (original) => ({
  ...(await original<typeof import("$shared/lib/motion")>()),
  duration: () => 300,
  reducedMotion: () => false,
}));

vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    settingGet: async (key) => (key === "sidebar.mode" ? "hidden" : null),
    settingSet: async () => ({ operation_id: null, accepted: true }),
    sidebarPeek: async () => {},
    sidebarSetWidth: async () => {},
    sidebarSetHidden: async () => {},
    sidebarResize: async () => false,
  });
});

afterEach(async () => {
  await cleanup();
  sidebar.holdShown(false);
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

describe(IS_WINDOWS ? "Windows" : "macOS", () => {
  const windows = IS_WINDOWS;
  for (const interruption of ["pin", "visible", "unmount", "reload"] as const) {
    test(`${interruption} ends a peek exactly once`, async () => {
      const native = vi.spyOn(commands, "sidebarPeek");
      const aside = await peek();
      await expect.poll(() => native.mock.calls.some(([raised]) => raised)).toBe(true);
      if (["pin", "visible"].includes(interruption)) {
        window.dispatchEvent(new CustomEvent("zephium:sidebar-edge", { detail: { near: false } }));
        aside.dispatchEvent(new PointerEvent("pointerleave"));
        await vi.advanceTimersByTimeAsync(520);
        // The card is closing, but macOS has not lowered its native chrome yet.
        if (!windows) {
          expect(aside.dataset.peek).toBeUndefined();
          expect(native.mock.calls.filter(([raised]) => !raised)).toHaveLength(0);
        }
        if (interruption === "pin") flushSync(() => sidebar.setMode("default"));
        if (interruption === "visible") flushSync(() => sidebar.holdShown(true));
      } else if (interruption === "unmount") {
        await cleanup();
      } else {
        window.dispatchEvent(new Event("pagehide"));
      }
      await vi.advanceTimersByTimeAsync(1000);
      await cleanup();
      expect(native.mock.calls.filter(([raised]) => !raised)).toHaveLength(1);
    });
  }

  test("unmount cancels a queued peek animation", async () => {
    await sidebar.init();
    await render(SidebarHost);
    const native = vi.spyOn(commands, "sidebarPeek");
    const frames: FrameRequestCallback[] = [];
    vi.spyOn(window, "requestAnimationFrame").mockImplementation((callback) => {
      frames.push(callback);
      return frames.length;
    });
    window.dispatchEvent(new CustomEvent("zephium:sidebar-edge", { detail: { near: true } }));
    await Promise.resolve();
    await cleanup();
    const raisesBeforeFrames = native.mock.calls.filter(([raised]) => raised).length;
    expect(raisesBeforeFrames).toBe(windows ? 0 : 1);
    for (const callback of frames) callback(performance.now());
    expect(native.mock.calls.filter(([raised]) => raised)).toHaveLength(raisesBeforeFrames);
    expect(native.mock.calls.filter(([raised]) => !raised)).toHaveLength(windows ? 0 : 1);
  });
});
