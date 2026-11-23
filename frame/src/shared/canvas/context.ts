import { getContext } from "svelte";
import type { Attachment } from "svelte/attachments";

/** `(id) => boolean`: whether an item has come near the view; until then it may stand as an empty box. */
export const canvasSeen = Symbol("canvas-seen");
/** `{ far }`: the canvas is seen from far, so pictures may draw as small copies. */
export const canvasFar = Symbol("canvas-far");
/** `(id) => Duration | null`: how an item that just appeared arrives, if it does. */
export const canvasArrival = Symbol("canvas-arrival");
/** The keyboard's stop on a canvas that takes keyboard focus. */
export const canvasFocus = Symbol("canvas-focus");

export type CanvasFocusState = {
  /** The canvas takes keyboard focus: its stops rove. */
  readonly on: boolean;
  readonly current: string | null;
  /** The keyboard came to a stop by itself (a click, a Tab). */
  arrived: (id: string) => void;
};

/**
 * The attachment that makes an element a keyboard stop on its canvas: only
 * the current stop is in the tab order. Call it while a component starts;
 * off a canvas that takes keyboard focus, it does nothing.
 */
export function focusStop(): (id: string) => Attachment<HTMLElement> {
  const focus = getContext<CanvasFocusState | undefined>(canvasFocus);
  return (id) => (element) => {
    if (!focus?.on) return;
    element.dataset.canvasStop = id;
    element.tabIndex = focus.current === id ? 0 : -1;
    const arrived = () => focus.arrived(id);
    element.addEventListener("focus", arrived);
    return () => {
      element.removeEventListener("focus", arrived);
      delete element.dataset.canvasStop;
    };
  };
}
