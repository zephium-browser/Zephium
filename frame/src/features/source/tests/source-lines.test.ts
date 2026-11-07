import { describe, expect, it } from "vitest";
import { CHUNK, MAX_LINES, sourceLines } from "../lib/source-lines";

describe("sourceLines", () => {
  it("splits on every kind of line ending and keeps blank lines", () => {
    const { chunks, total, cut } = sourceLines("a\r\nb\rc\n\nd");
    expect(chunks).toEqual([{ start: 0, lines: ["a", "b", "c", "", "d"] }]);
    expect(total).toBe(5);
    expect(cut).toBe(false);
  });

  it("an empty source is one empty line, so the viewer still draws something", () => {
    expect(sourceLines("")).toEqual({ chunks: [{ start: 0, lines: [""] }], total: 1, cut: false });
  });

  it("groups lines into blocks that carry where they start", () => {
    const text = Array.from({ length: CHUNK * 2 + 5 }, (_, at) => `l${at}`).join("\n");
    const { chunks } = sourceLines(text);
    expect(chunks.map((chunk) => [chunk.start, chunk.lines.length])).toEqual([
      [0, CHUNK],
      [CHUNK, CHUNK],
      [CHUNK * 2, 5],
    ]);
    expect(chunks[2]!.lines[0]).toBe(`l${CHUNK * 2}`);
  });

  it("draws no more than the bound and says how many lines there are", () => {
    const text = "x\n".repeat(MAX_LINES + 500);
    const { chunks, total, cut } = sourceLines(text);
    expect(chunks.reduce((sum, chunk) => sum + chunk.lines.length, 0)).toBe(MAX_LINES);
    expect(total).toBe(MAX_LINES + 501);
    expect(cut).toBe(true);
  });
});
