<script lang="ts">
  import { PinIcon, PinOffIcon } from "@hugeicons/core-free-icons";
  import { extensions } from "$domain/extensions";
  import { tabs } from "$domain/tabs";
  import { webext } from "$domain/webext";
  import * as m from "$shared/i18n/messages";
  import Icon from "$shared/ui/Icon";
  import ExtensionActionIcon from "./ExtensionActionIcon.svelte";
  import { holdWhilePopupOpen } from "../lib/popup-hold";

  let profileId = $derived(tabs.profile()?.id ?? null);
  let tabId = $derived(tabs.activeId());
  let actions = $derived(extensions.activeActions(profileId, tabId));
  let shortcut = $derived(extensions.actionShortcut(profileId, tabId));
  let actionRoot = $state<HTMLDivElement>();

  // The menu holding the rows closes when one is chosen, so a popup hangs
  // from the extension's pinned button when it has one, else from the
  // menu's own button, which stays, rather than from a vanished row.
  const anchor = (row: HTMLElement) =>
    (
      document.querySelector<HTMLElement>(
        `[data-pinned-extension="${row.dataset.extensionInstall}"]`,
      ) ??
      row.closest(".ui-disclosure")?.querySelector<HTMLElement>(":scope > button") ??
      row
    ).getBoundingClientRect();

  function invoke(event: MouseEvent, action: (typeof actions)[number]) {
    const target = event.currentTarget;
    if (profileId === null || tabId === null || !(target instanceof HTMLButtonElement)) return;
    if (action.presents_popup) holdWhilePopupOpen();
    void extensions.invoke(profileId, tabId, action, anchor(target));
  }

  function openMenu(event: MouseEvent, action: (typeof actions)[number]) {
    event.preventDefault();
    event.stopPropagation();
    const target = event.currentTarget;
    if (profileId === null || !(target instanceof HTMLElement)) return;
    extensions.menu(profileId, action, target.getBoundingClientRect());
  }

  $effect(() => {
    const request = shortcut;
    const root = actionRoot;
    if (request === null || root === undefined || profileId === null || tabId === null) return;
    const action = actions.find(
      (candidate) =>
        candidate.runtime.install_id === request.runtime.install_id &&
        candidate.runtime.generation === request.runtime.generation &&
        candidate.revision === request.action_revision,
    );
    if (action === undefined || !action.enabled) return;
    const button = [
      ...root.querySelectorAll<HTMLButtonElement>("button[data-extension-install]"),
    ].find((candidate) => candidate.dataset.extensionInstall === request.runtime.install_id);
    if (button === undefined || button.disabled) return;
    if (!extensions.consumeActionShortcut(request.projection_revision)) return;
    void extensions.invoke(profileId, tabId, action, anchor(button));
  });
</script>

<!--
  The extensions' own buttons, as rows of the tray: the icon and its badge,
  the name, and whether it is pinned to the address field. Choosing one
  closes the panel around them. At rail width they are rows of the column's
  native menu instead.
-->
<div
  bind:this={actionRoot}
  class="actions"
  role="group"
  aria-label={m.webext_page_title()}
  data-zephium-extension-actions
>
  {#each actions as action (action.runtime.install_id)}
    {@const installed = webext.byInstall(action.runtime.install_id)}
    {@const name = action.label || installed?.name || m.webext_action()}
    {@const pinned = installed?.pinned ?? false}
    {@const full = !pinned && webext.pinnedCount() >= webext.MAX_PINNED}
    <div class="row">
      <button
        type="button"
        data-dismiss
        aria-haspopup={action.presents_popup ? "dialog" : undefined}
        title={name}
        disabled={!action.enabled || extensions.isInvoking(action.runtime.install_id)}
        data-extension-install={action.runtime.install_id}
        class="item"
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
        <span class="name">{name}</span>
      </button>
      {#if installed}
        <button
          type="button"
          class="pin"
          aria-pressed={pinned}
          aria-label={pinned ? m.webext_unpin() : m.webext_pin()}
          title={full ? m.webext_pin_full() : pinned ? m.webext_unpin() : m.webext_pin()}
          disabled={full}
          onclick={() => void webext.setPinned(installed.id, !pinned)}
        >
          <Icon icon={pinned ? PinOffIcon : PinIcon} size={14} />
        </button>
      {/if}
    </div>
  {/each}
</div>

<style>
  .actions {
    display: flex;
    flex-direction: column;
    padding-block-end: 2px;
  }

  .actions:empty {
    display: none;
  }

  .row {
    display: flex;
    align-items: center;
    padding-inline-end: calc(var(--menu-item-inset) - 4px);
  }

  .item {
    display: flex;
    flex: 1;
    align-items: center;
    gap: 10px;
    min-inline-size: 0;
    block-size: var(--menu-row);
    padding-inline: calc(var(--menu-item-inset) - 1px) 6px;
    border: 0;
    border-radius: var(--radius-inset);
    background: none;
    color: var(--color-text);
    font: inherit;
    font-size: var(--text-body);
    text-align: start;
    cursor: default;
    outline: none;
    transition: background-color var(--motion-instant) var(--ease-smooth);
  }

  .item:hover,
  .item:focus-visible {
    background: var(--row-hover);
  }

  .item:focus-visible {
    outline: 2px solid var(--color-ring);
    outline-offset: -2px;
  }

  .item:active {
    background: var(--row-pressed);
  }

  .item:disabled {
    opacity: 0.45;
  }

  .name {
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }

  /* Shown with the pointer on its row, and always once it is pinned. */
  .pin {
    display: grid;
    flex: none;
    place-items: center;
    inline-size: 26px;
    block-size: 26px;
    padding: 0;
    border: 0;
    border-radius: var(--radius-inset);
    background: none;
    color: var(--color-faint);
    cursor: default;
    opacity: 0;
    outline: none;
    transition:
      opacity var(--motion-instant) var(--ease-smooth),
      background-color var(--motion-instant) var(--ease-smooth);
  }

  .pin:focus-visible {
    opacity: 1;
    outline: 2px solid var(--color-ring);
    outline-offset: -2px;
  }

  .pin[aria-pressed="true"] {
    opacity: 1;
  }

  .pin:hover:not(:disabled) {
    background: var(--row-hover);
    color: var(--color-text);
  }

  .row:hover .pin {
    opacity: 1;
  }

  .row:hover .pin:disabled {
    opacity: 0.35;
  }

  .glyph {
    position: relative;
    display: grid;
    place-items: center;
    width: 18px;
    height: 18px;
  }

  .glyph :global(canvas) {
    width: 18px;
    height: 18px;
  }

  .badge {
    position: absolute;
    top: -5px;
    right: -8px;
    max-width: 30px;
    overflow: hidden;
    padding-inline: 4px;
    border-radius: var(--radius-capsule);
    background: var(--color-accent);
    box-shadow: 0 0 0 1.5px var(--color-menu);
    font-size: 9px;
    font-weight: 600;
    line-height: 14px;
    color: var(--color-on-accent);
    white-space: nowrap;
    text-overflow: ellipsis;
    pointer-events: none;
  }

  .badge[data-unread="true"] {
    background: var(--color-warning);
  }
</style>
