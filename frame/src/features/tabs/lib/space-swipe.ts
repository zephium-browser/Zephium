import { tabs } from "$domain/tabs";
import { haptic } from "$shared/lib/haptic";
import { reducedMotion } from "$shared/lib/motion";
import { attachNativeSwipe, nativeSwipeLive, type SwipeSink } from "./native-swipe";
import { SwipeTracker, type Reading, type SwipeStep } from "./swipe-track";
import { advance, atRest, launch, type Motion } from "./sheet-spring";

/** Without native, wheel events stop arriving this long after the fingers lift and momentum fades. */
const IDLE = 90;
/** A native swipe that goes this long without a word from the fingers has lost its lift. */
const LOST = 1500;
/** How long a committed sheet waits for its space to arrive before coming back. */
const ARRIVAL = 700;
/** The room between one space's sheet and the next. */
const SHEET_GAP = 12;
/** How far a space that something else switched to slides in. */
const SOFT = 56;
/** What a sheet gives up as it leaves the middle: light, and a little size. */
const DIM = 0.5;
const SHRINK = 0.03;

type Options = {
  canStep: (step: SwipeStep) => boolean;
  /** Resolves false when native refused the step. */
  step: (step: SwipeStep) => Promise<boolean> | void;
  /** The group of sheets that moves as one: the neighbours either side of the current list. */
  sheet: () => HTMLElement | undefined;
  /** Whether the neighbours need drawing: true from the first sideways travel until the sheet is home. */
  peek: (on: boolean) => void;
};

type Wheel = WheelEvent & { webkitDirectionInvertedFromDevice?: boolean };

type Control = { arrive: (side: SwipeStep) => void };

/** Native's answer to a step, as the list needs it: whether a space is coming. */
export async function stepSpace(step: SwipeStep): Promise<boolean> {
  const result = await tabs.stepSpace(step);
  return result.outcome === "applied" || result.outcome === "deferred";
}

const controls = new WeakMap<HTMLElement, Control>();

/**
 * A two-finger sideways swipe over the column moves to the neighbouring space.
 * The spaces are laid side by side as one sheet that follows the fingers one
 * to one: the neighbour is already there, arriving from the edge, and the
 * space being left steps back a little. Past the distance it takes over and
 * a spring carries it the rest of the way from the speed the fingers had; a
 * short one springs home. Everything that moves is a transform or an opacity,
 * written by one frame callback that only exists while something is moving.
 * Native reads the trackpad and says when the fingers land and lift; where it
 * cannot, the wheel is read instead. See SwipeTracker for what counts as a swipe.
 */
export function spaceSwipe(node: HTMLElement, options: Options) {
  let current = options;
  // Over the whole column, not only the list: the pointer is on the sidebar.
  const host = node.closest<HTMLElement>(".sidebar-browser-column") ?? node;
  const tracker = new SwipeTracker({
    extent: () => node.clientWidth || 240,
    canStep: (step) => current.canStep(step),
  });
  const motion: Motion = { x: 0, v: 0 };
  let mode: "rest" | "track" | "spring" = "rest";
  let target = 0;
  let pending = 0;
  let base = 0;
  let heard = 0;
  let last = 0;
  let frame = 0;
  /** The step this sheet has committed to and is still waiting for. */
  let held: { step: SwipeStep; animated: boolean } | null = null;
  let soft = false;
  let peeking = false;
  let idle: ReturnType<typeof setTimeout> | undefined;
  let lost: ReturnType<typeof setTimeout> | undefined;
  let waiting: ReturnType<typeof setTimeout> | undefined;

  const spread = () => (node.clientWidth || 240) + SHEET_GAP;
  const part = (sheet: HTMLElement, which: string) =>
    sheet.querySelector<HTMLElement>(`[data-sheet="${which}"]`);

  function depth(element: HTMLElement | null, away: number, span: number) {
    if (!element) return;
    const t = Math.min(1, Math.abs(away) / span);
    element.style.opacity = t < 0.002 ? "" : (1 - DIM * t).toFixed(3);
    element.style.scale = t < 0.002 ? "" : (1 - SHRINK * t).toFixed(4);
  }

  function paint() {
    const sheet = current.sheet();
    if (!sheet) return;
    const x = motion.x;
    sheet.style.transform = x === 0 ? "" : `translate3d(${x}px, 0, 0)`;
    if (soft) {
      depth(part(sheet, "current"), x, SOFT);
      return;
    }
    const reach = spread();
    depth(part(sheet, "prev"), x - reach, reach);
    depth(part(sheet, "current"), x, reach);
    depth(part(sheet, "next"), x + reach, reach);
  }

  function clear() {
    const sheet = current.sheet();
    if (!sheet) return;
    sheet.style.transform = "";
    sheet.style.willChange = "";
    for (const which of ["prev", "current", "next"]) {
      const element = part(sheet, which);
      if (element) {
        element.style.opacity = "";
        element.style.scale = "";
      }
    }
  }

  function home() {
    mode = "rest";
    soft = false;
    motion.x = 0;
    motion.v = 0;
    clear();
    if (peeking) {
      peeking = false;
      current.peek(false);
    }
  }

  function tick(now: number) {
    frame = 0;
    if (mode === "track") {
      motion.x = pending;
      paint();
      return;
    }
    if (mode !== "spring") return;
    advance(motion, target, now - last);
    last = now;
    if (atRest(motion, target)) {
      motion.x = target;
      motion.v = 0;
      paint();
      // A committed sheet stays where it has arrived until its space does.
      if (!held) home();
      return;
    }
    paint();
    frame = requestAnimationFrame(tick);
  }

  function run(to: number) {
    target = to;
    mode = "spring";
    last = performance.now();
    if (!frame) frame = requestAnimationFrame(tick);
  }

  const wake = () => {
    if (peeking) return;
    peeking = true;
    current.peek(true);
  };

  /** Back to the space it was in: the one thing a refused or late step leaves to do. */
  function recover(request: NonNullable<typeof held>) {
    if (held !== request) return;
    clearTimeout(waiting);
    waiting = undefined;
    held = null;
    if (request.animated) run(0);
  }

  function commit(reading: Extract<Reading, { kind: "commit" }>) {
    clearTimeout(idle);
    idle = undefined;
    haptic("level");
    const request = { step: reading.step, animated: !reducedMotion() };
    held = request;
    if (request.animated) {
      wake();
      motion.x = Math.max(-spread(), Math.min(spread(), base + reading.offset));
      motion.v = launch(reading.velocity);
      run(-reading.step * spread());
    }
    waiting = setTimeout(() => recover(request), ARRIVAL);
    void Promise.resolve(current.step(reading.step)).then(
      (accepted) => {
        if (accepted === false) recover(request);
      },
      () => recover(request),
    );
  }

  function apply(reading: Reading) {
    if (reading.kind === "track") {
      if (reducedMotion()) return;
      wake();
      if (mode !== "track") {
        mode = "track";
        soft = false;
        motion.v = 0;
      }
      const reach = spread();
      pending = Math.max(-reach, Math.min(reach, base + reading.offset));
      current.sheet()?.style.setProperty("will-change", "transform");
      if (!frame) frame = requestAnimationFrame(tick);
    } else if (reading.kind === "commit") commit(reading);
    else if (reading.kind === "cancel") {
      if (reducedMotion() || mode !== "track") return;
      motion.x = pending;
      motion.v = 0;
      run(0);
    }
  }

  function watch() {
    if (idle !== undefined || !tracker.tracking) return;
    idle = setTimeout(function check() {
      const quiet = performance.now() - heard;
      if (quiet < IDLE) {
        idle = setTimeout(check, IDLE - quiet);
        return;
      }
      idle = undefined;
      apply(tracker.idle());
    }, IDLE);
  }

  const sink: SwipeSink = {
    began(dx, dy, time) {
      if (held) return;
      clearTimeout(idle);
      idle = undefined;
      clearTimeout(lost);
      // A sheet still settling is taken up from where it is.
      base = mode === "spring" ? motion.x : mode === "track" ? pending : 0;
      tracker.begin();
      if (dx !== 0 || dy !== 0) sink.moved(dx, dy, time);
    },
    moved(dx, dy, time) {
      if (held) return;
      apply(tracker.move(dx, dy, time));
      clearTimeout(lost);
      if (tracker.tracking) lost = setTimeout(() => sink.lifted(), LOST);
    },
    lifted() {
      clearTimeout(lost);
      if (!held) apply(tracker.lift());
    },
  };

  function onwheel(event: Wheel) {
    if (nativeSwipeLive() || event.ctrlKey || event.deltaMode !== 0 || held) return;
    if (event.target instanceof Element && event.target.closest("input, textarea, select")) return;
    heard = performance.now();
    const natural = event.webkitDirectionInvertedFromDevice !== false;
    const fingers = natural ? -event.deltaX : event.deltaX;
    // A sheet still settling is taken up from where it is.
    if (!tracker.tracking) base = mode === "spring" ? motion.x : 0;
    apply(tracker.wheel(fingers, event.deltaY, heard));
    watch();
  }

  controls.set(node, {
    arrive(side) {
      clearTimeout(waiting);
      waiting = undefined;
      if (held) {
        const request = held;
        held = null;
        if (!request.animated) return;
        // The space this sheet was carried to has come: it is the current
        // list now, and the one left behind is its neighbour.
        motion.x += request.step * spread();
        paint();
        run(0);
        return;
      }
      if (reducedMotion() || mode === "track") return;
      tracker.reset();
      soft = true;
      motion.x = side * SOFT;
      motion.v = 0;
      paint();
      run(0);
    },
  });

  host.addEventListener("wheel", onwheel, { passive: true });
  const detach = attachNativeSwipe(host, sink);
  return {
    update(next: Options) {
      current = next;
    },
    destroy() {
      held = null;
      clearTimeout(idle);
      clearTimeout(lost);
      clearTimeout(waiting);
      if (frame) cancelAnimationFrame(frame);
      home();
      tracker.reset();
      controls.delete(node);
      host.removeEventListener("wheel", onwheel);
      detach();
    },
  };
}

/** The list arriving from the side its space stands on, when something other
 *  than a swipe moved the window there. A swipe's own sheet is already on its way. */
export function arrive(node: HTMLElement | undefined, side: SwipeStep) {
  if (!node) return;
  controls.get(node)?.arrive(side);
}
