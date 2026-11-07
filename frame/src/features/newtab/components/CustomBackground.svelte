<script lang="ts">
  import { untrack } from "svelte";
  import { mediaUrl } from "$domain/resources";

  let {
    profile,
    digest,
    width,
    height,
    dim,
    onfailure,
  }: {
    profile: string;
    digest: string;
    width: number;
    height: number;
    dim: boolean;
    onfailure: () => void;
  } = $props();
  // One image element and one bounded decode, with no resize observer or
  // preloading image. Reopening reuses the protocol/browser cache. Never ask
  // beyond the pipeline's largest thumbnail (larger asks serve the original).
  const src = untrack(() =>
    mediaUrl(
      profile,
      digest,
      Math.min(1600, Math.max(1, Math.ceil(Math.max(width, height) * window.devicePixelRatio))),
    ),
  );
</script>

{#if src}
  <img {src} alt="" decoding="async" onerror={onfailure} />
  {#if dim}<div class="dim"></div>{/if}
{/if}

<style>
  img,
  .dim {
    position: absolute;
    inset: 0;
    inline-size: 100%;
    block-size: 100%;
    pointer-events: none;
  }

  img {
    object-fit: cover;
    object-position: center;
  }

  .dim {
    background: var(--newtab-image-dim);
  }
</style>
