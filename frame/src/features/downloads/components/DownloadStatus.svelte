<script lang="ts">
  import { untrack } from "svelte";
  import { Cancel01Icon, FolderOpenIcon } from "@hugeicons/core-free-icons";
  import { DownloadSession, downloadProgress, formatDownloadBytes } from "$domain/downloads";
  import type { DownloadState, DownloadView } from "$shared/ipc/bindings";
  import IconButton from "$shared/ui/IconButton";
  import * as m from "$shared/i18n/messages";
  import FileGlyph from "./FileGlyph.svelte";
  import { TransferRate, transferLine } from "../lib/transfer";
  import { downloadReason } from "../lib/errors";

  let { profile, onopen }: { profile: string; onopen: () => void } = $props();
  let session = $state.raw(untrack(() => new DownloadSession(profile)));
  /** The finished download whose card has gone, by id and state. */
  let dismissed = $state<string | null>(null);
  let leaving = $state(false);
  let hovered = $state(false);
  const rates = new TransferRate();
  $effect(() => {
    const next = new DownloadSession(profile);
    session = next;
    // Read one bounded native snapshot, then listen without idle polling.
    void next.start(false);
    return () => next.stop();
  });

  /** How long a finished card stays: long enough to read why one failed. */
  const LINGER: Partial<Record<DownloadState, number>> = {
    completed: 4000,
    cancelled: 3000,
    failed: 8000,
    interrupted: 8000,
  };
  /** After the pointer leaves a card it was resting on. */
  const LINGER_AFTER_HOVER = 2500;
  /** Matches the exit animation below; reduced motion just removes the card. */
  const EXIT = 140;

  const live = (entry: DownloadView) =>
    ["pending", "receiving", "paused", "cancelling", "finalizing"].includes(entry.state);
  let active = $derived(session.entries.filter(live));
  let current = $derived(active[0] ?? session.entries[0]);
  let progress = $derived(current ? downloadProgress(current) : undefined);
  let problem = $derived(
    current?.state === "failed" || current?.state === "interrupted" || current?.state === "paused",
  );
  let rate = $derived.by(() => {
    rates.retain(active.map((entry) => entry.id));
    return current?.state === "receiving"
      ? rates.observe(current.id, Number(current.received))
      : null;
  });
  let line = $derived.by(() => {
    if (!current) return "";
    switch (current.state) {
      case "receiving":
        return transferLine(current, rate) || m.download_receiving();
      case "pending":
        return m.download_pending();
      case "cancelling":
        return m.download_cancelling();
      case "finalizing":
        return m.download_finalizing();
      case "completed":
        return formatDownloadBytes(current.total ?? current.received);
      case "cancelled":
        return m.download_cancelled();
      default:
        return downloadReason(current.error, current.state === "paused");
    }
  });
  let finishedKey = $derived(
    active.length === 0 && current ? `${current.id}:${current.state}` : "",
  );
  let shown = $derived(
    Boolean(current) && (active.length > 0 || (finishedKey !== "" && finishedKey !== dismissed)),
  );

  function dismiss() {
    const key = finishedKey;
    leaving = true;
    setTimeout(() => {
      dismissed = key;
      leaving = false;
    }, EXIT);
  }

  /** The finished card the pointer rested on; it then leaves sooner once let go. */
  let rested = $state<string | null>(null);

  // A finished card leaves on its own; resting the pointer on it holds it.
  $effect(() => {
    const key = finishedKey;
    if (!key || key === dismissed || hovered) return;
    const state = current?.state;
    const wait = rested === key ? LINGER_AFTER_HOVER : ((state && LINGER[state]) ?? 4000);
    const timer = setTimeout(dismiss, wait);
    return () => clearTimeout(timer);
  });

  function enter() {
    hovered = true;
    if (finishedKey) rested = finishedKey;
  }
</script>

{#if current && shown}
  <!-- Hover only pauses the auto-hide. -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div
    class="download-card"
    class:leaving={leaving && active.length === 0}
    data-state={current.state}
    onpointerenter={enter}
    onpointerleave={() => (hovered = false)}
  >
    <button class="body" type="button" onclick={onopen} aria-label={m.download_show_status()}>
      <FileGlyph filename={current.filename} size={30} alert={problem} />
      <span class="copy">
        <strong title={current.filename}>{current.filename}</strong>
        <span class="line"
          >{line}{#if active.length > 1}<span class="more"
              >{m.download_more_active({ count: active.length - 1 })}</span
            >{/if}</span
        >
      </span>
    </button>
    {#if current.state === "pending" || current.state === "receiving" || current.state === "paused"}
      <IconButton
        icon={Cancel01Icon}
        label={m.download_cancel()}
        size={13}
        buttonSize={24}
        disabled={session.busy}
        onclick={() => void session.perform({ kind: "cancel", id: current.id })}
      />
    {:else if current.state === "completed"}
      <IconButton
        icon={FolderOpenIcon}
        label={m.download_reveal()}
        size={14}
        buttonSize={24}
        disabled={session.busy}
        onclick={() => void session.perform({ kind: "reveal", id: current.id })}
      />
    {:else if active.length === 0}
      <IconButton
        icon={Cancel01Icon}
        label={m.download_dismiss()}
        size={13}
        buttonSize={24}
        onclick={dismiss}
      />
    {/if}
    {#if current.state === "pending" || current.state === "receiving"}
      <div
        class="track"
        role="progressbar"
        aria-label={m.download_progress({ name: current.filename })}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={progress === undefined ? undefined : Math.round(progress * 100)}
        data-indeterminate={progress === undefined}
      >
        <i style:transform={progress === undefined ? undefined : `scaleX(${progress})`}></i>
      </div>
    {/if}
  </div>
{/if}

<style>
  .download-card {
    position: relative;
    display: flex;
    flex: none;
    align-items: center;
    gap: 2px;
    margin: 6px 8px;
    padding: 8px 6px 10px 8px;
    overflow: hidden;
    border-radius: var(--radius-row);
    background: var(--row-active);
    box-shadow: var(--row-rim);
    animation: card-in var(--motion-base) var(--ease-emphasized) both;
  }

  .body {
    display: flex;
    flex: 1;
    align-items: center;
    gap: 10px;
    min-width: 0;
    padding: 0;
    border: 0;
    background: none;
    color: inherit;
    font: inherit;
    text-align: start;
    cursor: default;
  }

  .copy {
    display: grid;
    gap: 2px;
    min-width: 0;
  }

  strong {
    overflow: hidden;
    color: var(--color-text);
    font-size: var(--text-label);
    font-weight: 550;
    white-space: nowrap;
    text-overflow: ellipsis;
  }

  .line {
    overflow: hidden;
    color: var(--color-muted);
    font-size: 11px;
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
    text-overflow: ellipsis;
  }

  .more {
    margin-inline-start: 6px;
    color: var(--color-faint);
  }

  .download-card.leaving {
    opacity: 0;
    translate: 0 4px;
    transition:
      opacity var(--motion-fast) var(--ease-exit),
      translate var(--motion-fast) var(--ease-exit);
  }

  .track {
    position: absolute;
    inset-inline: 10px;
    inset-block-end: 4px;
    height: 3px;
    overflow: hidden;
    border-radius: 2px;
    background: var(--color-fill);
  }

  .track i {
    display: block;
    height: 100%;
    border-radius: inherit;
    background: var(--color-accent);
    transform-origin: left center;
    transition: transform var(--motion-base) var(--ease-smooth);
  }

  .track:dir(rtl) i {
    transform-origin: right center;
  }

  .track[data-indeterminate="true"] i {
    width: 35%;
    animation: sweep 1.2s var(--ease-smooth) infinite;
  }

  .body:focus-visible {
    outline: 2px solid var(--color-ring);
    outline-offset: 2px;
    border-radius: var(--radius-inset);
  }

  @keyframes card-in {
    from {
      opacity: 0;
      translate: 0 6px;
    }
  }

  @keyframes sweep {
    from {
      translate: -100% 0;
    }

    to {
      translate: 300% 0;
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .track i {
      transition: none;
    }

    .download-card,
    .track[data-indeterminate="true"] i {
      animation: none;
    }

    .download-card.leaving {
      transition: none;
    }
  }
</style>
