const QUIET_MS = 250;
/** A moving canvas keeps its layers this long after its last move, across a pause in a gesture. */
const HELD_MS = 1000;

/**
 * Temporarily requests transform layers while the canvas moves. Wheel and
 * dragging input engage a hold, renewed during motion; visibility loss or
 * the hold timeout clears `will-change` and `data-held` on the viewport.
 * The canvas styles use `data-held` for its cards too.
 *
 * After a quiet interval, release waits for running or pending subtree
 * animations to settle. It then inserts a zero-sized probe, forces layout
 * and removes the probe to encourage release of an idle backing layer.
 * Layer allocation, reclamation and rendering cost depend on the native
 * engine and require measurement; the layout change does not guarantee them.
 */
export function layerRelease(root: HTMLElement) {
  let timer: ReturnType<typeof setTimeout> | undefined;
  let held: ReturnType<typeof setTimeout> | undefined;
  let holding = false;
  let gone = false;
  const viewportOf = () => root.querySelector<HTMLElement>(".svelte-flow__viewport");
  const watched = new WeakSet<Animation>();
  const release = () => {
    timer = undefined;
    const viewport = viewportOf();
    if (gone || holding || !viewport) return;
    const moving = viewport
      .getAnimations({ subtree: true })
      .filter((animation) => animation.playState === "running" || animation.pending);
    if (moving.length) {
      // Asked again as each one ends: never while something still moves, never on a timer.
      for (const animation of moving) {
        if (watched.has(animation)) continue;
        watched.add(animation);
        void animation.finished.then(moved, moved);
      }
      return;
    }
    const probe = document.createElement("div");
    probe.setAttribute("aria-hidden", "true");
    probe.style.position = "absolute";
    probe.style.inlineSize = "0";
    probe.style.blockSize = "0";
    viewport.append(probe);
    void viewport.offsetHeight;
    probe.remove();
  };
  /** Something on the canvas started or stopped moving: once all is still, the layer goes. */
  const moved = () => {
    if (gone) return;
    clearTimeout(timer);
    timer = setTimeout(release, QUIET_MS);
  };
  const letGo = () => {
    clearTimeout(held);
    held = undefined;
    if (!holding) return;
    holding = false;
    const viewport = viewportOf();
    if (viewport) {
      viewport.style.willChange = "";
      delete viewport.dataset.held;
    }
    moved();
  };
  /** The canvas moves: its layers are held, and the hold is renewed. */
  const engage = () => {
    if (gone) return;
    if (!holding) {
      const viewport = viewportOf();
      if (!viewport) return;
      holding = true;
      viewport.style.willChange = "transform";
      viewport.dataset.held = "";
    }
    clearTimeout(held);
    held = setTimeout(letGo, HELD_MS);
  };
  const hidden = () => {
    if (document.visibilityState === "hidden") letGo();
  };
  const events = ["transitionend", "transitioncancel", "animationend", "animationcancel"];
  for (const name of events) root.addEventListener(name, moved, true);
  // A wheel step arrives before the move it causes, so the layers exist for its first frame.
  const dragging = (event: PointerEvent) => {
    if (event.buttons) engage();
  };
  root.addEventListener("wheel", engage, { passive: true });
  root.addEventListener("pointermove", dragging, { passive: true });
  document.addEventListener("visibilitychange", hidden);
  return {
    moved,
    engage,
    destroy() {
      gone = true;
      clearTimeout(timer);
      clearTimeout(held);
      for (const name of events) root.removeEventListener(name, moved, true);
      root.removeEventListener("wheel", engage);
      root.removeEventListener("pointermove", dragging);
      document.removeEventListener("visibilitychange", hidden);
    },
  };
}
