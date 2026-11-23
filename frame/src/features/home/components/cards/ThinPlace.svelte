<script lang="ts">
  import { getContext } from "svelte";
  import { focusStop } from "$shared/canvas";
  import * as m from "$shared/i18n/messages";
  import { homeTop, type HomeTop } from "../../lib/context";
  import { SIZE, type HomeNode } from "../../lib/layout";
  import PlaceMark from "./PlaceMark.svelte";

  let { id, data }: { id: string; data: { item: Extract<HomeNode, { kind: "thin" }> } } = $props();
  const top = getContext<HomeTop>(homeTop);
  const at = focusStop();
  let item = $derived(data.item);
  let place = $derived(item.place);
  let search = $derived(item.search.id);
  let chosen = $derived(item.links.find((link) => top.marked(search, link.id))?.id ?? place.asked);
</script>

<section
  class="thin"
  aria-label={place.title}
  style:inline-size={`${item.box.width}px`}
  style:block-size={`${item.box.height}px`}
>
  <button
    type="button"
    class="head"
    style:inline-size={`${item.head}px`}
    {@attach at(id)}
    onclick={() => top.intent({ kind: "site", search, place: place.id })}
  >
    <PlaceMark {place} size={20} />
    <span class="title" dir="auto">{place.title}</span>
    <span class="host" dir="auto">{place.host}</span>
  </button>
  {#if item.links.length}
    <span class="links" role="group" aria-label={m.home_on_site()}>
      {#each item.links as link (link.id)}
        <button
          type="button"
          class:action={link.kind === "action"}
          aria-current={link.id === chosen ? "true" : undefined}
          style:inline-size={`${link.width}px`}
          style:block-size={`${SIZE.section}px`}
          {@attach at(`${id}/${link.kind === "row" ? link.id : `action:${link.id}`}`)}
          onclick={() => top.intent({ kind: "row", search, row: link.id })}
          ><span dir="auto">{link.label}</span></button
        >
      {/each}
    </span>
  {/if}
</section>

<style>
  /* A place with too little to read for a card: its name on one line, and
     the few links it has after it. */
  .thin {
    box-sizing: border-box;
    display: flex;
    align-items: center;
    gap: 4px;
    padding-inline-end: 8px;
    overflow: hidden;
    border-radius: var(--radius-card);
    background: var(--color-surface);
    box-shadow: var(--shadow-card);
    color: var(--color-text);
  }

  .head {
    display: flex;
    flex: none;
    align-items: center;
    gap: 8px;
    box-sizing: border-box;
    block-size: 100%;
    min-inline-size: 0;
    padding-inline: 12px 8px;
    border: 0;
    border-radius: inherit;
    background: none;
    color: inherit;
    font: inherit;
    text-align: start;
    cursor: default;
  }

  .head:hover {
    background: var(--row-hover);
  }

  .head :global(*) {
    flex: none;
  }

  .title,
  .host,
  .links span {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .head .title {
    flex: 0 1 auto;
    min-inline-size: 0;
    font-size: var(--text-page-title);
    font-weight: 600;
  }

  .head .host {
    flex: 0 1 auto;
    min-inline-size: 0;
    color: var(--color-faint);
    font-size: var(--text-label);
  }

  .links {
    display: flex;
    flex: none;
    gap: 4px;
  }

  .links button {
    flex: none;
    padding-inline: 6px;
    border: 0;
    border-radius: var(--radius-control-compact);
    background: none;
    color: var(--color-text);
    font: inherit;
    font-size: var(--text-label);
    text-align: start;
    cursor: default;
  }

  .links span {
    display: block;
  }

  .links .action {
    color: var(--color-muted);
  }

  .links button:hover {
    background: var(--color-fill-hover);
  }
</style>
