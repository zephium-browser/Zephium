import type { MiniIntent, MiniView, TabView } from "$shared/ipc/bindings";

const revision = /^[0-9a-f]{32}$/;

/** The window a toolbar's label names: canonical decimal only, as native parses it. */
export function windowOfLabel(label: string): number | null {
  const match = /^mini-([1-9]\d*)$/.exec(label);
  if (!match) return null;
  const window = Number(match[1]);
  return Number.isSafeInteger(window) ? window : null;
}

/** A view for this window replaces the page shown, unless it is an older revision of it. */
export function acceptMiniView(window: number, current: TabView | null, next: MiniView): boolean {
  if (next.window !== window) return false;
  return next.tab === null || acceptMiniTab(current, next.tab);
}

export function acceptMiniTab(current: TabView | null, next: TabView): boolean {
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

type Keys = Pick<KeyboardEvent, "key" | "metaKey" | "ctrlKey" | "altKey" | "shiftKey">;

/** The window's own keys while its toolbar holds focus, under the platform's
 *  command modifier: open in Zephium, address, close, reload. */
export function miniShortcut(event: Keys, mac: boolean): MiniIntent | null {
  const command = mac ? event.metaKey && !event.ctrlKey : event.ctrlKey && !event.metaKey;
  if (!command || event.altKey || event.shiftKey) return null;
  switch (event.key.toLowerCase()) {
    case "o":
      return "open_in_main";
    case "l":
      return "edit_address";
    case "w":
      return "close";
    case "r":
      return "reload";
    case "[":
      return "back";
    case "]":
      return "forward";
    default:
      return null;
  }
}
