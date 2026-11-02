<script lang="ts">
  import { scene, skyPlacement } from "$shared/sky";
  import EDGE_LIGHT from "$shared/sky/assets/edge-light.json";

  /** Whether a new tab is in front, so the window takes the sky's light. */
  let { shown }: { shown: boolean } = $props();

  let placement = $derived(skyPlacement());
  // The picture's left edge, band by band, laid where those bands lie beside
  // the page: a gradient holds its first and last colours beyond its stops,
  // so the light runs on above and below the picture without an edge.
  let edge = $derived.by(() => {
    if (!placement) return null;
    const bands = EDGE_LIGHT[scene()];
    const { y, h } = placement;
    const stops = bands.map(
      ([r, g, b], band) =>
        `rgb(${r} ${g} ${b}) ${(y + ((band + 0.5) * h) / bands.length).toFixed(1)}px`,
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
  .sky-light {
    position: fixed;
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
