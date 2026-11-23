import { duration, easing, reducedMotion, type Duration } from "$shared/lib/motion";

/**
 * A node the run just placed rises 12 px into place as it fades in; under
 * reduced motion it only fades, quickly. Individual `translate` composes with
 * the flow's own transform, so the node keeps its place throughout.
 */
export function arrive(element: HTMLElement, motion: Duration): void {
  if (reducedMotion()) {
    element.animate([{ opacity: 0 }, { opacity: 1 }], {
      duration: duration("fast"),
      easing: easing("out"),
    });
    return;
  }
  element.animate(
    [
      { translate: "0 12px", opacity: 0 },
      { translate: "0 0", opacity: 1 },
    ],
    { duration: duration(motion), easing: easing("emphasized") },
  );
}

/**
 * The canvas's record of what arrived when: a node asks once, as it mounts,
 * and only an arrival this recent moves it. A node mounted again later, when
 * it scrolls back into view, finds its arrival long past and stays still.
 */
export function arrivals() {
  const known = new Set<string>();
  const arrived = new Map<string, { at: number; motion: Duration }>();
  let seeded = false;
  const WINDOW = 500;
  return {
    /** Registers what the scene holds; before the first scene is seen nothing arrives. */
    see(items: readonly { id: string }[]) {
      const now = performance.now();
      for (const [id, entry] of arrived) if (now - entry.at >= WINDOW) arrived.delete(id);
      for (const item of items) {
        if (known.has(item.id)) continue;
        known.add(item.id);
        if (seeded) arrived.set(item.id, { at: now, motion: "base" });
      }
      if (items.length) seeded = true;
    },
    /** How the node arrives, if it arrived just now. */
    motion(id: string): Duration | null {
      const entry = arrived.get(id);
      return entry && performance.now() - entry.at < WINDOW ? entry.motion : null;
    },
  };
}
