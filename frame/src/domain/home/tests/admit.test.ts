import { describe, expect, it } from "vitest";
import type { MapEntry_Serialize, TabView } from "$shared/ipc/bindings";
import {
  also,
  children,
  comparison,
  ended,
  entry,
  first,
  mapEntries,
  menu,
  opened,
  placed,
  snapshot,
  tab,
  wirePlace,
  wireSearch,
  type Wire,
} from "$shared/testing/home-wire";
import { fromSnapshot } from "../admit";
import {
  clean,
  MAX_ACTIONS,
  MAX_ALSO,
  MAX_CHILDREN,
  MAX_KNOWN,
  MAX_LABEL,
  MAX_MORE,
  MAX_PAGES,
  MAX_ROWS,
  MAX_SEARCHES,
  MAX_SOCIAL,
  MAX_TEXT,
  type Place,
  type Search,
} from "../model";

const NOW = new Date(2026, 10, 23, 17, 0).getTime();
const many = <T>(count: number, make: (index: number) => T) =>
  Array.from({ length: count }, (_, index) => make(index));
const SITE = { title: "P", host: "p.example" };

const search = (
  rows: MapEntry_Serialize[],
  shape: {
    more?: string[];
    actions?: string[];
    also?: number;
    social?: number;
    focus?: string;
  } = {},
) =>
  wireSearch({
    id: "q",
    text: "p",
    at: NOW - 60_000,
    changes: [
      first(
        1,
        SITE,
        many(shape.also ?? 0, () => also("t", "h")),
      ),
      placed(
        2,
        wirePlace({
          ...SITE,
          rows,
          focus: shape.focus ?? null,
          more: shape.more,
          actions: shape.actions,
          social: many(
            shape.social ?? 0,
            (i) => (["twitter", "git_hub", "reddit"] as const)[i % 3]!,
          ),
        }),
      ),
      ended(3),
    ],
  });

type Held = Parameters<typeof fromSnapshot>[1];
const drawn = (wire: Wire, held: Held = {}) =>
  fromSnapshot(snapshot([wire]), held, NOW).today.entries[0] as Search;
const placeOf = (search: Search): Place => {
  if (search.kind !== "place") throw new Error("a place");
  return search.place;
};

describe("Home's model from native state", () => {
  it("keeps each list at its limit and refuses the one past it", () => {
    const at = (count: number) =>
      placeOf(
        drawn(
          search(
            many(count, (i) =>
              entry(`r${i}`, `Row ${i}`, {
                open: i === 0,
                read: i === 0 ? "read" : null,
                children: children(
                  `c${i}-`,
                  many(count, (c) => `c${c}`),
                ),
              }),
            ),
            {
              more: many(count, (i) => `m${i}`),
              actions: many(count, (i) => `x${i}`),
              social: count,
            },
          ),
          { unfolded: new Set(["q/r0"]) },
        ),
      );
    const full = at(30);
    expect(full.rows).toHaveLength(MAX_ROWS);
    expect(full.more).toHaveLength(MAX_MORE);
    expect(full.actions).toHaveLength(MAX_ACTIONS);
    expect(full.rows[0]!.known).toHaveLength(MAX_KNOWN);
    const inside = full.rows[0]!.inside;
    expect(inside?.state === "open" && inside.rows.length).toBe(MAX_CHILDREN);
    expect(inside?.state === "open" && inside.more).toBe(true);
    const exact = at(MAX_CHILDREN).rows[0]!.inside;
    expect(exact?.state === "open" && exact.more).toBe(false);
    expect(at(MAX_ROWS + 1).rows).toHaveLength(MAX_ROWS);
    expect(at(MAX_ACTIONS + 1).actions).toHaveLength(MAX_ACTIONS);
    expect(at(MAX_MORE + 1).more).toHaveLength(MAX_MORE);
    // Networks are counted once each, and never past the limit.
    expect(at(30).social.length).toBeLessThanOrEqual(MAX_SOCIAL);
    expect(new Set(at(30).social).size).toBe(at(30).social.length);

    const alsos = (count: number) => drawn(search([], { also: count })).also;
    expect(alsos(MAX_ALSO + 1)).toHaveLength(MAX_ALSO);
    const pages = (count: number) => {
      const shown = drawn(
        wireSearch({
          id: "q",
          text: "p",
          at: NOW,
          kind: "other",
          pages: many(count, (i) => opened(null, `t${i}`, "h")),
        }),
      );
      return shown.kind === "pages" ? shown.pages : [];
    };
    expect(pages(MAX_PAGES + 1)).toHaveLength(MAX_PAGES);
    const searches = many(MAX_SEARCHES + 1, (i) => ({ ...search([]), id: `q${i}` }));
    expect(fromSnapshot(snapshot(searches), {}, NOW).today.entries).toHaveLength(MAX_SEARCHES);
  });

  it("shows a row's inside only once the person opened it, and says how it stands", () => {
    const rows = [
      entry("read", "Read", { open: true, read: "read", children: children("k", ["One", "Two"]) }),
      entry("queued", "Queued", { open: true, read: "queued" }),
      entry("waiting", "Waiting", { open: true }),
      entry("refused", "Refused", { open: true, read: "refused" }),
      entry("closed", "Closed", { open: true, read: "read", children: children("z", ["Z"]) }),
    ];
    const held = {
      unfolded: new Set(["q/read", "q/queued", "q/waiting", "q/refused"]),
      waiting: new Set(["q/waiting"]),
    };
    const shown = placeOf(drawn(search(rows), held)).rows;
    expect(shown.map((row) => row.inside?.state ?? null)).toEqual([
      "open",
      "opening",
      "opening",
      "refused",
      null,
    ]);
    // What is held of a row shows on hover even while its inside is folded.
    expect(shown[4]!.known.map((one) => one.label)).toEqual(["Z"]);
    const refused = placeOf(
      drawn(search(rows), { ...held, refused: new Map([["q/waiting", "busy" as const]]) }),
    ).rows[2]!.inside;
    expect(refused).toEqual({ state: "refused", reason: "busy" });
  });

  it("keeps a menu to what it holds: no page, no host, and none at all when empty", () => {
    const shown = placeOf(
      drawn(
        search([
          { ...menu("full", "Full", ["Child"]), off_site: true, host: "h.example", open: true },
          menu("empty", "Empty", []),
        ]),
      ),
    ).rows;
    expect(shown.map((row) => row.id)).toEqual(["full"]);
    expect(shown[0]).toMatchObject({ menu: true, opens: false, host: null, opened: false });
    const inside = shown[0]!.inside;
    expect(inside?.state === "open" && inside.rows.map((row) => row.label)).toEqual(["Child"]);
    expect(inside?.state === "open" && inside.rows[0]!.menu).toBe(false);
  });

  it("reads no deeper than a row's inside", () => {
    const deep = entry("c", "Child", { open: true, read: "read", children: children("g", ["G"]) });
    const shown = placeOf(
      drawn(search([entry("r", "Row", { open: true, read: "read", children: [deep] })]), {
        unfolded: new Set(["q/r", "q/c"]),
      }),
    ).rows[0]!.inside;
    if (shown?.state !== "open") throw new Error("open");
    expect(shown.rows[0]).toMatchObject({ opens: false, inside: null });
  });

  it("drops pairs and the asked row that point at nothing kept", () => {
    const wire = wireSearch({
      id: "q",
      text: "a vs b",
      at: NOW,
      kind: "compare",
      changes: [
        first(1, SITE),
        placed(2, wirePlace({ ...SITE, rows: [entry("a", "A")], focus: "nowhere" }), 0),
        placed(3, wirePlace({ title: "B", host: "b.example", rows: [entry("b", "B")] }), 1),
        comparison(4, [
          { label: "A", left: "a", right: "b" },
          { label: "X", left: "gone", right: "also-gone" },
          { label: "Y", left: "gone", right: "b" },
        ]),
      ],
    });
    const shown = drawn(wire);
    if (shown.kind !== "compare") throw new Error("compare");
    expect(shown.pairs).toEqual([
      { left: "a", right: "b" },
      { left: null, right: "b" },
    ]);
    expect(shown.places[0].asked).toBeNull();
  });

  it("names an open page by its tab, marks the row it came from, and takes its icon", () => {
    const icon = { key: "fav-1" } as unknown as NonNullable<TabView["icon"]>;
    const wire = {
      ...search([entry("pricing", "Pricing"), entry("blog", "Blog")]),
      pages: [opened("pricing", "Old title", "p.example", "tab-1")],
    };
    const tabs = new Map([["tab-1", tab("tab-1", "Pricing | P", { icon })]]);
    const place = placeOf(drawn(wire, { tabs }));
    expect(place.rows.map((row) => row.opened)).toEqual([true, false]);
    expect(place.icon).toBe(icon);
    const other = drawn(
      wireSearch({
        id: "q",
        text: "p",
        at: NOW,
        kind: "other",
        pages: [
          opened(null, "Old title", "p.example", "tab-1"),
          opened(null, "Closed", "c.example"),
        ],
      }),
      { tabs },
    );
    expect(other.kind === "pages" && other.pages.map((page) => [page.title, page.tab])).toEqual([
      ["Pricing | P", "tab-1"],
      ["Closed", null],
    ]);
  });

  it("draws the engine a search was asked with, and groups searches by the day they were asked", () => {
    const yesterday = new Date(2026, 10, 22, 9, 30).getTime();
    const model = fromSnapshot(
      snapshot([
        { ...search([]), id: "a", engine: "duckduckgo" },
        { ...search([]), id: "b", at_ms: yesterday },
      ]),
      {},
      NOW,
    );
    expect(model.today.entries.map((one) => one.id)).toEqual(["a"]);
    expect((model.today.entries[0] as Search).engine).toBe("duckduckgo");
    expect(model.earlier).toEqual([
      { key: "2026-11-22", entries: [expect.objectContaining({ id: "b", at: 9 * 60 + 30 })] },
    ]);
  });

  it("fails a place the map ended without, and one whose results named no site", () => {
    const noSite = wireSearch({
      id: "q",
      text: "p",
      at: NOW,
      live: true,
      changes: [first(1, null)],
    });
    expect(placeOf(drawn(noSite))).toMatchObject({ state: "failed", failure: "missing" });
    const gone = wireSearch({
      id: "q",
      text: "p",
      at: NOW,
      changes: [first(1, SITE), ended(2, "timeout")],
    });
    expect(placeOf(drawn(gone))).toMatchObject({ state: "failed", failure: "timeout" });
    const reading = wireSearch({
      id: "q",
      text: "p",
      at: NOW,
      live: true,
      changes: [first(1, SITE)],
    });
    expect(placeOf(drawn(reading))).toMatchObject({ state: "arriving", title: "P" });
  });

  it("drops a search or a row whose id could stand for an address", () => {
    const model = fromSnapshot(
      snapshot([
        { ...search([]), id: "https://evil.example/" },
        mapEntries(search([entry("ok", "Ok"), entry("bad", "Bad")]), (one) =>
          one.id === "bad" ? { ...one, id: "/a/path" } : one,
        ),
      ]),
      {},
      NOW,
    );
    expect(model.today.entries.map((one) => one.id)).toEqual(["q"]);
    expect(placeOf(model.today.entries[0] as Search).rows.map((row) => row.id)).toEqual(["ok"]);
  });

  it("shows page text as one bounded line with nothing that reorders or hides it", () => {
    expect(clean("Pricing‮ and⁦ fees\u0007\n\tnow")).toBe("Pricing and fees now");
    expect(clean("‏؜")).toBe("");
    const text = "x".repeat(MAX_TEXT + 10);
    expect(Array.from(clean(text))).toHaveLength(MAX_TEXT);
    expect(clean(text).endsWith("…")).toBe(true);
    expect(clean("y".repeat(MAX_TEXT))).toBe("y".repeat(MAX_TEXT));
    const long = placeOf(
      drawn(
        wireSearch({
          id: "q",
          text: "p",
          at: NOW,
          changes: [
            placed(
              1,
              wirePlace({ title: "t".repeat(500), host: "h", rows: [entry("r", "r".repeat(500))] }),
            ),
          ],
        }),
      ),
    );
    expect(Array.from(long.title)).toHaveLength(MAX_LABEL);
    expect(Array.from(long.rows[0]!.label)).toHaveLength(MAX_LABEL);
    expect(Array.from(clean("😀".repeat(MAX_TEXT + 1))).at(-2)).toBe("😀");
  });
});
