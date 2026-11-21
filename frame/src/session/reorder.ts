import { duration, easing } from "$shared/lib/motion";

/**
 * Reordering a list in place. The rows it passes slide aside to open a slot
 * for the row being dragged, which waits in that slot as a placeholder while
 * a card of it follows the pointer; all by transform, so the list never lays
 * itself out while a finger is down. Nothing is committed
 * until the drop; then the transforms are held until the reordered list
 * arrives and hands its rows to the list's own motion, which lands each one
 * from wherever it is drawn.
 */

const KEY = "data-motion-key";

type Slot = { element: HTMLElement; rect: DOMRect };

export class Reorder {
  #slots: Slot[];
  #from: number;
  #span: number;
  #index: number;
  #startY: number;
  #step: number;
  #reach: number;
  #inside = false;
  #shiftX = 0;

  private constructor(
    slots: Slot[],
    from: number,
    span: number,
    startY: number,
    step: number,
    reach: number,
  ) {
    this.#slots = slots;
    this.#from = from;
    this.#span = span;
    this.#index = from;
    this.#startY = startY;
    this.#step = step;
    this.#reach = reach;
  }

  /** Starts a reorder of the direct row `key` in `list`, or declines when
   *  the row is not one the list can reorder by itself. A row with a depth
   *  carries the deeper rows after it, as a folder carries what it holds. */
  static begin(
    list: HTMLElement | undefined,
    key: string,
    pointerY: number,
    region?: HTMLElement,
  ): Reorder | null {
    if (!list) return null;
    const slots = [...list.querySelectorAll<HTMLElement>(`:scope > [${KEY}]`)]
      .filter((element) => element.getClientRects().length > 0)
      .map((element) => ({ element, rect: element.getBoundingClientRect() }));
    const from = slots.findIndex((slot) => slot.element.getAttribute(KEY) === key);
    if (from < 0) return null;
    const depth = Number(slots[from]!.element.dataset.depth ?? NaN);
    let span = 1;
    if (!Number.isNaN(depth))
      while (
        from + span < slots.length &&
        Number(slots[from + span]!.element.dataset.depth ?? 0) > depth
      )
        span += 1;
    if (slots.length - span < 1) return null;
    const next = slots[from + 1] ?? slots[from - 1]!;
    const step =
      from + 1 < slots.length
        ? next.rect.top - slots[from]!.rect.top
        : slots[from]!.rect.top - next.rect.top;
    const motion = `transform ${duration("base")}ms ${easing("emphasized")}`;
    // The dragged row steps from slot to slot with the rest; only its dimming
    // eases in.
    const lift = ["opacity"]
      .map((property) => `${property} ${duration("fast")}ms ${easing("out")}`)
      .join(", ");
    for (const [index, slot] of slots.entries()) {
      slot.element.style.transition =
        index >= from && index < from + span ? `${lift}, ${motion}` : motion;
    }
    for (let at = from; at < from + span; at++) slots[at]!.element.dataset.lifted = "";
    // Past the last row the list's own space still means the end of it.
    const reach = Math.max(slots.at(-1)!.rect.bottom, region?.getBoundingClientRect().bottom ?? 0);
    return new Reorder(slots, from, span, pointerY, Math.abs(step), reach);
  }

  #block() {
    return this.#slots.slice(this.#from, this.#from + this.#span);
  }

  #others() {
    return [...this.#slots.slice(0, this.#from), ...this.#slots.slice(this.#from + this.#span)];
  }

  /** Follows the pointer. Returns whether it is over the list, where a drop
   *  reorders; elsewhere the list rests and the drop means something else. */
  update(x: number, y: number): boolean {
    const first = this.#slots[0]!.rect;
    const last = this.#slots.at(-1)!.rect;
    const inside =
      x >= first.left - 12 &&
      x <= first.right + 12 &&
      y >= first.top - this.#step / 2 &&
      y <= Math.max(last.bottom + this.#step / 2, this.#reach);
    const block = this.#block();
    const head = block[0]!;
    if (!inside) {
      if (this.#inside) this.#rest();
      this.#inside = false;
      return false;
    }
    this.#inside = true;
    for (const { element } of block) element.dataset.lifted = "";

    // Where the row would land if let go decides the slot; the row itself
    // waits there.
    const others = this.#others();
    const travel = Math.max(
      Math.min(0, others[0]!.rect.top - head.rect.top),
      Math.min(Math.max(0, others.at(-1)!.rect.top - head.rect.top), y - this.#startY),
    );
    const centre = head.rect.top + head.rect.height / 2 + travel;
    let index = 0;
    // A tie counts as passed for the row it moves towards: travel stops with
    // the row's centre on the first or last row's, and both ends of the list
    // must still be reachable.
    for (const [at, slot] of others.entries()) {
      const middle = slot.rect.top + slot.rect.height / 2;
      if (at < this.#from ? middle < centre : middle <= centre) index += 1;
    }
    this.#index = index;
    this.#draw();
    return true;
  }

  /** Sets the block sideways, as deep as a drop there would nest it. */
  indent(px: number) {
    if (px === this.#shiftX) return;
    this.#shiftX = px;
    if (this.#inside) this.#draw();
  }

  #draw() {
    const index = this.#index;
    const dy = (index - this.#from) * this.#step;
    const dx = this.#shiftX;
    for (const { element } of this.#block())
      element.style.transform =
        dx !== 0 ? `translate(${dx}px, ${dy}px)` : dy !== 0 ? `translateY(${dy}px)` : "";
    const lift = this.#span * this.#step;
    for (const [at, slot] of this.#others().entries()) {
      const shift =
        at >= this.#from && at < index ? -lift : at < this.#from && at >= index ? lift : 0;
      slot.element.style.transform = shift === 0 ? "" : `translateY(${shift}px)`;
    }
  }

  /** The slot the dragged row would take. */
  get slot() {
    return this.#index;
  }

  /** Whether the pointer is over the list. */
  get inside() {
    return this.#inside;
  }

  /** The rows the dragged one passes, in order, and the gap it waits in among them. */
  gap(): { rows: HTMLElement[]; index: number } {
    return { rows: this.#others().map((slot) => slot.element), index: this.#index };
  }

  /** Where the drop lands: whether anything moved, and the tab it goes before. */
  target(): { changed: boolean; before: string | null } {
    if (!this.#inside || this.#index === this.#from) return { changed: false, before: null };
    // The first tab at or after the slot: a folder has no tab of its own to
    // stand before, so the drop lands before the next row that does.
    for (const { element } of this.#others().slice(this.#index)) {
      const tab = element.dataset.zephiumTabId
        ? element
        : element.querySelector<HTMLElement>("[data-zephium-tab-id]");
      if (tab?.dataset.zephiumTabId) return { changed: true, before: tab.dataset.zephiumTabId };
    }
    return { changed: true, before: null };
  }

  /** No reorder after all: everything slides back to where it was. */
  release() {
    for (const { element } of this.#block())
      element.style.transition = `transform ${duration("base")}ms ${easing("emphasized")}`;
    this.#rest();
    const done = () => this.clear();
    setTimeout(done, duration("base"));
  }

  /** The reordered list has arrived: drop the transforms without animating,
   *  after its motion has measured where every row was drawn. */
  clear() {
    for (const { element } of this.#slots) {
      element.style.transition = "none";
      element.style.transform = "";
      delete element.dataset.lifted;
    }
    requestAnimationFrame(() => {
      for (const { element } of this.#slots) element.style.transition = "";
    });
  }

  #rest() {
    for (const { element } of this.#slots) element.style.transform = "";
    for (const { element } of this.#block()) delete element.dataset.lifted;
  }
}
