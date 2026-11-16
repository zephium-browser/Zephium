/** Only rows with a close button get this shortcut. */
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

const onBackground = ({ target, currentTarget }: MouseEvent) =>
  target === currentTarget ||
  (target instanceof HTMLElement && target.hasAttribute("data-tab-list-background"));

/** A middle press that begins and ends on empty list space, outside its rows. */
export function openOnBackgroundMiddleClick(open: () => void) {
  let pressed = false;
  let released = false;
  return {
    onmousedown(event: MouseEvent) {
      if (event.button !== 1) return;
      pressed = onBackground(event);
      released = false;
      if (pressed) event.preventDefault();
    },
    onmouseup(event: MouseEvent) {
      if (event.button === 1) released = onBackground(event);
    },
    onauxclick(event: MouseEvent) {
      if (event.button !== 1) return;
      // auxclick can target the common ancestor even if the release hit a row.
      const hit = pressed && released && onBackground(event);
      pressed = released = false;
      if (!hit) return;
      event.preventDefault();
      open();
    },
  };
}
