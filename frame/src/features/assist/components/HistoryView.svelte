<script lang="ts">
  import * as m from "$shared/i18n/messages";
  import Icon from "$shared/ui/Icon";
  import IconButton from "$shared/ui/IconButton";
  import { getLocale } from "$shared/i18n/runtime";
  import type { AssistSession } from "../lib/assist-session.svelte";
  import { ago } from "../lib/format";
  import { ArrowLeft01Icon, Delete02Icon } from "../lib/icons";

  let { session, onback }: { session: AssistSession; onback: () => void } = $props();

  $effect(() => {
    void session.history();
  });

  const now = Date.now();
  const past = $derived(session.past);

  async function open(id: string) {
    if (await session.openPast(id)) onback();
  }
</script>

<section class="sheet" aria-label={m.assist_history()}>
  <header>
    <button type="button" class="back" onclick={onback}
      ><Icon icon={ArrowLeft01Icon} size={15} strokeWidth={1.9} />{m.assist_history()}</button
    >
  </header>
  {#if past === null}
    <span class="sr-only" role="status">{m.panel_loading()}</span>
  {:else if past.length === 0}
    <p class="empty">{m.assist_history_empty()}</p>
  {:else}
    <ul class="rows">
      {#each past as entry (entry.id)}
        <li class="row" data-current={entry.id === session.conversation.id || undefined}>
          <button type="button" class="open" onclick={() => void open(entry.id)}>
            <span class="title">{entry.title || m.assist_history_untitled()}</span>
            <span class="when">{ago(entry.updated_ms ?? now, now, getLocale())}</span>
          </button>
          <IconButton
            icon={Delete02Icon}
            size={14}
            buttonSize={26}
            label={m.assist_history_delete({ title: entry.title || m.assist_history_untitled() })}
            onclick={() => void session.deletePast(entry.id)}
          />
        </li>
      {/each}
    </ul>
  {/if}
  {#if past !== null}
    <p class="retention">{m.assist_history_retention()}</p>
  {/if}
</section>

<style>
  .sheet {
    display: flex;
    flex: 1;
    flex-direction: column;
    min-block-size: 0;
    overflow: auto;
    padding: 0 10px 12px;
  }

  header {
    padding: 0 0 6px;
  }

  .back {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    block-size: 28px;
    padding: 0 10px 0 6px;
    border: 0;
    border-radius: var(--radius-capsule);
    background: transparent;
    color: var(--color-text);
    font: inherit;
    font-size: 13px;
    font-weight: 550;
    cursor: default;
  }

  .back:hover {
    background: var(--color-fill-hover);
  }

  .back:focus-visible {
    outline: 2px solid var(--color-ring);
  }

  .empty {
    margin: 0;
    padding: 18px 8px;
    color: var(--color-muted);
    font-size: 13px;
  }

  .rows {
    display: flex;
    flex-direction: column;
    gap: 1px;
    margin: 0;
    padding: 0;
    list-style: none;
  }

  .retention {
    margin: 12px 8px 0;
    color: var(--color-muted);
    font-size: 12px;
  }

  .row {
    display: flex;
    align-items: center;
    gap: 2px;
    padding-inline-end: 4px;
    border-radius: var(--radius-row);
    transition: background-color var(--motion-instant) var(--ease-smooth);
  }

  .row:hover,
  .row:focus-within {
    background: var(--row-hover);
  }

  .row[data-current] {
    background: var(--row-active);
  }

  .row :global(button[aria-label]) {
    opacity: 0;
  }

  .row:hover :global(button[aria-label]),
  .row :global(button[aria-label]:focus-visible) {
    opacity: 1;
  }

  .open {
    display: flex;
    flex: 1;
    align-items: baseline;
    gap: 10px;
    min-inline-size: 0;
    min-block-size: 36px;
    padding: 9px 6px 9px 10px;
    border: 0;
    background: transparent;
    color: var(--color-text);
    font: inherit;
    text-align: start;
    cursor: default;
  }

  .open:focus-visible {
    outline: 2px solid var(--color-ring);
    outline-offset: -2px;
    border-radius: var(--radius-row);
  }

  .title {
    flex: 1;
    min-inline-size: 0;
    overflow: hidden;
    font-size: 13.5px;
    line-height: 18px;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .when {
    flex: none;
    color: var(--color-faint);
    font-size: 12px;
    font-variant-numeric: tabular-nums;
  }
</style>
