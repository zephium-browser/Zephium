<script lang="ts">
  import { getContext, untrack } from "svelte";
  import { focusStop, thumbnail } from "$shared/canvas";
  import * as m from "$shared/i18n/messages";
  import { homeTop, type HomeTop } from "../../lib/context";
  import type { HomeNode } from "../../lib/layout";
  import { clockOf } from "../../lib/time";

  let { id, data }: { id: string; data: { item: Extract<HomeNode, { kind: "search" }> } } =
    $props();
  const top = getContext<HomeTop>(homeTop);
  const at = focusStop();
  let search = $derived(data.item.search);
  let picture = $derived(search.results ? top.picture(search.results) : null);
  let missing = $state<string | null>(null);
  const born = untrack(() => search.kind === "place" && search.place.state === "arriving");
</script>

<button
  type="button"
  class="search"
  class:enter={born}
  aria-label={m.home_results_of({ text: search.text })}
  style:inline-size={`${data.item.box.width}px`}
  style:block-size={`${data.item.box.height}px`}
  {@attach at(id)}
  onclick={() => top.intent({ kind: "results", search: search.id })}
>
  <span class="words">
    <span class="text" dir="auto">{search.text}</span>
    <span class="time">{clockOf(search.at)}</span>
  </span>
  {#if picture && missing !== picture}<canvas
      class="results"
      aria-hidden="true"
      use:thumbnail={{ url: picture, width: 64, onmissing: () => (missing = picture) }}
    ></canvas>{/if}
</button>

<style>
  /* What was asked, as it was asked: the field's own tone, a step under a card. */
  .search {
    box-sizing: border-box;
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 0 10px 0 14px;
    border: 0;
    border-radius: var(--radius-card);
    background: var(--color-surface);
    box-shadow: var(--shadow-card);
    color: var(--color-text);
    font: inherit;
    text-align: start;
    cursor: default;
  }

  .words {
    display: grid;
    flex: 1;
    gap: 2px;
    min-inline-size: 0;
  }

  .text {
    display: -webkit-box;
    overflow: hidden;
    font-size: var(--text-body);
    font-weight: 600;
    line-height: 17px;
    -webkit-box-orient: vertical;
    -webkit-line-clamp: 2;
    line-clamp: 2;
  }

  .time {
    color: var(--color-faint);
    font-size: var(--text-caption);
    font-variant-numeric: tabular-nums;
  }

  .results {
    flex: none;
    inline-size: 64px;
    block-size: 40px;
    border-radius: 5px;
    background: var(--color-fill);
  }

  .enter {
    animation: search-in var(--motion-page) var(--ease-emphasized) backwards;
  }

  @keyframes search-in {
    from {
      opacity: 0;
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .enter {
      animation: none;
    }
  }

  :global(:root[data-reduce-motion="true"]) .enter {
    animation: none;
  }
</style>
