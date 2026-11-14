<script lang="ts">
  import { ArrowDown01Icon, ArrowUp01Icon } from "@hugeicons/core-free-icons";
  import Icon from "../Icon/Icon.svelte";

  /** One end of a menu that scrolls: shown while there is more past it, and
   *  resting the pointer on it glides the list that way. */
  let { dir }: { dir: "up" | "down" } = $props();

  /** Points a millisecond: a long list passes in about a second. */
  const SPEED = 0.45;
  /** The end's own height, as popover.css draws it. */
  const HEIGHT = 22;

  let node = $state<HTMLElement>();
  let shown = $state(false);
  let frame = 0;

  function stop() {
    cancelAnimationFrame(frame);
    frame = 0;
  }

  function start() {
    const scroller = node?.parentElement;
    if (!scroller || frame !== 0) return;
    let last = performance.now();
    const step = (now: number) => {
      scroller.scrollTop += (dir === "up" ? -SPEED : SPEED) * (now - last);
      last = now;
      frame = shown ? requestAnimationFrame(step) : 0;
    };
    frame = requestAnimationFrame(step);
  }

  $effect(() => {
    const scroller = node?.parentElement;
    if (!scroller) return;
    const update = () => {
      const rest = scroller.scrollHeight - scroller.clientHeight - scroller.scrollTop;
      // A list too short for both ends has none: only its rows.
      const room = scroller.clientHeight > 4 * HEIGHT;
      shown = room && (dir === "up" ? scroller.scrollTop > 1 : rest > 1);
      if (!shown) stop();
    };
    // The rows arrive with the menu, after this end of it.
    const first = requestAnimationFrame(update);
    const observer = new ResizeObserver(update);
    observer.observe(scroller);
    scroller.addEventListener("scroll", update, { passive: true });
    return () => {
      cancelAnimationFrame(first);
      observer.disconnect();
      scroller.removeEventListener("scroll", update);
      stop();
    };
  });
</script>

<div
  bind:this={node}
  class="ui-menu-step"
  data-dir={dir}
  data-shown={shown || undefined}
  aria-hidden="true"
  onpointerenter={start}
  onpointerleave={stop}
>
  <Icon icon={dir === "up" ? ArrowUp01Icon : ArrowDown01Icon} size={12} strokeWidth={2} />
</div>
