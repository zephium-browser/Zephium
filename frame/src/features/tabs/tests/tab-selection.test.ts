import { describe, expect, test } from "vitest";
import {
  emptySelection,
  isSelectionModifier,
  plainSelection,
  pruneSelection,
  rangeSelection,
  toggleSelection,
} from "../lib/tab-selection";

describe("tab selection", () => {
  test.each([true, false])("selection modifiers respect the platform (mac: %s)", (mac) => {
    const plain = { metaKey: false, ctrlKey: false, shiftKey: false };
    expect(isSelectionModifier(plain, mac)).toBe(false);
    expect(isSelectionModifier({ ...plain, metaKey: true }, mac)).toBe(mac);
    expect(isSelectionModifier({ ...plain, ctrlKey: true }, mac)).toBe(!mac);
    expect(isSelectionModifier({ ...plain, shiftKey: true }, mac)).toBe(true);
  });

  test("the first toggle includes the active tab and never changes it", () => {
    const state = toggleSelection(emptySelection(), "b", "a");
    expect(state).toEqual({ ids: ["a", "b"], anchor: "b" });
    expect(toggleSelection(state, "c", "a")).toEqual({ ids: ["a", "b", "c"], anchor: "c" });
    expect(toggleSelection(state, "b", "a")).toEqual({ ids: ["a"], anchor: "b" });
    expect(state.ids).toEqual(["a", "b"]);
  });

  test("toggling the active tab or the last selected tab can empty the selection", () => {
    expect(toggleSelection(emptySelection(), "a", "a")).toEqual({ ids: [], anchor: "a" });
    expect(toggleSelection({ ids: ["b"], anchor: "b" }, "b", "a").ids).toEqual([]);
    expect(toggleSelection(emptySelection(), "b", null)).toEqual({ ids: ["b"], anchor: "b" });
  });

  test("ranges follow display order, not tab or ID order, in either direction", () => {
    const order = ["d", "b", "a", "c"];
    const state = rangeSelection(emptySelection(), "c", "b", order);
    expect(state).toEqual({ ids: ["b", "a", "c"], anchor: "b" });
    expect(rangeSelection(state, "d", "a", order)).toEqual({
      ids: ["d", "b"],
      anchor: "b",
    });
  });

  test("plain clicks and toggles set the anchor; repeated ranges retain it", () => {
    const order = ["a", "b", "c", "d"];
    const plain = plainSelection("b");
    const range = rangeSelection(plain, "d", "a", order);
    expect(range.ids).toEqual(["b", "c", "d"]);
    expect(rangeSelection(range, "c", "a", order).ids).toEqual(["b", "c"]);
    expect(rangeSelection(toggleSelection(range, "a", "a"), "c", "d", order).ids).toEqual([
      "a",
      "b",
      "c",
    ]);
    expect(plain).toEqual({ ids: [], anchor: "b" });
  });

  test("hidden anchors fall back to a visible active tab, then the target", () => {
    const state = { ids: ["hidden"], anchor: "hidden" };
    expect(rangeSelection(state, "d", "b", ["b", "d"])).toEqual({ ids: ["b", "d"], anchor: "b" });
    expect(rangeSelection(state, "d", "elsewhere", ["b", "d"])).toEqual({
      ids: ["d"],
      anchor: "d",
    });
    expect(rangeSelection(state, "missing", "b", ["b", "d"])).toBe(state);
  });

  test("pruning removes missing tabs and anchors without reseeding the active tab", () => {
    const state = { ids: ["a", "b", "c"], anchor: "b" };
    const pruned = pruneSelection(state, new Set(["a", "c"]));
    expect(pruned).toEqual({ ids: ["a", "c"], anchor: null });
    expect(pruneSelection(pruned, new Set(["a", "c"]))).toBe(pruned);
    expect(pruneSelection(pruned, new Set(["a"]))).toEqual({ ids: ["a"], anchor: null });
    expect(pruneSelection(state, new Set())).toEqual(emptySelection());
  });
});
