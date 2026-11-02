import { IS_MAC } from "$shared/platform";
import { reducedMotion } from "./motion";

const IDLE_MS = 4000;

/**
 * WebKit on macOS fades its text caret, and every step of the fade redraws
 * the interface: an empty field left focused on an idle page keeps the
 * window drawing at the display's rate. After a quiet spell the field lets
 * go of focus and shows a caret drawn here instead, an opacity animation
 * the system compositor runs without waking the page. The next key the
 * window receives gives the field its focus back before the text arrives.
 */
export function parkCaret(input: HTMLInputElement): () => void {
  // The fading caret is WebKit on macOS; elsewhere the field keeps its own.
  if (!IS_MAC) return () => {};
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
    const style = getComputedStyle(input);
    const height = parseFloat(style.fontSize) * 1.2;
    const start =
      style.textAlign === "center"
        ? input.offsetLeft + input.offsetWidth / 2
        : input.offsetLeft + parseFloat(style.paddingInlineStart || "0");
    if (getComputedStyle(field).position === "static") field.style.position = "relative";
    caret = document.createElement("span");
    caret.setAttribute("aria-hidden", "true");
    Object.assign(caret.style, {
      position: "absolute",
      left: `${start}px`,
      top: `${input.offsetTop + (input.offsetHeight - height) / 2}px`,
      width: "2px",
      height: `${height}px`,
      borderRadius: "1px",
      background: style.caretColor === "auto" ? style.color : style.caretColor,
      pointerEvents: "none",
      willChange: "opacity",
    });
    field.append(caret);
    if (!reducedMotion()) {
      caret.animate(
        [
          { opacity: 1, offset: 0 },
          { opacity: 1, offset: 0.45 },
          { opacity: 0, offset: 0.6 },
          { opacity: 0, offset: 0.9 },
          { opacity: 1, offset: 1 },
        ],
        { duration: 1100, iterations: Infinity },
      );
    }
    input.dataset.caretParked = "";
    window.addEventListener("keydown", wake, true);
    document.addEventListener("focusin", release, true);
    document.addEventListener("pointerdown", release, true);
    input.blur();
  }

  // Anything that can start text brings focus back: characters, AltGr and
  // dead keys, input methods, deletion and paste; other shortcuts don't.
  const STARTS_TEXT = new Set(["Backspace", "Delete", "Dead", "Process", "Unidentified"]);
  function wake(event: KeyboardEvent) {
    const altGraph = event.getModifierState("AltGraph");
    if (event.ctrlKey && !altGraph) return;
    if (event.metaKey && event.key.toLowerCase() !== "v") return;
    if (event.key.length !== 1 && !STARTS_TEXT.has(event.key) && !event.isComposing) return;
    input.focus({ preventScroll: true });
  }

  function release(event: Event) {
    if (event.target !== input) unpark();
  }

  function unpark() {
    if (caret === null) return;
    caret.remove();
    caret = null;
    delete input.dataset.caretParked;
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
