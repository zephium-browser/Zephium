/** Manual position within a column.
 *
 * Keys are fixed-width decimal strings so text comparison preserves numeric
 * order in the store index and `compareRows`. A midpoint normally lets a
 * moved task change position with one write.
 *
 * Gaps are finite. `orderBetween` returns null when no midpoint is available;
 * board insertion then keeps the current order and reports the unavailable
 * position. List `reorderKeys` instead rebalances the supplied ordered list
 * when a midpoint is unavailable or unpositioned neighbours require it,
 * returning every key that changes for the caller to persist.
 */
const WIDTH = 12;
const STEP = 65_536;
const BASE = 1_000_000_000;

export function orderKey(value: number): string {
  return String(Math.max(1, Math.round(value))).padStart(WIDTH, "0");
}

export function orderValue(key: string | null): number | null {
  if (key === null || !/^\d{1,12}$/u.test(key)) return null;
  const value = Number(key);
  return Number.isSafeInteger(value) && value > 0 ? value : null;
}

/** A key that sorts between two neighbours, or null when the gap is spent. */
export function orderBetween(before: string | null, after: string | null): string | null {
  const low = orderValue(before);
  const high = orderValue(after);
  if (low === null && high === null) return orderKey(BASE);
  if (low === null) return high! > STEP ? orderKey(high! - STEP) : null;
  if (high === null) return orderKey(low + STEP);
  if (high - low < 2) return null;
  return orderKey(Math.floor((low + high) / 2));
}

/** Where a card lands when dropped onto `over` within `ids`.
 *
 *  Dropping onto the upper half of a card means before it, the lower half after
 *  it — the same reading as every list that accepts a drop.
 */
export function placement(
  ids: readonly string[],
  moving: string,
  over: string | null,
  after: boolean,
): { before: string | null; after: string | null } {
  const without = ids.filter((id) => id !== moving);
  if (over === null || over === moving) return { before: without.at(-1) ?? null, after: null };
  const at = without.indexOf(over);
  if (at < 0) return { before: without.at(-1) ?? null, after: null };
  return after
    ? { before: over, after: without[at + 1] ?? null }
    : { before: without[at - 1] ?? null, after: over };
}

/** Positions that make `ordered` the drawn order after `moved` was placed.
 *
 *  A task with no position sorts after every task with one, so placing one
 *  between two unpositioned neighbours cannot be a single write: the group is
 *  positioned once, in the order it is already drawn, and only keys that change
 *  are returned. Otherwise the moved task alone takes the midpoint.
 */
export function reorderKeys(
  ordered: readonly { id: string; sortKey: string | null }[],
  moved: string,
): Record<string, string> {
  const at = ordered.findIndex((row) => row.id === moved);
  if (at < 0) return {};
  const before = ordered[at - 1];
  const after = ordered[at + 1];
  if (!before || before.sortKey !== null) {
    const key = orderBetween(before?.sortKey ?? null, after?.sortKey ?? null);
    if (key !== null) return { [moved]: key };
  }
  const keys: Record<string, string> = {};
  ordered.forEach((row, index) => {
    const key = orderKey(BASE + index * STEP);
    if (row.sortKey !== key) keys[row.id] = key;
  });
  return keys;
}
