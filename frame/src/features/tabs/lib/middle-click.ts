function middleClick(action: () => void, backgroundOnly = false) {
  let pressedOnBackground = false;
  return {
    // A middle press starts autoscroll in Chromium (WebView2); WebKit has none.
    onmousedown: (event: MouseEvent) => {
      if (event.button !== 1) return;
      pressedOnBackground = event.target === event.currentTarget;
      if (!backgroundOnly || pressedOnBackground) event.preventDefault();
    },
    onauxclick: (event: MouseEvent) => {
      if (event.button !== 1) return;
      const hit = pressedOnBackground && event.target === event.currentTarget;
      pressedOnBackground = false;
      if (backgroundOnly && !hit) return;
      event.preventDefault();
      action();
    },
  };
}

/** Only rows with a close button get this shortcut. */
export const closeOnMiddleClick = (close: () => void) => middleClick(close);

/** A middle press that begins and ends on the scroller, outside its rows. */
export const openOnBackgroundMiddleClick = (open: () => void) => middleClick(open, true);
