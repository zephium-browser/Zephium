const clock = new Intl.DateTimeFormat(undefined, { hour: "numeric", minute: "2-digit" });
const date = new Intl.DateTimeFormat(undefined, {
  weekday: "long",
  month: "long",
  day: "numeric",
  timeZone: "UTC",
});

const midnight = (key: string) => Date.parse(`${key}T00:00:00Z`);

/** A time of day from its minutes after midnight, in the person's own format. */
export const clockOf = (minutes: number) =>
  clock.format(new Date(2000, 0, 1, Math.floor(minutes / 60) % 24, minutes % 60));

/** A day's name: null for yesterday, which has a word of its own, else its date. */
export function dayName(key: string, today: string): string | null {
  const days = Math.round((midnight(today) - midnight(key)) / 86_400_000);
  return days === 1 ? null : date.format(midnight(key));
}
