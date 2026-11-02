import * as m from "$shared/i18n/messages";
import type { SectionKey } from "./sections";

const locale = () => (typeof navigator === "undefined" ? "en" : navigator.language);

let formats: {
  time: Intl.DateTimeFormat;
  weekday: Intl.DateTimeFormat;
  date: Intl.DateTimeFormat;
  dateYear: Intl.DateTimeFormat;
  month: Intl.DateTimeFormat;
  monthYear: Intl.DateTimeFormat;
  relative: Intl.RelativeTimeFormat;
  plural: Intl.PluralRules;
} | null = null;

function f() {
  formats ??= {
    time: new Intl.DateTimeFormat(locale(), { hour: "numeric", minute: "2-digit" }),
    weekday: new Intl.DateTimeFormat(locale(), { weekday: "long" }),
    date: new Intl.DateTimeFormat(locale(), { day: "numeric", month: "short" }),
    dateYear: new Intl.DateTimeFormat(locale(), {
      day: "numeric",
      month: "short",
      year: "numeric",
    }),
    month: new Intl.DateTimeFormat(locale(), { month: "long" }),
    monthYear: new Intl.DateTimeFormat(locale(), { month: "long", year: "numeric" }),
    relative: new Intl.RelativeTimeFormat(locale(), { numeric: "auto" }),
    plural: new Intl.PluralRules(locale()),
  };
  return formats;
}

function startOfDay(time: number, daysAgo = 0): number {
  const date = new Date(time);
  return new Date(date.getFullYear(), date.getMonth(), date.getDate() - daysAgo).getTime();
}

/** When a note last changed, as its row shows it: a time today, a weekday
 *  this week, a date before that. */
export function rowDate(modified: number, now: number): string {
  const today = startOfDay(now);
  if (modified >= today) return f().time.format(modified);
  if (modified >= startOfDay(today, 1)) return m.note_yesterday();
  if (modified >= startOfDay(today, 6)) return f().weekday.format(modified);
  return new Date(modified).getFullYear() === new Date(now).getFullYear()
    ? f().date.format(modified)
    : f().dateYear.format(modified);
}

/** Milliseconds until the next local midnight, when every date shown
 *  relative to today reads differently. */
export function untilTomorrow(now: number): number {
  const date = new Date(now);
  return new Date(date.getFullYear(), date.getMonth(), date.getDate() + 1).getTime() - now;
}

/** Milliseconds until `edited(modified, now)` reads differently. */
export function untilEditedChanges(modified: number, now: number): number {
  const age = now - modified;
  if (age < 45_000) return 45_000 - age;
  if (age < 45 * 60_000) {
    // Minutes are rounded, so "3 minutes" turns into "4" half a minute in.
    const next = (Math.floor((age - 30_000) / 60_000) + 1) * 60_000 + 30_000;
    return Math.min(next, 45 * 60_000) - age;
  }
  return untilTomorrow(now);
}

/** "Edited 3 minutes ago", for the open note. */
export function edited(modified: number, now: number): string {
  const seconds = Math.round((modified - now) / 1000);
  const when =
    Math.abs(seconds) < 45
      ? f().relative.format(0, "second")
      : Math.abs(seconds) < 45 * 60
        ? f().relative.format(Math.round(seconds / 60), "minute")
        : modified >= startOfDay(now)
          ? f().time.format(modified)
          : rowDate(modified, now);
  return m.note_edited({ when });
}

export function sectionLabel(key: SectionKey, now: number): string {
  switch (key.kind) {
    case "pinned":
      return m.note_section_pinned();
    case "today":
      return m.note_section_today();
    case "yesterday":
      return m.note_section_yesterday();
    case "week":
      return m.note_section_week();
    case "month":
      return m.note_section_month();
    case "results":
      return m.note_section_results();
    case "calendar": {
      const date = new Date(key.year, key.month, 1);
      return key.year === new Date(now).getFullYear()
        ? f().month.format(date)
        : f().monthYear.format(date);
    }
  }
}

export function noteCount(count: number): string {
  return f().plural.select(count) === "one"
    ? m.note_count_one({ count })
    : m.note_count_other({ count });
}
