import * as attention from "$session/attention.svelte";

/** How long a popup has to take the keyboard before the hold lets go. */
const TAKE_MS = 1500;
/** Where nothing says the popup went, the hold never outlasts this. */
const LONGEST_MS = 45_000;

/**
 * An extension's popup takes the keyboard from the window, which reads to a
 * hidden column like the pointer leaving it. The column stays out until the
 * window has the keyboard back, the popup gone, so the popup never hangs
 * from a column that slid away.
 */
export function holdWhilePopupOpen(): void {
  const release = attention.hold();
  let taken = false;
  const blurred = () => (taken = true);
  const focused = () => {
    if (!taken) return;
    done();
  };
  // A click on the page closes the popup but leaves the keyboard there;
  // native says the popup went instead.
  const closed = () => done();
  const unused = setTimeout(() => {
    if (!taken) done();
  }, TAKE_MS);
  const longest = setTimeout(done, LONGEST_MS);
  function done() {
    clearTimeout(unused);
    clearTimeout(longest);
    window.removeEventListener("blur", blurred);
    window.removeEventListener("focus", focused);
    window.removeEventListener("zephium:popover-closed", closed);
    release();
  }
  window.addEventListener("blur", blurred);
  window.addEventListener("focus", focused);
  window.addEventListener("zephium:popover-closed", closed);
}
