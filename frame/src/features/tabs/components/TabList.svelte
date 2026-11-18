<script lang="ts">
  import * as tabDrag from "$session/tab-drag.svelte";
  import * as folderEdit from "$session/folder-edit.svelte";
  import { FOLDER_LEVELS } from "$session/tree-drop";
  import type { SidebarSectionView, TabView } from "$shared/ipc/bindings";
  import { tabs } from "$domain/tabs";
  import { preferences } from "$domain/preferences";
  import type { Snippet } from "svelte";
  import type { TabTileProps } from "../lib/tab-tile";
  import FolderRow from "./FolderRow.svelte";
  import {
    sidebarDisplayUnits,
    collapsedSidebarUnits,
    folderContext,
    type SidebarDisplayUnit,
    type SidebarEntry,
    type SidebarFolderEntry,
  } from "../lib/sidebar-model";
  import SplitGroupRow from "./SplitGroupRow.svelte";
  import TabRow from "./TabRow.svelte";
  import { untrack } from "svelte";
  import { ListMotion } from "$shared/lib/list-motion";
  import { RowDrag } from "$session/row-drag.svelte";
  import { favicons } from "$domain/favicons";
  import DragMark from "$shared/ui/DragMark";
  import { IS_MAC } from "$shared/platform";
  import type { TabSelection } from "../lib/tab-selection.svelte";
  import { isSelectionModifier } from "../lib/tab-selection";

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
    selection,
    rangeOrder,
    onVisibleOrder,
    selectionEscapeBlocked = false,
  }: {
    /** Lay the tiles out in rows of this many even columns, filling upwards. */
    columns?: number;
    selection?: TabSelection;
    rangeOrder?: readonly string[];
    onVisibleOrder?: (order: string[]) => void;
    selectionEscapeBlocked?: boolean;
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
  // A fold is native's; one just asked for is drawn until native answers.
  let folded = $derived(
    new Set(
      displayUnits.flatMap((unit) =>
        unit.kind === "folder" && folderEdit.folded(unit.node.id, unit.node.kind.collapsed)
          ? [unit.key]
          : [],
      ),
    ),
  );
  let context = $derived(folderContext(displayUnits));
  let hiddenUnits = $derived(collapsedSidebarUnits(displayUnits, folded, tabs.activeId()));

  const selectable = $derived(selection !== undefined && variant === "list" && !splitting);
  const selectedIds = $derived(selectable ? (selection?.value.ids ?? []) : []);
  const visibleOrder = $derived(
    displayUnits.flatMap((unit) => {
      if (hiddenUnits.has(unit.key) || unit.kind === "folder") return [];
      return unit.kind === "split" ? unit.tabs.map((entry) => entry.tab.id) : [unit.tab.id];
    }),
  );
  $effect(() => {
    onVisibleOrder?.(visibleOrder);
  });
  $effect(() => {
    const current = selection;
    if (!current) return;
    const scope = JSON.stringify([tabs.profile()?.id, tabs.activeSpaceId()]);
    const order = rangeOrder ?? visibleOrder;
    const ids = tabs.tabs().flatMap((tab) => (order.includes(tab.id) ? [tab.id] : []));
    const split = splitting;
    untrack(() => {
      current.reconcile(scope, ids);
      if (split) current.clear();
    });
  });

  function escape(event: KeyboardEvent) {
    if (
      !selection ||
      selectionEscapeBlocked ||
      event.defaultPrevented ||
      event.key !== "Escape" ||
      event.isComposing
    )
      return;
    const selected = selection.value.ids.length > 0;
    if (!selected && !list?.contains(document.activeElement)) return;
    selection.clear();
    if (selected) {
      event.preventDefault();
      event.stopPropagation();
    }
  }

  const modified = (event: MouseEvent) => selectable && isSelectionModifier(event, IS_MAC);

  const nodeOf = (unit: SidebarDisplayUnit) =>
    unit.kind === "split" ? (unit.tabs[0]?.node.id ?? "") : unit.node.id;
  const tree = () => variant === "list" && section === "today";

  const drag = new RowDrag({
    tree: {
      block: (id) => {
        if (!tree()) return null;
        const at = displayUnits.findIndex((unit) => unit.kind !== "split" && nodeOf(unit) === id);
        const unit = displayUnits[at];
        if (unit === undefined || unit.kind === "split") return null;
        let next = at + 1;
        while (next < displayUnits.length && displayUnits[next]!.depth > unit.depth) next += 1;
        const after = displayUnits[next];
        return {
          depth: unit.depth,
          folder: unit.kind === "folder",
          levels: context.summaries.get(unit.key)?.levels ?? 1,
          fileable: unit.kind === "folder" || (unit.tab.content ?? "web") === "web",
          place: {
            depth: unit.depth,
            parent: unit.node.parent_id,
            before: after !== undefined && after.depth === unit.depth ? nodeOf(after) : null,
          },
        };
      },
      commit: async (id, place) => {
        // A tab to the top level goes as every open-tab reorder does.
        if (place.parent === null && tabs.tabs().some((tab) => tab.id === id)) {
          await tabDrag.move(id, false, place.before);
          return !tabDrag.moveFailed();
        }
        const result = await tabs.folder({
          type: "move",
          id,
          parent: place.parent,
          before: place.before,
        });
        return result.outcome === "applied" || result.outcome === "deferred";
      },
    },
    list: () => list,
    region: () => list?.closest<HTMLElement>("[data-tabs-drop]") ?? undefined,
    // Only open tabs reorder in place; native places anything dropped in
    // this list among them.
    reorder: () => (variant === "list" && section === "today" ? { essential: false } : null),
    drop: ({ id, x, y, over, before, page }) => {
      // A split's tabs share one row, so they never reorder in place; let go
      // anywhere in the sidebar and the tab leaves its split for there.
      // A tab that cannot be kept goes back where it was.
      if (over?.closest("[data-essentials-drop]") && !tabDrag.keepable(id)) return;
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
        // Into the open tabs at the top level; a folder takes only what is
        // already among them.
        const row = before
          ? over
              .closest("[data-tabs-drop]")
              ?.querySelector<HTMLElement>(`[data-node-id="${CSS.escape(before)}"]`)
          : null;
        void tabDrag.move(id, false, row && row.dataset.depth !== "0" ? null : before);
        return;
      }
      if (page) tabs.dropTab(id, x, y);
    },
  });
  const handlePointerDown = (event: PointerEvent, tab: TabView) => {
    if (event.button === 0 && modified(event)) {
      drag.abandon();
      drag.swallowClick();
      return;
    }
    drag.down(event, tab.id);
  };
  const handlePointerMove = (event: PointerEvent) => drag.move(event);
  const handlePointerUp = (event: PointerEvent) => drag.up(event);
  const handlePointerCancel = (event: PointerEvent) => drag.cancel(event);

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

  function toggleFolder(unit: SidebarFolderEntry) {
    void folderEdit.setFolded(unit.node.id, !folded.has(unit.key));
  }

  function folderMenu(unit: SidebarFolderEntry, x: number | null, y: number | null) {
    drag.abandon();
    tabs.openFolderMenu(unit.node.id, x, y, {
      color: unit.node.kind.color,
      can_nest: unit.depth + 1 < FOLDER_LEVELS,
      awake: (context.summaries.get(unit.key)?.awake ?? 0) > 0,
    });
  }

  // A folder just made opens for its name once it is drawn.
  $effect(() => {
    for (const unit of displayUnits)
      if (unit.kind === "folder" && folderEdit.fresh(unit.node.id))
        untrack(() => folderEdit.claim(unit.node.id));
  });

  const emptySummary = {
    count: 0,
    faces: [],
    audible: false,
    loading: false,
    awake: 0,
    levels: 1,
  };

  function handleContextMenu(event: MouseEvent, tab: TabView) {
    // A DOM menu cannot reach past the chrome WebView, so the tab menu is a
    // real native popup owned by Rust.
    event.preventDefault();
    drag.abandon();
    tabs.openTabMenu(tab.id, event.clientX, event.clientY);
  }

  function select(id: string, event?: MouseEvent) {
    if (selection && event && modified(event)) {
      drag.swallowClick();
      const order = rangeOrder ?? visibleOrder;
      const active = tabs.activeId();
      if (event.shiftKey) selection.range(id, active, order);
      else selection.toggle(id, active !== null && order.includes(active) ? active : null);
      return;
    }
    if (drag.swallowClick()) return;
    selection?.plain(id);
    onSelect(id);
  }
</script>

<!--
  Mounted even when empty, and hidden instead: the first site kept, or the
  first tab opened, is an arrival the list has to be present to see.
-->
<svelte:window onkeydown={escape} />

<nav
  class={["tab-list", variant === "essentials" && "tab-list-flush"]}
  aria-label={label}
  hidden={entries.length === 0}
  data-close-buttons={preferences.value("tabs.close-button")}
  data-tab-list-background={variant === "list" || undefined}
>
  <ul
    bind:this={list}
    class:flex={variant === "list"}
    class:dock-row={variant === "essentials" && columns === undefined}
    class:dock-stack={variant === "essentials" && columns !== undefined}
    style:--dock-columns={columns}
    class:flex-col={variant === "list"}
    class:tab-list-rows={variant === "list"}
    data-tab-list-background={variant === "list" || undefined}
    role="list"
  >
    {#each displayUnits as unit, index (unit.key)}
      {#if unit.kind === "folder"}
        <FolderRow
          entry={unit}
          summary={context.summaries.get(unit.key) ?? emptySummary}
          spines={context.spines.get(unit.key)}
          expanded={!folded.has(unit.key)}
          editing={folderEdit.renamingId() === unit.node.id}
          ontoggle={() => toggleFolder(unit)}
          onrename={() => folderEdit.rename(unit.node.id)}
          onrenamed={(name) =>
            name === null
              ? folderEdit.stopRenaming(unit.node.id)
              : void folderEdit.commitName(unit.node.id, name, unit.node.kind.name)}
          onmenu={(x, y) => folderMenu(unit, x, y)}
          onPointerDown={(event) => drag.down(event, unit.node.id, true)}
          onPointerMove={handlePointerMove}
          onPointerUp={handlePointerUp}
          onPointerCancel={handlePointerCancel}
          class={[
            variant === "essentials" ? "col-span-full" : "",
            hiddenUnits.has(unit.key) ? "hidden" : "",
          ].join(" ")}
        />
      {:else if unit.kind === "split"}
        <SplitGroupRow
          motionKey={unit.key}
          depth={unit.depth}
          tabs={unit.tabs.map((entry) => entry.tab)}
          activeId={tabs.activeId()}
          {selectedIds}
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
          selected={selectedIds.includes(unit.tab.id)}
          multiSelecting={selectedIds.length >= 2}
          depth={unit.depth}
          spines={context.spines.get(unit.key)}
          closable={section === "today" && !tabs.loneNewTab(unit.tab.id)}
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

{#if drag.ghost !== null}{@const held = tabs
    .tabs()
    .find((tab) => tab.id === drag.ghost?.id)}{@const folder = held
    ? undefined
    : displayUnits.find(
        (unit): unit is SidebarFolderEntry =>
          unit.kind === "folder" && unit.node.id === drag.ghost?.id,
      )}<DragMark
    x={drag.ghost.x}
    y={drag.ghost.y}
    image={favicons.image(held?.icon)}
    tone={favicons.tone(held?.icon)}
    title={held?.title ?? folder?.node.kind.name}
  />{/if}

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

  /* Rows a folded folder holds are not drawn; the rows' own display would
     otherwise win over the utility. */
  .tab-list-rows > :global(li.hidden) {
    display: none;
  }

  /* One pixel between rows read as a stack of stripes rather than a list. */
  .tab-list-rows {
    gap: var(--sidebar-row-gap);
  }
</style>
