<script lang="ts">
  import { onMount } from "svelte";
  import type { DividerView } from "$shared/ipc/bindings";
  import { commands } from "$shared/ipc/bindings";
  import { layout } from "$domain/layout";

  const KEYBOARD_STEP = 12;
  const KEYBOARD_LARGE_STEP = 36;

  let animationFrame = 0;
  let activePointer: number | null = null;
  let captureTarget: HTMLElement | null = null;
  let previousCursor = "";
  let keyboardBusy = false;

  const coordinate = (value: number | null): number => value ?? 0;

  function center(divider: DividerView): { x: number; y: number } {
    return {
      x: coordinate(divider.x) + coordinate(divider.width) / 2,
      y: coordinate(divider.y) + coordinate(divider.height) / 2,
    };
  }

  function relativePosition(divider: DividerView): number {
    const point = center(divider);
    const total = divider.vertical ? window.innerWidth : window.innerHeight;
    const position = divider.vertical ? point.x : point.y;
    if (total <= 0) return 50;
    return Math.round(Math.min(100, Math.max(0, (position / total) * 100)));
  }

  function restorePointerState(): void {
    const target = captureTarget;
    const pointer = activePointer;
    activePointer = null;
    captureTarget = null;
    // Releasing capture can deliver lostpointercapture synchronously.
    if (target && pointer != null && target.hasPointerCapture(pointer)) {
      target.releasePointerCapture(pointer);
    }
    document.body.style.cursor = previousCursor;
  }

  function handleMove(event: PointerEvent): void {
    if (event.pointerId !== activePointer || animationFrame !== 0) return;

    const { clientX: x, clientY: y } = event;
    animationFrame = requestAnimationFrame(() => {
      animationFrame = 0;
      layout.drag(x, y);
    });
  }

  function finishPointer(x: number | null, y: number | null): void {
    if (activePointer == null) return;
    if (animationFrame !== 0) {
      cancelAnimationFrame(animationFrame);
      animationFrame = 0;
    }

    layout.release(x, y);
    restorePointerState();
  }

  function handleUp(event: PointerEvent): void {
    if (event.pointerId !== activePointer) return;

    // The final coordinate and release remain one ordered native command.
    // Separate fire-and-forget drag/release calls could persist the previous
    // animation frame's ratio.
    finishPointer(
      event.type === "pointerup" ? event.clientX : null,
      event.type === "pointerup" ? event.clientY : null,
    );
  }

  const handleBlur = () => finishPointer(null, null);

  function handleDown(event: PointerEvent, vertical: boolean): void {
    if (activePointer != null || event.button !== 0) return;

    event.preventDefault();
    activePointer = event.pointerId;
    captureTarget = event.currentTarget as HTMLElement;
    previousCursor = document.body.style.cursor;
    captureTarget.setPointerCapture(event.pointerId);
    layout.grab(event.clientX, event.clientY);
    document.body.style.cursor = vertical ? "col-resize" : "row-resize";
  }

  async function handleKeydown(event: KeyboardEvent, divider: DividerView): Promise<void> {
    let direction = 0;
    if (divider.vertical && event.key === "ArrowLeft") direction = -1;
    if (divider.vertical && event.key === "ArrowRight") direction = 1;
    if (!divider.vertical && event.key === "ArrowUp") direction = -1;
    if (!divider.vertical && event.key === "ArrowDown") direction = 1;
    if (direction === 0 || keyboardBusy) return;

    event.preventDefault();
    const step = event.shiftKey ? KEYBOARD_LARGE_STEP : KEYBOARD_STEP;
    const point = center(divider);
    keyboardBusy = true;

    try {
      // Awaiting admission preserves command order for keyboard interaction;
      // the native protocol and its authoritative captured divider stay the
      // same as for pointer dragging.
      await commands.dividerGrab(point.x, point.y);
      await commands.dividerRelease(
        divider.vertical ? point.x + direction * step : point.x,
        divider.vertical ? point.y : point.y + direction * step,
      );
    } catch {
      // Native owns admission and diagnostics. A disappearing split or window
      // during the gesture is a benign no-op for this transient control.
    } finally {
      keyboardBusy = false;
    }
  }

  onMount(() => {
    window.addEventListener("pointermove", handleMove);
    window.addEventListener("pointerup", handleUp);
    window.addEventListener("pointercancel", handleUp);
    window.addEventListener("lostpointercapture", handleUp);
    window.addEventListener("blur", handleBlur);

    return () => {
      window.removeEventListener("pointermove", handleMove);
      window.removeEventListener("pointerup", handleUp);
      window.removeEventListener("pointercancel", handleUp);
      window.removeEventListener("lostpointercapture", handleUp);
      window.removeEventListener("blur", handleBlur);

      finishPointer(null, null);
    };
  });
</script>

<!-- The index is the only stable identity in the native projection. Keeping it
     as the key preserves each hit target while coordinates change, so browser
     pointer capture survives a drag frame. -->
{#each layout.dividers() as divider, index (index)}
  <div
    class="group fixed z-40 touch-none"
    style:left={`${coordinate(divider.x)}px`}
    style:top={`${coordinate(divider.y)}px`}
    style:width={`${coordinate(divider.width)}px`}
    style:height={`${coordinate(divider.height)}px`}
    style:cursor={divider.vertical ? "col-resize" : "row-resize"}
  >
    <input
      type="range"
      aria-label="Split position"
      aria-orientation={divider.vertical ? "vertical" : "horizontal"}
      min="0"
      max="100"
      value={relativePosition(divider)}
      class="peer absolute inset-0 m-0 h-full w-full touch-none appearance-none bg-transparent opacity-0 outline-none"
      style:cursor={divider.vertical ? "col-resize" : "row-resize"}
      onpointerdown={(event) => handleDown(event, divider.vertical)}
      onkeydown={(event) => void handleKeydown(event, divider)}
      oninput={(event) => (event.currentTarget.value = `${relativePosition(divider)}`)}
    />
    <span
      aria-hidden="true"
      class={`pointer-events-none absolute bg-border opacity-0 transition-opacity group-hover:opacity-80 peer-focus-visible:opacity-100 ${
        divider.vertical
          ? "top-0 bottom-0 left-1/2 w-px -translate-x-1/2"
          : "top-1/2 right-0 left-0 h-px -translate-y-1/2"
      }`}
    ></span>
  </div>
{/each}
