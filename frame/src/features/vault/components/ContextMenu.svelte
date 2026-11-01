<!--
  A right-click menu for the explorer. It lives inside the Vault page, which
  fills the chrome view, so it never crosses into page content.
-->
<script lang="ts" module>
  export type ContextItem =
    | { id: string; label: string; hint?: string; danger?: boolean; disabled?: boolean }
    | { separator: true };
</script>

<script lang="ts">
  import { tick } from "svelte";

  let {
    x,
    y,
    items,
    onselect,
    onclose,
  }: {
    x: number;
    y: number;
    items: ContextItem[];
    onselect: (id: string) => void;
    onclose: () => void;
  } = $props();

  let menu = $state<HTMLDivElement | null>(null);
  let place = $state({ left: 0, top: 0 });

  $effect(() => {
    const element = menu;
    if (!element) return;
    const box = element.getBoundingClientRect();
    place = {
      left: Math.max(4, Math.min(x, window.innerWidth - box.width - 4)),
      top: Math.max(4, Math.min(y, window.innerHeight - box.height - 4)),
    };
    void tick().then(() =>
      element.querySelector<HTMLButtonElement>("button:not(:disabled)")?.focus(),
    );
  });

  function keydown(event: KeyboardEvent) {
    const buttons = [...(menu?.querySelectorAll<HTMLButtonElement>("button:not(:disabled)") ?? [])];
    const at = buttons.indexOf(document.activeElement as HTMLButtonElement);
    if (event.key === "Escape") {
      event.preventDefault();
      onclose();
    } else if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      const step = event.key === "ArrowDown" ? 1 : -1;
      buttons[(at + step + buttons.length) % buttons.length]?.focus();
    }
    event.stopPropagation();
  }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
  class="scrim"
  onpointerdown={onclose}
  oncontextmenu={(event) => {
    event.preventDefault();
    onclose();
  }}
></div>
<div
  bind:this={menu}
  class="menu"
  role="menu"
  tabindex="-1"
  style:left="{place.left}px"
  style:top="{place.top}px"
  onkeydown={keydown}
>
  {#each items as item, index (index)}
    {#if "separator" in item}
      <div class="separator" role="separator"></div>
    {:else}
      <button
        type="button"
        role="menuitem"
        class:danger={item.danger}
        disabled={item.disabled}
        onclick={() => {
          onclose();
          onselect(item.id);
        }}
      >
        <span>{item.label}</span>
        {#if item.hint}<kbd>{item.hint}</kbd>{/if}
      </button>
    {/if}
  {/each}
</div>

<style>
  .scrim {
    position: fixed;
    inset: 0;
    z-index: 40;
  }

  .menu {
    position: fixed;
    z-index: 41;
    display: flex;
    flex-direction: column;
    min-inline-size: 210px;
    padding: 4px;
    border: 1px solid var(--color-border);
    border-radius: var(--radius-menu);
    background: var(--color-menu);
    box-shadow: var(--shadow-popover);
  }

  button {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
    block-size: 28px;
    padding-inline: 10px;
    border: 0;
    border-radius: var(--radius-row);
    background: transparent;
    color: var(--color-text);
    font: inherit;
    font-size: 13px;
    text-align: start;
  }

  button:hover,
  button:focus-visible {
    outline: none;
    background: var(--row-hover);
  }

  button:disabled {
    color: var(--color-faint);
  }

  button.danger {
    color: var(--color-danger);
  }

  kbd {
    color: var(--color-muted);
    font: inherit;
    font-size: 11px;
  }

  .separator {
    block-size: 1px;
    margin: 4px 6px;
    background: var(--color-menu-separator);
  }
</style>
