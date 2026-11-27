import { everyWhileVisible } from "$shared/lib/visible-interval";
import { todayKey } from "./task-sections";

/** The local calendar day, shared by every task surface.
 *
 *  One timer rather than one per list, so two surfaces can never disagree about
 *  when today ends, and a hidden panel is not paying for a second interval.
 */
let current = $state(todayKey());
let stop: (() => void) | undefined;
let watchers = 0;

export function today(): string {
  return current;
}

/** Keeps the day current while a surface is showing it. */
export function watchToday(): () => void {
  const now = todayKey();
  if (now !== current) current = now;
  watchers += 1;
  // Midnight while a list is open must not leave yesterday's work in Today.
  stop ??= everyWhileVisible(60_000, () => {
    const now = todayKey();
    if (now !== current) current = now;
  });
  return () => {
    watchers -= 1;
    if (watchers > 0 || stop === undefined) return;
    stop();
    stop = undefined;
  };
}
