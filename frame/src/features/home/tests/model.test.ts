import { describe, expect, it } from "vitest";
import { admit } from "$features/home";
import {
  clean,
  MAX_ACTIONS,
  MAX_ALSO,
  MAX_CHILDREN,
  MAX_DAYS,
  MAX_ENTRIES,
  MAX_KNOWN,
  MAX_LABEL,
  MAX_MORE,
  MAX_PAGES,
  MAX_ROWS,
  MAX_SOCIAL,
  MAX_TEXT,
  type Place,
  type Row,
  type Search,
} from "../lib/model";
import { compare, grinders, home, large, place, row } from "./fixtures";

const many = <T>(count: number, make: (index: number) => T) =>
  Array.from({ length: count }, (_, index) => make(index));

const admitted = (search: Search) => admit(home([search])).today.entries![0] as Search;
const placeOf = (search: Search): Place => {
  if (search.kind !== "place") throw new Error("a place");
  return search.place;
};

describe("Home's model", () => {
  it("keeps each list at its limit and refuses the one past it", () => {
    const base = large(null);
    const at = (count: number) =>
      placeOf(
        admitted({
          ...base,
          also: many(count, (i) => ({ id: `a${i}`, title: "t", host: "h", kind: "page" as const })),
          place: place("p", {
            title: "P",
            host: "p.example",
            rows: many(count, (i) =>
              row(`r${i}`, `Row ${i}`, {
                known: many(count, (k) => ({ id: `k${k}`, label: "k" })),
                opens: i === 0,
                inside:
                  i === 0
                    ? { state: "open", more: false, rows: many(count, (c) => row(`c${c}`, "c")) }
                    : null,
              }),
            ),
            more: many(count, (i) => ({ id: `m${i}`, label: "m" })),
            actions: many(count, (i) => ({ id: `x${i}`, kind: "signin" as const, label: "x" })),
            social: many(count, () => ({ network: "x" as const, name: "n" })),
          }),
        }),
      );
    const full = at(30);
    expect(full.rows).toHaveLength(MAX_ROWS);
    expect(full.more).toHaveLength(MAX_MORE);
    expect(full.actions).toHaveLength(MAX_ACTIONS);
    expect(full.social).toHaveLength(MAX_SOCIAL);
    expect(full.rows[0]!.known).toHaveLength(MAX_KNOWN);
    const inside = full.rows[0]!.inside;
    expect(inside?.state === "open" && inside.rows.length).toBe(MAX_CHILDREN);
    // Children past the limit are still there to read: the inside says there is more.
    expect(inside?.state === "open" && inside.more).toBe(true);
    const exact = at(MAX_CHILDREN);
    const kept = exact.rows[0]!.inside;
    expect(kept?.state === "open" && kept.more).toBe(false);
    expect(at(MAX_ROWS).rows).toHaveLength(MAX_ROWS);
    expect(at(MAX_ROWS + 1).rows).toHaveLength(MAX_ROWS);
    expect(at(MAX_ACTIONS + 1).actions).toHaveLength(MAX_ACTIONS);
    expect(at(MAX_MORE + 1).more).toHaveLength(MAX_MORE);
    expect(at(MAX_SOCIAL + 1).social).toHaveLength(MAX_SOCIAL);
    expect(at(MAX_KNOWN + 1).rows[0]!.known).toHaveLength(MAX_KNOWN);

    const also = (count: number) =>
      admitted({
        ...base,
        also: many(count, (i) => ({ id: `a${i}`, title: "t", host: "h", kind: "page" as const })),
      }).also;
    expect(also(MAX_ALSO)).toHaveLength(MAX_ALSO);
    expect(also(MAX_ALSO + 1)).toHaveLength(MAX_ALSO);
    const pages = (count: number) => {
      const search = admitted({
        ...grinders,
        pages: many(count, (i) => ({ id: `g${i}`, title: "t", host: "h", picture: null })),
      });
      return search.kind === "pages" ? search.pages : [];
    };
    expect(pages(MAX_PAGES)).toHaveLength(MAX_PAGES);
    expect(pages(MAX_PAGES + 1)).toHaveLength(MAX_PAGES);
  });

  it("keeps a day's entries and the earlier days at their limits", () => {
    const entries = (count: number) => many(count, (i) => ({ ...grinders, id: `q${i}` }));
    expect(admit(home(entries(MAX_ENTRIES + 1))).today.entries).toHaveLength(MAX_ENTRIES);
    expect(admit(home(entries(MAX_ENTRIES))).today.entries).toHaveLength(MAX_ENTRIES);
    const days = (count: number) =>
      admit(
        home(
          [],
          many(count, (i) => ({ key: `d${i}`, count: 0, entries: null })),
        ),
      ).earlier;
    expect(days(MAX_DAYS + 1)).toHaveLength(MAX_DAYS);
    expect(days(MAX_DAYS)).toHaveLength(MAX_DAYS);
  });

  it("reads no deeper than a row's inside", () => {
    const deep = row("c", "Child", {
      opens: true,
      inside: { state: "open", more: false, rows: [row("g", "Grandchild")] },
    });
    const top = row("r", "Row", {
      opens: true,
      inside: { state: "open", more: false, rows: [deep] },
    });
    const kept = placeOf(
      admitted({ ...large(null), place: place("p", { title: "P", host: "h", rows: [top] }) }),
    );
    const inside = kept.rows[0]!.inside;
    if (inside?.state !== "open") throw new Error("open");
    expect(inside.rows[0]).toMatchObject({ opens: false, inside: null });
  });

  it("keeps a menu to what it holds: no page, no host, no picture, and none at all when empty", () => {
    const menu = (id: string, inside: Row["inside"]) =>
      row(id, id, {
        menu: true,
        opens: true,
        host: "h.example",
        picture: "p",
        opened: true,
        inside,
      });
    const kept = placeOf(
      admitted({
        ...large(null),
        place: place("p", {
          title: "P",
          host: "h",
          rows: [
            menu("full", { state: "open", more: false, rows: [row("c", "Child", { menu: true })] }),
            menu("empty", { state: "open", more: false, rows: [] }),
            menu("asked", { state: "opening" }),
          ],
        }),
      }),
    );
    expect(kept.rows.map((each) => each.id)).toEqual(["full"]);
    expect(kept.rows[0]).toMatchObject({
      menu: true,
      opens: false,
      host: null,
      picture: null,
      opened: false,
    });
    const inside = kept.rows[0]!.inside;
    expect(inside?.state === "open" && inside.rows[0]!.menu).toBe(false);
  });

  it("drops pairs and the asked row that point at nothing kept", () => {
    const search = admitted({
      ...compare,
      pairs: [
        ...compare.pairs,
        { left: "gone", right: "also-gone" },
        { left: "gone", right: "o-sync" },
      ],
    });
    if (search.kind !== "compare") throw new Error("compare");
    expect(search.pairs).toHaveLength(compare.pairs.length + 1);
    expect(search.pairs.at(-1)).toEqual({ left: null, right: "o-sync" });
    const asked = placeOf(
      admitted({ ...large(null), place: { ...large(null).place, asked: "nowhere" } }),
    );
    expect(asked.asked).toBeNull();
  });

  it("shows page text as one bounded line with nothing that reorders or hides it", () => {
    expect(clean("Pricing‮ and⁦ fees\u0007\n\tnow")).toBe("Pricing and fees now");
    expect(clean("‏؜")).toBe("");
    const text = "x".repeat(MAX_TEXT + 10);
    expect(Array.from(clean(text))).toHaveLength(MAX_TEXT);
    expect(clean(text).endsWith("…")).toBe(true);
    expect(clean("y".repeat(MAX_TEXT))).toBe("y".repeat(MAX_TEXT));
    const label = placeOf(
      admitted({
        ...large(null),
        place: place("p", { title: "t".repeat(500), host: "h", rows: [row("r", "r".repeat(500))] }),
      }),
    );
    expect(Array.from(label.title)).toHaveLength(MAX_LABEL);
    expect(Array.from(label.rows[0]!.label)).toHaveLength(MAX_LABEL);
    // A cut never splits a character in two.
    expect(Array.from(clean("😀".repeat(MAX_TEXT + 1))).at(-2)).toBe("😀");
  });
});
