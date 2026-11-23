<script lang="ts">
  import { getContext } from "svelte";
  import { Cancel01Icon } from "@hugeicons/core-free-icons";
  import { focusStop } from "$shared/canvas";
  import Icon from "$shared/ui/Icon";
  import * as m from "$shared/i18n/messages";
  import type { Place, Search } from "$domain/home";
  import { homeTop, type HomeTop } from "../../lib/context";
  import { SPLIT, type HomeNode } from "../../lib/layout";
  import PlaceMark from "./PlaceMark.svelte";
  import { clockOf } from "../../lib/time";

  let { data }: { data: { item: Extract<HomeNode, { kind: "compact" }> } } = $props();
  const top = getContext<HomeTop>(homeTop);
  const at = focusStop();
  let item = $derived(data.item);
  let search = $derived(item.search);
  let places = $derived<readonly Place[]>(
    search.kind === "place" ? [search.place] : search.kind === "compare" ? search.places : [],
  );

  const SUMMARY = 6;
  function said(search: Search): string {
    if (search.kind === "pages") return search.pages.map((page) => page.title).join(" · ");
    const place = search.kind === "place" ? search.place : search.places[0];
    if (place.state === "failed") return m.home_failed({ name: place.title || place.host });
    if (place.state === "arriving") return m.home_reading_site({ host: place.host });
    const rows =
      search.kind === "compare"
        ? search.pairs.flatMap((pair) =>
            pair.left && pair.right ? place.rows.filter((row) => row.id === pair.left) : [],
          )
        : place.rows;
    return rows
      .slice(0, SUMMARY)
      .map((row) => row.label)
      .join(", ");
  }
  let summary = $derived(said(search));
  const forget = () => top.intent({ kind: "forget", search: search.id });
  function keys(event: KeyboardEvent) {
    if (event.key !== "Delete" && event.key !== "Backspace") return;
    event.preventDefault();
    event.stopPropagation();
    forget();
  }
</script>

<div
  class="compact"
  style:inline-size={`${item.box.width}px`}
  style:block-size={`${item.box.height}px`}
  style:--split={`${SPLIT}px`}
>
  <button
    type="button"
    class="line"
    aria-expanded="false"
    {@attach at(`s:${search.id}`)}
    onclick={() => top.open(search.id)}
    onfocus={() => top.open(search.id)}
    onkeydown={keys}
  >
    <span class="asked">
      <span class="text" dir="auto">{search.text}</span>
      <span class="time">{clockOf(search.at)}</span>
    </span>
    <span class="held">
      {#each places as place (place.id)}
        <PlaceMark {place} size={16} />
        <span class="name" dir="auto">{place.title || place.host}</span>
      {/each}
      <span class="what" dir="auto">{summary}</span>
    </span>
  </button>
  <button
    type="button"
    class="forget"
    tabindex="-1"
    aria-label={m.home_forget({ text: search.text })}
    onclick={forget}><Icon icon={Cancel01Icon} size={12} strokeWidth={2} /></button
  >
</div>

<style>
  /* A search that is not in front: one line, read across like the open one,
     what was asked on the left and what it held where its place would stand. */
  .compact {
    position: relative;
    display: flex;
    border-radius: var(--radius-card);
    background: var(--color-surface);
    box-shadow: var(--shadow-card);
    transition: background-color var(--motion-instant) var(--ease-smooth);
  }

  .compact:hover {
    background: linear-gradient(var(--row-hover), var(--row-hover)), var(--color-surface);
  }

  .line {
    box-sizing: border-box;
    display: flex;
    flex: 1;
    align-items: center;
    gap: 6px;
    min-inline-size: 0;
    padding: 0 34px 0 0;
    border: 0;
    border-radius: inherit;
    background: none;
    color: var(--color-text);
    font: inherit;
    text-align: start;
    cursor: default;
  }

  /* Removing is there for the line in hand, and quiet. */
  .forget {
    position: absolute;
    inset-block-start: 50%;
    inset-inline-end: 8px;
    display: grid;
    place-items: center;
    inline-size: 20px;
    block-size: 20px;
    padding: 0;
    border: 0;
    border-radius: var(--radius-control-compact);
    background: none;
    color: var(--color-faint);
    opacity: 0;
    translate: 0 -50%;
    cursor: default;
  }

  .forget:hover {
    background: var(--color-fill-hover);
    color: var(--color-text);
  }

  .compact:hover .forget,
  .compact:focus-within .forget {
    opacity: 1;
  }

  .asked {
    display: flex;
    flex: none;
    align-items: baseline;
    gap: 8px;
    box-sizing: border-box;
    inline-size: var(--split);
    padding-inline: 14px 12px;
  }

  .text {
    min-inline-size: 0;
    overflow: hidden;
    font-size: var(--text-body);
    font-weight: 600;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .time {
    flex: none;
    margin-inline-start: auto;
    color: var(--color-faint);
    font-size: var(--text-caption);
    font-variant-numeric: tabular-nums;
  }

  .held {
    display: flex;
    flex: 1;
    align-items: center;
    gap: 6px;
    min-inline-size: 0;
    padding-inline-start: 10px;
  }

  .held > :global(*) {
    flex: none;
  }

  .name {
    max-inline-size: 40%;
    overflow: hidden;
    font-size: var(--text-body);
    font-weight: 500;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .held .what {
    flex: 1;
    min-inline-size: 0;
    overflow: hidden;
    color: var(--color-faint);
    font-size: var(--text-label);
    text-overflow: ellipsis;
    white-space: nowrap;
  }
</style>
