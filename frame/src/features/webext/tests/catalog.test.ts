import { expect, test } from "vitest";
import { catalog, storeListing } from "../lib/catalog";

test("every catalog entry is a distinct Chrome Web Store extension", () => {
  const entries = catalog.flatMap((group) => group.entries);
  const ids = entries.map((entry) => entry.id);
  expect(new Set(ids).size).toBe(ids.length);
  for (const group of catalog) expect(group.title().length).toBeGreaterThan(0);
  for (const entry of entries) {
    expect(entry.id).toMatch(/^[a-p]{32}$/);
    expect(entry.icon).toBeTruthy();
    expect(entry.blurb().length).toBeGreaterThan(0);
    expect(storeListing(entry.id)).toBe(`https://chromewebstore.google.com/detail/${entry.id}`);
  }
});
