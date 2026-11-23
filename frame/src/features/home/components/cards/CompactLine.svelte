<script lang="ts">
  import { getContext } from "svelte";
  import { focusStop, thumbnail } from "$shared/canvas";
  import FavIcon from "$shared/ui/FavIcon";
  import * as m from "$shared/i18n/messages";
  import { homeTop, type HomeTop } from "../../lib/context";
  import { SPLIT, type HomeNode } from "../../lib/layout";
  import type { Place, Search } from "../../lib/model";
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
  const PICTURES = 3;
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
  let pictures = $derived(
    (search.kind === "pages"
      ? search.pages.map((page) => page.picture)
      : places.flatMap((place) => place.rows.map((row) => (row.opened ? row.picture : null)))
    )
      .filter((key): key is string => key !== null)
      .slice(0, PICTURES)
      .map((key) => top.picture(key))
      .filter((url): url is string => url !== null),
  );
  let missing = $state<readonly string[]>([]);
</script>

<button
  type="button"
  class="compact"
  aria-expanded="false"
  style:inline-size={`${item.box.width}px`}
  style:block-size={`${item.box.height}px`}
  style:--split={`${SPLIT}px`}
  {@attach at(`s:${search.id}`)}
  onclick={() => top.open(search.id)}
  onfocus={() => top.open(search.id)}
>
  <span class="asked">
    <span class="text" dir="auto">{search.text}</span>
    <span class="time">{clockOf(search.at)}</span>
  </span>
  <span class="held">
    {#each places as place (place.id)}
      {@const mark = top.mark(place)}
      {#if mark}<FavIcon image={mark} size={16} lit />{/if}
      <span class="name" dir="auto">{place.title || place.host}</span>
    {/each}
    <span class="what" dir="auto">{summary}</span>
  </span>
  {#each pictures as url (url)}
    {#if !missing.includes(url)}<canvas
        class="shot"
        aria-hidden="true"
        use:thumbnail={{ url, width: 30, onmissing: () => (missing = [...missing, url]) }}
      ></canvas>{/if}
  {/each}
</button>

<style>
  /* A search that is not in front: one line, read across like the open one,
     what was asked on the left and what it held where its place would stand. */
  .compact {
    box-sizing: border-box;
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 0 10px 0 0;
    border: 0;
    border-radius: var(--radius-card);
    background: var(--color-surface);
    box-shadow: var(--shadow-card);
    color: var(--color-text);
    font: inherit;
    text-align: start;
    cursor: default;
    transition: background-color var(--motion-instant) var(--ease-smooth);
  }

  .compact:hover {
    background: linear-gradient(var(--row-hover), var(--row-hover)), var(--color-surface);
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

  .shot {
    flex: none;
    inline-size: 30px;
    block-size: 19px;
    border-radius: 4px;
    background: var(--color-fill);
  }
</style>
