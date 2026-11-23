<script lang="ts">
  import { getContext } from "svelte";
  import { focusStop } from "$shared/canvas";
  import { homeTop, type HomeTop } from "../../lib/context";
  import type { HomeNode } from "../../lib/layout";

  let { id, data }: { id: string; data: { item: Extract<HomeNode, { kind: "page" }> } } = $props();
  const top = getContext<HomeTop>(homeTop);
  const at = focusStop();
  let item = $derived(data.item);
  let page = $derived(item.search.kind === "pages" ? item.search.pages[item.page]! : null);
</script>

{#if page}
  <button
    type="button"
    class="page"
    style:inline-size={`${item.box.width}px`}
    style:block-size={`${item.box.height}px`}
    aria-disabled={!page.tab || undefined}
    {@attach at(id)}
    onclick={() => top.intent({ kind: "page", search: item.search.id, page: page.id })}
  >
    <span class="words">
      <span class="title" dir="auto">{page.title}</span>
      <span class="host" dir="auto">{page.host}</span>
    </span>
  </button>
{/if}

<style>
  .page {
    box-sizing: border-box;
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 0 10px 0 14px;
    border: 0;
    border-radius: var(--radius-card);
    background: var(--color-surface);
    box-shadow: var(--shadow-card);
    color: var(--color-text);
    font: inherit;
    text-align: start;
    cursor: default;
  }

  .words {
    display: grid;
    flex: 1;
    gap: 2px;
    min-inline-size: 0;
  }

  .title,
  .host {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .title {
    font-size: var(--text-body);
    font-weight: 500;
  }

  .host {
    color: var(--color-faint);
    font-size: var(--text-label);
  }
</style>
