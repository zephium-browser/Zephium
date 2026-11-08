<script lang="ts">
  import { frameFor, type SkyFrame } from "./dither";
  import type { Box } from "./place";
  import { loadLuma } from "./luma";
  import { frostFrom, printed } from "./print.svelte";
  import type { Scene } from "./scenes";

  let {
    scene,
    width,
    height,
    sky,
    horizon,
    pictureHorizon,
    zoom = 1,
    onready,
  }: {
    scene: Scene;
    /** The page's size, in page pixels. */
    width: number;
    height: number;
    /** Where the picture lies on the page (placeSky). */
    sky: Box;
    /** The page's horizon, in page pixels. */
    horizon: number;
    pictureHorizon: number;
    /** How much larger than its own size the page is ever drawn: a page a
     *  camera closes in on is printed for its closest view. */
    zoom?: number;
    /** Called once the scene first shows. */
    onready?: (scene: Scene) => void;
  } = $props();

  let density = $state(globalThis.devicePixelRatio || 1);
  // Moving the window to a screen of another density reprints it there.
  $effect(() => {
    const query = matchMedia(`(resolution: ${density}dppx)`);
    const moved = () => (density = globalThis.devicePixelRatio || 1);
    query.addEventListener("change", moved, { once: true });
    return () => query.removeEventListener("change", moved);
  });

  let canvas = $state<HTMLCanvasElement>();
  let scale = $derived(density * zoom);
  let size = $derived(frameFor(width, height, scale));
  let frame = $derived<SkyFrame>({ ...size, density: scale, sky, horizon, pictureHorizon });

  // Printed as soon as its light is read, once per change the page settles:
  // a resize prints each size it is laid out at, and nothing runs at rest. No
  // animation frame is waited for, so a window still hidden is printed before
  // it is shown.
  $effect(() => {
    const wanted = frame;
    const target = canvas;
    const wantedScene = scene;
    if (!target || wanted.columns <= 0 || wanted.rows <= 0 || wanted.sky.w <= 0) return;
    let live = true;
    void loadLuma(wantedScene).then(
      (luma) => {
        if (!live) return;
        const pixels = printed(wantedScene, luma, wanted);
        if (target.width !== wanted.columns) target.width = wanted.columns;
        if (target.height !== wanted.rows) target.height = wanted.rows;
        target.getContext("2d")?.putImageData(pixels, 0, 0);
        frostFrom(wantedScene, target);
        onready?.(wantedScene);
      },
      () => {
        if (live) onready?.(wantedScene);
      },
    );
    return () => {
      live = false;
    };
  });
</script>

<!-- One dot of the print is a whole number of device pixels, so the screen
     stays crisp at any size: the canvas is the dots, scaled up without
     smoothing. -->
<canvas
  bind:this={canvas}
  class="sky-print"
  aria-hidden="true"
  style:width={`${(size.columns * size.dot) / scale}px`}
  style:height={`${(size.rows * size.dot) / scale}px`}
></canvas>

<style>
  .sky-print {
    position: absolute;
    inset-block-start: 0;
    inset-inline-start: 0;
    image-rendering: pixelated;
    pointer-events: none;
  }
</style>
