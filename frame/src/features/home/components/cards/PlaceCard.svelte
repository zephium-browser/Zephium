<script lang="ts">
  import { getContext, untrack } from "svelte";
  import { ArrowDown01Icon } from "@hugeicons/core-free-icons";
  import { focusStop } from "$shared/canvas";
  import FavIcon from "$shared/ui/FavIcon";
  import Icon from "$shared/ui/Icon";
  import * as m from "$shared/i18n/messages";
  import { homeTop, type HomeTop } from "../../lib/context";
  import { SIZE } from "../../lib/layout";
  import type { HomeNode } from "../../lib/layout";
  import RowView from "./Row.svelte";
  import Card from "./Card.svelte";
  import { refusal } from "../../lib/refusal";

  let { id, data }: { id: string; data: { item: Extract<HomeNode, { kind: "place" }> } } = $props();
  const top = getContext<HomeTop>(homeTop);
  const at = focusStop();
  let item = $derived(data.item);
  let place = $derived(item.place);
  let search = $derived(item.search.id);
  let key = $derived(`${search}/${place.id}`);
  let expanded = $derived(top.expanded(key));
  let mark = $derived(top.mark(place));

  // Rows already here when the card is drawn are simply there; the ones that
  // come in after it arrive, in the order they came.
  const born = untrack(() => place.state === "arriving");
  const present = untrack(() => new Set(place.rows.map((row) => row.id)));
  let late = $derived(
    new Map(
      place.rows
        .filter((row) => !present.has(row.id))
        .map((row, index) => [row.id, index] as const),
    ),
  );
</script>

<Card width={item.box.width} height={item.box.height} enter={born} label={place.title}>
  <button
    type="button"
    class="head"
    style:block-size={`${SIZE.header}px`}
    {@attach at(id)}
    onclick={() => top.intent({ kind: "site", search, place: place.id })}
  >
    {#if mark}<FavIcon image={mark} size={22} lit />{/if}
    <span class="names">
      <span class="title" dir="auto">{place.title}</span>
      <span class="host" dir="auto">{place.host}</span>
    </span>
  </button>

  {#if place.state === "failed"}
    <div class="failed" style:block-size={`${SIZE.failed}px`}>
      <p class="said">{m.home_failed({ name: place.title || place.host })}</p>
      <p class="why">{refusal(place.failure ?? "unavailable")}</p>
      <div class="ways">
        <button type="button" onclick={() => top.intent({ kind: "results", search })}
          >{m.home_open_results()}</button
        >
        <button type="button" onclick={() => top.intent({ kind: "site", search, place: place.id })}
          >{m.home_open_site()}</button
        >
      </div>
    </div>
  {:else}
    {#each item.rows as row, index (row.id)}
      {#if index === item.split && index > 0}<hr
          class="divide"
          style:block-size={`${SIZE.divide}px`}
        />{/if}
      <RowView
        {row}
        stop={`${id}/${row.id}`}
        {search}
        height={SIZE.row}
        selected={row.id === place.asked}
        arriving={late.get(row.id) ?? null}
      />
    {/each}
    {#if place.state === "arriving"}
      <p class="reading" role="status" style:block-size={`${SIZE.row * SIZE.coming}px`}>
        {m.home_reading_site({ host: place.host })}
      </p>
    {/if}
    {#if place.more.length}
      <button
        type="button"
        class="more"
        class:expanded
        aria-expanded={expanded}
        style:block-size={`${SIZE.row}px`}
        {@attach at(`${id}/more`)}
        onclick={() => top.toggle(key)}
      >
        <span>{expanded ? m.home_fewer() : m.home_more({ count: place.more.length })}</span>
        <Icon icon={ArrowDown01Icon} size={13} strokeWidth={2} />
      </button>
      {#if expanded}
        <div class="sections">
          {#each place.more as section (section.id)}
            <button
              type="button"
              style:block-size={`${SIZE.section}px`}
              {@attach at(`${id}/more:${section.id}`)}
              onclick={() => top.intent({ kind: "row", search, row: section.id })}
              ><span dir="auto">{section.label}</span></button
            >
          {/each}
        </div>
      {/if}
    {/if}
    {#if place.actions.length}
      <div
        class="foot"
        role="group"
        aria-label={m.home_on_site()}
        style:block-size={`${SIZE.foot}px`}
      >
        {#each place.actions as action, index (action.id)}
          <button
            type="button"
            style:inline-size={`${item.actions[index]}px`}
            style:block-size={`${SIZE.section}px`}
            {@attach at(`${id}/action:${action.id}`)}
            onclick={() => top.intent({ kind: "row", search, row: action.id })}
            ><span dir="auto">{action.label}</span></button
          >
        {/each}
      </div>
    {/if}
  {/if}
</Card>

<style>
  .head {
    display: flex;
    flex: none;
    align-items: center;
    gap: 10px;
    padding-inline: 10px;
    border: 0;
    border-radius: var(--radius-row);
    background: none;
    color: inherit;
    font: inherit;
    text-align: start;
    cursor: default;
    transition: background-color var(--motion-instant) var(--ease-smooth);
  }

  .head:hover {
    background: var(--row-hover);
  }

  .names {
    display: grid;
    min-inline-size: 0;
  }

  .title,
  .host {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .title {
    font-size: var(--text-page-title);
    font-weight: 600;
    line-height: 18px;
  }

  .host {
    color: var(--color-faint);
    font-size: var(--text-label);
    line-height: 16px;
  }

  .failed {
    box-sizing: border-box;
    padding: 2px 10px 0;
  }

  .said {
    margin: 0;
    overflow: hidden;
    font-size: var(--text-body);
    font-weight: 500;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .why {
    margin: 2px 0 0;
    color: var(--color-muted);
    font-size: var(--text-label);
    line-height: 16px;
  }

  .ways {
    display: flex;
    gap: 6px;
    margin-block-start: 12px;
  }

  .ways button,
  .more,
  .sections button,
  .foot button {
    min-inline-size: 0;
    border: 0;
    background: none;
    color: var(--color-muted);
    font: inherit;
    font-size: var(--text-label);
    text-align: start;
    cursor: default;
    transition:
      background-color var(--motion-instant) var(--ease-smooth),
      color var(--motion-instant) var(--ease-smooth);
  }

  .ways button {
    block-size: 28px;
    padding-inline: 10px;
    border-radius: var(--radius-control-compact);
    background: var(--color-fill);
    color: var(--color-text);
  }

  /* Rows both places have stand above it; each place's own below. */
  .divide {
    box-sizing: border-box;
    flex: none;
    margin: 0 10px;
    border: 0;
    background: linear-gradient(var(--color-border), var(--color-border)) center / 100% 1px
      no-repeat;
  }

  /* Rows still on their way: said once, quietly, where they will stand. */
  .reading {
    box-sizing: border-box;
    display: flex;
    align-items: center;
    margin: 0;
    padding-inline: 10px;
    overflow: hidden;
    color: var(--color-faint);
    font-size: var(--text-label);
    white-space: nowrap;
    animation: breathe 1.8s var(--ease-in-out) infinite alternate;
  }

  @keyframes breathe {
    to {
      opacity: 0.45;
    }
  }

  .more {
    display: flex;
    flex: none;
    align-items: center;
    gap: 4px;
    padding-inline: 10px;
    border-radius: var(--radius-row);
  }

  .more :global(svg) {
    color: var(--color-faint);
    transition: rotate var(--motion-base) var(--ease-smooth);
  }

  .more.expanded :global(svg) {
    rotate: 180deg;
  }

  .sections {
    display: grid;
    grid-template-columns: 1fr 1fr;
    padding-block-end: 4px;
  }

  .sections button {
    overflow: hidden;
    padding-inline: 10px;
    border-radius: var(--radius-control-compact);
  }

  .sections span,
  .foot span {
    display: block;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  /* What the site asks of the person, apart from its sections: one quiet
     line of links under a hairline at the card's foot. */
  .foot {
    box-sizing: border-box;
    display: flex;
    flex: none;
    gap: 4px;
    margin-inline: 4px;
    padding-block-start: 4px;
    border-block-start: 1px solid var(--color-border);
  }

  .foot button {
    flex: none;
    overflow: hidden;
    padding-inline: 6px;
    border-radius: var(--radius-control-compact);
    color: var(--color-muted);
  }

  .ways button:hover,
  .more:hover,
  .sections button:hover,
  .foot button:hover {
    background: var(--color-fill-hover);
    color: var(--color-text);
  }

  @media (prefers-reduced-motion: reduce) {
    .reading {
      animation: none;
    }
  }

  :global(:root[data-reduce-motion="true"]) .reading {
    animation: none;
  }
</style>
