import { describe, expect, it } from "vitest";
import { columnIndex, columnName, readCsv } from "../lib/sheet";

describe("sheet", () => {
  it("names columns", () => {
    expect(columnIndex("A1")).toBe(0);
    expect(columnIndex("AB12")).toBe(27);
    expect(columnName(27)).toBe("AB");
  });

  it("reads quoted CSV", () => {
    expect(readCsv('a,"b, c"\n"say ""hi""",2\n').rows).toEqual([
      ["a", "b, c"],
      ['say "hi"', "2"],
    ]);
  });
});
