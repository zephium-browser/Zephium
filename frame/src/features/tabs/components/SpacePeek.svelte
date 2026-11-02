<script lang="ts" module>
  const ROWS = 24;
</script>

<script lang="ts">
  import { Folder01Icon } from "@hugeicons/core-free-icons";
  import { tabFallbackIcon } from "../lib/tab-icon";
  import type { SpaceView, TabView } from "$shared/ipc/bindings";
  import { favicons } from "$domain/favicons";
  import FavIcon from "$shared/ui/FavIcon";
  import Icon from "$shared/ui/Icon";
  import { recall } from "../lib/space-memory";
  import SpaceEmpty from "./SpaceEmpty.svelte";

  let {
    space,
    side,
    rail = false,
  }: {
    space: SpaceView;
    side: "prev" | "next";
    /** Icons alone, as the rail draws a list. */
    rail?: boolean;
  } = $props();

  type Row = { key: string; depth: number; folder: string | null; tab: TabView | null };

  // What the space showed when it was last in front. One it has not shown yet
  // is a row of quiet placeholders, one per tab it holds.
  const sheet = $derived(recall(space.id));
  const rows = $derived.by(() => {
    if (!sheet) return null;
    const byId: Record<string, TabView> = Object.fromEntries(
      sheet.tabs.map((tab) => [tab.id, tab]),
    );
    const depths: Record<string, number> = {};
    const pinned: Row[] = [];
    const open: Row[] = [];
    for (const node of sheet.nodes) {
      if (node.section === "favorites") continue;
      const depth = node.parent_id === null ? 0 : (depths[node.parent_id] ?? 0) + 1;
      depths[node.id] = depth;
      const row =
        node.kind.type === "folder"
          ? { key: node.id, depth, folder: node.kind.name, tab: null }
          : { key: node.id, depth, folder: null, tab: byId[node.kind.tab_id] ?? null };
      if (row.folder === null && row.tab === null) continue;
      (node.section === "pinned" ? pinned : open).push(row);
    }
    // Only the first screenful is ever in view.
    const shown = pinned.slice(0, ROWS);
    return { pinned: shown, open: open.slice(0, Math.max(0, ROWS - shown.length)) };
  });
  const placeholders = $derived(Math.min(space.tabs, 9));
</script>

{#snippet line(row: Row)}
  {#if rail}
    {#if row.tab}<li class="peek-rail" data-selected={row.tab.id === sheet?.active}>
        <FavIcon
          image={favicons.image(row.tab.icon)}
          tone={favicons.tone(row.tab.icon)}
          size={16}
          lit={row.tab.id === sheet?.active}
          fallback={tabFallbackIcon(row.tab.content)}
        />
      </li>{/if}
  {:else}
    <li
      class="peek-row"
      data-selected={row.tab !== null && row.tab.id === sheet?.active}
      style:padding-inline-start={`${8 + Math.min(row.depth, 8) * 13}px`}
    >
      {#if row.tab}
        <FavIcon
          image={favicons.image(row.tab.icon)}
          tone={favicons.tone(row.tab.icon)}
          size={16}
          lit={row.tab.id === sheet?.active}
          fallback={tabFallbackIcon(row.tab.content)}
        />
        <span class="peek-label">{row.tab.title}</span>
      {:else}
        <Icon icon={Folder01Icon} size={15} /><span class="peek-label">{row.folder}</span>
      {/if}
    </li>
  {/if}
{/snippet}

<div class="peek" data-sheet={side} data-rail={rail || undefined} inert aria-hidden="true">
  {#if rows}
    {#if rows.pinned.length + rows.open.length === 0}
      {#if !rail}<SpaceEmpty {space} />{/if}
    {:else}
      <ul class="peek-list">
        {#each rows.pinned as row (row.key)}{@render line(row)}{/each}
      </ul>
      {#if rows.pinned.length > 0 && rows.open.length > 0}<div class="peek-rule"></div>{/if}
      <ul class="peek-list">
        {#each rows.open as row (row.key)}{@render line(row)}{/each}
      </ul>
    {/if}
  {:else if placeholders === 0}
    {#if !rail}<SpaceEmpty {space} />{/if}
  {:else}
    <ul class="peek-list">
      {#each { length: placeholders } as _, index (index)}
        <li class={rail ? "peek-rail peek-hollow" : "peek-row peek-hollow"}>
          <i></i>{#if !rail}<b style:inline-size={`${42 + ((index * 37) % 38)}%`}></b>{/if}
        </li>
      {/each}
    </ul>
  {/if}
</div>

<style>
  /* One sheet's width beyond the list it neighbours, with a gap between. */
  .peek {
    position: absolute;
    inset-block: 0;
    inline-size: 100%;
    overflow: hidden;
    padding-block: 2px 4px;
    pointer-events: none;
  }

  .peek[data-sheet="prev"] {
    inset-inline-start: calc(-100% - 12px);
  }

  .peek[data-sheet="next"] {
    inset-inline-start: calc(100% + 12px);
  }

  .peek-list {
    display: flex;
    flex-direction: column;
    gap: var(--sidebar-row-gap);
    padding-inline: var(--sidebar-inset);
    padding-block: 2px;
  }

  .peek[data-rail] .peek-list {
    align-items: center;
    padding-block: 4px 8px;
  }

  .peek-rule {
    height: 1px;
    margin: 5px calc(var(--sidebar-inset) + 8px);
    background: var(--color-border);
  }

  .peek-row {
    display: flex;
    align-items: center;
    gap: 10px;
    height: var(--row-sidebar);
    padding-inline-end: 4px;
    border-radius: var(--radius-row);
    color: var(--color-label-secondary);
    font-size: var(--sidebar-row-text);
    font-weight: var(--sidebar-row-weight);
    letter-spacing: -0.005em;
  }

  .peek-row[data-selected="true"] {
    background: var(--row-active);
    box-shadow: var(--row-rim);
    color: var(--color-row-current, var(--color-text));
    font-weight: var(--sidebar-row-weight-current);
  }

  .peek-label {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    white-space: nowrap;
    mask-image: var(--mask-fade-end);
  }

  .peek-rail {
    display: grid;
    place-items: center;
    inline-size: 40px;
    block-size: var(--row-sidebar);
    border-radius: var(--radius-row);
  }

  .peek-rail[data-selected="true"] {
    background: var(--row-active);
    box-shadow: var(--row-rim);
  }

  /* A tab it has not shown yet: its place, kept, until it can be drawn. */
  .peek-hollow i {
    flex: none;
    inline-size: 16px;
    block-size: 16px;
    border-radius: 5px;
    background: var(--color-fill);
  }

  .peek-hollow b {
    block-size: 8px;
    border-radius: var(--radius-capsule);
    background: var(--color-fill);
  }

  .peek-row.peek-hollow {
    padding-inline-start: 8px;
  }
</style>
