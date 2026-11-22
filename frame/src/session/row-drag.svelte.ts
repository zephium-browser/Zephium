import { tabs } from "$domain/tabs";
import * as tabDrag from "./tab-drag.svelte";
import { haptic } from "$shared/lib/haptic";
import { Reorder } from "./reorder";
import { INDENT, treePlace, treeRows, type TreeBlock, type TreePlace } from "./tree-drop";

/** Where a drag was let go, for the list to decide what that means. */
type Drop = {
  id: string;
  x: number;
  y: number;
  /** The element under the pointer, resolved before anything moved. */
  over: Element | null;
  /** The tab under the pointer, which a placement goes before. */
  before: string | null;
  /** Whether native may read the point as the page: never over a sidebar
   *  that floats above it. */
  page: boolean;
};

type Options = {
  /** The list whose direct rows reorder in place. */
  list: () => HTMLElement | undefined;
  /** Whether this list reorders in place, and whether its rows are kept. */
  reorder: () => { essential: boolean } | null;
  /** The region the list scrolls in, whose room past the last row is the end of it. */
  region?: () => HTMLElement | undefined;
  /** A drop anywhere but back into the list's own order. */
  drop: (drop: Drop) => void;
  /** A list of nested rows: a drop among them lands at a place in the tree,
   *  which the list commits; it answers whether native took it. */
  tree?: {
    block: (id: string) => (TreeBlock & { place: TreePlace }) | null;
    commit: (id: string, place: TreePlace) => Promise<boolean>;
  };
};

const samePlace = (a: TreePlace, b: TreePlace) => a.parent === b.parent && a.before === b.before;

/** The tab a drop goes before: the one under the pointer, or the one after it
 *  when the pointer is past its middle, so the end of a row or a list can be
 *  chosen by dropping on the last tab's far half. */
function placeBefore(over: Element | null, x: number, y: number, dragged: string): string | null {
  const row = over?.closest<HTMLElement>("[data-zephium-tab-id]");
  if (!row) return null;
  const rect = row.getBoundingClientRect();
  const sideways = !!over?.closest("[data-essentials-drop]");
  const after = sideways ? x > rect.left + rect.width / 2 : y > rect.top + rect.height / 2;
  if (!after && row.dataset.zephiumTabId !== dragged) return row.dataset.zephiumTabId ?? null;
  const rows = [
    ...(row.closest("ul")?.querySelectorAll<HTMLElement>("[data-zephium-tab-id]") ?? []),
  ];
  const next = rows
    .slice(rows.indexOf(row) + 1)
    .find((candidate) => candidate.dataset.zephiumTabId !== dragged);
  return next?.dataset.zephiumTabId ?? null;
}

/**
 * One drag gesture for every list of tabs: the sidebar's rows, the rail, the
 * kept sites. A press becomes a drag only past a small movement, so a click
 * stays a click; the pointer is captured and followed once per frame by a
 * card of the tab, whatever the list is. Over the list's own rows the row
 * itself waits in the slot a drop would take (see reorder.ts). Letting go
 * inside the list reorders it; letting go anywhere else is the list's to
 * interpret.
 */
export class RowDrag {
  /** The card that follows the pointer, wherever it is and whatever the list does with the row. */
  ghost = $state<{ id: string; x: number; y: number } | null>(null);

  #options: Options;
  #down: { x: number; y: number } | null = null;
  #id = "";
  #pointerId: number | null = null;
  #captured: HTMLElement | null = null;
  #listening = false;
  #dragging = false;
  #pointer = { x: 0, y: 0 };
  #frame = 0;
  #swallow = false;
  #reorder: Reorder | null = null;
  // A drop already committed, held where it was drawn until the reordered
  // list arrives.
  #landing: Reorder | null = null;
  #essential = false;
  #bounds: DOMRect | null = null;
  #floating = false;
  #native = false;
  #slot = -1;
  #page = false;
  #folder = false;
  #startX = 0;
  /** Which way is deeper: +1 left to right, -1 in a right-to-left list. */
  #inward = 1;
  #block: (TreeBlock & { place: TreePlace }) | null = null;
  #place: TreePlace | null = null;
  #into: HTMLElement | null = null;

  constructor(options: Options) {
    this.#options = options;
  }

  #onLostCapture = (event: PointerEvent) => {
    if (this.#pointerId !== null && event.pointerId !== this.#pointerId) return;
    this.abandon();
  };

  #onWindowBlur = () => {
    this.abandon();
  };

  #onWindowPointerEnd = (event: PointerEvent) => {
    if (this.#pointerId !== null && event.pointerId !== this.#pointerId) return;
    this.abandon();
  };

  #onContextMenu = () => {
    if (this.#dragging) this.abandon();
  };

  #onKeyDown = (event: KeyboardEvent) => {
    if (!this.#dragging || event.key !== "Escape") return;
    event.preventDefault();
    event.stopPropagation();
    this.#swallow = true;
    this.abandon();
  };

  #attachWindowListeners() {
    if (this.#listening || typeof window === "undefined") return;
    this.#listening = true;
    window.addEventListener("blur", this.#onWindowBlur);
    window.addEventListener("pointerup", this.#onWindowPointerEnd);
    window.addEventListener("pointercancel", this.#onWindowPointerEnd);
    window.addEventListener("contextmenu", this.#onContextMenu, true);
    window.addEventListener("keydown", this.#onKeyDown, true);
  }

  #detachWindowListeners() {
    if (!this.#listening || typeof window === "undefined") return;
    this.#listening = false;
    window.removeEventListener("blur", this.#onWindowBlur);
    window.removeEventListener("pointerup", this.#onWindowPointerEnd);
    window.removeEventListener("pointercancel", this.#onWindowPointerEnd);
    window.removeEventListener("contextmenu", this.#onContextMenu, true);
    window.removeEventListener("keydown", this.#onKeyDown, true);
  }

  /** A press on a row; `folder` when the row is a folder rather than a tab. */
  down(event: PointerEvent, id: string, folder = false) {
    if (event.button !== 0) return;
    this.#swallow = false;
    if (this.#dragging || this.#down !== null) this.abandon();
    this.#down = { x: event.clientX, y: event.clientY };
    this.#id = id;
    this.#folder = folder;
    this.#pointerId = event.pointerId;
    this.#dragging = false;
    this.#attachWindowListeners();
  }

  move(event: PointerEvent) {
    const start = this.#down;
    if (start === null) return;
    if (this.#pointerId !== null && event.pointerId !== this.#pointerId) return;
    if (event.isTrusted && event.pointerType === "mouse" && (event.buttons & 1) === 0) {
      this.abandon();
      return;
    }
    if (
      !this.#dragging &&
      Math.abs(event.clientX - start.x) + Math.abs(event.clientY - start.y) > 4
    ) {
      const target = event.currentTarget;
      if (target instanceof HTMLElement) {
        try {
          target.setPointerCapture(event.pointerId);
          this.#captured = target;
          target.addEventListener("lostpointercapture", this.#onLostCapture);
        } catch {
          // Synthetic pointer events in tests may not map to an active pointer.
        }
      }
      this.#dragging = true;
      if (!this.#folder) tabDrag.begin(this.#id);
      document.body.style.cursor = "grabbing";
      const sidebar = target instanceof Element ? target.closest(".browser-sidebar") : null;
      this.#bounds = sidebar?.getBoundingClientRect() ?? null;
      this.#floating = sidebar?.hasAttribute("data-hidden") ?? false;
      this.#native = false;
      this.#slot = -1;
      this.#page = false;
      const reorder = this.#options.reorder();
      this.#essential = reorder?.essential ?? false;
      this.#startX = event.clientX;
      const list = this.#options.list();
      this.#inward = list && getComputedStyle(list).direction === "rtl" ? -1 : 1;
      this.#block = this.#options.tree?.block(this.#id) ?? null;
      this.#place = null;
      this.#reorder = reorder
        ? Reorder.begin(
            this.#options.list(),
            `${this.#folder ? "folder" : "tab"}:${this.#id}`,
            event.clientY,
            this.#options.region?.(),
          )
        : null;
    }
    if (!this.#dragging) return;

    this.#pointer = { x: event.clientX, y: event.clientY };
    if (this.#frame !== 0) return;
    this.#frame = requestAnimationFrame(() => {
      this.#frame = 0;
      const { x, y } = this.#pointer;
      const inPlace = this.#reorder?.update(x, y) ?? false;
      this.#nest(inPlace, x);
      this.#feel(inPlace, x);
      this.ghost = { id: this.#id, x, y };
      if (this.#folder) return;
      tabDrag.hover(x, y);
      if (this.#onPage(x, y)) {
        this.#native = true;
        tabs.dragOver(x, y);
      } else if (this.#native) {
        this.#native = false;
        tabs.dragOver(null, null);
      }
    });
  }

  up(event: PointerEvent) {
    const over = document.elementFromPoint(event.clientX, event.clientY);
    const before = placeBefore(over, event.clientX, event.clientY, this.#id);
    const dragged = this.#dragging;
    const id = this.#id;
    const reorder = this.#reorder;
    const essential = this.#essential;
    const folder = this.#folder;
    const tree = this.#options.tree;
    const block = this.#block;
    if (reorder && this.#frame !== 0) {
      const inPlace = reorder.update(event.clientX, event.clientY);
      this.#nest(inPlace, event.clientX);
    }
    const place = this.#place;
    this.#release(event);
    this.#reset();
    if (!dragged) return;
    this.#swallow = true;

    // Among nested rows the drop lands at a place in the tree; elsewhere it
    // goes before the tab at its slot.
    let commit: (() => Promise<boolean>) | null = null;
    if (reorder?.inside && tree && block) {
      if (place && !samePlace(place, block.place)) commit = () => tree.commit(id, place);
    } else {
      const placed = reorder?.target();
      if (placed?.changed)
        commit = () => tabDrag.move(id, essential, placed.before).then(() => !tabDrag.moveFailed());
    }
    if (reorder && commit) {
      this.#landing?.clear();
      this.#landing = reorder;
      void commit().then((taken) => {
        if (this.#landing !== reorder) return;
        // Refused: slide home at once. Accepted: the reordered list is on
        // its way and lands the rows itself; if it never comes, slide home.
        const home = () => {
          if (this.#landing !== reorder) return;
          this.#landing = null;
          reorder.release();
        };
        if (!taken) home();
        else setTimeout(home, 600);
      });
      return;
    }
    reorder?.release();
    if (folder) return;
    this.#options.drop({
      id,
      x: event.clientX,
      y: event.clientY,
      over,
      before,
      page: this.#onPage(event.clientX, event.clientY),
    });
  }

  #onPage(x: number, y: number) {
    const bounds = this.#bounds;
    if (!this.#floating || bounds === null) return true;
    return x < bounds.left || x > bounds.right || y < bounds.top || y > bounds.bottom;
  }

  cancel(event: PointerEvent) {
    this.#release(event);
    this.#reorder?.release();
    this.#reset();
  }

  /** Abandons any drag in progress, as a context menu opening does. */
  abandon() {
    this.#reorder?.release();
    this.#reset();
  }

  /** Whether a click is the end of a drag, and so not a click at all. */
  swallowClick(): boolean {
    const swallow = this.#swallow;
    this.#swallow = false;
    return swallow;
  }

  /** The list has changed shape: a held drop has arrived. Call after the
   *  list's own motion has measured where every row was drawn. */
  landed() {
    this.#landing?.clear();
    this.#landing = null;
  }

  /** Among nested rows: how deep a drop here would go, shown by setting the
   *  row in by that many indents and lighting the folder it would go into. */
  #nest(inPlace: boolean, x: number) {
    const reorder = this.#reorder;
    const block = this.#block;
    if (!reorder || !block || !this.#options.tree) return;
    const gap = inPlace ? reorder.gap() : null;
    const inward = (x - this.#startX) * this.#inward;
    const place = gap ? treePlace(treeRows(gap.rows), gap.index, block, inward) : null;
    const level = place?.depth ?? block.depth;
    if (inPlace && this.#place !== null && place !== null && level !== this.#place.depth)
      haptic("align");
    this.#place = place;
    reorder.indent((level - block.depth) * INDENT * this.#inward);
    const into =
      place?.parent && gap
        ? (gap.rows.find((row) => row.dataset.nodeId === place.parent) ?? null)
        : null;
    if (into !== this.#into) {
      if (this.#into) delete this.#into.dataset.dropInto;
      if (into) into.dataset.dropInto = "";
      this.#into = into;
    }
  }

  /** A tick as the row snaps into a new slot, and a firmer one on reaching the page. */
  #feel(inPlace: boolean, x: number) {
    const slot = this.#reorder?.slot ?? -1;
    if (inPlace && slot !== this.#slot) {
      if (this.#slot !== -1) haptic("align");
      this.#slot = slot;
    } else if (!inPlace) this.#slot = -1;
    const page = !inPlace && this.#bounds !== null && x > this.#bounds.right;
    if (page && !this.#page) haptic("level");
    this.#page = page;
  }

  #releaseCapture(pointerId = this.#pointerId) {
    const target = this.#captured;
    if (!target) return;
    this.#captured = null;
    target.removeEventListener("lostpointercapture", this.#onLostCapture);
    if (pointerId !== null && target.hasPointerCapture(pointerId)) {
      try {
        target.releasePointerCapture(pointerId);
      } catch {
        // Ignore if capture was already released.
      }
    }
  }

  #release(event: PointerEvent) {
    this.#releaseCapture(event.pointerId);
    const target = event.currentTarget;
    if (target instanceof HTMLElement && target.hasPointerCapture(event.pointerId)) {
      try {
        target.releasePointerCapture(event.pointerId);
      } catch {
        // Ignore if capture was already released.
      }
    }
  }

  #reset() {
    const wasDragging = this.#dragging;
    this.#detachWindowListeners();
    this.#releaseCapture();
    if (this.#frame !== 0) {
      cancelAnimationFrame(this.#frame);
      this.#frame = 0;
    }
    this.#down = null;
    this.#id = "";
    this.#pointerId = null;
    this.#dragging = false;
    this.#reorder = null;
    this.#block = null;
    this.#place = null;
    if (this.#into) delete this.#into.dataset.dropInto;
    this.#into = null;
    this.ghost = null;
    if (wasDragging) {
      document.body.style.cursor = "";
      if (!this.#folder) tabDrag.end();
    }
    this.#folder = false;
  }
}
