import { IS_MAC } from "$shared/platform";

const IDLE_MS = 4000;

/**
 * Replaces a focused empty field's native caret after an idle interval to
 * reduce caret-driven drawing. The field loses focus and an aria-hidden
 * caret takes its place. It stands still: on macOS any blink, as an
 * animation or a timer, kept WebKit drawing frames, and over the window's
 * glass an opacity fade held the GPU near half busy on an idle new tab.
 *
 * A parked field still takes every key it would have taken focused. Text
 * and Tab land natively once focus is back; other keys, such as Escape,
 * Enter or the arrows, are handed to the field so its own handlers run, and
 * a text area's Enter still breaks the line when they leave it.
 * Paste (Cmd+V, and Ctrl+V or Ctrl+Shift+V off macOS) brings focus back
 * before its default action, so the text lands in the field. Other Ctrl
 * shortcuts without AltGraph and other Cmd shortcuts leave it parked.
 * Focus and pointer interaction release the parked caret.
 */
export function parkCaret(
  input: HTMLInputElement | HTMLTextAreaElement,
  onpark?: (parked: boolean) => void,
): () => void {
  let timer: ReturnType<typeof setTimeout> | undefined;
  let caret: HTMLElement | null = null;

  const quiet = () => document.activeElement === input && input.value === "";

  function schedule() {
    clearTimeout(timer);
    if (quiet()) timer = setTimeout(park, IDLE_MS);
  }

  function park() {
    if (!quiet() || !document.hasFocus()) return;
    const field = input.parentElement;
    if (!field) return;
    // Offsets are measured against the field, so it must be positioned first.
    if (getComputedStyle(field).position === "static") field.style.position = "relative";
    const style = getComputedStyle(input);
    const height = parseFloat(style.fontSize) * 1.2;
    // A text area's caret sits on its first line, not in its middle.
    const top =
      input instanceof HTMLTextAreaElement
        ? input.offsetTop +
          parseFloat(style.borderTopWidth || "0") +
          parseFloat(style.paddingTop || "0") +
          ((parseFloat(style.lineHeight) || height) - height) / 2
        : input.offsetTop + (input.offsetHeight - height) / 2;
    const start =
      style.textAlign === "center"
        ? input.offsetLeft + input.offsetWidth / 2
        : input.offsetLeft + parseFloat(style.paddingInlineStart || "0");
    caret = document.createElement("span");
    caret.setAttribute("aria-hidden", "true");
    Object.assign(caret.style, {
      position: "absolute",
      left: `${start}px`,
      top: `${top}px`,
      width: "2px",
      height: `${height}px`,
      borderRadius: "1px",
      background: style.caretColor === "auto" ? style.color : style.caretColor,
      pointerEvents: "none",
      willChange: "opacity",
    });
    field.append(caret);
    input.dataset.caretParked = "";
    onpark?.(true);
    window.addEventListener("keydown", wake, true);
    document.addEventListener("focusin", release, true);
    document.addEventListener("pointerdown", release, true);
    input.blur();
  }

  const STARTS_TEXT = new Set(["Backspace", "Delete", "Dead", "Process", "Unidentified"]);
  const MODIFIERS = new Set(["Alt", "AltGraph", "CapsLock", "Control", "Fn", "Meta", "Shift"]);
  function wake(event: KeyboardEvent) {
    const altGraph = event.getModifierState("AltGraph");
    // The physical key too, so paste still wakes it on a non-Latin layout.
    const v = event.key.toLowerCase() === "v" || event.code === "KeyV";
    const paste = v && !event.altKey && (event.metaKey || (!IS_MAC && event.ctrlKey));
    if (event.ctrlKey && !altGraph && !paste) return;
    if (event.metaKey && !paste) return;
    if (MODIFIERS.has(event.key)) return;
    input.focus({ preventScroll: true });
    const text = paste || event.key.length === 1 || STARTS_TEXT.has(event.key) || event.isComposing;
    if (text || event.key === "Tab" || event.target === input) return;
    event.stopImmediatePropagation();
    const echo = new KeyboardEvent("keydown", {
      key: event.key,
      code: event.code,
      location: event.location,
      repeat: event.repeat,
      shiftKey: event.shiftKey,
      altKey: event.altKey,
      metaKey: event.metaKey,
      ctrlKey: event.ctrlKey,
      bubbles: true,
      cancelable: true,
      composed: true,
    });
    input.dispatchEvent(echo);
    // Only the native key inserts a text area's line break, so it runs
    // unless the field's own handler took the key.
    const lineBreak = input instanceof HTMLTextAreaElement && event.key === "Enter";
    if (echo.defaultPrevented || !lineBreak) event.preventDefault();
  }

  function release(event: Event) {
    if (event.target !== input) unpark();
  }

  function unpark() {
    if (caret === null) return;
    caret.remove();
    caret = null;
    delete input.dataset.caretParked;
    onpark?.(false);
    window.removeEventListener("keydown", wake, true);
    document.removeEventListener("focusin", release, true);
    document.removeEventListener("pointerdown", release, true);
  }

  const focused = () => {
    unpark();
    schedule();
  };
  const blurred = () => clearTimeout(timer);

  input.addEventListener("focus", focused);
  input.addEventListener("blur", blurred);
  input.addEventListener("input", schedule);
  input.addEventListener("keydown", schedule);
  window.addEventListener("focus", schedule);
  schedule();

  return () => {
    clearTimeout(timer);
    unpark();
    input.removeEventListener("focus", focused);
    input.removeEventListener("blur", blurred);
    input.removeEventListener("input", schedule);
    input.removeEventListener("keydown", schedule);
    window.removeEventListener("focus", schedule);
  };
}
