import { describe, expect, it } from "vitest";
import { treePlace, type TreeBlock, type TreeRow } from "../tree-drop";

const tab = (id: string, depth = 0): TreeRow => ({ id, depth, folder: false });
const folder = (id: string, depth = 0): TreeRow => ({ id, depth, folder: true });
const block = (overrides: Partial<TreeBlock> = {}): TreeBlock => ({
  depth: 0,
  folder: false,
  levels: 1,
  fileable: true,
  ...overrides,
});

// a
// F
//   b
//   c
// d
const rows = [tab("a"), folder("F"), tab("b", 1), tab("c", 1), tab("d")];

describe("tree drop placement", () => {
  it("lands between top-level rows before the one below", () => {
    expect(treePlace(rows, 1, block(), 0)).toEqual({ depth: 0, parent: null, before: "F" });
    expect(treePlace(rows, 5, block(), 0)).toEqual({ depth: 0, parent: null, before: null });
  });

  it("goes into a folder when it waits among the folder's rows", () => {
    expect(treePlace(rows, 3, block(), 0)).toEqual({ depth: 1, parent: "F", before: "c" });
    expect(treePlace(rows, 2, block(), -40)).toEqual({ depth: 1, parent: "F", before: "b" });
  });

  it("picks the level by sideways travel at the end of a folder", () => {
    expect(treePlace(rows, 4, block(), 0)).toEqual({ depth: 0, parent: null, before: "d" });
    expect(treePlace(rows, 4, block(), 13)).toEqual({ depth: 1, parent: "F", before: null });
    expect(treePlace(rows, 4, block(), 80)).toEqual({ depth: 1, parent: "F", before: null });
  });

  it("goes into an empty or folded folder only when set in", () => {
    const empty = [tab("a"), folder("E"), tab("d")];
    expect(treePlace(empty, 2, block(), 0)).toEqual({ depth: 0, parent: null, before: "d" });
    expect(treePlace(empty, 2, block(), 14)).toEqual({ depth: 1, parent: "E", before: null });
  });

  it("keeps folders within the level limit and pages the browser owns out of folders", () => {
    const deep = [folder("A"), folder("B", 1), folder("C", 2), tab("x", 3)];
    expect(treePlace(deep, 4, block({ folder: true }), 60)?.depth).toBe(2);
    expect(treePlace(deep, 4, block({ folder: true, levels: 2 }), 60)?.depth).toBe(1);
    expect(treePlace(deep, 3, block({ folder: true }), 0)).toBeNull();
    expect(treePlace(rows, 3, block({ fileable: false }), 0)).toBeNull();
    expect(treePlace(rows, 4, block({ fileable: false }), 30)).toEqual({
      depth: 0,
      parent: null,
      before: "d",
    });
  });

  it("refuses a level whose holder is not a folder", () => {
    const broken = [tab("a"), tab("b", 1)];
    expect(treePlace(broken, 2, block(), 13)).toBeNull();
  });
});
