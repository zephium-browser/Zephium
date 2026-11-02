<script lang="ts">
  import "$styles/global.css";
  import { tabFixture } from "$shared/testing/fixtures";
  import type { SidebarNodeView } from "$shared/ipc/bindings";
  import { EssentialTile, EssentialsRail } from "$features/essentials";
  import { SidebarBody, TabList, TabRail, sidebarTree } from "$features/tabs";
  import { Dock } from "$features/dock";
  import { ModeTabs } from "$features/sidebar";
  import { loadAccountButton } from "$features/account";
  import * as m from "$shared/i18n/messages";

  let { compact = false, work = false }: { compact?: boolean; work?: boolean } = $props();

  const open = [
    "rust programming language at DuckDuckGo",
    "Raycast - Your shortcut to everything",
    "X. It's what's happening / X",
    "Agentic Infrastructure - Vercel",
    "New Tab",
  ].map((title, index) => tabFixture({ id: `t${index}`, title }));
  // The first has no page open, as a kept site is until it is visited.
  const kept = ["YouTube", "Spotify", "X"].map((title, index) =>
    tabFixture({
      id: `s${index}`,
      title,
      availability: index === 0 ? { state: "sleeping" } : null,
    }),
  );
  const node = (id: string, section: "favorites" | "today"): SidebarNodeView => ({
    id,
    parent_id: null,
    section,
    kind: { type: "tab", tab_id: id },
  });
  const tree = sidebarTree(
    [...kept.map((tab) => node(tab.id, "favorites")), ...open.map((tab) => node(tab.id, "today"))],
    [...kept, ...open],
  );
  const noop = () => {};
</script>

<div class={["browser-sidebar", "column", compact && "column-rail"]}>
  <div class="sidebar-columns">
    <div class="sidebar-browser-column">
      {#if compact}
        <ModeTabs compact standalone />
        <TabRail entries={open} onSelect={noop} />
        <Dock compact tools={!work} foot={work ? account : undefined}>
          {#snippet sites()}<EssentialsRail entries={kept.slice(0, 2)} onSelect={noop} />{/snippet}
        </Dock>
        {#snippet account()}{#await loadAccountButton() then { default: AccountButton }}<AccountButton
              name="Personal"
            />{/await}{/snippet}
      {:else}
        <div class="sidebar-head"><ModeTabs /></div>
        <SidebarBody pinned={tree.pinned} today={tree.today} splitting={false} onSelect={noop} />
        <Dock>
          {#snippet sites()}
            <TabList
              entries={tree.favorites}
              section="favorites"
              variant="essentials"
              label={m.essentials()}
              splitting={false}
              onSelect={noop}
              >{#snippet essentialTile(props)}<EssentialTile {...props} />{/snippet}</TabList
            >
          {/snippet}
        </Dock>
      {/if}
    </div>
  </div>
</div>

<style>
  .column {
    display: flex;
    flex-direction: column;
    width: 240px;
    height: 640px;
    background: var(--color-chrome);
  }

  .column-rail {
    align-items: center;
    width: 56px;
  }

  .column :global(.sidebar-browser-column) {
    align-items: stretch;
  }

  .column-rail :global(.sidebar-browser-column) {
    align-items: center;
  }
</style>
