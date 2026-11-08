import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import type { SearchShortcut } from "$shared/ipc/bindings";
import { emptyShortcutBackspace, shortcutPrefix } from "../search-shortcuts";

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
