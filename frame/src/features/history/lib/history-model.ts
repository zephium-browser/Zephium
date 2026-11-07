import type { HistoryVisitView } from "$shared/ipc/bindings";

/** A visit row, or the heading that opens the day it belongs to. */
export type HistoryRow =
  | { kind: "day"; id: string; day: string; label: string; count: number }
  | { kind: "visit"; id: string; day: string; visit: HistoryVisitView };

const DAY_LABEL = new Intl.DateTimeFormat(undefined, {
  weekday: "long",
  day: "numeric",
  month: "long",
});
const DAY_LABEL_WITH_YEAR = new Intl.DateTimeFormat(undefined, {
  day: "numeric",
  month: "long",
  year: "numeric",
});
const TIME_LABEL = new Intl.DateTimeFormat(undefined, { hour: "numeric", minute: "2-digit" });

export function visitedAt(visit: HistoryVisitView): Date {
  return new Date(Number(visit.visited_at) * 1000);
}

/** Local calendar day, which is what a reader means by "yesterday". */
export function dayKey(at: Date): string {
  return `${at.getFullYear()}-${at.getMonth() + 1}-${at.getDate()}`;
}

export function dayLabel(at: Date, today: Date, labels: { today: string; yesterday: string }) {
  const key = dayKey(at);
  if (key === dayKey(today)) return labels.today;
  const yesterday = new Date(today);
  yesterday.setDate(today.getDate() - 1);
  if (key === dayKey(yesterday)) return labels.yesterday;
  return at.getFullYear() === today.getFullYear()
    ? DAY_LABEL.format(at)
    : DAY_LABEL_WITH_YEAR.format(at);
}

export function timeLabel(at: Date): string {
  return TIME_LABEL.format(at);
}

export function hostOf(url: string): string {
  try {
    return new URL(url).host.replace(/^www\./u, "");
  } catch {
    return url;
  }
}

/** Flattens visits into headed day runs. Visits arrive newest first, so a day
 *  is closed as soon as a different one appears. */
export function toRows(
  visits: readonly HistoryVisitView[],
  today: Date,
  labels: { today: string; yesterday: string },
): HistoryRow[] {
  const rows: HistoryRow[] = [];
  let openDay: string | null = null;
  let heading: Extract<HistoryRow, { kind: "day" }> | null = null;
  for (const visit of visits) {
    const at = visitedAt(visit);
    const day = dayKey(at);
    if (day !== openDay) {
      openDay = day;
      heading = {
        kind: "day",
        id: `day:${day}`,
        day,
        label: dayLabel(at, today, labels),
        count: 0,
      };
      rows.push(heading);
    }
    if (heading) heading.count += 1;
    rows.push({ kind: "visit", id: `visit:${visit.id}`, day, visit });
  }
  return rows;
}

export { matchRange } from "$shared/lib/match-range";
