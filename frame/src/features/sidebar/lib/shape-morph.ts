import { duration, easing, reducedMotion } from "$shared/lib/motion";

/**
 * The column changing shape — the full list becoming the rail, or the rail
 * opening back into the list — as one continuous change rather than one
 * column vanishing and another appearing. Everything the two shapes have in
 * common is a site's mark: each one travels from where it was drawn to where
 * it is drawn now, on the same curve native slides the page with, so the
 * page and the column move as one surface. What only the old shape had fades
 * where it stands while the page slides over it; what only the new shape has
 * fades in around the marks.
 *
 * Every step is a transform or an opacity, carried by the compositor; the
 * only measuring is once before the change and once after it.
 */

const KEY = "data-motion-key";
/** Slow out of the rail and settling late, so a mark stays behind the edge
 *  of a page sliding away on the emphasized curve. */
const TRAILING = "cubic-bezier(0.55, 0, 0.25, 1)";
const COUNTERPARTS = `[${KEY}], [data-morph]`;

export type Snapshot = {
  column: HTMLElement;
  host: HTMLElement;
  rect: DOMRect;
  marks: Map<string, DOMRect>;
  /** Scroll offsets, which an element loses when it leaves the document. */
  scrolled: [HTMLElement, number][];
};

function keyOf(element: HTMLElement): string | null {
  return element.getAttribute(KEY) ?? element.dataset.morph ?? null;
}

/** The part of a counterpart the eye follows: its site's mark, if it has one. */
function markOf(element: HTMLElement): HTMLElement {
  return element.querySelector<HTMLElement>("[data-favicon]") ?? element;
}

/** On screen, not merely in the document: a row scrolled out of view has
 *  nothing to be followed from. */
function shownRect(element: HTMLElement): DOMRect | null {
  if (element.getClientRects().length === 0) return null;
  const rect = element.getBoundingClientRect();
  if (rect.bottom < 0 || rect.top > window.innerHeight) return null;
  return rect;
}

function columnOf(host: HTMLElement): HTMLElement | null {
  return host.querySelector<HTMLElement>(":scope > .sidebar-browser-column");
}

/** Before the change: where every mark in the column is drawn. */
export function capture(host: HTMLElement | undefined): Snapshot | null {
  if (!host || reducedMotion()) return null;
  const column = columnOf(host);
  if (!column) return null;
  const marks = new Map<string, DOMRect>();
  for (const element of column.querySelectorAll<HTMLElement>(COUNTERPARTS)) {
    const key = keyOf(element);
    const rect = shownRect(markOf(element));
    if (key && rect) marks.set(key, rect);
  }
  const scrolled = [...column.querySelectorAll<HTMLElement>("[data-glide-scroller]")].map(
    (scroller): [HTMLElement, number] => [scroller, scroller.scrollTop],
  );
  return { column, host, rect: column.getBoundingClientRect(), marks, scrolled };
}

/** The old column, back for a moment where it was: unreachable, anonymous,
 *  its marks withdrawn because they are travelling, and fading. */
function release(snapshot: Snapshot) {
  const { column, host, rect } = snapshot;
  for (const node of [column, ...column.querySelectorAll<HTMLElement>("*")]) {
    for (const name of node.getAttributeNames()) {
      if (name.startsWith("data-zephium-") || name === KEY || name === "id") {
        node.removeAttribute(name);
      }
    }
  }
  for (const mark of column.querySelectorAll<HTMLElement>("[data-favicon]")) {
    mark.style.visibility = "hidden";
  }
  for (const plate of column.querySelectorAll<HTMLElement>("[data-plate]")) {
    plate.dataset.gliding = "";
  }
  column.setAttribute("aria-hidden", "true");
  column.inert = true;
  const bounds = host.getBoundingClientRect();
  Object.assign(column.style, {
    position: "absolute",
    left: `${rect.left - bounds.left}px`,
    top: `${rect.top - bounds.top}px`,
    width: `${rect.width}px`,
    height: `${rect.height}px`,
    pointerEvents: "none",
  });
  host.append(column);
  for (const [scroller, top] of snapshot.scrolled) scroller.scrollTop = top;
  const fade = column.animate([{ opacity: 1 }, { opacity: 0 }], {
    duration: duration("base"),
    easing: easing("exit"),
    fill: "forwards",
  });
  void fade.finished.catch(() => undefined).then(() => column.remove());
}

/** Fades in whatever in `root` has no counterpart, at the highest level at
 *  which it has none, so each travelling mark stays fully drawn throughout. */
function reveal(root: HTMLElement, travelling: Set<HTMLElement>) {
  for (const child of root.children) {
    if (!(child instanceof HTMLElement) || travelling.has(child)) continue;
    let holds = false;
    for (const element of travelling) {
      if (child.contains(element)) {
        holds = true;
        break;
      }
    }
    if (holds) {
      reveal(child, travelling);
      continue;
    }
    child.animate([{ opacity: 0 }, { opacity: 1 }], {
      duration: duration("slow"),
      delay: duration("fast") * 0.6,
      easing: easing("out"),
      fill: "backwards",
    });
  }
}

/** Starts a paused flight once the view is wide enough to show where it
 *  lands, or after a short while regardless. */
function holdUntilRoom(flight: Animation, right: number) {
  flight.pause();
  let timer = 0;
  const go = () => {
    if (window.innerWidth < right && timer !== 0) return;
    window.removeEventListener("resize", go);
    clearTimeout(timer);
    if (flight.playState === "paused") flight.play();
  };
  window.addEventListener("resize", go);
  timer = window.setTimeout(() => {
    timer = 0;
    go();
  }, 250);
}

/** After the change: carry every mark from the old shape into the new one. */
export function play(snapshot: Snapshot | null) {
  if (!snapshot) return;
  const incoming = columnOf(snapshot.host);
  if (!incoming || incoming === snapshot.column || snapshot.column.isConnected) return;
  release(snapshot);

  const travelling = new Set<HTMLElement>();
  const time = duration("page");
  // Widening, the page slides off the column's new width as the marks travel
  // into it, and a native page is drawn above the column: a mark bound for
  // somewhere the page still covers, flying on the page's own curve, would
  // pass underneath it and be seen only once it had landed. Those travel on a
  // gentler curve that keeps them behind the page's edge all the way.
  const right =
    snapshot.host.closest("[data-sidebar-position]")?.getAttribute("data-sidebar-position") ===
    "right";
  const edgeFrom = right ? snapshot.rect.left : snapshot.rect.right;
  const widening = incoming.getBoundingClientRect().width - snapshot.rect.width > 1;
  for (const element of incoming.querySelectorAll<HTMLElement>(COUNTERPARTS)) {
    const key = keyOf(element);
    const from = key ? snapshot.marks.get(key) : undefined;
    const to = from ? shownRect(markOf(element)) : null;
    if (!from || !to) continue;
    const center = to.left + to.width / 2;
    const trailing = widening && (right ? center < edgeFrom : center > edgeFrom);
    const dx = from.left + from.width / 2 - (to.left + to.width / 2);
    const dy = from.top + from.height / 2 - (to.top + to.height / 2);
    travelling.add(element);
    if (Math.abs(dx) < 0.5 && Math.abs(dy) < 0.5) continue;
    const flight = element.animate(
      [{ transform: `translate(${dx}px, ${dy}px)` }, { transform: "none" }],
      {
        duration: trailing ? time * 1.15 : time,
        easing: trailing ? TRAILING : easing("emphasized"),
        fill: "backwards",
      },
    );
    // The view itself is widened by native a moment after the shape changes;
    // until it is, the destination is outside it and the flight would be
    // drawn nowhere. It waits, still where it was, until there is room.
    if (trailing && (right ? to.left < 0 : to.right > window.innerWidth))
      holdUntilRoom(flight, right ? window.innerWidth - to.left : to.right);
    // A row's name is new to the list shape; it arrives behind its mark.
    element.querySelector<HTMLElement>(".tab-label")?.animate([{ opacity: 0 }, { opacity: 1 }], {
      duration: duration("slow"),
      delay: duration("fast"),
      easing: easing("out"),
      fill: "backwards",
    });
  }
  reveal(incoming, travelling);
}
