import { commands } from "$shared/ipc/bindings";
import { IS_MAC } from "$shared/platform";

export type SidebarMode = "default" | "compact";

/** The rail width. Native admits it as a real sidebar width, not a mode flag. */
export const COMPACT_WIDTH = 56;
export const MIN_EXPANDED_WIDTH = 180;
export const MAX_EXPANDED_WIDTH = 460;

/**
 * Dragging the rail out past this lands on the minimum expanded width; short
 * of it, the rail stays. An expanded column's drag stops at its minimum and
 * never folds into the rail; released short of this, it hides (macOS). Nothing
 * settles in the gap, so the sidebar is always in one of its two designed
 * shapes.
 */
export const SNAP_THRESHOLD = 140;

const MODE_SETTING = "sidebar.mode";

let mode = $state.raw<SidebarMode>("default");
let desiredMode: SidebarMode = "default";
let expandedWidth = $state.raw(240);
let drag: { mode: SidebarMode; desired: SidebarMode; expanded: number; pending: number } | null =
  null;
let resizeSettlement = $state(0);
// One document-local ordering counter, seeded above earlier document owners.
let resizeRevision = $state(Math.trunc(performance.timeOrigin * 1024));
export const sidebarResizeRevision = () => resizeRevision;
export function claimSidebarResizeOwner() {
  return ++resizeRevision;
}
export const sidebarResizeActive = () => drag !== null;
export const sidebarResizeSettlement = () => resizeSettlement;

export const sidebarMode = () => mode;
export const isCompact = () => mode === "compact";
export const expanded = () => expandedWidth;
/** The chrome gap between the rail and an open utility panel. */
export const PANEL_GAP = 8;
let panelExtent = $state.raw(0);

export const effectiveWidth = () =>
  panelExtent > 0
    ? COMPACT_WIDTH + PANEL_GAP + panelExtent
    : mode === "compact"
      ? COMPACT_WIDTH
      : expandedWidth;

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
    return { mode: "compact", expanded: expandedWidth };
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

/** The non-native capture fallback; an expanded column follows the drag. */
export function beginSidebarResize() {
  if (drag) return;
  drag = { mode, desired: desiredMode, expanded: expandedWidth, pending: effectiveWidth() };
}

export function finishSidebarResize(value: number) {
  if (!drag) return;
  drag = null;
  const before = mode;
  adoptResizeWidth(value);
  // Only a change of shape travels; the column animates its width then too.
  void commands.sidebarSetWidth(effectiveWidth(), mode !== before, resizeRevision);
}

/**
 * An expanded column follows the drag as it goes. A width that would snap to
 * the rail is left to release, which changes the shape.
 */
export function previewDragWidth(value: number) {
  if (!drag || drag.mode !== "default" || mode !== "default") return;
  const next = clampExpanded(value);
  if (next === expandedWidth) return;
  expandedWidth = next;
  void commands.sidebarSetWidth(effectiveWidth(), false, resizeRevision);
}

export function cancelSidebarResize() {
  const started = drag;
  drag = null;
  // A drag the column followed gives back the width it started from.
  if (started && mode === started.mode && expandedWidth !== started.expanded) {
    expandedWidth = started.expanded;
    void commands.sidebarSetWidth(effectiveWidth(), false, resizeRevision);
  }
}

/** Native has admitted one final width; adopt its display shape without another layout command. */
export function adoptResizeWidth(value: number, revision = resizeRevision) {
  if (revision !== resizeRevision) return;
  if (!Number.isFinite(value)) return;
  // A fast drag overshoots the bounds; it lands on the nearest one.
  const next = resolveDragWidth(Math.max(COMPACT_WIDTH, Math.min(MAX_EXPANDED_WIDTH, value)));
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
  window.addEventListener("zephium:sidebar-hide-selected", (event) => {
    if (!(event instanceof CustomEvent) || !event.detail || typeof event.detail !== "object")
      return;
    const { width, revision } = event.detail;
    if (typeof width === "number" && typeof revision === "number") hideFromDrag(width, revision);
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

/** A stored preference reached this column: from here, or from elsewhere. */
export function adoptMode(next: SidebarMode) {
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
  if (mode !== "compact") return () => {};
  mode = "default";
  publish(true);
  return () => {
    if (mode !== "default" || desiredMode !== "compact") return;
    cancelSidebarResize();
    mode = "compact";
    publish(true);
  };
}

export function toggleMode() {
  cancelSidebarResize();
  desiredMode = desiredMode === "compact" ? "default" : "compact";
  setMode(desiredMode);
}

const HIDDEN_SETTING = "sidebar.hidden";

// Hidden, the page takes the column's place and the column floats over it
// while called up. Only macOS can raise the chrome over a page.
let hiddenChosen = $state.raw(false);
// Settings, tools and the rail pages need the column beside them.
let hiddenHeld = $state.raw(false);
let peeking = $state.raw(false);
let publishedHidden = false;
let peekSeq = 0;
let raising = false;
let savingHidden: { hidden: boolean; timer: ReturnType<typeof setTimeout> } | null = null;

export const isHidden = () => IS_MAC && hiddenChosen && !hiddenHeld;
export const isPeeking = () => peeking;

function publishHidden() {
  const next = isHidden();
  if (next === publishedHidden) return;
  publishedHidden = next;
  if (!next) {
    peekSeq++;
    peeking = false;
  }
  void commands.sidebarSetHidden(next).catch(() => {});
}

export function toggleHidden() {
  setHidden(!hiddenChosen);
}

function setHidden(next: boolean) {
  if (!IS_MAC || next === hiddenChosen) return;
  hiddenChosen = next;
  if (savingHidden) clearTimeout(savingHidden.timer);
  savingHidden = { hidden: hiddenChosen, timer: setTimeout(() => (savingHidden = null), 2000) };
  void commands.settingSet(HIDDEN_SETTING, String(hiddenChosen));
  publishHidden();
}

/**
 * A drag released past the column's narrowest hides it, as in Arc. Called up
 * again, it floats at `width`, the width it had before the drag.
 */
export function hideFromDrag(width: number, revision = resizeRevision) {
  // A column already floating over the page stops at its narrowest instead.
  if (!IS_MAC || revision !== resizeRevision || isHidden()) return;
  drag = null;
  setHidden(true);
  if (!Number.isFinite(width) || mode !== "default") return;
  const next = clampExpanded(width);
  if (next === expandedWidth) return;
  expandedWidth = next;
  resizeSettlement++;
  void commands.sidebarSetWidth(effectiveWidth(), false, resizeRevision);
}

/** A stored preference reached this column; see `adoptMode`. */
export function adoptHidden(next: boolean) {
  if (savingHidden) {
    if (next === savingHidden.hidden) {
      clearTimeout(savingHidden.timer);
      savingHidden = null;
    }
    return;
  }
  hiddenChosen = next;
  publishHidden();
}

export function holdHidden(held: boolean) {
  hiddenHeld = held;
  publishHidden();
}

/**
 * Floats the hidden column over the page. It is raised first and then slides
 * in, so the whole of the slide is seen rather than its start under the page.
 * Resolves whether the pointer is on the column's side once it is up, or
 * `null` when it did not come up: a pointer that left while it was rising has
 * no departure left to announce.
 */
export async function peek(): Promise<boolean | null> {
  if (!isHidden() || peeking || raising) return null;
  const seq = ++peekSeq;
  raising = true;
  const onSide = await commands.sidebarOverlay(true, effectiveWidth()).catch(() => null);
  raising = false;
  if (onSide === null || seq !== peekSeq || !isHidden()) return null;
  peeking = true;
  return onSide;
}

/** Sends the column away, then returns it beneath the page once it has gone. */
export function unpeek(settle: number) {
  if (!peeking) return;
  const seq = ++peekSeq;
  peeking = false;
  setTimeout(() => {
    if (seq === peekSeq) void commands.sidebarOverlay(false, effectiveWidth()).catch(() => {});
  }, settle);
}

/**
 * Adopts the persisted mode. The width is published afterwards so native
 * layout matches the restored shape on the first paint rather than after the
 * first interaction.
 */
export async function init(): Promise<void> {
  installResizeListener();
  // A missing or unreadable preference is not a startup failure; the
  // default shape is always valid.
  const [stored, hidden] = await Promise.all([
    commands.settingGet(MODE_SETTING).catch(() => null),
    commands.settingGet(HIDDEN_SETTING).catch(() => null),
  ]);
  if (stored === "compact") {
    mode = "compact";
    desiredMode = "compact";
  }
  hiddenChosen = hidden === "true";
  publish();
  publishHidden();
}
