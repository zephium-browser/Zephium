/** The nearest ancestor that scrolls vertically, which is what a resizing
 *  field can push around. */
export function scrollParent(node: Element): HTMLElement | null {
  for (let at = node.parentElement; at; at = at.parentElement) {
    const overflow = getComputedStyle(at).overflowY;
    if (overflow === "auto" || overflow === "scroll") return at;
  }
  return null;
}

/** Sizes a textarea to its text. Measuring collapses it for a moment, which
 *  shortens the content around it, and the browser clamps the scroll
 *  position to fit: put back, the page holds still while the field grows. */
export function fitTextarea(node: HTMLTextAreaElement, scroller: HTMLElement | null): void {
  const top = scroller?.scrollTop ?? 0;
  node.style.height = "auto";
  node.style.height = `${node.scrollHeight}px`;
  if (scroller && scroller.scrollTop !== top) scroller.scrollTop = top;
}
