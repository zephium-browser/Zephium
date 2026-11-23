<script lang="ts">
  import { getContext } from "svelte";
  import { ArrowUpRight01Icon } from "@hugeicons/core-free-icons";
  import { focusStop } from "$shared/canvas";
  import Icon from "$shared/ui/Icon";
  import * as m from "$shared/i18n/messages";
  import { homeTop, type HomeTop } from "../../lib/context";
  import { SIZE, type HomeNode } from "../../lib/layout";
  import { refusal } from "../../lib/refusal";
  import RowView from "./Row.svelte";
  import Card from "./Card.svelte";

  let { id, data }: { id: string; data: { item: Extract<HomeNode, { kind: "inside" }> } } =
    $props();
  const top = getContext<HomeTop>(homeTop);
  const at = focusStop();
  let item = $derived(data.item);
  let inside = $derived(item.inside);
  let search = $derived(item.search.id);
</script>

<Card width={item.box.width} height={item.box.height} label={item.row.label}>
  <p class="head" style:block-size={`${SIZE.insideHeader}px`} dir="auto">{item.row.label}</p>
  {#if inside.state === "opening"}
    <div class="waiting" role="status" style:block-size={`${SIZE.waiting}px`}>
      <span class="said">{m.home_reading()}</span>
      <span class="bar" style:--order={0}></span>
      <span class="bar" style:--order={1}></span>
    </div>
  {:else if inside.state === "refused"}
    {@const gone = inside.reason === "gone"}
    <button
      type="button"
      class="refused"
      style:block-size={`${SIZE.refused}px`}
      {@attach at(id)}
      onclick={() =>
        top.intent(gone ? { kind: "again", search } : { kind: "row", search, row: item.row.id })}
    >
      <span class="why">{refusal(inside.reason)}</span>
      <span class="way"
        >{gone ? m.home_search_again() : m.home_open_page()}{#if !gone}<Icon
            icon={ArrowUpRight01Icon}
            size={12}
          />{/if}</span
      >
    </button>
  {:else}
    {#each inside.rows as row (row.id)}
      <RowView {row} stop={`${id}/${row.id}`} {search} height={SIZE.child} inner />
    {/each}
    {#if inside.more}
      <button
        type="button"
        class="more"
        style:block-size={`${SIZE.child}px`}
        {@attach at(`${id}/more`)}
        onclick={() =>
          item.row.menu
            ? top.menuAll(search, item.row.id)
            : top.intent({ kind: "row", search, row: item.row.id })}
        >{#if item.row.menu}{m.home_more({ count: item.rest })}{:else}{m.home_more_on_page()}<Icon
            icon={ArrowUpRight01Icon}
            size={12}
          />{/if}</button
      >
    {/if}
  {/if}
</Card>

<style>
  .head {
    box-sizing: border-box;
    flex: none;
    margin: 0;
    padding: 10px 10px 0;
    overflow: hidden;
    color: var(--color-label-secondary);
    font-size: var(--text-label);
    font-weight: 600;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .waiting {
    display: grid;
    align-content: start;
    gap: 9px;
    padding: 4px 10px 0;
  }

  .said {
    color: var(--color-faint);
    font-size: var(--text-label);
  }

  .bar {
    inline-size: calc(78% - var(--order) * 30%);
    block-size: 8px;
    border-radius: var(--radius-capsule);
    background: var(--color-fill);
    animation: breathe 1.6s var(--ease-in-out) calc(var(--order) * 180ms) infinite alternate;
  }

  @keyframes breathe {
    to {
      opacity: 0.35;
    }
  }

  .refused {
    box-sizing: border-box;
    display: grid;
    align-content: start;
    justify-items: start;
    padding: 2px 10px 0;
    border: 0;
    border-radius: var(--radius-row);
    background: none;
    font: inherit;
    text-align: start;
    cursor: default;
  }

  .why {
    color: var(--color-muted);
    font-size: var(--text-label);
    line-height: 16px;
  }

  .way,
  .more {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    color: var(--color-text);
    font-size: var(--text-label);
  }

  .way {
    margin-block-start: 10px;
    padding: 5px 10px;
    border-radius: var(--radius-control-compact);
    background: var(--color-fill);
  }

  .refused:hover .way {
    background: var(--color-fill-hover);
  }

  .more {
    padding-inline: 10px;
    border: 0;
    border-radius: var(--radius-row);
    background: none;
    color: var(--color-muted);
    font: inherit;
    font-size: var(--text-label);
    cursor: default;
  }

  .more:hover {
    background: var(--row-hover);
    color: var(--color-text);
  }

  .way :global(svg),
  .more :global(svg) {
    color: var(--color-faint);
  }

  @media (prefers-reduced-motion: reduce) {
    .bar {
      animation: none;
    }
  }

  :global(:root[data-reduce-motion="true"]) .bar {
    animation: none;
  }
</style>
