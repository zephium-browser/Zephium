import type { PlanView } from "./types";

const encoder = new TextEncoder();

function escapeText(value: string): string {
  return value
    .replace(/\\/g, "\\\\")
    .replace(/;/g, "\\;")
    .replace(/,/g, "\\,")
    .replace(/\r\n|\r|\n/g, "\\n");
}

/** Folds at 75 octets without splitting a character; continuations start with one space. */
function fold(line: string): string {
  const lines: string[] = [];
  let current = "";
  let size = 0;
  for (const char of line) {
    const width = encoder.encode(char).length;
    const limit = lines.length === 0 ? 75 : 74;
    if (size + width > limit) {
      lines.push(current);
      current = "";
      size = 0;
    }
    current += char;
    size += width;
  }
  lines.push(current);
  return lines.join("\r\n ");
}

const pad = (value: number, length = 2) => String(value).padStart(length, "0");

function dayAfter(date: string): string | null {
  const match = /^(\d{4})-(\d{2})-(\d{2})$/.exec(date);
  if (!match) return null;
  const [year, month, day] = [Number(match[1]), Number(match[2]), Number(match[3])];
  const next = new Date(Date.UTC(year, month - 1, day + 1));
  if (Number.isNaN(next.getTime())) return null;
  return `${pad(next.getUTCFullYear(), 4)}${pad(next.getUTCMonth() + 1)}${pad(next.getUTCDate())}`;
}

function stamp(now: Date): string {
  const iso = now.toISOString();
  return `${iso.slice(0, 4)}${iso.slice(5, 7)}${iso.slice(8, 10)}T${iso.slice(11, 13)}${iso.slice(14, 16)}${iso.slice(17, 19)}Z`;
}

/** The plan's dated steps as an iCalendar file, or null when no step falls on a day. */
export function planIcs(
  plan: PlanView,
  options: { now?: Date; product?: string } = {},
): string | null {
  const dtstamp = stamp(options.now ?? new Date());
  const events: string[] = [];
  plan.steps.forEach((step, index) => {
    if (!step.date) return;
    const end = dayAfter(step.date);
    if (!end) return;
    events.push(
      "BEGIN:VEVENT",
      `UID:${plan.id}-${index}@zephium.work`,
      `DTSTAMP:${dtstamp}`,
      `DTSTART;VALUE=DATE:${step.date.replaceAll("-", "")}`,
      `DTEND;VALUE=DATE:${end}`,
      `SUMMARY:${escapeText(step.title)}`,
    );
    if (step.detail) events.push(`DESCRIPTION:${escapeText(step.detail)}`);
    if (step.place) events.push(`LOCATION:${escapeText(step.place)}`);
    events.push("END:VEVENT");
  });
  if (events.length === 0) return null;
  const lines = [
    "BEGIN:VCALENDAR",
    "VERSION:2.0",
    `PRODID:-//${options.product ?? "Zephium"}//Work//EN`,
    "CALSCALE:GREGORIAN",
    "METHOD:PUBLISH",
    ...(plan.title ? [`X-WR-CALNAME:${escapeText(plan.title)}`] : []),
    ...events,
    "END:VCALENDAR",
  ];
  return lines.map(fold).join("\r\n") + "\r\n";
}

/** A file-system-safe name for the plan's calendar file. */
export function icsFileName(plan: PlanView): string {
  let base = (plan.title ?? "")
    .replace(/[\\/:*?"<>|]/g, " ")
    .replace(/\p{Cc}/gu, " ")
    .replace(/\s+/g, " ")
    .trim()
    .replace(/^\.+/, "")
    .trim()
    .replace(/[. ]+$/u, "");
  const device = base.split(".")[0]!.trimEnd();
  if (/^(?:CON|PRN|AUX|NUL|CLOCK\$|COM[1-9¹²³]|LPT[1-9¹²³])$/iu.test(device)) base = `plan ${base}`;
  // Native admits at most 120 UTF-8 bytes, including the extension.
  let stem = "";
  let bytes = 0;
  let characters = 0;
  for (const character of base) {
    const width = encoder.encode(character).length;
    if (bytes + width > 116 || characters === 80) break;
    stem += character;
    bytes += width;
    characters++;
  }
  return `${stem.trimEnd().replace(/\.+$/u, "") || "plan"}.ics`;
}
