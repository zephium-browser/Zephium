import { describe, expect, it } from "vitest";
import type { HistoryVisitView } from "$shared/ipc/bindings";
import { dayKey, dayLabel, matchRange, toRows, visitedAt } from "../lib/history-model";

const LABELS = { today: "Today", yesterday: "Yesterday" };

function at(date: string): HistoryVisitView {
  const when = new Date(date);
  return {
    id: String(when.getTime()),
    url: `https://example.com/${when.getTime()}`,
    title: "Example Domain",
    visited_at: String(Math.floor(when.getTime() / 1000)),
    icon: null,
  };
}

describe("history days", () => {
  it("names the reader's own calendar days", () => {
    const today = new Date("2026-03-15T12:00:00");
    expect(dayLabel(new Date("2026-03-15T23:59:00"), today, LABELS)).toBe("Today");
    expect(dayLabel(new Date("2026-03-14T00:01:00"), today, LABELS)).toBe("Yesterday");
    expect(dayLabel(new Date("2026-03-13T12:00:00"), today, LABELS)).toMatch(/13/u);
  });

  it("keeps midnight on the right side of the boundary", () => {
    // 23:59 and 00:01 are minutes apart and different days to a reader.
    expect(dayKey(new Date("2026-03-14T23:59:00"))).not.toBe(
      dayKey(new Date("2026-03-15T00:01:00")),
    );
    expect(dayKey(new Date("2026-03-15T00:00:00"))).toBe(dayKey(new Date("2026-03-15T23:59:59")));
  });

  it("distinguishes the same calendar day in different years", () => {
    const today = new Date("2026-03-15T12:00:00");
    const lastYear = dayLabel(new Date("2025-03-15T12:00:00"), today, LABELS);
    expect(lastYear).toMatch(/2025/u);
    expect(dayKey(new Date("2025-03-15T12:00:00"))).not.toBe(
      dayKey(new Date("2026-03-15T12:00:00")),
    );
  });
});

describe("history rows", () => {
  it("opens a heading per day and counts what it covers", () => {
    const rows = toRows(
      [at("2026-03-15T10:00:00"), at("2026-03-15T09:00:00"), at("2026-03-14T18:00:00")],
      new Date("2026-03-15T12:00:00"),
      LABELS,
    );

    expect(rows.map((row) => row.kind)).toEqual(["day", "visit", "visit", "day", "visit"]);
    expect(rows[0]).toMatchObject({ kind: "day", label: "Today", count: 2 });
    expect(rows[3]).toMatchObject({ kind: "day", label: "Yesterday", count: 1 });
  });

  it("reopens a day that recurs after another one", () => {
    // Visits arrive newest first; an out-of-order page must not merge days.
    const rows = toRows(
      [at("2026-03-15T10:00:00"), at("2026-03-14T18:00:00"), at("2026-03-15T08:00:00")],
      new Date("2026-03-15T12:00:00"),
      LABELS,
    );

    expect(rows.filter((row) => row.kind === "day")).toHaveLength(3);
  });

  it("reads seconds, not milliseconds, from the wire", () => {
    const visit = at("2026-03-15T10:00:00");
    expect(visitedAt(visit).getFullYear()).toBe(2026);
  });
});

describe("match emphasis", () => {
  it("maps lowercase expansions back to the original title", () => {
    expect(matchRange("İstanbul", "stan")).toEqual([1, 5]);
    expect(matchRange("İstanbul News", "news")).toEqual([9, 13]);
    expect(matchRange("İstanbul", "i")).toEqual([0, 1]);
    expect(matchRange("🙂İstanbul", "tan")).toEqual([4, 7]);
    expect(matchRange("İstanbul", "İ")).toEqual([0, 1]);
    expect(matchRange("İ ΟΣ", "ος")).toEqual([2, 4]);
  });

  it("marks the span the query accounts for, case-insensitively", () => {
    expect(matchRange("Example Domain", "domain")).toEqual([8, 14]);
    expect(matchRange("Example Domain", "  ")).toBeNull();
    expect(matchRange("Example Domain", "absent")).toBeNull();
  });
});
