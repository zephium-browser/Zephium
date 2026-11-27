import { describe, expect, it } from "vitest";
import {
  sameSearch,
  resultIdentity,
  resultHost,
  completionTarget,
  resultSection,
  groupRows,
  resultDetail,
  matchRange,
  completionSuffix,
} from "../lib/search-model";
import type { SearchContext, SearchResult } from "$shared/ipc/bindings";

const context = (session_id = "one", request_id = "one"): SearchContext => ({
  window_id: "window",
  session_id,
  request_id,
  profile_id: "p",
  space_id: "s",
});
const tab = (id: string, title = id): SearchResult => ({
  kind: "tab",
  title,
  detail: "example.com",
  icon: null,
  action: { type: "ActivateTab", id },
});

describe("search identity", () => {
  it("does not admit an ABA query from another session or profile", () => {
    expect(sameSearch(context(), context())).toBe(true);
    expect(sameSearch(context(), context("two"))).toBe(false);
    expect(sameSearch(context(), { ...context(), profile_id: "other" })).toBe(false);
    expect(sameSearch(context(), context("one", "two"))).toBe(false);
  });
  it("identifies a row by its action, so a retitled tab keeps its identity", () => {
    expect(resultIdentity(tab("a"))).toBe(resultIdentity(tab("a", "Changed title")));
    expect(resultIdentity(tab("a"))).not.toBe(resultIdentity(tab("b")));
  });
});

describe("result presentation", () => {
  it("groups adjacent runs without reordering what native sent", () => {
    const rows = [
      { section: resultSection({ ...tab("x"), kind: "search" }) },
      { section: resultSection({ ...tab("x"), kind: "suggestion" }) },
      { section: resultSection(tab("a")) },
      { section: resultSection({ ...tab("x"), kind: "history" }) },
      { section: resultSection({ ...tab("x"), kind: "note" }) },
    ];
    expect(groupRows(rows).map((group) => group.section)).toEqual([
      "search",
      "tabs",
      "history",
      "notes",
    ]);
    // A bookmark leads the visited pages it ties with, in the same section.
    expect(resultSection({ ...tab("x"), kind: "bookmark" })).toBe("history");
    // Two runs of the same section stay two runs; grouping never gathers rows
    // across the list, which is what used to let a late result jump sections.
    const split = [
      { section: "tabs" as const },
      { section: "notes" as const },
      { section: "tabs" as const },
    ];
    expect(groupRows(split).map((group) => group.section)).toEqual(["tabs", "notes", "tabs"]);
  });
  it("shows the bare host as secondary text, and the engine for a search", () => {
    expect(resultDetail(tab("a"))).toBe("example.com");
    // Either spelling native may use for a host resolves the same way.
    expect(resultDetail({ ...tab("a"), detail: "https://www.example.com/x" })).toBe("example.com");
    expect(resultDetail({ ...tab("a"), detail: "www.example.com" })).toBe("example.com");
    expect(
      resultDetail({ ...tab("a"), kind: "bookmark", detail: "https://www.example.com/x" }),
    ).toBe("example.com");
    expect(resultDetail({ ...tab("a"), kind: "search", detail: "DuckDuckGo" })).toBe("DuckDuckGo");
    expect(resultDetail({ ...tab("a"), kind: "note", detail: "Pack the adapter" })).toBe(
      "Pack the adapter",
    );
  });
  it("emphasises one contiguous match and never fragments a title", () => {
    expect(matchRange("Rust Book", "rust")).toEqual([0, 4]);
    expect(matchRange("The Rust Book", "rust")).toEqual([4, 8]);
    expect(matchRange("Rust Book", "book rust")).toBeNull();
    expect(matchRange("Rust Book", "   ")).toBeNull();
  });
  it("maps expanded lowercase characters back to the original title", () => {
    expect(matchRange("İstanbul News", "news")).toEqual([9, 13]);
    expect(matchRange("İstanbul", "i")).toEqual([0, 1]);
    expect(matchRange("🙂İstanbul", "tan")).toEqual([4, 7]);
  });
});

describe("inline completion", () => {
  it("resolves the row a visible completion stands for", () => {
    const rows = [
      { result: { ...tab("a"), kind: "search", detail: "DuckDuckGo" } as SearchResult },
      { result: { ...tab("b"), detail: "www.notion.so" } as SearchResult },
    ];
    expect(resultHost(rows[1]!.result)).toBe("notion.so");
    // Once the field reads "notion.so" that is the destination on screen, so
    // Enter must open it rather than search the fragment that was typed.
    expect(completionTarget(rows, "notion.so")).toBe(rows[1]);
    expect(completionTarget(rows, "example.com")).toBeNull();
    expect(completionTarget(rows, null)).toBeNull();
  });

  it("only offers a strict extension of exactly what is typed", () => {
    expect(completionSuffix("git", "github.com")).toBe("hub.com");
    expect(completionSuffix("GIT", "github.com")).toBe("hub.com");
    expect(completionSuffix("github.com", "github.com")).toBeNull();
    expect(completionSuffix("gitl", "github.com")).toBeNull();
    expect(completionSuffix("", "github.com")).toBeNull();
    expect(completionSuffix("git", null)).toBeNull();
  });
});
