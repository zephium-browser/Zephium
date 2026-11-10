<script lang="ts">
  import { scene, skyEdgeLight, skyPlacement } from "$shared/sky";

  /** Whether a new tab is in front, so the window takes the sky's light. */
  let { shown }: { shown: boolean } = $props();

  let placement = $derived(skyPlacement());
  // The printed sky's left edge, band by band down the page, laid beside it.
  let edge = $derived.by(() => {
    const light = skyEdgeLight();
    if (!placement || !light || light.scene !== scene()) return null;
    const { bands } = light;
    const h = placement.height;
    const stops = bands.map(
      ([r, g, b], band) =>
        `rgb(${r} ${g} ${b}) ${(((band + 0.5) * h) / bands.length).toFixed(1)}px`,
    );
    return `linear-gradient(${stops.join(", ")})`;
  });
</script>

<!-- On a new tab the window's glass is evened out, so whatever the wallpaper
     puts through it is only a hint, and lit from the side by the sky: each
     band of the picture's left edge throws its own light across the column,
     strongest beside the page and gone by the window's far edge. It is only
     ever beside the page, easing in and out round its corners, so no light
     shows where there is no picture to give it. -->
<div class="sky-light" aria-hidden="true" data-scene={scene()} data-shown={shown || undefined}>
  {#if placement && edge}<div
      class="sky-edge"
      style:top={`${placement.top}px`}
      style:height={`${placement.height}px`}
      style:width={`${placement.left}px`}
      style:background-image={edge}
    ></div>{/if}
</div>

<style>
  /* Absolute, not fixed: the document never scrolls, so both cover the
     window, but WebKit keeps a fixed one on window-sized layers of its own
     (30 MB at 1200x800) for as long as a new tab is open. */
  .sky-light {
    position: absolute;
    inset: 0;
    background: var(--sky-glass);
    opacity: 0;
    pointer-events: none;
    transition: opacity 560ms cubic-bezier(0.4, 0, 0.2, 1);

    --sky-glass: var(--sky-day-glass);
    --sky-edge-strength: var(--sky-day-edge-strength);
  }

  .sky-light[data-scene="night"] {
    --sky-glass: var(--sky-night-glass);
    --sky-edge-strength: var(--sky-night-edge-strength);
  }

  .sky-light[data-shown] {
    opacity: 1;
    transition-duration: 320ms;
    transition-timing-function: cubic-bezier(0.2, 0, 0, 1);
  }

  .sky-edge {
    position: absolute;
    left: 0;
    opacity: var(--sky-edge-strength);
    mask-image: var(--sky-edge-falloff), var(--sky-edge-ends);
    mask-composite: intersect;
  }

  @media (prefers-reduced-motion: reduce) {
    .sky-light {
      transition: none;
    }
  }
</style>
