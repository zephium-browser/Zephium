<script lang="ts">
  import { getContext } from "svelte";
  import { focusStop } from "$shared/canvas";
  import Icon from "$shared/ui/Icon";
  import * as m from "$shared/i18n/messages";
  import { homeTop, type HomeTop } from "../../lib/context";
  import { SIZE, type HomeNode } from "../../lib/layout";
  import { KIND_MARK } from "../../lib/marks";
  import Card from "./Card.svelte";

  let { id, data }: { id: string; data: { item: Extract<HomeNode, { kind: "also" }> } } = $props();
  const top = getContext<HomeTop>(homeTop);
  const at = focusStop();
  let search = $derived(data.item.search);
  let listed = $derived(search.also.slice(0, search.also.length - data.item.hidden));
  const kinds = { page: null, discussion: m.home_discussion, video: m.home_video } as const;
</script>

<Card width={data.item.box.width} height={data.item.box.height} label={m.home_also()}>
  <p class="label" style:block-size={`${SIZE.label}px`}>{m.home_also()}</p>
  {#each listed as also, index (also.id)}
    {@const mark = KIND_MARK[also.kind]}
    <button
      type="button"
      style:block-size={`${data.item.heights[index] ?? SIZE.also}px`}
      {@attach at(`${id}/${also.id}`)}
      onclick={() => top.intent({ kind: "result", search: search.id, result: also.id })}
    >
      <span class="title" dir="auto">{also.title}</span>
      <span class="host"
        >{#if mark}<Icon icon={mark} size={11} label={kinds[also.kind]?.()} />{/if}<span dir="auto"
          >{also.host}</span
        ></span
      >
    </button>
  {/each}
  {#if data.item.hidden}
    <button
      type="button"
      class="more"
      style:block-size={`${SIZE.moreAlso}px`}
      {@attach at(`${id}/more`)}
      onclick={() => top.all(search.id)}>{m.home_more({ count: data.item.hidden })}</button
    >
  {/if}
</Card>

<style>
  .label {
    box-sizing: border-box;
    flex: none;
    margin: 0;
    padding: 8px 10px 0;
    color: var(--color-faint);
    font-size: var(--text-caption);
    font-weight: 600;
  }

  button {
    display: grid;
    flex: none;
    align-content: center;
    gap: 1px;
    min-inline-size: 0;
    padding-inline: 10px;
    border: 0;
    border-radius: var(--radius-row);
    background: none;
    color: inherit;
    font: inherit;
    text-align: start;
    cursor: default;
  }

  button:hover {
    background: var(--row-hover);
  }

  .more {
    color: var(--color-muted);
    font-size: var(--text-label);
  }

  .host span {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  /* A title takes two lines before it is cut. */
  .title {
    display: -webkit-box;
    overflow: hidden;
    font-size: var(--text-label);
    line-height: 16px;
    overflow-wrap: anywhere;
    -webkit-box-orient: vertical;
    -webkit-line-clamp: 2;
    line-clamp: 2;
  }

  .host {
    display: flex;
    align-items: center;
    gap: 4px;
    min-inline-size: 0;
    color: var(--color-faint);
    font-size: var(--text-caption);
    line-height: 14px;
  }

  .host :global(svg) {
    flex: none;
  }
</style>
