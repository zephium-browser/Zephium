/** How long work must run before a surface says it is still running. Most
 *  saves finish well inside it, and saying so for each would only flicker. */
const STILL_WORKING_MS = 500;

/** Follows `source` into true only once it has stayed true for `delay`, and
 *  back out at once. Call during component setup: it owns an effect. */
export function lagging(
  source: () => boolean,
  delay = STILL_WORKING_MS,
): { readonly current: boolean } {
  let current = $state(false);
  $effect(() => {
    if (!source()) {
      current = false;
      return;
    }
    const timer = setTimeout(() => (current = true), delay);
    return () => clearTimeout(timer);
  });
  return {
    get current() {
      return current;
    },
  };
}
