<script lang="ts">
  import { untrack } from "svelte";
  import {
    Cancel01Icon,
    Delete02Icon,
    Download04Icon,
    FolderOpenIcon,
  } from "@hugeicons/core-free-icons";
  import {
    DownloadSession,
    downloadProgress,
    filenameParts,
    finished,
    formatDownloadBytes,
  } from "$domain/downloads";
  import type { DownloadError, DownloadState, DownloadView } from "$shared/ipc/bindings";
  import { IS_MAC } from "$shared/platform";
  import Button from "$shared/ui/Button";
  import EmptyState from "$shared/ui/EmptyState";
  import Icon from "$shared/ui/Icon";
  import IconButton from "$shared/ui/IconButton";
  import * as m from "$shared/i18n/messages";
  import FileGlyph from "./FileGlyph.svelte";
  import { TransferRate, transferLine } from "../lib/transfer";
  import { downloadDetail, downloadErrors, downloadReason } from "../lib/errors";

  let { profile }: { profile: string } = $props();
  let session = $state.raw(untrack(() => new DownloadSession(profile)));
  $effect(() => {
    const next = new DownloadSession(profile);
    session = next;
    void next.start();
    return () => next.stop();
  });

  const labels: Record<DownloadState, () => string> = {
    pending: m.download_pending,
    receiving: m.download_receiving,
    paused: m.download_paused,
    cancelling: m.download_cancelling,
    finalizing: m.download_finalizing,
    completed: m.download_completed,
    cancelled: m.download_cancelled,
    interrupted: m.download_interrupted,
    failed: m.download_failed,
  };
  /** Only a folder problem has a fix here; anything else is the site's to retry. */
  const folderProblem = (error: DownloadError | null) =>
    error === "permission" ||
    error === "destination" ||
    error === "disk_full" ||
    error === "file_too_large";

  const rates = new TransferRate();
  const day = new Intl.DateTimeFormat(undefined, { dateStyle: "medium" });

  function dayOf(entry: DownloadView) {
    const at = new Date(Number(entry.created_at) * 1000);
    const today = new Date();
    const yesterday = new Date(today.getFullYear(), today.getMonth(), today.getDate() - 1);
    if (at.toDateString() === today.toDateString()) return m.history_today();
    if (at.toDateString() === yesterday.toDateString()) return m.history_yesterday();
    return day.format(at);
  }

  /** Newest first, under the day each began. */
  let days = $derived.by(() => {
    rates.retain(session.entries.map((entry) => entry.id));
    const grouped: { label: string; entries: DownloadView[] }[] = [];
    const newest = session.entries.toSorted((a, b) => Number(b.created_at) - Number(a.created_at));
    for (const entry of newest) {
      const label = dayOf(entry);
      const last = grouped.at(-1);
      if (last?.label === label) last.entries.push(entry);
      else grouped.push({ label, entries: [entry] });
    }
    return grouped;
  });
  let clearable = $derived(session.entries.some(finished));

  function hostOf(source: string) {
    try {
      return new URL(source).host || source;
    } catch {
      return source;
    }
  }

  function line(entry: DownloadView) {
    if (entry.state === "receiving") {
      const rate = rates.observe(entry.id, Number(entry.received));
      return transferLine(entry, rate) || labels.receiving();
    }
    if (entry.state === "failed" || entry.state === "interrupted" || entry.state === "paused")
      return entry.error
        ? downloadReason(entry.error, entry.state === "paused")
        : labels[entry.state]();
    const where = entry.source_is_context
      ? m.download_source_context({ origin: entry.source })
      : hostOf(entry.source);
    if (entry.state === "completed")
      return `${formatDownloadBytes(entry.total ?? entry.received)} · ${where}`;
    return `${labels[entry.state]()} · ${where}`;
  }
</script>

{#snippet name(entry: DownloadView)}
  {@const parts = filenameParts(entry.filename)}
  <span class="name" title={entry.filename}
    ><span class="stem">{parts.stem}</span>{#if parts.extension}<span class="extension"
        >{parts.extension}</span
      >{/if}</span
  >
{/snippet}

<section
  class="downloads-list"
  aria-label={m.browser_downloads_title()}
  aria-busy={session.loading}
>
  {#if session.error}
    <div class="notice" role="alert">
      <p>{downloadErrors[session.error]()}</p>
      <Button size="compact" variant="ghost" onclick={() => void session.retry()}
        >{m.surface_retry()}</Button
      >
    </div>
  {/if}
  {#if session.cleanup.error}
    <div class="notice" role="alert">
      <p>{m.download_cleanup_error()}</p>
      <Button
        size="compact"
        variant="ghost"
        disabled={session.busy || session.cleanup.running}
        onclick={() => void session.perform({ kind: "retry_cleanup" })}
      >
        {session.cleanup.running ? m.download_cleanup_running() : m.download_cleanup_retry()}
      </Button>
    </div>
  {/if}
  {#if !session.supported}<p class="notice" role="status">{m.download_error_unsupported()}</p>{/if}
  {#if !session.entries.length && !session.error}
    {#if session.loading}<p class="loading" role="status">{m.download_loading()}</p>
    {:else}<EmptyState title={m.download_empty_title()} description={m.download_empty()}>
        {#snippet icon()}<Icon icon={Download04Icon} size={22} />{/snippet}
      </EmptyState>{/if}
  {/if}
  {#each days as group, index (group.label)}
    <div class="day">
      <h3>{group.label}</h3>
      {#if index === 0 && clearable}<Button
          size="compact"
          variant="ghost"
          aria-label={m.download_clear_label()}
          disabled={session.busy}
          onclick={() => void session.perform({ kind: "clear" })}>{m.download_clear()}</Button
        >{/if}
    </div>
    <ul>
      {#each group.entries as entry (entry.id)}
        {@const progress = downloadProgress(entry)}
        {@const problem =
          entry.state === "failed" || entry.state === "interrupted" || entry.state === "paused"}
        <li data-state={entry.state}>
          {#if entry.state === "completed"}<button
              type="button"
              class="body"
              aria-label={m.download_open_named({ name: entry.filename })}
              disabled={session.busy}
              onclick={() => void session.perform({ kind: "open", id: entry.id })}
            >
              <FileGlyph filename={entry.filename} />
              <span class="copy">{@render name(entry)}<span class="line">{line(entry)}</span></span>
            </button>{:else}<div class="body">
              <FileGlyph filename={entry.filename} alert={problem} />
              <span class="copy"
                >{@render name(entry)}<span
                  class="line"
                  title={entry.error
                    ? downloadDetail(entry.error, entry.state === "paused")
                    : undefined}>{line(entry)}</span
                >
                {#if entry.state === "receiving" || entry.state === "pending"}<span
                    class="track"
                    role="progressbar"
                    aria-label={m.download_progress({ name: entry.filename })}
                    aria-valuemin={0}
                    aria-valuemax={100}
                    aria-valuenow={progress === undefined ? undefined : Math.round(progress * 100)}
                    data-indeterminate={progress === undefined}
                    ><i style:transform={progress === undefined ? undefined : `scaleX(${progress})`}
                    ></i></span
                  >{/if}
                {#if problem && folderProblem(entry.error)}<span class="fixes">
                    {#if entry.error === "permission" && IS_MAC}<Button
                        size="compact"
                        variant="secondary"
                        onclick={() => session.openAccessSettings()}
                        >{m.download_allow_access()}</Button
                      >{/if}<Button
                      size="compact"
                      variant="ghost"
                      disabled={session.busy}
                      onclick={() => void session.perform({ kind: "choose_directory" })}
                      >{m.download_change_folder()}</Button
                    ></span
                  >{/if}</span
              >
            </div>{/if}
          <div class="actions">
            {#if entry.state === "paused"}<Button
                size="compact"
                variant="secondary"
                disabled={session.busy}
                onclick={() => void session.perform({ kind: "resume", id: entry.id })}
                >{m.download_resume()}</Button
              >{/if}
            {#if entry.state === "pending" || entry.state === "receiving" || entry.state === "paused"}<IconButton
                icon={Cancel01Icon}
                label={m.download_cancel()}
                size={14}
                buttonSize={26}
                disabled={session.busy}
                onclick={() => void session.perform({ kind: "cancel", id: entry.id })}
              />{/if}
            {#if entry.state === "completed"}<IconButton
                icon={FolderOpenIcon}
                label={m.download_reveal()}
                size={15}
                buttonSize={26}
                disabled={session.busy}
                onclick={() => void session.perform({ kind: "reveal", id: entry.id })}
              />{/if}
            {#if finished(entry)}<IconButton
                icon={Delete02Icon}
                label={m.download_forget()}
                size={15}
                buttonSize={26}
                disabled={session.busy}
                onclick={() => void session.perform({ kind: "forget", id: entry.id })}
              />{/if}
          </div>
        </li>
      {/each}
    </ul>
  {/each}
  {#if session.next}<Button
      variant="ghost"
      disabled={session.loading || session.entries.length >= 2000}
      onclick={() => void session.reload(true)}>{m.download_more()}</Button
    >{/if}
</section>

<style>
  .downloads-list {
    display: grid;
    align-content: start;
    gap: 2px;
    min-width: 0;
  }

  .notice {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 10px;
    margin: 4px 0 6px;
    padding: 8px 8px 8px 12px;
    border-radius: var(--radius-row);
    background: var(--color-fill);
    color: var(--color-text);
    font-size: var(--text-label);
  }

  .notice p,
  p.notice {
    margin: 0;
  }

  .loading {
    margin: 0;
    padding: 12px 8px;
    color: var(--color-muted);
    font-size: var(--text-label);
  }

  .day {
    display: flex;
    align-items: center;
    justify-content: space-between;
    min-height: 28px;
    margin: 8px 0 2px;
    padding-inline: 8px 2px;
  }

  .day:first-of-type {
    margin-block-start: 0;
  }

  h3 {
    margin: 0;
    color: var(--color-faint);
    font-size: 11px;
    font-weight: 550;
  }

  ul {
    display: grid;
    gap: 1px;
    margin: 0;
    padding: 0;
    list-style: none;
  }

  li {
    position: relative;
    display: flex;
    align-items: center;
    gap: 4px;
    padding-inline-end: 6px;
    border-radius: var(--radius-row);
    transition: background-color var(--motion-instant) var(--ease-smooth);
  }

  li:hover,
  li:focus-within {
    background: var(--row-hover);
  }

  .body {
    display: flex;
    flex: 1;
    align-items: center;
    gap: 11px;
    min-width: 0;
    padding: 8px;
    border: 0;
    border-radius: inherit;
    background: none;
    color: inherit;
    font: inherit;
    text-align: start;
  }

  /* With fixes under it the glyph stays beside the name, not mid-block. */
  .body:has(.fixes) {
    align-items: flex-start;
  }

  button.body {
    cursor: default;
  }

  button.body:focus-visible {
    outline: 2px solid var(--color-ring);
    outline-offset: -2px;
  }

  button.body:active:not(:disabled) {
    scale: 0.99;
  }

  .copy {
    display: grid;
    flex: 1;
    gap: 2px;
    min-width: 0;
  }

  /* Truncate the stem, never the extension: "Terax_0.8.6_aarc….dmg". */
  .name {
    display: flex;
    min-width: 0;
    color: var(--color-text);
    font-size: var(--text-label);
    font-weight: 500;
    white-space: nowrap;
  }

  .stem {
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .extension {
    flex: none;
  }

  .line {
    overflow: hidden;
    color: var(--color-muted);
    font-size: 11.5px;
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
    text-overflow: ellipsis;
  }

  li:is([data-state="cancelled"], [data-state="failed"], [data-state="interrupted"]) .name {
    color: var(--color-muted);
  }

  .track {
    position: relative;
    height: 3px;
    margin-block-start: 5px;
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

  .fixes {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
    margin-block-start: 6px;
    margin-inline-start: -2px;
  }

  /* Faded rather than hidden, so the keyboard can still reach them. */
  .actions {
    display: flex;
    flex: none;
    gap: 2px;
    opacity: 0;
    transition: opacity var(--motion-instant) var(--ease-smooth);
  }

  li:hover .actions,
  li:focus-within .actions,
  li[data-state="receiving"] .actions,
  li[data-state="pending"] .actions {
    opacity: 1;
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
    li,
    .actions,
    .track i {
      transition: none;
    }

    button.body:active:not(:disabled) {
      scale: none;
    }

    .track[data-indeterminate="true"] i {
      animation: none;
    }
  }
</style>
