import { commands } from "$shared/ipc/bindings";

/** Compact is the rail that Work, Tasks and tools put the column into; the
 *  shape a person picks is expanded or hidden. */
export type SidebarMode = "default" | "compact" | "hidden";

/** What the toggle puts the column into. */
const PUT_AWAY: SidebarMode = "hidden";

/** The rail width. Native admits it as a real sidebar width, not a mode flag. */
export const COMPACT_WIDTH = 56;
export const MIN_EXPANDED_WIDTH = 180;
export const MAX_EXPANDED_WIDTH = 460;

/**
 * Dragging the handle below this snaps to the rail, and dragging back out of
 * the rail snaps to the minimum expanded width. Nothing settles in the gap,
 * so the sidebar is always in one of its two designed shapes.
 */
export const SNAP_THRESHOLD = 140;

const MODE_SETTING = "sidebar.mode";

let mode = $state.raw<SidebarMode>("default");
let desiredMode: SidebarMode = "default";
let expandedWidth = $state.raw(240);
let drag: { mode: SidebarMode; desired: SidebarMode; expanded: number; pending: number } | null =
  null;
let resizeSettlement = $state(0);
// One ordering counter, seeded above every earlier owner: an earlier
// document, and an earlier copy of this module in the same document, which a
// module reloaded in place leaves behind with native still holding its count.
let resizeRevision = $state(Math.trunc((performance.timeOrigin + performance.now()) * 1024));
export const sidebarResizeRevision = () => resizeRevision;
export function claimSidebarResizeOwner() {
  return ++resizeRevision;
}
export const sidebarResizeActive = () => drag !== null;
export const sidebarResizeSettlement = () => resizeSettlement;

export const sidebarMode = () => mode;
export const isCompact = () => mode === "compact";
export const isHidden = () => mode === "hidden" && !held;

/** A destination that wants the column in view, the rail of Work or a tool,
 *  holds a hidden sidebar out for as long as it is shown. */
let held = $state.raw(false);
export function holdShown(value: boolean) {
  if (value === held) return;
  held = value;
  if (mode === "hidden") publish(true);
}
export const expanded = () => expandedWidth;
/** The chrome gap between the rail and an open utility panel. */
export const PANEL_GAP = 8;
let panelExtent = $state.raw(0);

export const effectiveWidth = () =>
  panelExtent > 0
    ? COMPACT_WIDTH + PANEL_GAP + panelExtent
    : mode === "compact"
      ? COMPACT_WIDTH
      : // Hidden keeps its width: it comes back over the page at that width.
        expandedWidth;

export const hasPanel = () => panelExtent > 0;

/** A utility panel borrows width beside the rail without changing the persisted mode. */
export function setPanelExtent(extent: number) {
  const next = Math.max(0, Math.min(MAX_EXPANDED_WIDTH - COMPACT_WIDTH - PANEL_GAP, extent));
  if (next === panelExtent) return;
  panelExtent = next;
  publish(true);
}

function clampExpanded(value: number) {
  return Math.max(MIN_EXPANDED_WIDTH, Math.min(MAX_EXPANDED_WIDTH, Math.round(value)));
}

/** Resolves a raw drag width to whichever designed shape it lands in. */
export function resolveDragWidth(value: number): { mode: SidebarMode; expanded: number } {
  if (!Number.isFinite(value) || value < SNAP_THRESHOLD) {
    return { mode: PUT_AWAY, expanded: expandedWidth };
  }
  return { mode: "default", expanded: clampExpanded(value) };
}

/**
 * Tells native the column's width. `travel` marks a deliberate change of
 * shape — a toggle, a snap, a tool opening — which the page slides with;
 * a drag in progress or a restored preference moves it without ceremony.
 */
function publish(travel = false) {
  resizeRevision++;
  if (drag) cancelSidebarResize();
  const width = effectiveWidth();
  void commands.sidebarSetWidth(width, travel, resizeRevision);
  void commands.sidebarSetHidden(isHidden());
}

export function applyDragWidth(value: number) {
  if (drag) {
    drag.pending = value;
    return;
  }
  const next = resolveDragWidth(value);
  if (next.mode === mode && next.expanded === expandedWidth) return;
  const modeChanged = next.mode !== mode;
  mode = next.mode;
  expandedWidth = next.expanded;
  desiredMode = mode;
  publish(modeChanged);
  if (modeChanged) save(mode);
}

/** The non-native capture fallback also keeps the rendered layout unchanged. */
export function beginSidebarResize() {
  if (drag) return;
  drag = { mode, desired: desiredMode, expanded: expandedWidth, pending: effectiveWidth() };
}

export function finishSidebarResize(value: number) {
  if (!drag) return;
  drag = null;
  adoptResizeWidth(value);
  // The page travels to the new edge, as it does from the native guide.
  void commands.sidebarSetWidth(effectiveWidth(), true, resizeRevision);
}

export function cancelSidebarResize() {
  drag = null;
}

/** Native has admitted one final width; adopt its display shape without another layout command. */
export function adoptResizeWidth(value: number, revision = resizeRevision) {
  if (revision !== resizeRevision) return;
  if (!Number.isFinite(value)) return;
  // A fast drag overshoots the bounds; it lands on the nearest one.
  const next = resolveDragWidth(Math.max(COMPACT_WIDTH, Math.min(MAX_EXPANDED_WIDTH, value)));
  if (next.mode === "hidden") {
    expandedWidth = next.expanded;
    resizeSettlement++;
    setMode("hidden");
    return;
  }
  const changed = next.mode !== mode;
  mode = next.mode;
  desiredMode = mode;
  expandedWidth = next.expanded;
  resizeSettlement++;
  if (changed) save(mode);
}

let resizeListenerInstalled = false;
function installResizeListener() {
  if (resizeListenerInstalled) return;
  resizeListenerInstalled = true;
  window.addEventListener("zephium:sidebar-width-selected", (event) => {
    if (!(event instanceof CustomEvent) || !event.detail || typeof event.detail !== "object")
      return;
    const { width, revision } = event.detail;
    if (typeof width === "number" && typeof revision === "number")
      adoptResizeWidth(width, revision);
  });
}

// The shape this column last saved, until the store reports it back. Values
// arriving meanwhile are the store catching up with saves made here, not a
// change from elsewhere: adopting them would snap the column back to a shape
// it has already left. A save whose report never comes stops counting.
let saving: { mode: SidebarMode; timer: ReturnType<typeof setTimeout> } | null = null;

function save(next: SidebarMode) {
  if (saving) clearTimeout(saving.timer);
  saving = { mode: next, timer: setTimeout(() => (saving = null), 2000) };
  void commands.settingSet(MODE_SETTING, next);
}

/** The rail was once a shape a person picked; it is the hidden one now. */
const chosen = (stored: SidebarMode): SidebarMode => (stored === "compact" ? PUT_AWAY : stored);

/** A stored preference reached this column: from here, or from elsewhere. */
export function adoptMode(stored: SidebarMode) {
  const next = chosen(stored);
  if (saving) {
    if (next === saving.mode) {
      clearTimeout(saving.timer);
      saving = null;
    }
    return;
  }
  cancelSidebarResize();
  if (next === mode) return;
  mode = next;
  desiredMode = next;
  publish();
}

export function setMode(next: SidebarMode) {
  cancelSidebarResize();
  if (next === mode) return;
  mode = next;
  desiredMode = next;
  publish(true);
  save(next);
}

/**
 * Shows the full column for a moment without touching the preference. The
 * returned function puts the rail back, unless a shape was chosen meanwhile.
 */
export function expandBriefly(): () => void {
  cancelSidebarResize();
  const away = mode;
  if (away === "default") return () => {};
  mode = "default";
  publish(true);
  return () => {
    if (mode !== "default" || desiredMode !== away) return;
    cancelSidebarResize();
    mode = away;
    publish(true);
  };
}

export function toggleMode() {
  cancelSidebarResize();
  desiredMode = desiredMode === "default" ? PUT_AWAY : "default";
  setMode(desiredMode);
}

/**
 * Adopts the persisted mode. The width is published afterwards so native
 * layout matches the restored shape on the first paint rather than after the
 * first interaction.
 */
export async function init(): Promise<void> {
  installResizeListener();
  try {
    const stored = await commands.settingGet(MODE_SETTING);
    if (stored === "compact" || stored === "hidden") {
      mode = chosen(stored);
      desiredMode = mode;
    }
  } catch {
    // A missing or unreadable preference is not a startup failure; the
    // default shape is always valid.
  }
  publish();
}
