import { describe, expect, it } from "vitest";
import {
  entry,
  first,
  menu,
  placed,
  snapshot,
  wirePlace,
  wireSearch,
} from "$shared/testing/home-wire";
import { fromSnapshot } from "../admit";
import { MAX_ROW_SUGGESTIONS, MAX_SEARCH_SUGGESTIONS, suggest } from "../suggest";

const NOW = new Date(2026, 10, 23, 17, 0).getTime();
const stripe = wireSearch({
  id: "q-stripe",
  text: "stripe pricing",
  at: NOW - 60_000,
  changes: [
    first(1, { title: "Stripe", host: "stripe.com" }),
    placed(
      2,
      wirePlace({
        title: "Stripe",
        host: "stripe.com",
        rows: [
          menu("products", "Products", ["Payments"]),
          entry("pricing", "Pricing"),
          entry("payments", "Payments"),
          ...Array.from({ length: 6 }, (_, i) => entry(`p${i}`, `Plan ${i}`)),
        ],
      }),
    ),
  ],
});
const others = Array.from({ length: 5 }, (_, i) =>
  wireSearch({ id: `q${i}`, text: `pricing page ${i}`, at: NOW - (i + 2) * 60_000, kind: "other" }),
);
const model = fromSnapshot(snapshot([stripe, ...others]), {}, NOW);

describe("what Home offers in the field", () => {
  it("offers the rows of the search in front, then earlier searches, each bounded", () => {
    const found = suggest(model, "q-stripe", "p");
    const rows = found.filter((one) => one.kind === "row");
    expect(rows).toHaveLength(MAX_ROW_SUGGESTIONS);
    expect(rows.every((one) => one.kind === "row" && one.place === "Stripe")).toBe(true);
    // A menu has no page of its own, so it is never offered.
    expect(rows.some((one) => one.kind === "row" && one.entry === "products")).toBe(false);
    expect(found.filter((one) => one.kind === "search")).toHaveLength(MAX_SEARCH_SUGGESTIONS);
    expect(found.findIndex((one) => one.kind === "search")).toBe(MAX_ROW_SUGGESTIONS);
  });

  it("offers nothing for an empty field, and no rows without a search in front", () => {
    expect(suggest(model, "q-stripe", "   ")).toEqual([]);
    expect(suggest(model, null, "pricing").every((one) => one.kind === "search")).toBe(true);
    expect(suggest(model, null, "pricing").map((one) => one.search)).toContain("q-stripe");
  });

  it("matches without regard to case or width, and never the search in front again", () => {
    expect(suggest(model, "q-stripe", "ＰＲＩＣＩＮＧ").map((one) => one.kind)).toEqual([
      "row",
      "search",
      "search",
      "search",
    ]);
  });
});
