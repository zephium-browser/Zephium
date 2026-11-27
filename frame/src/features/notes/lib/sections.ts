import type { NoteSummary } from "$domain/notes";

export type SectionKey =
  | { kind: "pinned" }
  | { kind: "today" }
  | { kind: "yesterday" }
  | { kind: "week" }
  | { kind: "month" }
  | { kind: "calendar"; year: number; month: number }
  | { kind: "results" };

export type Section = { key: SectionKey; id: string; items: NoteSummary[] };

function startOfDay(time: number, daysAgo = 0): number {
  const date = new Date(time);
  return new Date(date.getFullYear(), date.getMonth(), date.getDate() - daysAgo).getTime();
}

type Boundaries = { today: number; yesterday: number; week: number; month: number };

function sectionOf(modified: number, { today, yesterday, week, month }: Boundaries): SectionKey {
  if (modified >= today) return { kind: "today" };
  if (modified >= yesterday) return { kind: "yesterday" };
  if (modified >= week) return { kind: "week" };
  if (modified >= month) return { kind: "month" };
  const date = new Date(modified);
  return { kind: "calendar", year: date.getFullYear(), month: date.getMonth() };
}

function idOf(key: SectionKey): string {
  return key.kind === "calendar" ? `${key.year}-${key.month}` : key.kind;
}

/** Notes grouped as they are read: pinned first, then by how recently they
 *  changed. A search result keeps its ranking and has no dates. */
export function sections(items: NoteSummary[], now: number, ranked: boolean): Section[] {
  if (ranked) return items.length ? [{ key: { kind: "results" }, id: "results", items }] : [];
  const today = startOfDay(now);
  const boundaries = {
    today,
    yesterday: startOfDay(today, 1),
    week: startOfDay(today, 7),
    month: startOfDay(today, 30),
  };
  const result: Section[] = [];
  const pinned = items.filter((item) => item.pinned);
  if (pinned.length) result.push({ key: { kind: "pinned" }, id: "pinned", items: pinned });
  for (const item of items) {
    if (item.pinned) continue;
    const key = sectionOf(Number(item.modified_at), boundaries);
    const id = idOf(key);
    const last = result.at(-1);
    if (last?.id === id) last.items.push(item);
    else result.push({ key, id, items: [item] });
  }
  return result;
}
