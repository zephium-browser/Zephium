import { describe, expect, it, vi } from "vitest";

const native = vi.hoisted(() => ({
  width: vi.fn(async () => undefined),
}));

vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    sidebarSetWidth: native.width,
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
  setPanelExtent,
  sidebarMode,
  toggleMode,
} = await import("../sidebar-mode.svelte");

describe("resolveDragWidth", () => {
  it("snaps to the rail below the threshold", () => {
    for (const value of [0, 1, COMPACT_WIDTH, SNAP_THRESHOLD - 1]) {
      expect(resolveDragWidth(value).mode).toBe("compact");
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

  it("treats a non-finite width as the rail rather than propagating it", () => {
    for (const value of [Number.NaN, Number.POSITIVE_INFINITY, Number.NEGATIVE_INFINITY]) {
      expect(resolveDragWidth(value).mode).toBe("compact");
    }
  });
});

describe("which width changes the page travels with", () => {
  const last = () => native.width.mock.calls.at(-1)?.slice(0, 2) as unknown as [number, boolean];

  it("slides for a deliberate change of shape and follows a drag directly", () => {
    toggleMode();
    expect(last()).toEqual([COMPACT_WIDTH, true]);
    toggleMode();
    expect(last()[1]).toBe(true);

    applyDragWidth(300);
    expect(last()).toEqual([300, false]);
    applyDragWidth(310);
    expect(last()).toEqual([310, false]);

    // Crossing the snap point is a change of shape, however it was reached.
    applyDragWidth(SNAP_THRESHOLD - 1);
    expect(last()).toEqual([COMPACT_WIDTH, true]);
    applyDragWidth(SNAP_THRESHOLD + 40);
    expect(last()[1]).toBe(true);
  });

  it("slides when a tool opens beside the rail and when it closes", () => {
    setPanelExtent(336);
    expect(last()[1]).toBe(true);
    setPanelExtent(0);
    expect(last()[1]).toBe(true);
  });

  it("keeps the sidebar and layout fixed while a guide moves, then commits once", () => {
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
    expect(last()).toEqual([315, false]);
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
    expect(last()).toEqual([MAX_EXPANDED_WIDTH, false]);
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
    expect(sidebarMode()).toBe("compact");
    expect(last()).toEqual([COMPACT_WIDTH, true]);
    toggleMode();
  });
});

describe("the stored preference catching up with a toggle", () => {
  it("never bounces the column back to the shape it just left", () => {
    adoptMode("default");
    vi.useFakeTimers();
    native.width.mockClear();
    toggleMode();
    expect(sidebarMode()).toBe("compact");
    expect(native.width.mock.calls.map((call) => call.slice(0, 2))).toEqual([
      [COMPACT_WIDTH, true],
    ]);

    // The store still holds the old value, then reports the new one: neither
    // is a change of shape, so neither moves anything.
    adoptMode("default");
    adoptMode("compact");
    expect(sidebarMode()).toBe("compact");
    expect(native.width.mock.calls).toHaveLength(1);
    vi.useRealTimers();
  });

  it("ignores the report of a save already overtaken by the next toggle", () => {
    vi.useFakeTimers();
    toggleMode();
    toggleMode();
    const shape = sidebarMode();
    const sent = native.width.mock.calls.length;
    adoptMode(shape === "compact" ? "default" : "compact");
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
    const other = shape === "compact" ? "default" : "compact";
    adoptMode(other);
    expect(sidebarMode()).toBe(other);
    expect((native.width.mock.calls.at(-1)?.slice(0, 2) as unknown as [number, boolean])[1]).toBe(
      false,
    );
    vi.useRealTimers();
  });
});
