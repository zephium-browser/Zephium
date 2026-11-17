<script lang="ts">
  import { bars } from "../lib/island";

  /**
   * Levels as bars. The microphone's stand on a floor and burn the live
   * colour; speech grows from the middle in the text's own ink. Each redraw
   * is a level event from native: nothing here keeps time.
   */
  let {
    level,
    kind,
    count = 5,
    size = 18,
  }: { level: number; kind: "input" | "output"; count?: number; size?: number } = $props();

  const heights = $derived(bars(level, count));
</script>

<span
  class="meter"
  data-kind={kind}
  style:--size="{size}px"
  style:--count={count}
  aria-hidden="true"
>
  {#each heights as height, at (at)}<span class="bar" style:--level={height}></span>{/each}
</span>

<style>
  .meter {
    display: inline-flex;
    align-items: flex-end;
    justify-content: center;
    gap: calc(var(--size) / 9);
    inline-size: var(--size);
    block-size: var(--size);
  }

  .meter[data-kind="output"] {
    align-items: center;
  }

  .bar {
    flex: none;
    inline-size: calc(var(--size) / 7.5);
    block-size: 100%;
    border-radius: var(--radius-capsule);
    background: var(--meter-ink, var(--voice-live));
    transform: scaleY(var(--level));
    transform-origin: 50% 100%;
    transition: transform 90ms linear;
  }

  .meter[data-kind="output"] .bar {
    background: var(--meter-ink, var(--color-text));
    transform-origin: 50% 50%;
  }

  @media (prefers-reduced-motion: reduce) {
    .bar {
      transition: none;
    }
  }

  :global(:root[data-reduce-motion="true"]) .bar {
    transition: none;
  }

  @media (forced-colors: active) {
    .bar,
    .meter[data-kind="output"] .bar {
      background: CanvasText;
    }
  }
</style>
