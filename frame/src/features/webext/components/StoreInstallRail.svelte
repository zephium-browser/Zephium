<script lang="ts">
  import { Delete02Icon, Download01Icon } from "@hugeicons/core-free-icons";
  import { extensions } from "$domain/extensions";
  import { tabs } from "$domain/tabs";
  import { surface } from "$domain/surface";
  import { webext } from "$domain/webext";
  import Icon from "$shared/ui/Icon";
  import * as m from "$shared/i18n/messages";

  let listedId = $derived(
    webext.isAvailable() && surface.currentPage() === null
      ? extensions.chromeStoreListingId(tabs.activeTab()?.url ?? "")
      : null,
  );
  let listing = $derived(listedId !== null);
  let installed = $derived(listedId === null ? null : webext.named(listedId));
  // Removal deletes the extension's data, so the rail asks for a second click.
  let armed = $state(false);
  $effect(() => {
    void listedId;
    armed = false;
  });
  $effect(() => {
    if (installed === null || !webext.isRemovalAsked(installed.id)) return;
    armed = true;
    webext.settleRemovalAsk();
  });
  $effect(() => {
    if (!armed) return;
    const timer = setTimeout(() => (armed = false), 3000);
    return () => clearTimeout(timer);
  });
  let failure = $derived(webext.review() === null ? webext.error() : null);
  // Downloading, installing and starting all happen here; the rail has no
  // room for the words the full sidebar shows.
  let busy = $derived(webext.isInstalling());
</script>

<!--
  At rail width the address field, and the Add button under it, are hidden;
  on a Web Store listing the rail carries the button instead.
-->
{#if installed}
  {@const name = installed.name}
  <button
    type="button"
    class="install remove"
    class:armed
    title={armed ? m.webext_store_remove_again({ name }) : m.webext_store_remove()}
    aria-label={armed ? m.webext_store_remove_again({ name }) : m.webext_store_remove()}
    onclick={() => {
      if (!armed) return void (armed = true);
      armed = false;
      void webext.uninstall(installed.id);
    }}
  >
    <Icon icon={Delete02Icon} size={16} />
  </button>
{:else if listing}
  <button
    type="button"
    class="install"
    title={busy ? m.webext_store_installing() : (failure ?? m.webext_store_install())}
    aria-label={busy ? m.webext_store_installing() : m.webext_store_install()}
    aria-busy={busy || undefined}
    disabled={busy}
    data-extension-store-install
    onclick={() => {
      const id = tabs.activeId();
      if (id !== null) void webext.prepare(id);
    }}
  >
    {#if busy}
      <span class="spinner" aria-hidden="true"></span>
    {:else}
      <Icon icon={Download01Icon} size={16} />
    {/if}
  </button>
{/if}

<style>
  .install {
    display: grid;
    flex: none;
    place-items: center;
    align-self: center;
    width: 40px;
    height: var(--row-sidebar);
    margin-block: 2px 6px;
    border: 0;
    border-radius: var(--radius-row);
    background: var(--color-lit);
    color: var(--color-on-lit);
    cursor: default;
    animation: interface-fade var(--motion-fast) var(--ease-out) both;
    transition: scale var(--motion-slow) var(--ease-spring);
  }

  .install:active {
    scale: 0.94;
    transition-duration: var(--motion-instant);
  }

  .install:disabled:not([aria-busy]) {
    opacity: 0.6;
  }

  /* The tab favicon's loading arc, at icon size. */
  .spinner {
    width: 16px;
    height: 16px;
    border-radius: var(--radius-capsule);
    background: conic-gradient(from 0deg, transparent 20%, currentcolor);
    mask: radial-gradient(farthest-side, transparent calc(100% - 2px), black calc(100% - 1.5px));
    animation:
      interface-fade var(--motion-fast) var(--ease-out) both,
      install-orbit 0.9s linear infinite;
  }

  @keyframes install-orbit {
    to {
      rotate: 1turn;
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .spinner {
      animation: none;
      opacity: 0.6;
    }
  }

  .install:hover:not(:disabled) {
    background: var(--color-lit-hover);
  }

  .remove,
  .remove:hover:not(:disabled) {
    background: var(--row-active);
    color: var(--color-muted);
  }

  .remove.armed {
    background: var(--color-danger);
    color: var(--color-on-lit);
  }
</style>
