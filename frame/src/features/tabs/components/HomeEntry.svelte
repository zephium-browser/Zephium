<script lang="ts">
  import * as m from "$shared/i18n/messages";
  import { Home01Icon } from "@hugeicons/core-free-icons";
  import Icon from "$shared/ui/Icon";
  import { tabs } from "$domain/tabs";
  import TabRow from "./TabRow.svelte";

  let { onSelect }: { onSelect: (id: string) => void } = $props();
  // Native keeps one Home a space; it is the untouched new tab, drawn here and not among today's pages.
  let home = $derived(tabs.tabs().find((tab) => tab.home) ?? null);
  const none = () => {};
</script>

<ul class="home-entry" role="list" aria-label={m.home_entry()}>
  {#if home}
    <TabRow
      tab={home}
      home
      active={home.id === tabs.activeId()}
      closable={false}
      splitCandidate={false}
      {onSelect}
      onClose={none}
      onContextMenu={(event, tab) => {
        event.preventDefault();
        tabs.openTabMenu(tab.id, event.clientX, event.clientY);
      }}
      onPointerDown={none}
      onPointerMove={none}
      onPointerUp={none}
      onPointerCancel={none}
    />
  {:else}
    <li class="open">
      <button type="button" onclick={tabs.open}
        ><Icon icon={Home01Icon} size={16} /><span>{m.home_entry()}</span></button
      >
    </li>
  {/if}
</ul>

<style>
  .home-entry {
    display: flex;
    flex: 1;
    flex-direction: column;
    min-width: 0;
    margin: 0;
    padding: 0;
    list-style: none;
  }

  /* Until native has made the space's Home, the row opens one. */
  .open button {
    display: flex;
    align-items: center;
    gap: 10px;
    inline-size: 100%;
    height: var(--row-sidebar);
    padding-inline: 8px 4px;
    border: 0;
    border-radius: var(--radius-row);
    background: transparent;
    color: var(--color-label-secondary);
    font: inherit;
    font-size: var(--sidebar-row-text);
    font-weight: var(--sidebar-row-weight);
    text-align: start;
    cursor: default;
  }

  .open button:hover {
    background: var(--row-hover);
    color: var(--color-text);
  }
</style>
