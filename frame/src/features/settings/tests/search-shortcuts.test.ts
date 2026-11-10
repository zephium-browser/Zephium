import { expect, it } from "vitest";
import { customShortcut, shortcutKeywords } from "../lib/search-shortcuts";

it("validates custom shortcuts and derives their home and icon domain", () => {
  const entry = customShortcut(
    "Example",
    "ex",
    "https://example.com/search?q={query}",
    [],
    "custom:example",
  );
  expect(entry).toMatchObject({
    keywords: ["ex"],
    home_url: "https://example.com/",
    domain: "example.com",
    suggestions: null,
  });
  expect(shortcutKeywords("EX", "custom:other", [entry!])).toBeNull();
  expect(shortcutKeywords("EX ex", "custom:example", [entry!])).toBeNull();
  expect(shortcutKeywords("new alias", "custom:example", [entry!])).toEqual(["new", "alias"]);
});

it.each([
  "http://example.com/?q={query}",
  "https://{query}.example.com/",
  "https://user:secret@example.com/?q={query}",
  "https://example.com/",
  "https://example.com/?q={query}&q={query}",
])("refuses %s", (url) => {
  expect(customShortcut("Example", "ex", url, [], "custom:example")).toBeNull();
});
