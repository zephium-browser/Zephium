<script lang="ts">
  import { getContext } from "svelte";
  import FavIcon from "$shared/ui/FavIcon";
  import type { Place } from "$domain/home";
  import { homeTop, type HomeTop } from "../../lib/context";

  let { place, size }: { place: Place; size: number } = $props();
  const top = getContext<HomeTop>(homeTop);
  let image = $derived(top.mark(place));
  // A place with no page open in a tab is drawn by its letter: no address is asked of anything for an icon.
  let letter = $derived(
    Array.from((place.title || place.host).trim())[0]?.toLocaleUpperCase() ?? "",
  );
</script>

{#if image}
  <FavIcon {image} {size} lit />
{:else if letter}
  <span
    class="letter"
    aria-hidden="true"
    style:inline-size={`${size}px`}
    style:block-size={`${size}px`}
    style:font-size={`${Math.round(size * 0.56)}px`}>{letter}</span
  >
{/if}

<style>
  .letter {
    display: grid;
    flex: none;
    place-items: center;
    border-radius: 28%;
    background: var(--color-fill);
    color: var(--color-muted);
    font-weight: 600;
    line-height: 1;
  }
</style>
