import { describe, expect, it } from "vitest";
import { markdownOutline, wikilinks } from "../lib/outline";

describe("outline", () => {
  it("lists headings outside code", () => {
    const text = "# Title\ntext\n```\n# not a heading\n```\n## Part A ##\n";
    expect(markdownOutline(text).map((item) => [item.title, item.level, item.target])).toEqual([
      ["Title", 1, 0],
      ["Part A", 2, 5],
    ]);
  });

  it("finds wikilinks", () => {
    expect(wikilinks("See [[ITF CA1]] and [[Brief.pdf|the brief]] and [[ITF CA1#Marks]]")).toEqual([
      "ITF CA1",
      "Brief.pdf",
    ]);
  });
});
