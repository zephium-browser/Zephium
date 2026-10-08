import { duration, easing, reducedMotion } from "./motion";

/**
 * Motion for a list whose items are added, removed and reordered by someone
 * else — here, by native projections. A list keeps its items' identity in
 * `data-motion-key` on each direct child; `capture` runs before the DOM
 * changes and `play` after, and everything in between is inferred:
 *
 * - an item that moved slides from where it was (FLIP, transform only);
 * - an item that is new rises into its place;
 * - an item that left is put back for a moment as an inert ghost and fades,
 *   while the items after it close the gap;
 * - an item that left one list and arrived in another in the same update is
 *   the same thing moving, so its icon flies from the old place to the new.
 *
 * A ghost is stripped of every `data-zephium-*` attribute before it is shown:
 * native resolves tabs through those sentinels, and a departed tab must never
 * be found in the document again.
 */

const KEY = "data-motion-key";
/** Past this many simultaneous arrivals the list was replaced, not edited. */
const REPLACEMENT = 12;

export type ListMotionOptions = {
  /** How a new item arrives. Rows rise; tiles grow out of their own centre. */
  enter?: () => "rise" | "grow";
};

type Before = { element: HTMLElement; rect: DOMRect };
type Departure = { ghost: HTMLElement; icon: HTMLElement | null };
type Arrival = { element: HTMLElement; enter: "rise" | "grow" };

const running = new WeakMap<Element, Animation[]>();
const VIEW_MARGIN = 120;
const MAX_MOVES = 40;
let departures = new Map<string, Departure>();
let arrivals = new Map<string, Arrival>();
let scheduled = false;

function children(container: HTMLElement): HTMLElement[] {
  return [...container.querySelectorAll<HTMLElement>(`:scope > [${KEY}]`)];
}

function visibleRect(element: Element): DOMRect | null {
  return element.getClientRects().length > 0 ? element.getBoundingClientRect() : null;
}

function track(element: Element, animation: Animation) {
  const list = running.get(element) ?? [];
  list.push(animation);
  running.set(element, list);
  void animation.finished
    .catch(() => undefined)
    .then(() => {
      const current = running.get(element);
      if (!current) return;
      const next = current.filter((entry) => entry !== animation);
      if (next.length > 0) running.set(element, next);
      else running.delete(element);
    });
}

function settleRunning(element: Element) {
  for (const animation of running.get(element) ?? []) animation.cancel();
  running.delete(element);
}

/** Puts a departed element back where it was, unreachable and anonymous. */
function ghost(element: HTMLElement, rect: DOMRect, container: HTMLElement): HTMLElement {
  for (const node of [element, ...element.querySelectorAll<HTMLElement>("*")]) {
    for (const name of node.getAttributeNames()) {
      if (name.startsWith("data-zephium-") || name === "id") node.removeAttribute(name);
    }
  }
  element.removeAttribute(KEY);
  element.setAttribute("aria-hidden", "true");
  element.inert = true;
  if (getComputedStyle(container).position === "static") container.style.position = "relative";
  const host = container.getBoundingClientRect();
  Object.assign(element.style, {
    position: "absolute",
    insetInlineStart: "auto",
    left: `${rect.left - host.left - container.clientLeft + container.scrollLeft}px`,
    top: `${rect.top - host.top - container.clientTop + container.scrollTop}px`,
    width: `${rect.width}px`,
    height: `${rect.height}px`,
    margin: "0",
    pointerEvents: "none",
  });
  container.append(element);
  return element;
}

function leave(ghost: HTMLElement, quick = false) {
  const animation = ghost.animate(
    [
      { opacity: 1, transform: "none" },
      { opacity: 0, transform: "scale(0.96)" },
    ],
    { duration: duration(quick ? "fast" : "base"), easing: easing("exit"), fill: "forwards" },
  );
  void animation.finished.catch(() => undefined).then(() => ghost.remove());
}

function enter(arrival: Arrival) {
  const frames =
    arrival.enter === "grow"
      ? [
          { opacity: 0, transform: "scale(0.86)" },
          { opacity: 1, transform: "none" },
        ]
      : [
          { opacity: 0, transform: "translateY(-6px) scale(0.98)" },
          { opacity: 1, transform: "none" },
        ];
  track(
    arrival.element,
    arrival.element.animate(frames, {
      duration: duration("slow"),
      easing: easing("emphasized"),
      fill: "backwards",
    }),
  );
}

/** A copy of a mark that keeps its pixels: cloning a canvas copies its
 *  element, never its bitmap. */
function replica(icon: HTMLElement): HTMLElement {
  const copy = icon.cloneNode(true) as HTMLElement;
  const sources = icon.querySelectorAll("canvas");
  copy.querySelectorAll("canvas").forEach((canvas, index) => {
    const source = sources[index];
    if (source) canvas.getContext("2d")?.drawImage(source, 0, 0);
  });
  copy.style.width = "100%";
  copy.style.height = "100%";
  return copy;
}

/** The same site leaving one list for another: its mark travels, its ground
 *  gives way at one end and forms at the other. */
function fly(departure: Departure, arrival: Arrival) {
  leave(departure.ghost, true);
  const target = arrival.element.querySelector<HTMLElement>("[data-favicon]");
  const icon = departure.icon;
  const from = icon ? visibleRect(icon) : null;
  const to = target ? visibleRect(target) : null;
  enter(arrival);
  if (!icon || !target || !from || !to || from.width === 0) return;

  const flyer = document.createElement("div");
  flyer.setAttribute("aria-hidden", "true");
  Object.assign(flyer.style, {
    position: "fixed",
    left: `${from.left}px`,
    top: `${from.top}px`,
    width: `${from.width}px`,
    height: `${from.height}px`,
    zIndex: "100",
    pointerEvents: "none",
    transformOrigin: "top left",
  });
  const mark = replica(icon);
  // In flight the mark is the one thing moving, so it travels lit and still.
  mark.querySelector(".favicon-plate")?.classList.add("favicon-plate-lit");
  mark.dataset.loading = "false";
  flyer.append(mark);
  document.body.append(flyer);

  const time = duration("page");
  target.style.visibility = "hidden";
  const scale = to.width / from.width;
  const flight = flyer.animate(
    [
      { transform: "none" },
      { transform: `translate(${to.left - from.left}px, ${to.top - from.top}px) scale(${scale})` },
    ],
    { duration: time, easing: easing("spring"), fill: "forwards" },
  );
  void flight.finished
    .catch(() => undefined)
    .then(() => {
      target.style.visibility = "";
      flyer.remove();
    });
}

function settle() {
  scheduled = false;
  const leaving = departures;
  const coming = arrivals;
  departures = new Map();
  arrivals = new Map();
  const replaced = coming.size > REPLACEMENT;
  for (const [key, arrival] of coming) {
    const departure = leaving.get(key);
    if (departure) {
      leaving.delete(key);
      fly(departure, arrival);
    } else if (!replaced) {
      enter(arrival);
    }
  }
  for (const departure of leaving.values()) leave(departure.ghost);
}

export class ListMotion {
  #before: Map<string, Before> | null = null;
  #enter: () => "rise" | "grow";

  constructor(options: ListMotionOptions = {}) {
    this.#enter = options.enter ?? (() => "rise");
  }

  /** Before the DOM changes: where everything is now. */
  capture(container: HTMLElement | undefined) {
    if (
      !container ||
      reducedMotion() ||
      container.closest('[data-sidebar-resize-settling="true"]')
    ) {
      this.#before = null;
      return;
    }
    const before = new Map<string, Before>();
    for (const element of children(container)) {
      const rect = visibleRect(element);
      const key = element.getAttribute(KEY);
      if (rect && key) before.set(key, { element, rect });
    }
    this.#before = before;
  }

  /** After the DOM changes: move everything from where it was. */
  play(container: HTMLElement | undefined) {
    const before = this.#before;
    this.#before = null;
    if (!container || !before) return;

    const now = children(container);
    // Stop anything mid-flight first, so every measurement is of the resting
    // layout; the capture above already recorded where each one was drawn.
    for (const element of now) settleRunning(element);
    if (container.closest('[data-sidebar-resize-settling="true"]')) return;

    const present = new Set<string>();
    let shared = 0;
    const moves: [HTMLElement, number, number][] = [];
    const fresh: [string, HTMLElement][] = [];
    for (const element of now) {
      const key = element.getAttribute(KEY);
      const rect = visibleRect(element);
      if (!key || !rect) continue;
      present.add(key);
      const previous = before.get(key);
      if (!previous) {
        fresh.push([key, element]);
        continue;
      }
      shared += 1;
      const dx = previous.rect.left - rect.left;
      const dy = previous.rect.top - rect.top;
      if (Math.abs(dx) > 0.5 || Math.abs(dy) > 0.5) moves.push([element, dx, dy]);
    }

    // A list that shares nothing with what it was is a different list — a
    // profile or a space changed — and arrives without ceremony.
    if (before.size > 0 && shared === 0) return;

    // Only rows that are, or were, in view travel; each one animating gets
    // its own layer, and hundreds sliding unseen would cost frames for
    // nothing. A reshuffle that big settles in place.
    const view = (container.closest("[data-glide-scroller]") ?? container).getBoundingClientRect();
    const seen = (top: number, bottom: number) =>
      bottom >= view.top - VIEW_MARGIN && top <= view.bottom + VIEW_MARGIN;
    const visibleMoves = moves.filter(([element, , dy]) => {
      const rect = element.getBoundingClientRect();
      return seen(rect.top, rect.bottom) || seen(rect.top + dy, rect.bottom + dy);
    });
    for (const [element, dx, dy] of visibleMoves.length > MAX_MOVES ? [] : visibleMoves) {
      track(
        element,
        element.animate([{ transform: `translate(${dx}px, ${dy}px)` }, { transform: "none" }], {
          duration: duration("slow"),
          easing: easing("spring"),
        }),
      );
    }
    const style = this.#enter();
    for (const [key, element] of fresh) arrivals.set(key, { element, enter: style });
    for (const [key, { element, rect }] of before) {
      if (present.has(key) || element.isConnected) continue;
      const shade = ghost(element, rect, container);
      departures.set(key, { ghost: shade, icon: shade.querySelector("[data-favicon]") });
    }
    if (!scheduled && (arrivals.size > 0 || departures.size > 0)) {
      scheduled = true;
      queueMicrotask(settle);
    }
  }
}
