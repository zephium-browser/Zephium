<script lang="ts">
  import { ArrowDown01Icon, PuzzleIcon } from "@hugeicons/core-free-icons";
  import { onMount } from "svelte";
  import type { WebExtensionReview } from "$shared/ipc/bindings";
  import { webext } from "$domain/webext";
  import Button from "$shared/ui/Button";
  import Icon from "$shared/ui/Icon";
  import PromptSheet from "./PromptSheet.svelte";
  import * as m from "$shared/i18n/messages";

  let { review }: { review: WebExtensionReview } = $props();
  let cancelButton = $state<HTMLButtonElement>();
  let busy = $derived(webext.isConfirming());
  let failure = $derived(webext.error());
  let detailed = $state(false);
  let limited = $state(false);
  // The broadest access leads; the rest waits behind "more", which is where
  // a list of every permission belongs in a sheet this narrow.
  let lead = $derived(review.warnings[0] ?? null);
  let rest = $derived(review.warnings.slice(1));

  onMount(() => queueMicrotask(() => cancelButton?.focus()));

  function onkeydown(event: KeyboardEvent) {
    if (event.key === "Escape" && !busy) {
      event.preventDefault();
      event.stopPropagation();
      webext.cancel(review.review_id);
    }
  }
</script>

<svelte:window {onkeydown} />

<PromptSheet labelledby="web-extension-review-title" describedby="web-extension-review-access">
  <div class="head">
    <span class="badge" aria-hidden="true">
      {#if review.icon}<img src={review.icon} alt="" />{:else}<Icon
          icon={PuzzleIcon}
          size={20}
        />{/if}
    </span>
    <div class="title">
      <h1 id="web-extension-review-title">{review.name}</h1>
      <p>{m.webext_review_version({ version: review.version })}</p>
    </div>
  </div>

  <div id="web-extension-review-access" class="access">
    {#if lead === null}
      <p>{m.webext_review_no_access()}</p>
    {:else}
      <p>{lead}</p>
      {#if detailed}
        <ul>
          {#each rest as warning (warning)}<li>{warning}</li>{/each}
        </ul>
      {/if}
      {#if rest.length > 0}
        <button type="button" class="more" onclick={() => (detailed = !detailed)}>
          {detailed ? m.webext_review_less() : m.webext_review_more({ count: rest.length })}
        </button>
      {/if}
    {/if}
    <!-- What won't work is worth knowing, not worth reading first. -->
    {#if review.limitations.length > 0}
      <button
        type="button"
        class="limits"
        aria-expanded={limited}
        aria-controls="web-extension-review-limits"
        onclick={() => (limited = !limited)}
      >
        {m.webext_review_limits()}
        <span class="chevron" aria-hidden="true"><Icon icon={ArrowDown01Icon} size={12} /></span>
      </button>
      {#if limited}
        <ul id="web-extension-review-limits">
          {#each review.limitations as limitation (limitation)}<li>{limitation}</li>{/each}
        </ul>
      {/if}
    {/if}
    {#if review.from_file}<p class="warning">{m.webext_from_file()}</p>{/if}
  </div>

  {#if failure !== null}<p role="alert" class="failure">{failure}</p>{/if}

  <div class="actions">
    <Button
      bind:ref={cancelButton}
      variant="secondary"
      disabled={busy}
      onclick={() => webext.cancel(review.review_id)}>{m.webext_review_cancel()}</Button
    >
    <Button variant="primary" pending={busy} onclick={() => void webext.confirm(review.review_id)}
      >{review.update ? m.webext_review_update() : m.webext_review_add()}</Button
    >
  </div>
</PromptSheet>

<style>
  .head {
    display: flex;
    align-items: center;
    gap: 12px;
  }

  .badge {
    display: grid;
    flex: none;
    place-items: center;
    width: 40px;
    height: 40px;
    border-radius: var(--radius-control);
    background: var(--color-fill);
    color: var(--color-muted);
  }

  .badge img {
    width: 32px;
    height: 32px;
  }

  .title {
    min-width: 0;
  }

  h1 {
    margin: 0;
    overflow: hidden;
    font-size: 14px;
    font-weight: 600;
    line-height: 20px;
    color: var(--color-text);
    white-space: nowrap;
    text-overflow: ellipsis;
  }

  .title p {
    margin: 0;
    font-size: var(--text-label);
    color: var(--color-muted);
  }

  .access {
    margin-block-start: 14px;
    font-size: var(--text-label);
    line-height: 1.45;
    color: var(--color-label-secondary);
  }

  .access p {
    margin: 0;
  }

  .access ul {
    display: flex;
    flex-direction: column;
    gap: 4px;
    margin: 6px 0 0;
    padding-inline-start: 16px;
    color: var(--color-muted);
  }

  .limits {
    display: flex;
    align-items: center;
    gap: 4px;
    margin-block-start: 10px;
    padding: 0;
    border: 0;
    background: none;
    font: inherit;
    color: var(--color-muted);
    text-align: start;
    cursor: default;
  }

  .limits:hover {
    color: var(--color-label-secondary);
  }

  .chevron {
    display: inline-grid;
    flex: none;
    transition: rotate var(--motion-fast) var(--ease-out);
  }

  .limits[aria-expanded="true"] .chevron {
    rotate: 180deg;
  }

  .more {
    margin-block-start: 4px;
    padding: 0;
    border: 0;
    background: none;
    font: inherit;
    color: var(--color-accent);
    cursor: pointer;
  }

  .warning {
    margin-block-start: 8px !important;
    color: var(--color-warning);
  }

  .failure {
    margin: 10px 0 0;
    font-size: var(--text-label);
    color: var(--color-danger);
  }

  .actions {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 8px;
    margin-block-start: 16px;
  }
</style>
