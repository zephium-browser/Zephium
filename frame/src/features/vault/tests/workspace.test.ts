import { describe, expect, it } from "vitest";
import { Workspace } from "../lib/workspace.svelte";

function memory() {
  const map = new Map<string, string>();
  return {
    getItem: (key: string) => map.get(key) ?? null,
    setItem: (key: string, value: string) => void map.set(key, value),
  };
}

describe("Workspace", () => {
  it("opens, cycles and closes tabs in the focused pane", () => {
    const space = new Workspace("p", memory());
    space.open("a.pdf");
    space.open("b.md");
    expect(space.pane.tabs).toEqual(["a.pdf", "b.md"]);
    expect(space.current).toBe("b.md");
    space.cycle(1);
    expect(space.current).toBe("a.pdf");
    space.cycle(-1);
    expect(space.current).toBe("b.md");
    space.closeCurrent();
    expect(space.current).toBe("a.pdf");
  });

  it("splits into up to four panes and merges tabs back", () => {
    const space = new Workspace("p", memory());
    space.open("a.pdf");
    space.openBeside("b.md");
    expect(space.layout).toBe("columns");
    expect(space.panes[1]?.tabs).toEqual(["b.md"]);
    space.setLayout("grid");
    expect(space.panes).toHaveLength(4);
    space.setLayout("single");
    expect(space.panes[0]?.tabs).toEqual(["a.pdf", "b.md"]);
  });

  it("follows renames and trashes", () => {
    const space = new Workspace("p", memory());
    space.open("ITF/CA1/Brief.pdf");
    space.renamed("ITF/CA1", "ITF/CA2");
    expect(space.current).toBe("ITF/CA2/Brief.pdf");
    space.removed("ITF");
    expect(space.pane.tabs).toEqual([]);
  });

  it("restores the last workspace and named layouts", () => {
    const storage = memory();
    const first = new Workspace("p", storage);
    first.open("a.pdf");
    first.openBeside("b.md");
    first.saveLayout("study");
    first.setLayout("single");
    const second = new Workspace("p", storage);
    expect(second.layout).toBe("single");
    expect(second.loadLayout("study")).toBe(true);
    expect(second.panes.map((pane) => pane.tabs)).toEqual([["a.pdf"], ["b.md"]]);
  });
});
