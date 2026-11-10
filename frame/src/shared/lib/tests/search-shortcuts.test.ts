import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import type { SearchShortcut } from "$shared/ipc/bindings";
import {
  emptyShortcutBackspace,
  shortcutFor,
  shortcutKeyword,
  shortcutPrefix,
  withShortcut,
} from "../search-shortcuts";

const base = new URL("../../../../../crates/zephium-core/src/search/", import.meta.url);
const defaults = JSON.parse(
  readFileSync(new URL("shortcuts.json", base), "utf8"),
) as SearchShortcut[];
const cases = JSON.parse(readFileSync(new URL("shortcut-cases.json", base), "utf8")) as {
  input: string;
  shortcut: string | null;
  query: string;
  trigger: string;
  disabled?: string[];
  custom?: boolean;
  custom_keywords?: string[];
}[];

describe("core prefix parity", () => {
  it.each(cases)("$input ($trigger)", (fixture) => {
    const entries = defaults.map((entry) => ({
      ...entry,
      enabled: !fixture.disabled?.includes(entry.id),
    }));
    if (fixture.custom)
      entries.push({
        ...defaults[0]!,
        id: "custom:example",
        name: "Example",
        keywords: fixture.custom_keywords ?? ["ex"],
      });
    const parsed = shortcutPrefix(fixture.input, entries);
    expect(parsed?.shortcut.id ?? null).toBe(
      fixture.trigger === "keyword" ? fixture.shortcut : null,
    );
    if (fixture.trigger === "keyword") expect(parsed?.query).toBe(fixture.query);
  });
  it("deactivates only an active empty query", () => {
    expect(emptyShortcutBackspace("yt ", defaults)).toBe("");
    expect(emptyShortcutBackspace("yt   ", defaults)).toBe("");
    expect(emptyShortcutBackspace("yt cats", defaults)).toBeNull();
    expect(emptyShortcutBackspace("!yt", defaults)).toBeNull();
    expect(emptyShortcutBackspace("a ", defaults)).toBeNull();
    expect(emptyShortcutBackspace("so ", defaults)).toBeNull();
  });
});

describe("where a query goes", () => {
  it.each(cases)("names what native routes for $input", (fixture) => {
    const entries = defaults.map((entry) => ({
      ...entry,
      enabled: !fixture.disabled?.includes(entry.id),
    }));
    if (fixture.custom) return;
    expect(shortcutFor(fixture.input, entries)?.id ?? null).toBe(fixture.shortcut);
  });

  it("offers every shortcut as a bang, which reaches it from anywhere in a query", () => {
    for (const entry of defaults.filter((item) => item.enabled)) {
      const keyword = shortcutKeyword(entry);
      expect(keyword.startsWith("!")).toBe(true);
      expect(shortcutFor(`${keyword} cats`, defaults)?.id).toBe(entry.id);
      expect(shortcutFor(`cats ${keyword}`, defaults)?.id).toBe(entry.id);
    }
    expect(shortcutKeyword(defaults.find((entry) => entry.id === "youtube")!)).toBe("!yt");
  });

  it("puts a keyword in front of the query in place of the one it named", () => {
    expect(withShortcut("cats", "yt", defaults)).toBe("yt cats");
    expect(withShortcut("", "yt", defaults)).toBe("yt ");
    expect(withShortcut("w cats", "yt", defaults)).toBe("yt w cats");
    expect(withShortcut("wiki cats", "yt", defaults)).toBe("yt wiki cats");
    expect(withShortcut("gh cats", "yt", defaults)).toBe("yt cats");
    expect(withShortcut("cats !gh", "yt", defaults)).toBe("yt cats");
    expect(withShortcut("cats !nothing", "yt", defaults)).toBe("yt cats !nothing");
  });
});
