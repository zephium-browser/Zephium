import { describe, expect, it } from "vitest";
import { ended, first, placed, wirePlace, entry } from "$shared/testing/home-wire";
import { EMPTY_MAP, fold, replay } from "../fold";

const place = (rows: number) =>
  wirePlace({
    title: "Vercel",
    host: "vercel.com",
    rows: Array.from({ length: rows }, (_, i) => entry(`r${i}`, `Row ${i}`)),
  });

describe("folding a map", () => {
  it("takes each change once, in seq order, and ignores one it already holds", () => {
    let map = fold(EMPTY_MAP, first(1, { title: "Vercel", host: "vercel.com" }));
    map = fold(map, placed(2, place(1), 0, false));
    const held = fold(map, placed(3, place(2), 0, false));
    expect(held.seq).toBe(3);
    // A change at or below what is held is already in it.
    expect(fold(held, placed(2, place(5)))).toBe(held);
    expect(fold(held, placed(3, place(5)))).toBe(held);
    const latest = held.places[0];
    expect(latest?.kind === "place" && latest.place.sections).toHaveLength(2);
  });

  it("keeps the latest place of each slot and ignores a slot a map does not have", () => {
    const map = replay(
      [placed(1, place(1), 0), placed(2, place(3), 1), placed(3, place(2), 7)],
      true,
    );
    expect(
      map.places.map((one) => (one?.kind === "place" ? one.place.sections.length : 0)),
    ).toEqual([1, 3]);
    expect(map.seq).toBe(3);
  });

  it("refuses a change whose seq is not a whole number", () => {
    const map = replay([placed(1, place(1))], true);
    expect(fold(map, { ...placed(2, place(2)), seq: Number.NaN })).toBe(map);
    expect(fold(map, { ...placed(2, place(2)), seq: 2.5 })).toBe(map);
  });

  it("holds why an inside was refused, and the map's end", () => {
    const map = replay(
      [
        placed(1, place(1)),
        {
          type: "opened",
          seq: 2,
          slot: 0,
          id: "r0",
          result: { kind: "refused", failure: "blocked" },
        },
        { type: "opened", seq: 3, slot: 0, id: "r1", result: { kind: "read" } },
        ended(4, "timeout"),
      ],
      false,
    );
    expect([...map.refused.keys()]).toEqual(["0/r0"]);
    expect(map.end).toEqual({ kind: "failed", failure: "timeout" });
  });
});
