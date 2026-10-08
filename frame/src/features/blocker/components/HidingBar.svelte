<script lang="ts">
  import { hiding } from "$domain/blocker";
  import { tabs } from "$domain/tabs";

  let added = $derived(hiding.added());
  let last = $derived(added.at(-1));

  // Leaving the page ends hiding; the picker belongs to that document.
  let page = $derived(`${tabs.activeId()} ${tabs.activeTab()?.url ?? ""}`);
  $effect(() => {
    void page;
    return () => hiding.finish();
  });

  function onkeydown(event: KeyboardEvent) {
    if (!hiding.isActive()) return;
    if (event.key === "Escape") {
      event.preventDefault();
      hiding.finish();
    } else if (event.key === "z" && (event.metaKey || event.ctrlKey) && !event.shiftKey) {
      event.preventDefault();
      void hiding.undo();
    }
  }
</script>

<svelte:window {onkeydown} />

{#if hiding.isActive()}
  <div class="hiding" role="status" aria-live="polite">
    <span class="text">
      <span class="title"
        >Hiding elements{#if added.length > 0}<span class="count">{added.length}</span>{/if}</span
      >
      <span class="detail"
        >{hiding.isSaving()
          ? "Saving…"
          : last
            ? `${last.label} hidden`
            : "Click anything on the page"}</span
      >
    </span>
    <button
      type="button"
      class="undo"
      disabled={!last || hiding.isSaving()}
      title="Undo (⌘Z)"
      onclick={() => void hiding.undo()}>Undo</button
    >
    <button type="button" class="done" onclick={() => hiding.finish()}>Done</button>
  </div>
{/if}

<style>
  .hiding {
    display: flex;
    flex: none;
    align-items: center;
    gap: 6px;
    margin: 2px 8px 6px;
    padding: 8px 8px 8px 12px;
    border-radius: var(--radius-row);
    background: var(--color-card);
    box-shadow: var(--row-rim);
    animation: rise var(--motion-base) var(--ease-out) both;
  }

  .text {
    display: flex;
    flex: 1;
    flex-direction: column;
    min-width: 0;
  }

  .title {
    display: flex;
    align-items: center;
    gap: 6px;
    color: var(--color-text);
    font-size: var(--text-body);
    font-weight: 500;
    line-height: 18px;
  }

  .count {
    min-width: 16px;
    padding: 0 5px;
    border-radius: var(--radius-capsule);
    background: var(--row-active);
    color: var(--color-muted);
    font-size: 11px;
    font-weight: 500;
    line-height: 16px;
    text-align: center;
    font-variant-numeric: tabular-nums;
  }

  .detail {
    overflow: hidden;
    color: var(--color-muted);
    font-size: var(--text-caption);
    line-height: 16px;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  button {
    flex: none;
    height: 26px;
    padding: 0 10px;
    border: 0;
    border-radius: var(--radius-inset);
    font: inherit;
    font-size: var(--text-caption);
    font-weight: 500;
    cursor: default;
  }

  .undo {
    background: transparent;
    color: var(--color-text);
  }

  .undo:disabled {
    color: var(--color-faint);
  }

  .undo:hover:not(:disabled) {
    background: var(--row-hover);
  }

  .done {
    background: var(--color-lit);
    color: var(--color-on-lit);
  }

  .done:hover {
    background: var(--color-lit-hover);
  }

  @keyframes rise {
    from {
      opacity: 0;
      translate: 0 -4px;
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .hiding {
      animation: none;
    }
  }
</style>
