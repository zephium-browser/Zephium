import { commands } from "$shared/ipc/bindings";

let open = 0;
let reported: boolean | null = null;

function report() {
  // One report per task, so a dialog that closes as another opens does not
  // flap the layout.
  queueMicrotask(() => {
    const next = open > 0;
    if (next === reported) return;
    reported = next;
    void commands.chromeModal(next).catch(() => {
      reported = null;
    });
  });
}

/**
 * Holds pages off the screen while one of the browser's own dialogs is open
 * over the window: pages are native views above the chrome, and would hide
 * it. Returns the release.
 */
export function holdChromeModal(): () => void {
  open += 1;
  report();
  let held = true;
  return () => {
    if (!held) return;
    held = false;
    open -= 1;
    report();
  };
}
