export type TabSelectionState = {
  readonly ids: readonly string[];
  readonly anchor: string | null;
};

export const emptySelection = (): TabSelectionState => ({ ids: [], anchor: null });

export function isSelectionModifier(
  event: Pick<MouseEvent, "metaKey" | "ctrlKey" | "shiftKey">,
  mac: boolean,
): boolean {
  return event.shiftKey || (mac ? event.metaKey : event.ctrlKey);
}

export function plainSelection(id: string): TabSelectionState {
  return { ids: [], anchor: id };
}

export function toggleSelection(
  state: TabSelectionState,
  id: string,
  active: string | null,
): TabSelectionState {
  const ids = new Set(state.ids);
  if (ids.size === 0 && active !== null) ids.add(active);
  if (ids.has(id)) ids.delete(id);
  else ids.add(id);
  return { ids: [...ids], anchor: id };
}

export function rangeSelection(
  state: TabSelectionState,
  id: string,
  active: string | null,
  order: readonly string[],
): TabSelectionState {
  const end = order.indexOf(id);
  if (end === -1) return state;
  // A hidden anchor or one in another section cannot define this list's range.
  const anchor = [state.anchor, active, id].find(
    (value) => value !== null && order.includes(value),
  )!;
  const start = order.indexOf(anchor);
  return {
    ids: order.slice(Math.min(start, end), Math.max(start, end) + 1),
    anchor,
  };
}

export function pruneSelection(
  state: TabSelectionState,
  available: ReadonlySet<string>,
): TabSelectionState {
  const ids = state.ids.filter((id) => available.has(id));
  const anchor = state.anchor !== null && available.has(state.anchor) ? state.anchor : null;
  return ids.length === state.ids.length && anchor === state.anchor ? state : { ids, anchor };
}
