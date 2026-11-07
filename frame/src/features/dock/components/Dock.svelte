<script lang="ts">
  import type { Snippet } from "svelte";
  import ToolShelf from "./ToolShelf.svelte";

  let {
    sites,
    above,
    compact = false,
    tools = true,
    foot,
  }: {
    sites?: Snippet;
    /** Kept sites that did not fit beside the shelf, in rows over it. */
    above?: Snippet;
    compact?: boolean;
    /** The profile and its tools; Work keeps its own on the canvas. */
    tools?: boolean;
    /** What closes the rail at its very foot, below the kept sites. */
    foot?: Snippet;
  } = $props();
</script>

<!--
  The base of the column. Its bottom row is the mini-apps and as many kept
  sites as fit beside them; the rest of the sites stack in full-width rows
  above, growing upwards, so the shelf never moves and the column above only
  gives up a row when a whole row has been filled.
-->
<!-- The base of the column settles last, once the list above it has. -->
<footer
  class="dock"
  data-compact={compact}
  data-foot={foot ? "" : undefined}
  data-cascade
  style:--cascade={8}
>
  {#if above}<div class="dock-above" data-under-panel>{@render above()}</div>{/if}
  <!-- In the row the profile leads; down the rail it closes the column. -->
  <div class="dock-base">
    {#if tools && !compact}<ToolShelf />
      <span class="dock-rule" aria-hidden="true"></span>{/if}
    {#if sites}<div class="dock-sites-slot">
        {@render sites()}
      </div>{/if}
    {#if tools && compact}<span class="dock-rule" aria-hidden="true"></span><ToolShelf
        compact
      />{/if}
    {#if foot}<span class="dock-rule" aria-hidden="true"></span>{@render foot()}{/if}
  </div>
</footer>

<style>
  .dock {
    flex: none;
    display: flex;
    flex-direction: column;
    gap: var(--dock-gap);
    margin-block-start: 8px;
    padding: 0 var(--sidebar-inset) var(--sidebar-inset);
  }

  .dock-above {
    display: flex;
    flex-direction: column;
    gap: var(--dock-gap);
  }

  .dock-above:not(:has(> :not([hidden]))) {
    display: none;
  }

  .dock-base {
    display: flex;
    align-items: center;
    gap: var(--dock-shelf-gap);
  }

  /* Ours on one side of the rule, the web's on the other. */
  .dock-rule {
    flex: none;
    align-self: center;
    width: 1px;
    height: calc(var(--dock-tile) - 14px);
    border-radius: var(--radius-capsule);
    background: var(--color-border);
  }

  .dock-sites-slot {
    flex: 1;
    min-width: 0;
  }

  /* The rail stacks the same three parts it does at full width. */
  .dock[data-compact="true"] {
    align-items: center;
    gap: 8px;
    padding-block-end: 8px;
  }

  /* Work's window has no bottom gutter of its own, so its foot brings Browse's: the profile stands where it does there. */
  .dock[data-compact="true"][data-foot] {
    padding-block-end: 16px;
  }

  .dock[data-compact="true"] .dock-base {
    flex-direction: column;
    gap: 8px;
  }

  .dock[data-compact="true"] .dock-rule {
    width: 22px;
    height: 1px;
  }

  /* The kept sites are a step of the rail themselves, so with none the rule
     stands one gap below what is above the dock, as the profile stands one
     below the rule. */
  .dock[data-compact="true"] .dock-sites-slot {
    display: contents;
  }
</style>
