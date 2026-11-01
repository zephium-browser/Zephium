<!--
  Up to four panes, each with an Obsidian-style tab strip. Only a pane's front
  tab is mounted; a file dropped from the tree onto a pane opens there.
-->
<script lang="ts">
  import type { VaultSession } from "$domain/vault";
  import { nameOf } from "$domain/vault";
  import * as m from "$shared/i18n/messages";
  import { badgeFor, displayName, extOf } from "../lib/kinds";
  import type { OutlineItem } from "../lib/outline";
  import type { Workspace } from "../lib/workspace.svelte";
  import FileView from "./FileView.svelte";

  let {
    session,
    workspace,
    onoutline,
    ontonote,
    onrename,
  }: {
    session: VaultSession;
    workspace: Workspace;
    onoutline: (pane: number, items: OutlineItem[], go: (target: unknown) => void) => void;
    ontonote: (source: string, text: string, page: number) => void;
    onrename: (path: string) => void;
  } = $props();

  let dropPane = $state<number | null>(null);
</script>

<div class="grid" data-layout={workspace.layout}>
  {#each workspace.panes as pane, index (pane.id)}
    <section
      class="pane"
      class:focused={workspace.focused === index && workspace.panes.length > 1}
      class:drop={dropPane === index}
      aria-label={m.vault_pane({ number: index + 1 })}
      onpointerdowncapture={() => {
        if (workspace.focused !== index) {
          workspace.focused = index;
          workspace.save();
        }
      }}
      ondragover={(event) => {
        if (!event.dataTransfer?.types.includes("application/x-zephium-vault")) return;
        event.preventDefault();
        dropPane = index;
      }}
      ondragleave={() => (dropPane = null)}
      ondrop={(event) => {
        const path = event.dataTransfer?.getData("application/x-zephium-vault");
        dropPane = null;
        if (!path) return;
        event.preventDefault();
        const entry = session.listings
          .get(path.includes("/") ? path.slice(0, path.lastIndexOf("/")) : "")
          ?.find((candidate) => candidate.path === path);
        if (entry?.kind !== "folder") workspace.open(path, { pane: index });
      }}
    >
      <div class="tabs" role="tablist" aria-label={m.vault_open_files()}>
        {#each pane.tabs as tab (tab)}
          <div
            class="tab"
            role="tab"
            tabindex={pane.active === tab ? 0 : -1}
            aria-selected={pane.active === tab}
            class:on={pane.active === tab}
            title={tab}
            onclick={() => workspace.activate(index, tab)}
            onauxclick={(event) => {
              if (event.button === 1) workspace.close(index, tab);
            }}
            ondblclick={() => onrename(tab)}
            onkeydown={(event) => {
              if (event.key === "Enter" || event.key === " ") workspace.activate(index, tab);
            }}
          >
            <span class="label">{displayName(nameOf(tab))}</span>
            {#if badgeFor(extOf(tab))}<span class="badge">{badgeFor(extOf(tab))}</span>{/if}
            <button
              type="button"
              class="close"
              aria-label={m.vault_close_tab({ name: nameOf(tab) })}
              onclick={(event) => {
                event.stopPropagation();
                workspace.close(index, tab);
              }}>×</button
            >
          </div>
        {/each}
      </div>
      <div class="body">
        {#if pane.active}
          {#key pane.active}
            <FileView
              {session}
              path={pane.active}
              host={`vault-pane-${pane.id}`}
              onoutline={(items, go) => onoutline(index, items, go)}
              {ontonote}
            />
          {/key}
        {:else}
          <div class="blank">
            <p>{m.vault_no_file_open()}</p>
            <p class="hint">{m.vault_no_file_hint()}</p>
          </div>
        {/if}
      </div>
    </section>
  {/each}
</div>

<style>
  .grid {
    display: grid;
    flex: 1;
    gap: 1px;
    min-inline-size: 0;
    min-block-size: 0;
    background: var(--color-border);
  }

  .grid[data-layout="single"] {
    grid-template: 1fr / 1fr;
  }

  .grid[data-layout="columns"] {
    grid-template: 1fr / 1fr 1fr;
  }

  .grid[data-layout="rows"] {
    grid-template: 1fr 1fr / 1fr;
  }

  .grid[data-layout="grid"] {
    grid-template: 1fr 1fr / 1fr 1fr;
  }

  .pane {
    display: flex;
    flex-direction: column;
    min-inline-size: 0;
    min-block-size: 0;
    background: var(--color-page);
  }

  .pane.drop {
    outline: 2px dashed var(--color-border-strong);
    outline-offset: -4px;
  }

  .tabs {
    display: flex;
    flex: none;
    gap: 2px;
    min-block-size: 36px;
    padding: 4px 6px 0;
    overflow-x: auto;
    background: var(--color-surface);
    scrollbar-width: none;
  }

  .tab {
    display: flex;
    flex: none;
    align-items: center;
    gap: 6px;
    max-inline-size: 220px;
    block-size: 32px;
    padding-inline: 12px 6px;
    border-start-start-radius: var(--radius-control);
    border-start-end-radius: var(--radius-control);
    color: var(--color-muted);
    font-size: 12.5px;
    cursor: default;
  }

  .tab:hover {
    background: var(--color-fill-hover);
    color: var(--color-text);
  }

  .tab.on {
    background: var(--color-page);
    color: var(--color-text);
  }

  .label {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .badge {
    color: var(--color-faint);
    font-size: 9px;
    font-weight: 600;
  }

  .close {
    inline-size: 18px;
    block-size: 18px;
    padding: 0;
    border: 0;
    border-radius: 4px;
    background: transparent;
    color: var(--color-faint);
    font: inherit;
    line-height: 1;
    opacity: 0;
  }

  .close:hover {
    background: var(--color-fill-active);
    color: var(--color-text);
  }

  .close:focus-visible,
  .tab:hover .close,
  .tab.on .close {
    opacity: 1;
  }

  .body {
    flex: 1;
    min-block-size: 0;
    overflow: hidden;
  }

  .blank {
    display: grid;
    place-content: center;
    block-size: 100%;
    gap: 6px;
    color: var(--color-muted);
    text-align: center;
  }

  .hint {
    color: var(--color-faint);
    font-size: 12px;
  }

  .pane.focused > .tabs {
    box-shadow: inset 0 -2px 0 var(--color-border-strong);
  }
</style>
