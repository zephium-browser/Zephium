import type { LittleView, TabView } from "$shared/ipc/bindings";

const revision = /^[0-9a-f]{32}$/;

/** The window a toolbar's label names: canonical decimal only, as native parses it. */
export function windowOfLabel(label: string): number | null {
  const match = /^little-([1-9]\d*)$/.exec(label);
  if (!match) return null;
  const window = Number(match[1]);
  return Number.isSafeInteger(window) ? window : null;
}

/** A view for this window replaces the page shown, unless it is an older revision of it. */
export function acceptLittleView(
  window: number,
  current: TabView | null,
  next: LittleView,
): boolean {
  if (next.window !== window) return false;
  return next.tab === null || acceptLittleTab(current, next.tab);
}

export function acceptLittleTab(current: TabView | null, next: TabView): boolean {
  return (
    revision.test(next.projection_revision) &&
    (!current || current.id !== next.id || next.projection_revision >= current.projection_revision)
  );
}

/** The host the toolbar shows. Native's presentation check reads it back by the same rule. */
export function hostOf(url: string | null): string {
  if (!url) return "";
  try {
    return new URL(url).host;
  } catch {
    return "";
  }
}
