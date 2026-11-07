import { describe, expect, it, vi } from "vitest";

const native = vi.hoisted(() => ({
  width: vi.fn(async (_width: number | null, _travel: boolean, _revision: number | null) => {}),
  hidden: vi.fn(async (_hidden: boolean) => {}),
}));

vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    sidebarSetWidth: native.width,
    sidebarSetHidden: native.hidden,
    settingSet: vi.fn(async () => ({ operation_id: null, accepted: true })),
    settingGet: vi.fn(async () => null),
  });
});

const {
  COMPACT_WIDTH,
  MAX_EXPANDED_WIDTH,
  MIN_EXPANDED_WIDTH,
  SNAP_THRESHOLD,
  applyDragWidth,
  adoptResizeWidth,
  beginSidebarResize,
  finishSidebarResize,
  cancelSidebarResize,
  expanded,
  resolveDragWidth,
  adoptMode,
  holdShown,
  setPanelExtent,
  sidebarMode,
  toggleMode,
} = await import("../sidebar-mode.svelte");

describe("resolveDragWidth", () => {
  it("puts the column away below the threshold", () => {
    for (const value of [0, 1, COMPACT_WIDTH, SNAP_THRESHOLD - 1]) {
      expect(resolveDragWidth(value).mode).toBe("hidden");
    }
  });

  it("expands at and above the threshold", () => {
    expect(resolveDragWidth(SNAP_THRESHOLD).mode).toBe("default");
    expect(resolveDragWidth(300).mode).toBe("default");
  });

  it("never settles between the two designed shapes", () => {
    // Anything that resolves to `default` must be a legal expanded width, so a
    // drag can never leave the sidebar at an in-between size.
    for (let value = 0; value <= 600; value += 7) {
      const resolved = resolveDragWidth(value);
      if (resolved.mode === "default") {
        expect(resolved.expanded).toBeGreaterThanOrEqual(MIN_EXPANDED_WIDTH);
        expect(resolved.expanded).toBeLessThanOrEqual(MAX_EXPANDED_WIDTH);
      }
    }
  });

  it("treats a non-finite width as put away rather than propagating it", () => {
    for (const value of [Number.NaN, Number.POSITIVE_INFINITY, Number.NEGATIVE_INFINITY]) {
      expect(resolveDragWidth(value).mode).toBe("hidden");
    }
  });
});

describe("which width changes the page travels with", () => {
  const last = () => native.width.mock.calls.at(-1)?.slice(0, 2) as unknown as [number, boolean];

  const hidden = () => native.hidden.mock.calls.at(-1)?.[0];

  it("slides for a deliberate change of shape and follows a drag directly", () => {
    const width = expanded();
    toggleMode();
    // Hidden, the column keeps its width for when it comes over the page.
    expect(last()).toEqual([width, true]);
    expect(hidden()).toBe(true);
    toggleMode();
    expect(last()[1]).toBe(true);
    expect(hidden()).toBe(false);

    applyDragWidth(300);
    expect(last()).toEqual([300, false]);
    applyDragWidth(310);
    expect(last()).toEqual([310, false]);

    // Crossing the snap point is a change of shape, however it was reached.
    applyDragWidth(SNAP_THRESHOLD - 1);
    expect(last()).toEqual([310, true]);
    expect(hidden()).toBe(true);
    applyDragWidth(SNAP_THRESHOLD + 40);
    expect(last()[1]).toBe(true);
    expect(hidden()).toBe(false);
  });

  it("keeps a hidden column out while a rail destination holds it", () => {
    toggleMode();
    expect(hidden()).toBe(true);
    holdShown(true);
    expect(hidden()).toBe(false);
    holdShown(false);
    expect(hidden()).toBe(true);
    toggleMode();
  });

  it("reads a rail chosen before hiding existed as hidden", () => {
    // The store reports the last toggle's save first, as it would.
    adoptMode("default");
    adoptMode("compact");
    expect(sidebarMode()).toBe("hidden");
    adoptMode("default");
  });

  it("slides when a tool opens beside the rail and when it closes", () => {
    setPanelExtent(336);
    expect(last()[1]).toBe(true);
    setPanelExtent(0);
    expect(last()[1]).toBe(true);
  });

  it("keeps the sidebar and layout fixed while a guide moves, then commits once, travelling", () => {
    const originalWidth = expanded();
    const originalMode = sidebarMode();
    const before = native.width.mock.calls.length;
    beginSidebarResize();
    applyDragWidth(315);
    expect(expanded()).toBe(originalWidth);
    expect(sidebarMode()).toBe(originalMode);
    expect(native.width.mock.calls.length).toBe(before);
    finishSidebarResize(315);
    expect(native.width.mock.calls.length).toBe(before + 1);
    expect(last()).toEqual([315, true]);
  });

  it("cancellation leaves the original shape untouched and emits no layout", () => {
    const originalMode = sidebarMode();
    const originalWidth = expanded();
    const before = native.width.mock.calls.length;
    beginSidebarResize();
    applyDragWidth(SNAP_THRESHOLD - 1);
    cancelSidebarResize();
    finishSidebarResize(300);
    expect(sidebarMode()).toBe(originalMode);
    expect(expanded()).toBe(originalWidth);
    expect(native.width.mock.calls.length).toBe(before);
  });

  it("lands an overshooting release on the nearest bound instead of dropping it", () => {
    beginSidebarResize();
    finishSidebarResize(MAX_EXPANDED_WIDTH + 200);
    expect(expanded()).toBe(MAX_EXPANDED_WIDTH);
    expect(last()).toEqual([MAX_EXPANDED_WIDTH, true]);
    beginSidebarResize();
    finishSidebarResize(315);
  });

  it("adopts a native selection without dispatching a second width change", () => {
    const before = native.width.mock.calls.length;
    adoptResizeWidth(320);
    expect(expanded()).toBe(320);
    expect(native.width.mock.calls.length).toBe(before);
  });

  it("a deliberate toggle retires a guide before a stale pointer release", () => {
    const before = native.width.mock.calls.length;
    beginSidebarResize();
    applyDragWidth(SNAP_THRESHOLD - 1);
    expect(sidebarMode()).toBe("default");
    toggleMode();
    cancelSidebarResize();
    finishSidebarResize(300);
    expect(native.width.mock.calls.length).toBe(before + 1);
    expect(sidebarMode()).toBe("hidden");
    expect(last()[1]).toBe(true);
    toggleMode();
  });
});

describe("the stored preference catching up with a toggle", () => {
  it("never bounces the column back to the shape it just left", () => {
    adoptMode("default");
    vi.useFakeTimers();
    native.width.mockClear();
    toggleMode();
    expect(sidebarMode()).toBe("hidden");
    expect(native.width.mock.calls.map((call) => call[1])).toEqual([true]);

    // The store still holds the old value, then reports the new one: neither
    // is a change of shape, so neither moves anything.
    adoptMode("default");
    adoptMode("hidden");
    expect(sidebarMode()).toBe("hidden");
    expect(native.width.mock.calls).toHaveLength(1);
    vi.useRealTimers();
  });

  it("ignores the report of a save already overtaken by the next toggle", () => {
    vi.useFakeTimers();
    toggleMode();
    toggleMode();
    const shape = sidebarMode();
    const sent = native.width.mock.calls.length;
    adoptMode(shape === "hidden" ? "default" : "hidden");
    adoptMode(shape);
    expect(sidebarMode()).toBe(shape);
    expect(native.width.mock.calls).toHaveLength(sent);
    vi.useRealTimers();
  });

  it("still follows a change made elsewhere, and a save whose report never came", () => {
    vi.useFakeTimers();
    toggleMode();
    const shape = sidebarMode();
    vi.advanceTimersByTime(2000);
    const other = shape === "hidden" ? "default" : "hidden";
    adoptMode(other);
    expect(sidebarMode()).toBe(other);
    expect((native.width.mock.calls.at(-1)?.slice(0, 2) as unknown as [number, boolean])[1]).toBe(
      false,
    );
    vi.useRealTimers();
  });
});
