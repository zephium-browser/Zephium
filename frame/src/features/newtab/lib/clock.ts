export type ClockFormat = "system" | "12h" | "24h";

/** The time split the way it is set: the figures carry the display size and
 *  the day period is typeset beside them, smaller, on whichever side the
 *  locale puts it. */
export type ClockFace = { time: string; period: string | null; periodLeading: boolean };

const faces = new Map<string, Intl.DateTimeFormat>();

/** The system's choice stands in for "system" when the host reports one;
 *  otherwise the locale keeps its own. A 24-hour clock counts 0-23, as
 *  `hour12: false` would not in every engine. */
function formatter(
  format: ClockFormat,
  locale: string | undefined,
  system24: boolean | null,
): Intl.DateTimeFormat {
  const chosen = format === "system" && system24 !== null ? (system24 ? "24h" : "12h") : format;
  const key = `${locale ?? ""}|${chosen}`;
  let found = faces.get(key);
  if (!found) {
    found = new Intl.DateTimeFormat(locale, {
      hour: "numeric",
      minute: "2-digit",
      ...(chosen === "system"
        ? {}
        : chosen === "24h"
          ? { hourCycle: "h23" as const }
          : { hour12: true }),
    });
    faces.set(key, found);
  }
  return found;
}

/** `system24` is whether the operating system is set to 24-hour time, or null
 *  where it cannot say. */
export function clockFace(
  at: Date,
  format: ClockFormat,
  locale?: string,
  system24: boolean | null = null,
): ClockFace {
  const parts = formatter(format, locale, system24).formatToParts(at);
  const period = parts.findIndex((part) => part.type === "dayPeriod");
  if (period < 0)
    return { time: parts.map((part) => part.value).join(""), period: null, periodLeading: false };
  // The spacing that joins the period to the figures goes with the period.
  const time = parts
    .filter(
      (part, index) =>
        index !== period && !(part.type === "literal" && Math.abs(index - period) === 1),
    )
    .map((part) => part.value)
    .join("")
    .trim();
  return {
    time,
    period: parts[period]!.value,
    periodLeading: parts.slice(0, period).every((part) => part.type === "literal"),
  };
}

export function clockFormat(value: string): ClockFormat {
  return value === "12h" || value === "24h" ? value : "system";
}

/** Milliseconds until the next minute begins, so a clock redraws on the turn
 *  rather than up to a minute late. */
export function untilNextMinute(now: number): number {
  return 60_000 - (now % 60_000);
}

/** The local calendar day as `YYYY-MM-DD`, the shape a task's due date takes. */
export function dayKey(at: Date): string {
  return `${at.getFullYear()}-${String(at.getMonth() + 1).padStart(2, "0")}-${String(at.getDate()).padStart(2, "0")}`;
}

/** A span of focused time, as hours and minutes: "2h 14m", "4h", "45m". */
export function focusSpan(minutes: number): string {
  const whole = Math.max(0, Math.round(minutes));
  const hours = Math.floor(whole / 60);
  const rest = whole % 60;
  if (hours === 0) return `${rest}m`;
  return rest === 0 ? `${hours}h` : `${hours}h ${rest}m`;
}
