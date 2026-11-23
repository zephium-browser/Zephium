import { describe, expect, it } from "vitest";
import type { SearchResult } from "$shared/ipc/bindings";
import type { HomeSuggestion } from "$domain/home";
import { withHome, type SurfaceRow } from "../lib/search-surface.svelte";

const native = (id: string, kind: string): SurfaceRow => ({
  id,
  title: id,
  section: "search",
  result: { kind, title: id, detail: "", icon: null } as unknown as SearchResult,
  tool: null,
  icon: null,
});
const held = (id: string, home: HomeSuggestion): SurfaceRow => ({
  id,
  title: id,
  section: "history",
  result: null,
  tool: null,
  icon: null,
  home,
});

describe("where Home's rows stand in the field", () => {
  const rows = [
    native("search", "search"),
    native("history", "history"),
    native("bookmark", "bookmark"),
    native("tab", "tab"),
  ];
  const earlier = held("earlier", { kind: "search", search: "q-1", text: "stripe" });
  const row = held("row", {
    kind: "row",
    search: "q-1",
    entry: "pricing",
    label: "Pricing",
    place: "Stripe",
  });

  it("puts open tabs and bookmarks first, then Home's rows, earlier searches and the search", () => {
    expect(withHome(rows, [earlier, row]).map((one) => one.id)).toEqual([
      "tab",
      "bookmark",
      "row",
      "earlier",
      "search",
      "history",
    ]);
  });

  it("leaves native's order alone when Home has nothing", () => {
    expect(withHome(rows, []).map((one) => one.id)).toEqual(rows.map((one) => one.id));
  });
});
