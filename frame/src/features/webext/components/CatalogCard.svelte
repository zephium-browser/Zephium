<script lang="ts">
  import Button from "$shared/ui/Button";
  import * as m from "$shared/i18n/messages";
  import type { CatalogEntry } from "../lib/catalog";

  let { entry, installed, onget }: { entry: CatalogEntry; installed: boolean; onget: () => void } =
    $props();
</script>

<li class="card">
  <span class="badge" aria-hidden="true">
    <img src={entry.icon} alt="" />
  </span>
  <span class="text">
    <span class="name">{entry.name}</span>
    <span class="blurb">{entry.blurb()}</span>
  </span>
  {#if installed}
    <span class="installed">{m.webext_already_installed()}</span>
  {:else}
    <Button
      size="compact"
      variant="secondary"
      aria-label={m.webext_get_named({ name: entry.name })}
      onclick={onget}>{m.webext_get()}</Button
    >
  {/if}
</li>

<style>
  .card {
    display: flex;
    align-items: center;
    gap: 10px;
    min-width: 0;
    padding: 10px 10px 10px 12px;
    border: 1px solid var(--color-border);
    border-radius: var(--radius-panel);
    background: var(--color-raised);
  }

  .badge {
    display: grid;
    flex: none;
    place-items: center;
    width: 32px;
    height: 32px;
  }

  .badge img {
    width: 32px;
    height: 32px;
  }

  .text {
    display: flex;
    flex: 1;
    flex-direction: column;
    min-width: 0;
  }

  .name,
  .blurb {
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }

  .name {
    font-size: var(--text-body);
    color: var(--color-text);
  }

  .blurb {
    font-size: var(--text-label);
    color: var(--color-muted);
  }

  .installed {
    flex: none;
    font-size: var(--text-label);
    color: var(--color-faint);
  }
</style>
