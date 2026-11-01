<!--
  Recent files in the Windows Downloads folder, under the tree. Enter or a
  click files one into the vault (it is copied; the download stays put).
-->
<script lang="ts">
  import type { VaultEntry, VaultSession } from "$domain/vault";
  import * as m from "$shared/i18n/messages";
  import { badgeFor } from "../lib/kinds";

  let { session, onfile }: { session: VaultSession; onfile: (name: string) => void } = $props();

  let open = $state(false);
  let items = $state.raw<VaultEntry[]>([]);
  let failed = $state(false);

  async function load() {
    const response = await session.call({ kind: "downloads" });
    failed = response.kind !== "entries";
    items = response.kind === "entries" ? response.entries : [];
  }

  $effect(() => {
    if (open) void load();
  });
</script>

<section class="shelf">
  <button
    type="button"
    class="head"
    aria-expanded={open}
    data-keytip="D"
    onclick={() => (open = !open)}
  >
    <span class="chevron" class:open aria-hidden="true">›</span>
    {m.vault_downloads()}
  </button>
  {#if open}
    <ul>
      {#each items as item (item.path)}
        <li>
          <button type="button" title={m.vault_file_download()} onclick={() => onfile(item.path)}>
            <span class="name">{item.name}</span>
            {#if badgeFor(item.ext)}<span class="badge">{badgeFor(item.ext)}</span>{/if}
          </button>
        </li>
      {:else}
        <li class="empty">
          {failed ? m.vault_downloads_unavailable() : m.vault_downloads_empty()}
        </li>
      {/each}
    </ul>
  {/if}
</section>

<style>
  .shelf {
    flex: none;
    max-block-size: 40%;
    overflow: auto;
    padding: 4px 6px 8px;
    border-block-start: 1px solid var(--color-border);
    font-size: 13px;
  }

  button {
    display: flex;
    align-items: center;
    gap: 6px;
    inline-size: 100%;
    block-size: 26px;
    padding-inline: 8px;
    border: 0;
    border-radius: var(--radius-row);
    background: transparent;
    color: var(--color-label-secondary);
    font: inherit;
    text-align: start;
  }

  button:hover {
    background: var(--row-hover);
    color: var(--color-text);
  }

  .head {
    color: var(--color-muted);
    font-size: 11.5px;
    font-weight: 600;
    letter-spacing: 0.03em;
    text-transform: uppercase;
  }

  .chevron {
    transition: transform var(--motion-fast) var(--ease-out);
  }

  .chevron.open {
    transform: rotate(90deg);
  }

  ul {
    margin: 0;
    padding: 0;
    list-style: none;
  }

  .name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .badge {
    flex: none;
    margin-inline-start: auto;
    color: var(--color-faint);
    font-size: 9px;
    font-weight: 600;
  }

  .empty {
    padding: 6px 8px;
    color: var(--color-faint);
  }

  @media (prefers-reduced-motion: reduce) {
    .chevron {
      transition: none;
    }
  }
</style>
