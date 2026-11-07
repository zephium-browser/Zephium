/**
 * What the column shows that the person has to see: a question it asks, a
 * download setting off. While anything holds it, a hidden column comes out as
 * the card the pointer at the window's edge brings, and goes back once
 * nothing does and the pointer isn't on it.
 */
import { untrack } from "svelte";

let holds = $state(0);

export const wanted = () => holds > 0;

/** Holds the column out until the returned function is called. Effects
 *  hold and release it without coming to depend on the count. */
export function hold(): () => void {
  untrack(() => (holds += 1));
  let released = false;
  return () => {
    if (released) return;
    released = true;
    untrack(() => (holds -= 1));
  };
}

/** Brings the column out for something brief. */
export function pulse(ms = 3000): void {
  setTimeout(hold(), ms);
}
