import type { ResourceSummary } from "$shared/ipc/bindings";
const MAX_LISTED = 1000;
export function mergePage(
  previous: readonly ResourceSummary[],
  next: readonly ResourceSummary[],
): ResourceSummary[] {
  return [...new Map([...previous, ...next].map((row) => [row.id, row])).values()].slice(
    0,
    MAX_LISTED,
  );
}
/** A record just settled, put where its row already is or else first: at
 *  the cap, the last row loaded gives way rather than the one just saved. */
export function settleInto(
  items: readonly ResourceSummary[],
  summary: ResourceSummary,
): ResourceSummary[] {
  const at = items.findIndex((item) => item.id === summary.id);
  if (at < 0) return [summary, ...items].slice(0, MAX_LISTED);
  const next = [...items];
  next[at] = summary;
  return next;
}
export function newerRevision(candidate: string, current: string): boolean {
  const valid = (value: string) => /^[1-9][0-9]{0,18}$/u.test(value);
  return valid(candidate) && valid(current) && BigInt(candidate) > BigInt(current);
}
