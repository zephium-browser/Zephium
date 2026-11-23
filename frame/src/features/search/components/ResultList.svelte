<script lang="ts">
  import { untrack } from "svelte";
  import * as m from "$shared/i18n/messages";
  import type { SurfaceRow } from "../lib/search-surface.svelte";
  import type { ResultSection } from "../lib/search-model";
  import { groupRows } from "../lib/search-model";
  import ResultRow from "./ResultRow.svelte";

  let {
    rows,
    selected,
    query,
    listId,
    busy,
    onhover,
    onrun,
    variant = "compact",
    ref = $bindable(),
  }: {
    rows: SurfaceRow[];
    selected: string | null;
    /** The query the rows answer, not the live text. See `answered`. */
    query: string;
    listId: string;
    busy: boolean;
    onhover: (id: string) => void;
    /** `background` is set for a Command/Ctrl-click, as on a link. */
    onrun: (row: SurfaceRow, background: boolean) => void;
    variant?: "compact" | "launcher";
    ref?: HTMLElement;
  } = $props();

  const sectionLabels: Record<ResultSection, () => string> = {
    search: m.panel_search_mode,
    tabs: m.search_tabs,
    history: m.search_history,
    notes: m.search_notes,
    commands: m.search_commands,
    destinations: m.launcher_destinations,
    calculator: m.launcher_kind_calculator,
  };
  let groups = $derived(groupRows(rows));
  // A heading over the only group on screen names something the user can
  // already see. Headings earn their line once the list mixes sources. The
  // launcher names each row's kind at its edge instead.
  let headed = $derived(variant === "compact" && groups.length > 1);

  // The launcher draws its selection once and moves it, rather than filling
  // one row and clearing another: a transform on a single layer, which is
  // what makes arrowing through the list feel continuous.
  let glide = $state<{ y: number; height: number } | null>(null);
  let gliding = $state(false);
  $effect(() => {
    if (variant !== "launcher") return;
    void rows;
    const target = selected;
    const row = target
      ? ref?.querySelector<HTMLElement>('[role="option"][aria-selected="true"]')
      : null;
    if (!row) {
      glide = null;
      gliding = false;
      return;
    }
    const next = { y: row.offsetTop, height: row.offsetHeight };
    // Arrives in place the first time; only later changes travel.
    gliding = untrack(() => glide) !== null;
    glide = next;
  });
</script>

<div
  id={listId}
  class={variant}
  bind:this={ref}
  role="listbox"
  aria-label={m.panel_results()}
  aria-busy={busy}
>
  {#if glide}<div
      class="glide"
      class:gliding
      aria-hidden="true"
      style:transform={`translateY(${glide.y}px)`}
      style:height={`${glide.height}px`}
    ></div>{/if}
  <!-- A section can recur: typed commands, then a destination, then the
       capture row, which is a command again. Its name alone is not a key. -->
  {#each groups as group, at (`${at}:${group.section}`)}
    <div
      role="group"
      aria-label={sectionLabels[group.section]()}
      aria-labelledby={headed ? `${listId}-${at}` : undefined}
    >
      {#if headed}<div id={`${listId}-${at}`} class="heading" role="presentation">
          {sectionLabels[group.section]()}
        </div>{/if}
      <!-- The row searching for what is typed changes its address with every
           key; it is held as one row whose words change, so typing reads as
           the line updating rather than a new line each time. -->
      {#each group.rows as row (row.result?.kind === "search" ? `search:${rows.indexOf(row)}` : row.id)}
        {@const index = rows.indexOf(row)}
        <ResultRow
          id={`${listId}-option-${index}`}
          title={row.title}
          result={row.result}
          icon={row.icon}
          keys={row.keys}
          calculation={row.calculation}
          detail={row.detail}
          kind={row.kind}
          {index}
          setsize={rows.length}
          selected={selected === row.id}
          {query}
          onhover={() => onhover(row.id)}
          onrun={(event) => onrun(row, event.metaKey || event.ctrlKey)}
          {variant}
        />
      {/each}
    </div>
  {/each}
</div>

<style>
  .heading {
    font-size: 11px;
    font-weight: 550;
    letter-spacing: 0.02em;
    color: var(--color-faint);
    padding-inline: 10px;
    margin-block: 10px 4px;
    /* stylelint-disable-next-line property-no-vendor-prefix */
    -webkit-user-select: none;
    user-select: none;
  }

  .launcher {
    position: relative;
  }

  .glide {
    position: absolute;
    inset-inline: 0;
    top: 0;
    border-radius: var(--row-radius, var(--radius-card));
    background: var(--color-fill-active);
    pointer-events: none;
    will-change: transform;
  }

  .glide.gliding {
    transition:
      transform var(--motion-fast) var(--ease-out),
      height var(--motion-fast) var(--ease-out);
  }

  :global(:root[data-theme="light"]) .glide {
    background: var(--color-fill-pressed);
  }

  div[role="group"]:first-child .heading {
    margin-block-start: 2px;
  }
</style>
