import type { TaskRow } from "$domain/resources";

/** What the list is asking about. Scope narrows the day; it never filters by
 *  who holds a task, because a person and an agent share one list. */
export type TaskScope = "today" | "upcoming" | "all" | "completed" | "inbox";

export type SectionKey = "overdue" | "today" | "tomorrow" | "upcoming" | "anytime" | "completed";

export type TaskSection = {
  key: SectionKey;
  label: string;
  rows: TaskRow[];
  /** Everything the section holds, which `rows` may be a clipped view of. */
  total: number;
};

/** How close a due date is, for the one place a task list earns colour. */
export type DueTone = "overdue" | "today" | "soon" | "later";

const DAY_MS = 86_400_000;
const WEEKDAY = new Intl.DateTimeFormat(undefined, { weekday: "long", timeZone: "UTC" });
const SHORT_DAY = new Intl.DateTimeFormat(undefined, {
  day: "numeric",
  month: "short",
  timeZone: "UTC",
});
const SHORT_DAY_WITH_YEAR = new Intl.DateTimeFormat(undefined, {
  day: "numeric",
  month: "short",
  year: "numeric",
  timeZone: "UTC",
});
const CLOCK = new Intl.DateTimeFormat(undefined, {
  hour: "numeric",
  minute: "2-digit",
  timeZone: "UTC",
});

/** `HH:MM` shown the way the reader's clock shows it, 12- or 24-hour. */
export function timeLabel(time: string): string {
  const parts = /^(\d{2}):(\d{2})$/u.exec(time);
  if (!parts) return time;
  return CLOCK.format(new Date(Date.UTC(2026, 0, 1, Number(parts[1]), Number(parts[2]))));
}

/** Right now, as a calendar day. */
export function todayKey(): string {
  return dayKey(new Date());
}

/** The local calendar day, which is what a reader means by "today". A due date
 *  is a calendar date, so it is never shifted through a zone or a clock time. */
export function dayKey(at: Date): string {
  return `${at.getFullYear()}-${String(at.getMonth() + 1).padStart(2, "0")}-${String(at.getDate()).padStart(2, "0")}`;
}

function midday(day: string): number | null {
  const parts = /^(\d{4})-(\d{2})-(\d{2})$/u.exec(day);
  if (!parts) return null;
  const at = Date.UTC(Number(parts[1]), Number(parts[2]) - 1, Number(parts[3]));
  return Number.isNaN(at) ? null : at;
}

/** Whole days from today to a due date: 0 today, 1 tomorrow, negative overdue. */
export function daysUntil(due: string, today: string): number | null {
  const target = midday(due);
  const origin = midday(today);
  if (target === null || origin === null) return null;
  return Math.round((target - origin) / DAY_MS);
}

export function dueTone(due: string | null, today: string): DueTone | null {
  const days = due === null ? null : daysUntil(due, today);
  if (days === null) return null;
  if (days < 0) return "overdue";
  if (days === 0) return "today";
  return days <= 7 ? "soon" : "later";
}

export type DueLabels = { today: string; tomorrow: string; yesterday: string };

/** A date a reader can act on without doing arithmetic. Anything outside the
 *  week ahead is a real date, because "in 23 days" is not a plan. */
export function dueLabel(
  due: string,
  today: string,
  labels: DueLabels,
  time: string | null = null,
): string {
  const days = daysUntil(due, today);
  const at = midday(due);
  if (days === null || at === null) return due;
  const clock = time === null ? "" : ` ${timeLabel(time)}`;
  if (days === 0) return labels.today + clock;
  if (days === 1) return labels.tomorrow + clock;
  if (days === -1) return labels.yesterday + clock;
  const date = new Date(at);
  if (days > 1 && days <= 6) return WEEKDAY.format(date) + clock;
  const origin = midday(today);
  const sameYear = origin !== null && new Date(origin).getUTCFullYear() === date.getUTCFullYear();
  return (sameYear ? SHORT_DAY : SHORT_DAY_WITH_YEAR).format(date) + clock;
}

/** The day a task is next answerable for: its planned day or its deadline,
 *  whichever comes first. Rust places tasks in views by the same rule. */
export function dayDue(row: Pick<TaskRow, "dueDate" | "deadline">): string | null {
  if (row.deadline === null) return row.dueDate;
  if (row.dueDate === null || row.deadline < row.dueDate) return row.deadline;
  return row.dueDate;
}

export type DurationLabels = {
  minutes: (count: number) => string;
  hours: (count: number) => string;
  mixed: (hours: number, minutes: number) => string;
};

export function durationLabel(minutes: number, labels: DurationLabels): string {
  const hours = Math.floor(minutes / 60);
  const rest = minutes % 60;
  if (hours === 0) return labels.minutes(rest);
  return rest === 0 ? labels.hours(hours) : labels.mixed(hours, rest);
}

const RANK: Record<TaskRow["status"], number> = { blocked: 0, active: 1, open: 2, done: 3 };

/** What decides a row's order, read once per sort rather than once per
 *  comparison. */
type Placed = {
  row: TaskRow;
  rank: number;
  pinned: boolean;
  sortKey: string | null;
  day: string | null;
  time: string | null;
  /** When it was finished, for a row that counts as finished. */
  finished: bigint | null;
};

/** `open`: placed as though it were still open, as a row just completed is. */
function placedOf(row: TaskRow, open = false): Placed {
  const status = open ? "open" : row.status;
  return {
    row,
    rank: RANK[status],
    pinned: row.pinned,
    sortKey: row.sortKey,
    day: dayDue(row),
    time: row.dueTime,
    finished: status === "done" ? BigInt(row.completedAt ?? "0") : null,
  };
}

function byId(left: TaskRow, right: TaskRow): number {
  return left.id === right.id ? 0 : left.id < right.id ? -1 : 1;
}

function comparePlaced(left: Placed, right: Placed): number {
  if (left.finished !== null && right.finished !== null) {
    if (left.finished !== right.finished) return left.finished > right.finished ? -1 : 1;
    return byId(left.row, right.row);
  }
  if (left.rank !== right.rank) return left.rank - right.rank;
  if (left.pinned !== right.pinned) return left.pinned ? -1 : 1;
  if (left.sortKey !== right.sortKey) {
    if (left.sortKey === null) return 1;
    if (right.sortKey === null) return -1;
    return left.sortKey < right.sortKey ? -1 : 1;
  }
  if (left.day !== right.day) {
    if (left.day === null) return 1;
    if (right.day === null) return -1;
    return left.day < right.day ? -1 : 1;
  }
  // Within a day, what has an hour comes before what merely has to happen.
  if (left.time !== right.time) {
    if (left.time === null) return 1;
    if (right.time === null) return -1;
    return left.time < right.time ? -1 : 1;
  }
  return byId(left.row, right.row);
}

/** Blocked first: a delegated task that stopped needs a person more than
 *  anything else in the section does. Then pinned, then manual position, then
 *  the nearest date, and newest last so equal rows keep a stable order.
 */
export function compareRows(left: TaskRow, right: TaskRow): number {
  return comparePlaced(placedOf(left), placedOf(right));
}

function sectionOf(row: TaskRow, today: string): SectionKey {
  if (row.status === "done") return "completed";
  const day = dayDue(row);
  const days = day === null ? null : daysUntil(day, today);
  if (days === null) return "anytime";
  if (days < 0) return "overdue";
  if (days === 0) return "today";
  if (days === 1) return "tomorrow";
  return "upcoming";
}

const ORDER: SectionKey[] = ["overdue", "today", "tomorrow", "upcoming", "anytime", "completed"];

const SCOPES: Record<TaskScope, SectionKey[]> = {
  // Today answers "what now", so it carries what is late and what is due, and
  // nothing that is merely eventual.
  today: ["overdue", "today"],
  inbox: ORDER.filter((key) => key !== "completed"),
  upcoming: ["tomorrow", "upcoming"],
  completed: ["completed"],
  all: ORDER,
};

export type SectionOptions = {
  scope: TaskScope;
  today: string;
  labels: Record<SectionKey, string>;
  /** Tasks just completed here, kept in the section they were completed in so
   *  the row does not vanish from under the pointer that ticked it. */
  holding?: ReadonlySet<string>;
  /** How much of the finished pile to draw. Everything you have ever done is
   *  not a list, and nobody scrolls it. */
  completedLimit?: number;
};

export function sections(rows: readonly TaskRow[], options: SectionOptions): TaskSection[] {
  const { scope, today, labels, holding } = options;
  const allowed = new Set(SCOPES[scope]);
  const grouped = new Map<SectionKey, TaskRow[]>();
  for (const row of rows) {
    const held = holding?.has(row.id) ?? false;
    // A held row is placed as though it were still open, so completing it does
    // not move it before its own animation has run.
    const key = held ? sectionOf({ ...row, status: "open" }, today) : sectionOf(row, today);
    if (!allowed.has(key)) continue;
    const bucket = grouped.get(key);
    if (bucket) bucket.push(row);
    else grouped.set(key, [row]);
  }
  const limit = options.completedLimit ?? Number.POSITIVE_INFINITY;
  return ORDER.filter((key) => allowed.has(key) && grouped.get(key)?.length).map((key) => {
    // A held row also keeps its place within the section until it leaves.
    const rows = grouped
      .get(key)!
      .map((row) => placedOf(row, holding?.has(row.id)))
      .sort(comparePlaced)
      .map((placed) => placed.row);
    return {
      key,
      label: labels[key],
      total: rows.length,
      rows: key === "completed" ? rows.slice(0, limit) : rows,
    };
  });
}

/** Whether two drawings of a task sit in the same place: in the same
 *  section, in the same order. Its words decide neither. */
function samePlace(left: TaskRow, right: TaskRow): boolean {
  return (
    left === right ||
    (left.id === right.id &&
      left.status === right.status &&
      left.pinned === right.pinned &&
      left.sortKey === right.sortKey &&
      left.dueDate === right.dueDate &&
      left.dueTime === right.dueTime &&
      left.deadline === right.deadline &&
      left.completedAt === right.completedAt)
  );
}

/** `sections` for a list redrawn on every keystroke. While no row has
 *  moved, the last grouping is kept with the new drawings put in, so typing
 *  into a task never sorts the list again. */
export function createSections(): (
  rows: readonly TaskRow[],
  options: SectionOptions,
) => TaskSection[] {
  let last: {
    rows: readonly TaskRow[];
    key: string;
    result: TaskSection[];
  } | null = null;
  return (rows, options) => {
    const key = [
      options.scope,
      options.today,
      options.completedLimit ?? "",
      ...ORDER.map((section) => options.labels[section]),
      ...(options.holding ?? []),
    ].join("\n");
    const previous = last;
    if (
      previous &&
      previous.key === key &&
      previous.rows.length === rows.length &&
      rows.every((row, index) => samePlace(row, previous.rows[index]!))
    ) {
      const redrawn = rows.filter((row, index) => row !== previous.rows[index]);
      if (redrawn.length) {
        const drawing = new Map(redrawn.map((row) => [row.id, row]));
        previous.result = previous.result.map((section) => ({
          ...section,
          rows: section.rows.map((row) => drawing.get(row.id) ?? row),
        }));
      }
      previous.rows = rows;
      return previous.result;
    }
    const result = sections(rows, options);
    last = { rows, key, result };
    return result;
  };
}

/** What a progress figure over a task set means. A row still held in its old
 *  section is already done, so the figure moves the moment the checkbox does. */
export function completion(rows: readonly TaskRow[]): {
  done: number;
  total: number;
  ratio: number;
} {
  const total = rows.length;
  const done = rows.filter((row) => row.status === "done").length;
  return { done, total, ratio: total === 0 ? 0 : done / total };
}

/** The site a task's page belongs to. A row shows where it leads, not the whole
 *  address, and an address that will not parse is shown as it was stored. */
export function hostOf(url: string): string {
  try {
    return new URL(url).host.replace(/^www\./u, "");
  } catch {
    return url;
  }
}

/** Where a query lands in a title, so the row can show why it matched. */
export function matchRange(title: string, query: string): [number, number] | null {
  const needle = query.trim().toLocaleLowerCase();
  if (!needle) return null;
  const at = title.toLocaleLowerCase().indexOf(needle);
  return at < 0 ? null : [at, at + needle.length];
}

/** Whether a task falls inside a scope's span of days.
 *
 *  The board groups by state rather than by day, so it needs the scope's date
 *  question on its own — a finished task still belongs to the day it was due.
 */
export function inScope(row: TaskRow, scope: TaskScope, today: string): boolean {
  if (scope === "all") return true;
  if (scope === "inbox") return row.inbox === true && row.status !== "done";
  if (scope === "completed") return row.status === "done";
  const day = dayDue(row);
  const days = day === null ? null : daysUntil(day, today);
  if (scope === "today") return days !== null && days <= 0;
  return days !== null && days >= 1;
}

/** Whether a row's date says anything its section has not already said.
 *
 *  A Today list whose every row reads "Today" is noise wearing the costume of
 *  information. The chip stays reachable either way — it is how a date is
 *  changed — but it only speaks when it has something to add.
 */
export function showsDue(row: TaskRow, section: SectionKey, today: string): boolean {
  if (row.dueDate === null) return false;
  // An hour is always news, and so is being late.
  if (row.dueTime !== null) return true;
  if (dueTone(row.dueDate, today) === "overdue") return true;
  return section !== "today" && section !== "tomorrow" && section !== "completed";
}
