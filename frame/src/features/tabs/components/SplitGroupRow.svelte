<script lang="ts">
  import type { TabView } from "$shared/ipc/bindings";
  import TabRow from "./TabRow.svelte";

  let {
    tabs,
    activeId,
    selectedIds = [],
    splitting,
    closable = true,
    motionKey,
    class: className = "",
    onSelect,
    onClose,
    onContextMenu,
    onPointerDown,
    onPointerMove,
    onPointerUp,
    onPointerCancel,
  }: {
    tabs: TabView[];
    activeId: string | null;
    selectedIds?: readonly string[];
    splitting: boolean;
    closable?: boolean;
    motionKey?: string;
    class?: string;
    onSelect: (id: string, event: MouseEvent) => void;
    onClose: (id: string) => void;
    onContextMenu: (event: MouseEvent, tab: TabView) => void;
    onPointerDown: (event: PointerEvent, tab: TabView) => void;
    onPointerMove: (event: PointerEvent) => void;
    onPointerUp: (event: PointerEvent) => void;
    onPointerCancel: (event: PointerEvent) => void;
  } = $props();

  let holdsActive = $derived(tabs.some((tab) => tab.id === activeId));
</script>

<!--
  One split group reads as a single object: the members share an enclosure and
  a hairline seam, so the pairing is visible without a badge or a label.
-->
<li
  data-motion-key={motionKey}
  class={[
    "overflow-hidden rounded-row transition-[box-shadow] duration-[var(--motion-fast)]",
    className,
  ]}
  class:bg-fill={!holdsActive}
  class:shadow-raised={holdsActive}
>
  <ul class="grid grid-cols-2" role="list" aria-label="Split group">
    {#each tabs as tab, index (tab.id)}
      <TabRow
        {tab}
        grouped
        {closable}
        class={index > 0 ? "border-s border-border" : ""}
        active={tab.id === activeId}
        selected={selectedIds.includes(tab.id)}
        multiSelecting={selectedIds.length >= 2}
        splitCandidate={splitting && tab.id !== activeId}
        {onSelect}
        {onClose}
        {onContextMenu}
        {onPointerDown}
        {onPointerMove}
        {onPointerUp}
        {onPointerCancel}
      />
    {/each}
  </ul>
</li>
