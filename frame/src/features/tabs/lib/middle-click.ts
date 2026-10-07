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
