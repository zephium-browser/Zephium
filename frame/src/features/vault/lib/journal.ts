/** Daily notes live in `Journal/YYYY-MM-DD.md`, the format Obsidian uses by default. */
export const JOURNAL = "Journal";

function pad(value: number): string {
  return String(value).padStart(2, "0");
}

export function journalName(date: Date): string {
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}.md`;
}

export function journalPath(date: Date): string {
  return `${JOURNAL}/${journalName(date)}`;
}

export function journalTemplate(date: Date): string {
  const title = date.toLocaleDateString(undefined, {
    weekday: "long",
    day: "numeric",
    month: "long",
    year: "numeric",
  });
  return `# ${title}\n\n`;
}

/** Six weeks starting on the Sunday on or before the first of the month. */
export function monthGrid(month: Date): { weekdays: string[]; days: Date[] } {
  const first = new Date(month.getFullYear(), month.getMonth(), 1);
  const start = new Date(first);
  start.setDate(1 - first.getDay());
  const days = Array.from({ length: 42 }, (_, index) => {
    const day = new Date(start);
    day.setDate(start.getDate() + index);
    return day;
  });
  const weekdays = days
    .slice(0, 7)
    .map((day) => day.toLocaleDateString(undefined, { weekday: "narrow" }));
  return { weekdays, days };
}
