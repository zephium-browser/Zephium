<script lang="ts">
  import { getContext } from "svelte";
  import { ArrowRight01Icon } from "@hugeicons/core-free-icons";
  import { focusStop, thumbnail } from "$shared/canvas";
  import Icon from "$shared/ui/Icon";
  import * as m from "$shared/i18n/messages";
  import { homeTop, type HomeTop } from "../../lib/context";
  import { ROLE_MARK } from "../../lib/marks";
  import type { Row } from "../../lib/model";

  let {
    row,
    stop,
    search,
    height,
    selected = false,
    arriving = null,
    inner = false,
  }: {
    row: Row;
    stop: string;
    search: string;
    height: number;
    selected?: boolean;
    /** Its place in the order rows came in, when it came in after the card was drawn. */
    arriving?: number | null;
    /** A row inside another: it opens its page and nothing more. */
    inner?: boolean;
  } = $props();
  const top = getContext<HomeTop>(homeTop);
  const at = focusStop();
  let picture = $derived(row.picture ? top.picture(row.picture) : null);
  let missing = $state<string | null>(null);
  let shown = $derived(!!row.inside);
  let canOpen = $derived(row.opens && !inner);
  let mark = $derived(row.role && !inner ? ROLE_MARK[row.role] : null);
</script>

<div
  class="row"
  class:selected
  class:visited={row.opened}
  class:arriving={arriving !== null}
  style:block-size={`${height}px`}
  style:--order={arriving ?? 0}
>
  <button
    type="button"
    class="open"
    class:marked={!!mark}
    aria-current={selected ? "true" : undefined}
    {@attach at(stop)}
    onclick={() => top.intent({ kind: "row", search, row: row.id })}
    onpointerenter={() => top.hover(row, stop)}
    onpointerleave={() => top.hover(null)}
  >
    <span class="label" dir="auto">{row.label}</span>
    {#if row.host}<span class="host" dir="auto">{row.host}</span>{/if}
    {#if mark}<Icon icon={mark} size={13} class="mark" />{/if}
    {#if picture && missing !== picture}<canvas
        class="shot"
        aria-hidden="true"
        use:thumbnail={{ url: picture, width: 30, onmissing: () => (missing = picture) }}
      ></canvas>{/if}
  </button>
  {#if canOpen}
    <button
      type="button"
      class="inside"
      class:shown
      tabindex="-1"
      aria-expanded={shown}
      aria-label={shown
        ? m.home_hide_inside({ label: row.label })
        : m.home_open_inside({ label: row.label })}
      onclick={() => top.intent({ kind: shown ? "fold" : "inside", search, row: row.id })}
    >
      <Icon icon={ArrowRight01Icon} size={14} strokeWidth={2} />
    </button>
  {/if}
</div>

<style>
  .row {
    position: relative;
    display: flex;
    align-items: stretch;
    border-radius: var(--radius-row);
    transition: background-color var(--motion-instant) var(--ease-smooth);
  }

  .row:hover {
    background: var(--row-hover);
  }

  .row.selected {
    background: var(--color-fill-active);
  }

  .open {
    display: flex;
    flex: 1;
    align-items: center;
    gap: 8px;
    min-inline-size: 0;
    padding-inline: 10px 8px;
    border: 0;
    border-radius: inherit;
    background: none;
    color: inherit;
    font: inherit;
    font-size: var(--text-body);
    text-align: start;
    cursor: default;
  }

  .label {
    flex: 0 1 auto;
    min-inline-size: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .label:dir(rtl) {
    flex-grow: 1;
  }

  .selected .label {
    font-weight: 600;
  }

  .host {
    flex: none;
    max-inline-size: 42%;
    overflow: hidden;
    color: var(--color-faint);
    font-size: var(--text-label);
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .open :global(.mark) {
    flex: none;
    margin-inline-start: auto;
    color: var(--color-faint);
  }

  .shot {
    flex: none;
    inline-size: 30px;
    block-size: 19px;
    margin-inline-start: auto;
    border-radius: 4px;
    background: var(--color-fill);
  }

  .marked .shot {
    margin-inline-start: 0;
  }

  .inside {
    display: grid;
    flex: none;
    place-items: center;
    inline-size: 28px;
    margin-inline-end: 2px;
    padding: 0;
    border: 0;
    border-radius: var(--radius-row);
    background: none;
    color: var(--color-faint);
    cursor: default;
    transition:
      color var(--motion-instant) var(--ease-smooth),
      background-color var(--motion-instant) var(--ease-smooth);
  }

  .inside:hover {
    background: var(--color-fill-hover);
    color: var(--color-text);
  }

  .inside :global(svg) {
    opacity: 0.7;
  }

  .row:hover .inside :global(svg),
  .selected .inside :global(svg),
  .inside.shown :global(svg) {
    opacity: 1;
  }

  .inside.shown {
    color: var(--color-text);
  }

  .arriving {
    animation: row-in 420ms var(--ease-out) calc(var(--order) * 55ms) backwards;
  }

  @keyframes row-in {
    from {
      opacity: 0;
      translate: 0 4px;
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .arriving {
      animation: none;
    }
  }

  :global(:root[data-reduce-motion="true"]) .arriving {
    animation: none;
  }
</style>
