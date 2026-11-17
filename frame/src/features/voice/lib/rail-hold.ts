import { expandBriefly } from "$session/sidebar-mode.svelte";

/**
 * A step waiting on the person opens the rail's column for its card. The
 * hold outlives the disc that asked for it, since the column it opens
 * replaces the disc; the card gives it back once the step is answered.
 */
let holding: (() => void) | null = null;

export const held = () => holding !== null;

export function hold() {
  holding ??= expandBriefly();
}

export function release() {
  const give = holding;
  holding = null;
  give?.();
}
