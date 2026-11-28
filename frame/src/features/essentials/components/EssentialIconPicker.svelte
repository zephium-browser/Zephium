<script lang="ts">
  import * as m from "$shared/i18n/messages";
  import type { SpaceIconView } from "$shared/ipc/bindings";
  import { SPACE_SYMBOLS, emojiOf } from "$shared/lib/space-symbols";
  import { tabs } from "$domain/tabs";
  import Icon from "$shared/ui/Icon";
  import { UnavailableIcon } from "@hugeicons/core-free-icons";
  import { untrack } from "svelte";

  let {
    id,
    title,
    current,
    onclose,
  }: {
    id: string;
    title: string;
    current: SpaceIconView | null;
    onclose: () => void;
  } = $props();

  // Work locally so typing an emoji does not send a command on each key.
  let icon = $state<SpaceIconView | null>(untrack(() => current));
  let emoji = $state(untrack(() => (current?.kind === "emoji" ? current.text : "")));
  let pending = $state(false);

  function choose(next: SpaceIconView | null) {
    icon = next;
  }

  function typeEmoji(text: string) {
    emoji = text;
    const found = emojiOf(text);
    if (found) choose({ kind: "emoji", text: found });
  }

  async function done() {
    if (pending) return;
    pending = true;
    await tabs.setCustomIcon(id, icon);
    pending = false;
    onclose();
  }
</script>

<!--
  The same marks Spaces offer, in a small sheet above the dock. The page's
  title stays in the tile's tooltip and accessible name: this is only what
  the tile draws.
-->
<div class="icon-picker" role="dialog" aria-modal="true" aria-label={m.essential_icon_title()}>
  <form
    onsubmit={(event) => {
      event.preventDefault();
      void done();
    }}
  >
    <header>
      <h2>{m.essential_icon_title()}</h2>
      <p>{title}</p>
    </header>
    <div class="group" role="radiogroup" aria-label={m.space_icon()}>
      <input
        class="emoji-field"
        value={emoji}
        oninput={(event) => typeEmoji(event.currentTarget.value)}
        placeholder={m.space_emoji_placeholder()}
        aria-label={m.space_emoji_placeholder()}
        maxlength={16}
      />
      <div class="symbols">
        <button
          type="button"
          role="radio"
          class="symbol symbol-none"
          aria-checked={icon === null}
          aria-label={m.space_icon_none()}
          title={m.space_icon_none()}
          onclick={() => choose(null)}><Icon icon={UnavailableIcon} size={15} /></button
        >
        {#each SPACE_SYMBOLS as symbol, index (index)}
          <button
            type="button"
            role="radio"
            class="symbol"
            aria-checked={icon?.kind === "symbol" && icon.index === index}
            aria-label={m.space_symbol({ index: String(index + 1) })}
            title={m.space_symbol({ index: String(index + 1) })}
            onclick={() => choose({ kind: "symbol", index })}
            ><Icon icon={symbol} size={15} /></button
          >
        {/each}
      </div>
    </div>
    <footer>
      <button type="button" class="cancel" onclick={onclose}>{m.action_cancel()}</button>
      <button type="submit" class="save" disabled={pending}>{m.space_done()}</button>
    </footer>
  </form>
</div>

<style>
  .icon-picker {
    position: fixed;
    inset-block-end: 84px;
    inset-inline-start: 50%;
    z-index: 100;
    transform: translateX(-50%);
    inline-size: min(340px, calc(100% - 32px));
    box-sizing: border-box;
    padding: 14px;
    border: 1px solid var(--color-border);
    border-radius: var(--radius-panel);
    background: var(--color-surface);
    box-shadow: var(--shadow-sheet);
    color: var(--color-text);
  }

  h2 {
    margin: 0;
    font-size: var(--text-label);
    font-weight: 600;
  }

  header p {
    margin: 2px 0 10px;
    overflow: hidden;
    color: var(--color-muted);
    font-size: var(--text-caption);
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .emoji-field {
    box-sizing: border-box;
    inline-size: 100%;
    margin-block-end: 8px;
    padding: 6px 8px;
    border: 1px solid var(--color-border);
    border-radius: var(--radius-control);
    background: transparent;
    color: inherit;
    font: inherit;
  }

  .symbols {
    display: grid;
    grid-template-columns: repeat(7, 1fr);
    gap: 4px;
  }

  .symbol {
    display: grid;
    aspect-ratio: 1;
    place-items: center;
    border: 1px solid transparent;
    border-radius: var(--radius-control);
    background: transparent;
    color: var(--color-muted);
    cursor: default;
  }

  .symbol:hover {
    background: var(--row-hover);
  }

  .symbol[aria-checked="true"] {
    border-color: var(--color-border-strong);
    background: var(--color-fill-active);
    color: var(--color-text);
  }

  footer {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-block-start: 12px;
  }

  footer button {
    padding: 5px 12px;
    border: 1px solid var(--color-border);
    border-radius: var(--radius-control);
    background: transparent;
    color: inherit;
    cursor: default;
    font: inherit;
  }

  footer .save {
    border-color: var(--color-lit);
    background: var(--color-lit);
    color: var(--color-on-lit);
  }
</style>
