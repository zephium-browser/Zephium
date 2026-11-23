<script lang="ts">
  import { getContext } from "svelte";
  import { focusStop } from "$shared/canvas";
  import { homeTop, type HomeTop } from "../../lib/context";
  import type { HomeNode } from "../../lib/layout";
  import { clockOf } from "../../lib/time";

  let { id, data }: { id: string; data: { item: Extract<HomeNode, { kind: "address" }> } } =
    $props();
  const top = getContext<HomeTop>(homeTop);
  const at = focusStop();
  let item = $derived(data.item);
</script>

<button
  type="button"
  class="address"
  style:inline-size={`${item.box.width}px`}
  style:block-size={`${item.box.height}px`}
  {@attach at(id)}
  onclick={() => top.intent({ kind: "address", entry: item.entry.id })}
>
  <span class="host" dir="auto">{item.entry.host}</span>
  <span class="title" dir="auto">{item.entry.title}</span>
  <span class="time">{clockOf(item.entry.at)}</span>
</button>

<style>
  /* An address leaves only a line: no card, just where it went. */
  .address {
    box-sizing: border-box;
    display: flex;
    align-items: center;
    gap: 8px;
    padding-inline: 14px;
    text-align: start;
    border: 0;
    border-radius: var(--radius-card);
    background: var(--color-surface);
    box-shadow: var(--shadow-card);
    color: var(--color-text);
    font: inherit;
    font-size: var(--text-label);
    cursor: default;
  }

  /* Where it went is said whole; what the page was called gives way. */
  .host {
    flex: none;
    max-inline-size: 75%;
    font-weight: 600;
  }

  .host,
  .title {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .title {
    flex: 1;
    min-inline-size: 0;
    color: var(--color-muted);
  }

  .time {
    flex: none;
    color: var(--color-faint);
    font-size: var(--text-caption);
    font-variant-numeric: tabular-nums;
  }
</style>
