import { describe, expect, test, vi } from "vitest";
import type { NoteSummary } from "$domain/notes";
import { sections } from "../lib/sections";
import { rowDate, untilEditedChanges, untilTomorrow } from "../lib/format";
import * as m from "$shared/i18n/messages";

const now = new Date(2026, 8, 24, 15, 0).getTime();

test("notes use local calendar boundaries across daylight saving changes", () => {
  vi.stubEnv("TZ", "Europe/Paris");
  try {
    const today = new Date(2026, 9, 26, 12).getTime();
    const yesterday = new Date(2026, 9, 25, 0, 30).getTime();
    expect(sections([note("early", yesterday)], today, false)[0]?.id).toBe("yesterday");
    expect(rowDate(yesterday, today)).toBe(m.note_yesterday());
    const spring = new Date(2026, 2, 30, 12).getTime();
    const beforeYesterday = new Date(2026, 2, 28, 23, 30).getTime();
    expect(sections([note("late", beforeYesterday)], spring, false)[0]?.id).toBe("week");
    expect(rowDate(beforeYesterday, spring)).not.toBe(m.note_yesterday());
  } finally {
    vi.unstubAllEnvs();
  }
});
const at = (days: number, hours = 12) => new Date(2026, 8, 24 - days, hours).getTime();
function note(id: string, modified: number, pinned = false): NoteSummary {
  return {
    id,
    revision: "0".repeat(32),
    title: id,
    preview: "",
    pinned,
    trashed: false,
    editable: true,
    created_at: String(modified),
    modified_at: String(modified),
    path: `${id}.md`,
  };
}

test("notes group as pinned, then by how recently they changed", () => {
  const items = [
    note("pinned", at(40), true),
    note("now", at(0, 9)),
    note("late last night", at(1, 23)),
    note("tuesday", at(3)),
    note("fortnight", at(14)),
    note("august", at(40)),
    note("last year", new Date(2025, 11, 2).getTime()),
  ];
  expect(
    sections(items, now, false).map((section) => [section.id, section.items.map((i) => i.id)]),
  ).toEqual([
    ["pinned", ["pinned"]],
    ["today", ["now"]],
    ["yesterday", ["late last night"]],
    ["week", ["tuesday"]],
    ["month", ["fortnight"]],
    ["2026-7", ["august"]],
    ["2025-11", ["last year"]],
  ]);
});

test("search results keep their ranking", () => {
  const items = [note("b", at(30)), note("a", at(0))];
  expect(sections(items, now, true)).toEqual([{ key: { kind: "results" }, id: "results", items }]);
  expect(sections([], now, true)).toEqual([]);
});

describe("when dates next read differently", () => {
  const at = new Date(2026, 8, 25, 14, 30).getTime();

  test("a day's dates change at midnight", () => {
    expect(untilTomorrow(at)).toBe(9.5 * 60 * 60 * 1000);
  });

  test("an edit reads 'now', then minutes, then a time", () => {
    const edit = at - 10_000;
    expect(untilEditedChanges(edit, at)).toBe(35_000);
    // 45s reads "1 minute"; it reads "2 minutes" from 90s.
    expect(untilEditedChanges(at - 45_000, at)).toBe(45_000);
    expect(untilEditedChanges(at - 60_000, at)).toBe(30_000);
    // The last minute before it turns into a time of day.
    expect(untilEditedChanges(at - 44.8 * 60_000, at)).toBe(12_000);
    expect(untilEditedChanges(at - 60 * 60_000, at)).toBe(untilTomorrow(at));
  });
});
