import { describe, expect, it } from "vitest";
import { journalPath, monthGrid } from "../lib/journal";

describe("journal", () => {
  it("names daily notes like Obsidian", () => {
    expect(journalPath(new Date(2026, 10, 1))).toBe("Journal/2026-11-01.md");
  });

  it("lays out six weeks from Sunday", () => {
    const grid = monthGrid(new Date(2026, 10, 15));
    expect(grid.days).toHaveLength(42);
    expect(grid.days[0]?.getDay()).toBe(0);
    expect(grid.days[0]?.getDate()).toBe(1);
  });
});
