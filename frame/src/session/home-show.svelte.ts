/**
 * The search native last asked Home to show, after a page closed or a group
 * title was clicked. It is held from startup, because the event comes before
 * Home's own code has loaded; Home takes it when it draws.
 */
import { events } from "$shared/ipc/native-events";

type Shown = {
  search: string;
  /** The row the closed page was opened from, when there is one. */
  row: { slot: number; entry: string } | null;
  /** Counts up with each request, so asking for the same search again still counts. */
  serial: number;
};

let shown = $state.raw<Shown | null>(null);
let serial = 0;

export const latest = () => shown;

/** Home went to it, so a Home drawn later does not go there again. */
export function taken(serial: number) {
  if (shown?.serial === serial) shown = null;
}

export function init(): Promise<() => void> {
  return events.homeShow.listen(({ payload }) => {
    const { search, slot, entry } = payload;
    if (typeof search !== "string" || !search) return;
    const row =
      typeof slot === "number" && typeof entry === "string" && entry ? { slot, entry } : null;
    shown = { search, row, serial: ++serial };
  });
}
