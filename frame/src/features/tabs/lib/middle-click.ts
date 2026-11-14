/**
 * Handlers that close a tab on a middle-click, spread onto its main button.
 * Only rows that show a close button get them.
 */
export function closeOnMiddleClick(close: () => void) {
  return {
    // A middle press starts autoscroll in Chromium (WebView2); WebKit has none.
    onmousedown: (event: MouseEvent) => {
      if (event.button === 1) event.preventDefault();
    },
    onauxclick: (event: MouseEvent) => {
      if (event.button !== 1) return;
      event.preventDefault();
      close();
    },
  };
}

const EMPTY_SURFACE_BLOCK =
  "[data-zephium-tab-id], button, a, [role='button'], input, textarea, select";

/** True when the event landed on empty tab-list surface, not a row or control. */
export function isEmptyTabSurface(event: MouseEvent): boolean {
  const el = event.target;
  // Duck-typed so unit tests can stub `closest` without a DOM environment.
  if (!el || typeof (el as Element).closest !== "function") return false;
  return !(el as Element).closest(EMPTY_SURFACE_BLOCK);
}

/**
 * Middle-click on empty space below the tabs opens a new tab (Firefox-style).
 * Spread onto the scrolling sidebar region that owns the lists.
 */
export function openOnEmptyMiddleClick(open: () => void) {
  return {
    onmousedown: (event: MouseEvent) => {
      if (event.button === 1 && isEmptyTabSurface(event)) event.preventDefault();
    },
    onauxclick: (event: MouseEvent) => {
      if (event.button !== 1 || !isEmptyTabSurface(event)) return;
      event.preventDefault();
      open();
    },
  };
}
