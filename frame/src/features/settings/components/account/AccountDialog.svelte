<script lang="ts">
  import type { Snippet } from "svelte";
  import { holdChromeModal } from "$domain/surface";

  let {
    open = $bindable(false),
    title,
    description,
    width = 480,
    children,
  }: {
    open?: boolean;
    title: string;
    description?: string;
    /** The dialog's width in px, before it yields to a narrow window. */
    width?: number;
    children: Snippet;
  } = $props();

  let dialog = $state<HTMLDialogElement>();
  let opener: HTMLElement | null = null;
  const uid = $props.id();

  // Pages leave the screen while it is open, or they would cover it.
  $effect(() => {
    if (open) return holdChromeModal();
  });

  // The platform's own modal: the page behind goes inert, Tab stays inside,
  // and Escape cancels. Focus goes back to what opened it.
  $effect(() => {
    if (!dialog) return;
    if (open && !dialog.open) {
      opener = document.activeElement instanceof HTMLElement ? document.activeElement : null;
      dialog.showModal();
    } else if (!open && dialog.open) dialog.close();
  });

  function closed() {
    open = false;
    if (opener?.isConnected) opener.focus();
    opener = null;
  }
</script>

<dialog
  bind:this={dialog}
  class="settings-modal account-dialog"
  style:--width="{width}px"
  aria-labelledby={`${uid}-title`}
  aria-describedby={description ? `${uid}-body` : undefined}
  onclose={closed}
  onclick={(event) => {
    if (event.target === dialog) open = false;
  }}
>
  {#if open}
    <h2 id={`${uid}-title`}>{title}</h2>
    {#if description}<p id={`${uid}-body`}>{description}</p>{/if}
    {@render children()}
  {/if}
</dialog>

<style>
  .account-dialog {
    box-sizing: border-box;
    inline-size: min(var(--width), calc(100vw - 48px));
    max-block-size: calc(100vh - 48px);
    margin: auto;
    padding: 26px;
    overflow-y: auto;
    border: 1px solid var(--color-border-strong);
    border-radius: var(--radius-card);
    color: var(--color-text);
    background: var(--color-raised);
    box-shadow: var(--shadow-overlay);
  }

  .account-dialog[open] {
    animation: interface-fade var(--motion-base) var(--ease-out);
  }

  h2 {
    margin: 0;
    font-size: 20px;
    font-weight: 550;
    line-height: 1.3;
    letter-spacing: -0.02em;
  }

  p {
    margin: 10px 0 22px;
    color: var(--color-muted);
    font-size: var(--text-label);
    line-height: 1.6;
  }

  .account-dialog :global(footer) {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-block-start: 28px;
  }

  @media (prefers-reduced-motion: reduce) {
    .account-dialog[open] {
      animation: none;
    }
  }
</style>
