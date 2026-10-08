import { expect, test } from "vitest";
import type { TaskRow } from "$domain/resources";
import {
  compareRows,
  completion,
  createSections,
  matchRange,
  dayKey,
  daysUntil,
  dueLabel,
  dueTone,
  dayDue,
  durationLabel,
  inScope,
  sections,
  showsDue,
} from "../lib/task-sections";

const LABELS = {
  overdue: "Overdue",
  today: "Today",
  tomorrow: "Tomorrow",
  upcoming: "Upcoming",
  anytime: "Anytime",
  completed: "Completed",
};
const DUE = { today: "Today", tomorrow: "Tomorrow", yesterday: "Yesterday" };
const TODAY = "2026-09-20";

function row(patch: Partial<TaskRow> & { id: string }): TaskRow {
  return {
    revision: "1",
    title: "Task",
    description: null,
    createdAt: null,
    pinned: false,
    updatedAt: "1",
    status: "open",
    assignee: "user",
    origin: "user",
    dueDate: null,
    dueTime: null,
    deadline: null,
    duration: null,
    context: null,
    sortKey: null,
    work: null,
    pending: false,
    list: null,
    inbox: false,
    priority: "none",
    stepCount: 0,
    stepDone: 0,
    completedAt: null,
    ...patch,
  };
}

test("a due date is a calendar day, not an instant", () => {
  expect(dayKey(new Date(2026, 8, 20, 23, 59))).toBe("2026-09-20");
  expect(daysUntil("2026-09-21", TODAY)).toBe(1);
  expect(daysUntil("2026-09-19", TODAY)).toBe(-1);
  // Across a month and a leap day, still whole days.
  expect(daysUntil("2026-10-01", TODAY)).toBe(11);
  expect(daysUntil("2028-03-01", "2028-02-28")).toBe(2);
  expect(daysUntil("not-a-date", TODAY)).toBeNull();
});

test("dates inside the week read as days and the rest read as dates", () => {
  expect(dueLabel(TODAY, TODAY, DUE)).toBe("Today");
  expect(dueLabel("2026-09-21", TODAY, DUE)).toBe("Tomorrow");
  expect(dueLabel("2026-09-19", TODAY, DUE)).toBe("Yesterday");
  expect(dueLabel("2026-09-24", TODAY, DUE)).toBe("Thursday");
  // Beyond the week ahead, "in 23 days" is not a plan.
  expect(dueLabel("2026-10-13", TODAY, DUE)).not.toContain("day");
  expect(dueLabel("2027-01-04", TODAY, DUE)).toContain("2027");
});

test("tone marks only what is late or now", () => {
  expect(dueTone("2026-09-19", TODAY)).toBe("overdue");
  expect(dueTone(TODAY, TODAY)).toBe("today");
  expect(dueTone("2026-09-25", TODAY)).toBe("soon");
  expect(dueTone("2026-12-25", TODAY)).toBe("later");
  expect(dueTone(null, TODAY)).toBeNull();
});

test("a stalled delegation sorts above everything else in its section", () => {
  const ordered = [
    row({ id: "c", status: "open", pinned: true }),
    row({ id: "b", status: "blocked" }),
    row({ id: "a", status: "active" }),
  ].sort(compareRows);
  expect(ordered.map((entry) => entry.id)).toEqual(["b", "a", "c"]);
});

test("manual position wins over the date, and an unplaced task follows a placed one", () => {
  const ordered = [
    row({ id: "a", dueDate: "2026-09-20" }),
    row({ id: "b", sortKey: "m", dueDate: "2026-12-01" }),
  ].sort(compareRows);
  expect(ordered.map((entry) => entry.id)).toEqual(["b", "a"]);
});

test("Today carries what is late and what is due, and nothing merely eventual", () => {
  const rows = [
    row({ id: "late", dueDate: "2026-09-18" }),
    row({ id: "now", dueDate: TODAY }),
    row({ id: "next", dueDate: "2026-09-21" }),
    row({ id: "someday" }),
    row({ id: "finished", status: "done" }),
  ];
  expect(
    sections(rows, { scope: "today", today: TODAY, labels: LABELS }).map((s) => s.key),
  ).toEqual(["overdue", "today"]);
  expect(
    sections(rows, { scope: "upcoming", today: TODAY, labels: LABELS }).map((s) => s.key),
  ).toEqual(["tomorrow"]);
  // An empty section is not drawn at all.
  expect(sections(rows, { scope: "all", today: TODAY, labels: LABELS }).map((s) => s.key)).toEqual([
    "overdue",
    "today",
    "tomorrow",
    "anytime",
    "completed",
  ]);
});

test("a task just completed keeps the place it was completed in", () => {
  const rows = [row({ id: "now", dueDate: TODAY, status: "done" })];
  const settled = sections(rows, { scope: "today", today: TODAY, labels: LABELS });
  expect(settled).toEqual([]);
  const held = sections(rows, {
    scope: "today",
    today: TODAY,
    labels: LABELS,
    holding: new Set(["now"]),
  });
  expect(held.map((section) => section.key)).toEqual(["today"]);
  // And the figure moves immediately even while the row is still drawn.
  expect(completion(rows)).toEqual({ done: 1, total: 1, ratio: 1 });
});

test("completion of an empty set is zero, not a division", () => {
  expect(completion([])).toEqual({ done: 0, total: 0, ratio: 0 });
});

test("the finished pile is clipped, and says how much it is hiding", () => {
  const rows = Array.from({ length: 40 }, (_unused, index) =>
    row({ id: `done-${String(index).padStart(2, "0")}`, status: "done" }),
  );
  const [section] = sections(rows, {
    scope: "all",
    today: TODAY,
    labels: LABELS,
    completedLimit: 20,
  });
  expect(section!.rows).toHaveLength(20);
  expect(section!.total).toBe(40);
  // Every other section is drawn whole.
  const open = sections([row({ id: "a", dueDate: TODAY })], {
    scope: "today",
    today: TODAY,
    labels: LABELS,
    completedLimit: 1,
  });
  expect(open[0]!.rows).toHaveLength(1);
  expect(open[0]!.total).toBe(1);
});

test("a match is located so the row can show why it matched", () => {
  expect(matchRange("Review the vendor contract", "vendor")).toEqual([11, 17]);
  expect(matchRange("Review the vendor contract", "VENDOR")).toEqual([11, 17]);
  expect(matchRange("Review", "  ")).toBeNull();
  expect(matchRange("Review", "missing")).toBeNull();
});

test("a scope asks about days, which a finished task still has", () => {
  const done = row({ id: "d", status: "done", dueDate: TODAY });
  expect(inScope(done, "today", TODAY)).toBe(true);
  expect(inScope(done, "upcoming", TODAY)).toBe(false);
  expect(inScope(done, "all", TODAY)).toBe(true);
  expect(inScope(row({ id: "late", dueDate: "2026-09-01" }), "today", TODAY)).toBe(true);
  expect(inScope(row({ id: "none" }), "upcoming", TODAY)).toBe(false);
  expect(inScope(row({ id: "none2" }), "today", TODAY)).toBe(false);
});

test("a date speaks only when its section has not already said it", () => {
  const dueToday = row({ id: "a", dueDate: TODAY });
  expect(showsDue(dueToday, "today", TODAY)).toBe(false);
  expect(showsDue(row({ id: "b", dueDate: "2026-09-21" }), "tomorrow", TODAY)).toBe(false);
  // An hour is news even under the day that already named itself.
  expect(showsDue(row({ id: "c", dueDate: TODAY, dueTime: "15:00" }), "today", TODAY)).toBe(true);
  // So is being late, and which day it was.
  expect(showsDue(row({ id: "d", dueDate: "2026-09-18" }), "overdue", TODAY)).toBe(true);
  // Upcoming spans many days, so each row has to say which.
  expect(showsDue(row({ id: "e", dueDate: "2026-10-01" }), "upcoming", TODAY)).toBe(true);
  // Nothing to say without a date at all.
  expect(showsDue(row({ id: "f" }), "anytime", TODAY)).toBe(false);
  expect(showsDue(row({ id: "g", dueDate: TODAY, status: "done" }), "completed", TODAY)).toBe(
    false,
  );
});

test("a deadline places a task by whichever day comes first", () => {
  const planned = row({ id: "a", dueDate: "2026-09-25", deadline: "2026-09-19" });
  const owed = row({ id: "b", deadline: TODAY });
  const later = row({ id: "c", dueDate: TODAY, deadline: "2026-10-01" });
  expect(dayDue(planned)).toBe("2026-09-19");
  expect(dayDue(owed)).toBe(TODAY);
  expect(dayDue(later)).toBe(TODAY);
  const grouped = sections([later, owed, planned], {
    scope: "today",
    today: TODAY,
    labels: LABELS,
  });
  expect(grouped.map((section) => [section.key, section.rows.map((task) => task.id)])).toEqual([
    ["overdue", ["a"]],
    ["today", ["b", "c"]],
  ]);
  expect(inScope(owed, "upcoming", TODAY)).toBe(false);
});

test("an estimate reads in the largest whole units", () => {
  const labels = {
    minutes: (count: number) => `${count}m`,
    hours: (count: number) => `${count}h`,
    mixed: (hours: number, minutes: number) => `${hours}h ${minutes}m`,
  };
  expect(durationLabel(45, labels)).toBe("45m");
  expect(durationLabel(120, labels)).toBe("2h");
  expect(durationLabel(90, labels)).toBe("1h 30m");
});

test("a task completed a moment ago keeps its place until it leaves", () => {
  const first = row({ id: "a", dueDate: TODAY, status: "done" });
  const second = row({ id: "b", dueDate: TODAY });
  const grouped = sections([first, second], {
    scope: "today",
    today: TODAY,
    labels: LABELS,
    holding: new Set(["a"]),
  });
  expect(grouped[0]!.rows.map((task) => task.id)).toEqual(["a", "b"]);
});

test("typing into a task keeps the list's grouping and puts in the new words", () => {
  const sectioned = createSections();
  const rows = [
    row({ id: "a", dueDate: TODAY, title: "Write" }),
    row({ id: "b", dueDate: TODAY, pinned: true }),
    row({ id: "c" }),
  ];
  const options = { scope: "all" as const, today: TODAY, labels: LABELS };
  const first = sectioned(rows, options);
  expect(sectioned(rows, { ...options, labels: { ...LABELS } })).toBe(first);
  const typed = { ...rows[0]!, title: "Write the report", description: "Draft" };
  const second = sectioned([typed, rows[1]!, rows[2]!], options);
  expect(second.map((section) => section.rows.map((entry) => entry.id))).toEqual(
    first.map((section) => section.rows.map((entry) => entry.id)),
  );
  expect(second[0]!.rows.find((entry) => entry.id === "a")).toBe(typed);
});

test("a task that moves is placed again", () => {
  const sectioned = createSections();
  const rows = [row({ id: "a", dueDate: TODAY }), row({ id: "b", dueDate: TODAY })];
  const options = { scope: "all" as const, today: TODAY, labels: LABELS };
  expect(sectioned(rows, options)[0]!.rows.map((entry) => entry.id)).toEqual(["a", "b"]);
  const pinned = [rows[0]!, { ...rows[1]!, pinned: true }];
  expect(sectioned(pinned, options)[0]!.rows.map((entry) => entry.id)).toEqual(["b", "a"]);
  const later = [rows[0]!, { ...rows[1]!, dueDate: "2026-09-30" }];
  expect(sectioned(later, options).map((section) => section.key)).toEqual(["today", "upcoming"]);
  const held = sectioned([{ ...rows[0]!, status: "done" as const }, rows[1]!], {
    ...options,
    holding: new Set(["a"]),
  });
  expect(held.map((section) => section.key)).toEqual(["today"]);
});
