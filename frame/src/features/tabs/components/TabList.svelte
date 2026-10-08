<script lang="ts">
  import * as tabDrag from "$session/tab-drag.svelte";
  import { SvelteSet } from "svelte/reactivity";
  import type { SidebarSectionView, TabView } from "$shared/ipc/bindings";
  import { tabs } from "$domain/tabs";
  import type { Snippet } from "svelte";
  import type { TabTileProps } from "../lib/tab-tile";
  import FolderRow from "./FolderRow.svelte";
  import {
    sidebarDisplayUnits,
    collapsedSidebarUnits,
    type SidebarEntry,
  } from "../lib/sidebar-model";
  import SplitGroupRow from "./SplitGroupRow.svelte";
  import TabRow from "./TabRow.svelte";
  import { untrack } from "svelte";
  import { ListMotion } from "$shared/lib/list-motion";
  import { RowDrag } from "$session/row-drag.svelte";

  let {
    entries,
    section,
    variant = "list",
    essentialTile,
    label,
    splitting,
    cascadeFrom = 0,
    columns,
    onSelect,
  }: {
    /** Lay the tiles out in rows of this many even columns, filling upwards. */
    columns?: number;
    /** Where this list's first row falls in the launch cascade. */
    cascadeFrom?: number;
    entries: SidebarEntry[];
    section: SidebarSectionView;
    variant?: "list" | "essentials";
    essentialTile?: Snippet<[TabTileProps]>;
    label: string;
    splitting: boolean;
    onSelect: (id: string) => void;
  } = $props();

  let displayUnits = $derived(sidebarDisplayUnits(section, entries, tabs.splitGroup()));
  const folded = new SvelteSet<string>();
  let hiddenUnits = $derived(collapsedSidebarUnits(displayUnits, folded, tabs.activeId()));

  const drag = new RowDrag({
    list: () => list,
    // Only open tabs reorder in place; native places anything dropped in
    // this list among them.
    reorder: () => (variant === "list" && section === "today" ? { essential: false } : null),
    drop: ({ id, x, y, over, before }) => {
      // A split's tabs share one row, so they never reorder in place; let go
      // anywhere in the sidebar and the tab leaves its split for there.
      if (tabs.inSplit(id) && over?.closest("[data-essentials-drop], [data-tabs-drop]")) {
        const essential = !!over.closest("[data-essentials-drop]");
        void tabDrag.leaveSplit(id, essential || before ? { essential, before } : null);
        return;
      }
      if (over?.closest("[data-essentials-drop]")) {
        void tabDrag.move(id, true, before);
        return;
      }
      // Dropped among the open tabs from anywhere else: it joins them there.
      // Dropping onto a row does not pair the two; that is Split View's job,
      // from the menu or by dropping onto the page.
      if (over?.closest("[data-tabs-drop]") && section !== "today") {
        void tabDrag.move(id, false, before);
        return;
      }
      tabs.dropTab(id, x, y);
    },
  });
  const handlePointerDown = (event: PointerEvent, tab: TabView) => drag.down(event, tab.id);
  const handlePointerMove = (event: PointerEvent) => drag.move(event);
  const handlePointerUp = (event: PointerEvent) => drag.up(event);
  const handlePointerCancel = (event: PointerEvent) => drag.cancel(event);
  let ghostTitle = $derived(
    drag.ghost ? (tabs.tabs().find((tab) => tab.id === drag.ghost?.id)?.title ?? "") : "",
  );

  // Motion follows the list's shape, not its content: a title or a loading
  // state changing moves nothing, so only a change of order or membership
  // is measured at all.
  let list = $state<HTMLUListElement>();
  const motion = new ListMotion({ enter: () => (variant === "essentials" ? "grow" : "rise") });
  let shape = $derived(
    `${displayUnits.map((unit) => unit.key).join(" ")}|${[...hiddenUnits].join(" ")}`,
  );
  $effect.pre(() => {
    void shape;
    untrack(() => {
      motion.capture(list);
      // A drop's rows are held where they were drawn until the reordered
      // list arrives; the capture above has just recorded exactly that, so
      // the list's own motion lands them from there.
      drag.landed();
    });
  });
  $effect(() => {
    void shape;
    untrack(() => motion.play(list));
  });

  function toggleFolder(key: string) {
    if (folded.has(key)) folded.delete(key);
    else folded.add(key);
  }

  function handleContextMenu(event: MouseEvent, tab: TabView) {
    // A DOM menu cannot reach past the chrome WebView, so the tab menu is a
    // real native popup owned by Rust.
    event.preventDefault();
    drag.abandon();
    tabs.openTabMenu(tab.id, event.clientX, event.clientY);
  }

  function select(id: string) {
    if (drag.swallowClick()) return;
    onSelect(id);
  }

  // A folded folder still shows as open while one of its own rows is shown;
  // only its subtree is looked at, which ends at the next row as shallow.
  function showsChild(position: number, depth: number): boolean {
    for (let next = position + 1; next < displayUnits.length; next++) {
      const child = displayUnits[next]!;
      if (child.depth <= depth) return false;
      if (!hiddenUnits.has(child.key)) return true;
    }
    return false;
  }
</script>

<!--
  Mounted even when empty, and hidden instead: the first site kept, or the
  first tab opened, is an arrival the list has to be present to see.
-->
<nav
  class={["tab-list", variant === "essentials" && "tab-list-flush"]}
  aria-label={label}
  hidden={entries.length === 0}
>
  <ul
    bind:this={list}
    class:flex={variant === "list"}
    class:dock-row={variant === "essentials" && columns === undefined}
    class:dock-stack={variant === "essentials" && columns !== undefined}
    style:--dock-columns={columns}
    class:flex-col={variant === "list"}
    class:tab-list-rows={variant === "list"}
    role="list"
  >
    {#each displayUnits as unit, index (unit.key)}
      {#if unit.kind === "folder"}
        <FolderRow
          motionKey={unit.key}
          expanded={!folded.has(unit.key) || showsChild(index, unit.depth)}
          ontoggle={() => toggleFolder(unit.key)}
          name={unit.node.kind.name}
          depth={unit.depth}
          class={[
            variant === "essentials" ? "col-span-full" : "",
            hiddenUnits.has(unit.key) ? "hidden" : "",
          ].join(" ")}
        />
      {:else if unit.kind === "split"}
        <SplitGroupRow
          motionKey={unit.key}
          tabs={unit.tabs.map((entry) => entry.tab)}
          activeId={tabs.activeId()}
          {splitting}
          closable={section === "today"}
          class={[
            variant === "essentials" ? "col-span-full" : "",
            hiddenUnits.has(unit.key) ? "hidden" : "",
          ].join(" ")}
          onSelect={select}
          onClose={tabs.close}
          onContextMenu={handleContextMenu}
          onPointerDown={handlePointerDown}
          onPointerMove={handlePointerMove}
          onPointerUp={handlePointerUp}
          onPointerCancel={handlePointerCancel}
        />
      {:else if variant === "essentials" && unit.depth === 0 && essentialTile}
        {@render essentialTile({
          tab: unit.tab,
          active: unit.tab.id === tabs.activeId(),
          splitCandidate: splitting && unit.tab.id !== tabs.activeId(),
          onSelect: select,
          onContextMenu: handleContextMenu,
          onPointerDown: handlePointerDown,
          onPointerMove: handlePointerMove,
          onPointerUp: handlePointerUp,
          onPointerCancel: handlePointerCancel,
        })}
      {:else}
        <TabRow
          cascade={cascadeFrom + index}
          tab={unit.tab}
          active={unit.tab.id === tabs.activeId()}
          depth={unit.depth}
          closable={section === "today"}
          class={[
            variant === "essentials" ? "col-span-full" : "",
            hiddenUnits.has(unit.key) ? "hidden" : "",
          ].join(" ")}
          splitCandidate={splitting && unit.tab.id !== tabs.activeId()}
          onSelect={select}
          onClose={tabs.close}
          onContextMenu={handleContextMenu}
          onPointerDown={handlePointerDown}
          onPointerMove={handlePointerMove}
          onPointerUp={handlePointerUp}
          onPointerCancel={handlePointerCancel}
        />
      {/if}
    {/each}
  </ul>
</nav>

{#if drag.ghost !== null}
  <div
    class="pointer-events-none fixed z-50 max-w-52 truncate rounded-control bg-raised px-3 py-2 text-[13px] text-text shadow-overlay"
    style:left={`${drag.ghost.x + 12}px`}
    style:top={`${drag.ghost.y + 8}px`}
    aria-hidden="true"
  >
    {ghostTitle}
  </div>
{/if}

<style>
  .tab-list {
    padding-inline: var(--sidebar-inset);
    padding-block: 2px;
  }

  /* The dock owns the band's inset, so its site row adds none of its own. */
  .tab-list-flush {
    padding: 0;
  }

  /* The row beside the shelf: its sites share its width, so one alone
     fills it. The caller hands it only as many as fit. */
  .dock-row {
    display: grid;
    grid-auto-flow: column;
    grid-auto-columns: minmax(0, 1fr);
    gap: var(--dock-gap);
  }

  /* Rows over the shelf's row: even columns, and wrapping upwards, so a row
     that is not yet full sits on top as a ragged edge rather than leaving a
     hole in the middle of the stack. */
  .dock-stack {
    display: flex;
    flex-wrap: wrap-reverse;
    gap: var(--dock-gap);
  }

  .dock-stack > :global(li) {
    flex: 0 0 calc((100% - (var(--dock-columns) - 1) * var(--dock-gap)) / var(--dock-columns));
  }

  /* One pixel between rows read as a stack of stripes rather than a list. */
  .tab-list-rows {
    gap: var(--sidebar-row-gap);
  }
</style>
