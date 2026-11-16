<script lang="ts">
  import type { Snippet } from "svelte";
  import Menu, { type MenuEntry } from "$shared/ui/Menu";

  // The host's frame: the panel's own entries, then closing, in one More menu.
  let {
    actions,
    footer,
    children,
    more = [],
    moreLabel = "More",
    onmore,
    onclose,
  }: {
    actions?: Snippet;
    footer?: Snippet;
    children?: Snippet;
    more?: MenuEntry[];
    moreLabel?: string;
    onmore?: (id: string) => void;
    onclose?: () => void;
    [key: string]: unknown;
  } = $props();
  let entries = $derived<MenuEntry[]>([
    ...more,
    { kind: "separator" },
    { kind: "item", id: "frame:close", label: "Close tool" },
  ]);
</script>

<section class="frame">
  <header>
    <h2>Zephium AI</h2>
    {#if actions}{@render actions()}{/if}
    <Menu
      label={moreLabel}
      {entries}
      onselect={(id) => (id === "frame:close" ? onclose?.() : onmore?.(id))}
    >
      {#snippet trigger()}⋯{/snippet}
    </Menu>
  </header>
  <div class="content">
    {#if children}{@render children()}{/if}
  </div>
  {#if footer}{@render footer()}{/if}
</section>

<style>
  .frame {
    display: flex;
    flex: 1;
    flex-direction: column;
    block-size: 100%;
    min-block-size: 0;
    color: var(--color-text);
  }

  header {
    display: flex;
    flex: none;
    align-items: center;
    gap: 8px;
    min-block-size: 48px;
    padding: 6px 12px;
  }

  h2 {
    flex: 1;
    margin: 0;
    font-size: 14px;
    font-weight: 550;
    letter-spacing: -0.015em;
  }

  .content {
    display: flex;
    flex: 1;
    flex-direction: column;
    min-block-size: 0;
    overflow: hidden;
  }
</style>
