import { expect, test } from "vitest";
import { controlOf, labelParts } from "../lib/format";
import { siteName, startsFor } from "../lib/suggestions";

test("at most four starts: what the page says first, then one restyle", () => {
  expect(startsFor(["summarize", "picks", "key_points", "redesign", "darker"])).toEqual([
    "summarize",
    "picks",
    "key_points",
    "redesign",
  ]);
  expect(startsFor(["summarize", "key_points", "catch", "explain", "steps"])).toHaveLength(4);
  expect(startsFor(["darker"])).toEqual(["darker"]);
});

test("a page's site name is short enough to say in a sentence", () => {
  expect(siteName("Hacker News", "news.ycombinator.com")).toBe("Hacker News");
  expect(siteName("crynta (Crynta) · GitHub", "github.com")).toBe("GitHub");
  expect(siteName("Spotify - Web Player: Music for everyone", "open.spotify.com")).toBe("Spotify");
  expect(siteName("Show HN: A tiny SQLite extension for vector search", "www.example.com")).toBe(
    "example.com",
  );
});

test("a step's label keeps what it quotes apart; its control is the last thing quoted", () => {
  expect(labelParts("Type “Terax” into “Search”")).toEqual([
    { text: "Type ", named: false },
    { text: "Terax", named: true },
    { text: " into ", named: false },
    { text: "Search", named: true },
  ]);
  expect(labelParts('Type into ""')).toEqual([{ text: "Type into ", named: false }]);
  expect(controlOf("Type “Terax” into “Search”")).toBe("Search");
  expect(controlOf('Click "Place order"')).toBe("Place order");
});

test("cross-site fill controls exclude quoted values and source sites", () => {
  for (const value of [
    '"curly “quotes”"',
    '"escaped \\"quotes\\" and \\n newline"',
    '" → Type into “fake”"',
  ]) {
    expect(controlOf(`${value} → Type into "Message"\nsource.example → target.example`)).toBe(
      "Message",
    );
    expect(controlOf(`${value} → Type into ""\nsource.example → target.example`)).toBe(
      'Type into ""',
    );
  }
});
