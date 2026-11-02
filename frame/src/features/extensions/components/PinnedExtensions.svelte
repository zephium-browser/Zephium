<script lang="ts">
  import { extensions } from "$domain/extensions";
  import { tabs } from "$domain/tabs";
  import { webext } from "$domain/webext";
  import * as m from "$shared/i18n/messages";
  import ExtensionActionIcon from "./ExtensionActionIcon.svelte";
  import { holdWhilePopupOpen } from "../lib/popup-hold";

  let profileId = $derived(tabs.profile()?.id ?? null);
  let tabId = $derived(tabs.activeId());
  let pinned = $derived(
    extensions
      .activeActions(profileId, tabId)
      .filter((action) => webext.byInstall(action.runtime.install_id)?.pinned)
      .slice(0, webext.MAX_PINNED),
  );

  function invoke(event: MouseEvent, action: (typeof pinned)[number]) {
    const target = event.currentTarget;
    if (profileId === null || tabId === null || !(target instanceof HTMLElement)) return;
    if (action.presents_popup) holdWhilePopupOpen();
    void extensions.invoke(profileId, tabId, action, target.getBoundingClientRect());
  }

  function openMenu(event: MouseEvent, action: (typeof pinned)[number]) {
    event.preventDefault();
    const target = event.currentTarget;
    if (profileId === null || !(target instanceof HTMLElement)) return;
    extensions.menu(profileId, action, target.getBoundingClientRect());
  }
</script>

<!--
  Pinned extensions keep their buttons at the head of the tab list, beside
  the plus; the tray on the address holds the rest.
-->
{#if pinned.length > 0}
  <div class="pins" role="group" aria-label={m.webext_page_title()}>
    {#each pinned as action (action.runtime.install_id)}
      {@const name = action.label || webext.byInstall(action.runtime.install_id)?.name || ""}
      <button
        type="button"
        class="pin"
        title={name}
        aria-label={name || m.webext_action()}
        aria-haspopup={action.presents_popup ? "dialog" : undefined}
        disabled={!action.enabled || extensions.isInvoking(action.runtime.install_id)}
        data-pinned-extension={action.runtime.install_id}
        onclick={(event) => invoke(event, action)}
        oncontextmenu={(event) => openMenu(event, action)}
      >
        <span class="glyph">
          <ExtensionActionIcon rgba={action.icon_rgba_base64} />
          {#if action.badge.length > 0}
            <span aria-hidden="true" class="badge" data-unread={action.unread_badge}>
              {action.badge}
            </span>
          {/if}
        </span>
      </button>
    {/each}
  </div>
{/if}

<style>
  /* Beside the plus that opens a tab, in the row's own shape, and shown as
     the address tray's glyph is: with the pointer in the column after a
     beat, at once over the row or with one of them in hand. */
  .pins {
    display: flex;
    flex: none;
    align-items: center;
    gap: 2px;
    opacity: 0;
    transition: opacity var(--motion-slow) var(--ease-smooth);
  }

  .pins:focus-within {
    opacity: 1;
  }

  :global(.browser-sidebar[data-pointer-in="true"]) .pins {
    opacity: 1;
    transition-delay: 120ms;
  }

  :global(.browser-sidebar[data-peek]) .pins,
  :global(.browser-sidebar[data-pointer-in="true"] .space-bar:hover) .pins,
  :global(.browser-sidebar[data-pointer-in="true"] .new-tab-slot:hover) .pins {
    opacity: 1;
    transition-delay: 0ms;
  }

  .pin {
    display: grid;
    place-items: center;
    inline-size: 30px;
    block-size: 30px;
    padding: 0;
    border: 0;
    border-radius: 50%;
    background: none;
    cursor: default;
    outline: none;
    transition: background-color var(--motion-instant) var(--ease-smooth);
  }

  .pin:hover,
  .pin:focus-visible {
    background: var(--row-hover);
  }

  .pin:focus-visible {
    outline: 2px solid var(--color-ring);
    outline-offset: -2px;
  }

  .pin:active {
    background: var(--row-pressed);
  }

  .pin:disabled {
    opacity: 0.45;
  }

  .glyph {
    position: relative;
    display: grid;
    place-items: center;
    width: 16px;
    height: 16px;
  }

  .glyph :global(canvas) {
    width: 16px;
    height: 16px;
  }

  .badge {
    position: absolute;
    top: -5px;
    right: -8px;
    max-width: 26px;
    overflow: hidden;
    padding-inline: 3px;
    border-radius: var(--radius-capsule);
    background: var(--color-accent);
    font-size: 9px;
    font-weight: 600;
    line-height: 13px;
    color: var(--color-on-accent);
    white-space: nowrap;
    text-overflow: ellipsis;
    pointer-events: none;
  }

  .badge[data-unread="true"] {
    background: var(--color-warning);
  }
</style>
