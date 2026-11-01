import { describe, expect, it } from "vitest";
import type { VaultEntry } from "$shared/ipc/bindings";
import { ancestors, renamedWithExtension, visibleRows } from "../lib/tree";
import { badgeFor, viewerFor } from "../lib/kinds";

const folder = (path: string): VaultEntry => ({
  name: path.split("/").at(-1) ?? path,
  path,
  kind: "folder",
  ext: "",
  size: "0",
  modified: "0",
});
const file = (path: string, ext: string): VaultEntry => ({ ...folder(path), kind: "file", ext });

describe("tree", () => {
  it("shows children of expanded folders only", () => {
    const listings = new Map([
      ["", [folder("DBKF"), file("a.md", "md")]],
      ["DBKF", [file("DBKF/Brief.pdf", "pdf")]],
    ]);
    expect(visibleRows(listings, new Set()).map((row) => row.entry.path)).toEqual(["DBKF", "a.md"]);
    const open = visibleRows(listings, new Set(["DBKF"]));
    expect(open.map((row) => [row.entry.path, row.depth])).toEqual([
      ["DBKF", 0],
      ["DBKF/Brief.pdf", 1],
      ["a.md", 0],
    ]);
  });

  it("knows ancestors and keeps extensions on rename", () => {
    expect(ancestors("a/b/c.pdf")).toEqual(["a", "a/b"]);
    expect(renamedWithExtension("Brief.pdf", "CA1 Brief")).toBe("CA1 Brief.pdf");
    expect(renamedWithExtension("Brief.pdf", "notes.txt")).toBe("notes.txt");
    expect(renamedWithExtension("Folder", "New")).toBe("New");
  });

  it("maps extensions to viewers and badges", () => {
    expect(viewerFor("PDF")).toBe("pdf");
    expect(viewerFor("xlsm")).toBe("sheet");
    expect(viewerFor("md")).toBe("markdown");
    expect(viewerFor("pptx")).toBe("other");
    expect(badgeFor("pdf")).toBe("PDF");
    expect(badgeFor("md")).toBe("");
  });
});
