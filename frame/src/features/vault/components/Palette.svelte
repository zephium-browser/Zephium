<!--
  A command palette over the vault: open any file by name (Ctrl+O), file the
  current file into a folder (Ctrl+M), or restore a saved layout.
-->
<script lang="ts" module>
  export type PaletteMode = "open" | "move" | "layout";
  export type PaletteItem = { id: string; title: string; detail: string };
</script>

<script lang="ts">
  import { onMount, tick } from "svelte";
  import * as m from "$shared/i18n/messages";

  let {
    mode,
    placeholder,
    search,
    onpick,
    onclose,
  }: {
    mode: PaletteMode;
    placeholder: string;
    search: (query: string) => Promise<PaletteItem[]>;
    onpick: (item: PaletteItem, query: string, alternate: boolean) => void;
    onclose: () => void;
  } = $props();

  let query = $state("");
  let items = $state.raw<PaletteItem[]>([]);
  let index = $state(0);
  let input = $state<HTMLInputElement | null>(null);
  let generation = 0;

  async function update() {
    const current = ++generation;
    const next = await search(query);
    if (current !== generation) return;
    items = next;
    index = 0;
  }

  onMount(() => {
    void update();
    void tick().then(() => input?.focus());
  });

  function keydown(event: KeyboardEvent) {
    event.stopPropagation();
    if (event.key === "Escape") {
      event.preventDefault();
      onclose();
    } else if (event.key === "ArrowDown") {
      event.preventDefault();
      index = Math.min(items.length - 1, index + 1);
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      index = Math.max(0, index - 1);
    } else if (event.key === "Enter") {
      event.preventDefault();
      const item = items[index];
      if (item) onpick(item, query, event.ctrlKey || event.metaKey);
      else if (mode === "layout" && query.trim())
        onpick({ id: "", title: query.trim(), detail: "" }, query, true);
    }
  }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="scrim" onpointerdown={onclose}></div>
<div class="palette" role="dialog" aria-label={placeholder}>
  <input
    bind:this={input}
    bind:value={query}
    {placeholder}
    aria-label={placeholder}
    role="combobox"
    aria-expanded="true"
    aria-controls="vault-palette-list"
    oninput={() => void update()}
    onkeydown={keydown}
  />
  <ul id="vault-palette-list" role="listbox">
    {#each items as item, at (item.id + item.title)}
      <li role="option" aria-selected={at === index}>
        <button
          type="button"
          class:on={at === index}
          onpointermove={() => (index = at)}
          onclick={(event) => onpick(item, query, event.ctrlKey || event.metaKey)}
        >
          <span class="title">{item.title}</span>
          {#if item.detail}<span class="detail">{item.detail}</span>{/if}
        </button>
      </li>
    {:else}
      <li class="none">{mode === "layout" ? m.vault_layout_save_hint() : m.vault_no_matches()}</li>
    {/each}
  </ul>
  <footer>
    {#if mode === "open"}{m.vault_palette_open_hint()}{:else if mode === "move"}{m.vault_palette_move_hint()}{:else}{m.vault_palette_layout_hint()}{/if}
  </footer>
</div>

<style>
  .scrim {
    position: fixed;
    inset: 0;
    z-index: 50;
    background: var(--color-visor);
  }

  .palette {
    position: fixed;
    inset-block-start: 12vh;
    inset-inline-start: 50%;
    z-index: 51;
    display: flex;
    flex-direction: column;
    inline-size: min(640px, 90vw);
    max-block-size: 60vh;
    overflow: hidden;
    border: 1px solid var(--color-border);
    border-radius: var(--radius-panel);
    background: var(--color-menu);
    box-shadow: var(--shadow-overlay);
    transform: translateX(-50%);
  }

  input {
    block-size: 48px;
    padding-inline: 16px;
    border: 0;
    border-block-end: 1px solid var(--color-border);
    outline: none;
    background: transparent;
    color: var(--color-text);
    font: inherit;
    font-size: 15px;
  }

  ul {
    flex: 1;
    margin: 0;
    padding: 6px;
    overflow: auto;
    list-style: none;
  }

  button {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    inline-size: 100%;
    padding: 6px 10px;
    border: 0;
    border-radius: var(--radius-row);
    background: transparent;
    color: var(--color-text);
    font: inherit;
    text-align: start;
  }

  button.on {
    background: var(--row-active);
    color: var(--row-active-ink);
  }

  .title {
    font-size: 13.5px;
  }

  .detail {
    color: var(--color-muted);
    font-size: 11.5px;
  }

  .none {
    padding: 12px;
    color: var(--color-muted);
    font-size: 13px;
  }

  footer {
    padding: 6px 12px;
    border-block-start: 1px solid var(--color-border);
    color: var(--color-faint);
    font-size: 11px;
  }
</style>
