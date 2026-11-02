<script lang="ts">
  import type { IconSvgElement } from "@hugeicons/svelte";
  import Icon from "../Icon/Icon.svelte";

  type Props = {
    /** Decoded 32x32 RGBA pixels, or null until they arrive. */
    image: ImageData | null;
    /** How the mark sits against a surface, so a near-neutral icon the colour
     *  of the chrome behind it can be given its own ground. */
    tone?: "dark" | "light" | "mid";
    loading?: boolean;
    size?: number;
    /** Resting rows dim and desaturate; the active row shows true color. */
    lit?: boolean;
    /** Drawn when a site supplies no icon, or has not supplied one yet. */
    fallback?: IconSvgElement;
    /** Cuts the corner a sound badge sits in. */
    notched?: boolean;
  };

  let {
    image,
    tone = "mid",
    loading = false,
    size = 17,
    lit = false,
    fallback,
    notched = false,
  }: Props = $props();
  let canvas: HTMLCanvasElement | undefined = $state();

  let radius = $derived(Math.max(4, Math.round(size * 0.28)));
  // Percentage padding resolves against the container's width, not the icon's,
  // so the inset is measured here and applied only where CSS draws a plate.
  let inset = $derived(Math.max(1, Math.round(size * 0.12)));

  $effect(() => {
    const pixels = image;
    if (!pixels || !canvas) return;
    canvas.getContext("2d")?.putImageData(pixels, 0, 0);
  });
</script>

<!--
  Site-controlled image formats are decoded in the sandboxed page renderer.
  Privileged chrome receives one fixed-size RGBA buffer and paints it directly;
  this process never exposes an image decoder or a custom favicon protocol.
-->
<span
  class="favicon"
  data-favicon
  data-loading={loading}
  data-notched={notched || undefined}
  style:width={`${size}px`}
  style:height={`${size}px`}
  aria-hidden="true"
>
  {#if image}
    <!-- The inset belongs to the plate, and only CSS knows whether this theme
         draws one: insetting by tone alone shrank marks that were never plated. -->
    <span
      data-tone={tone}
      style:border-radius={`${radius}px`}
      style:--favicon-inset={`${inset}px`}
      class:favicon-plate-lit={lit}
      class="favicon-plate"
      ><canvas bind:this={canvas} width={image.width} height={image.height}></canvas></span
    >
  {:else if fallback}
    <span class="favicon-fallback"><Icon icon={fallback} size={Math.round(size * 0.85)} /></span>
  {:else}
    <span class="favicon-blank"></span>
  {/if}
</span>
