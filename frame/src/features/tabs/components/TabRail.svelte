<script lang="ts">
  import { rememberScroll } from "$shared/ui/scroll-memory";
  import type { TabView } from "$shared/ipc/bindings";
  import { tabs } from "$domain/tabs";
  import FavIcon from "$shared/ui/FavIcon";
  import { favicons } from "$domain/favicons";
  import { Add01Icon, Globe02Icon, PuzzleIcon } from "@hugeicons/core-free-icons";
  import Icon from "$shared/ui/Icon";
  import * as m from "$shared/i18n/messages";
  import { untrack } from "svelte";
  import { ListMotion } from "$shared/lib/list-motion";
  import * as tabDrag from "$session/tab-drag.svelte";
  import { RowDrag } from "$session/row-drag.svelte";
  import DragMark from "$shared/ui/DragMark";
  import { closeOnMiddleClick } from "../lib/middle-click";

  let {
    entries,
    closable = new Set<string>(),
    onSelect,
  }: {
    entries: TabView[];
    /** Tabs the expanded list shows a close button for. */
    closable?: ReadonlySet<string>;
    onSelect: (id: string) => void;
  } = $props();

  function handleContextMenu(event: MouseEvent, tab: TabView) {
    event.preventDefault();
    drag.abandon();
    tabs.openTabMenu(tab.id, event.clientX, event.clientY);
  }

  // Order and membership, never content: see TabList.
  let list = $state<HTMLUListElement>();
  // The rail's rows reorder in place like the list's; let go over the kept
  // sites and the tab is kept, anywhere else and native decides (a page).
  const drag = new RowDrag({
    list: () => list,
    reorder: () => ({ essential: false }),
    drop: ({ id, x, y, over, before }) => {
      const essential = !!over?.closest("[data-essentials-drop]");
      if (tabs.inSplit(id) && (essential || over?.closest("[data-tabs-drop]")))
        void tabDrag.leaveSplit(id, essential || before ? { essential, before } : null);
      else if (essential) void tabDrag.move(id, true, before);
      else tabs.dropTab(id, x, y);
    },
  });
  const motion = new ListMotion({ enter: () => "rise" });
  let shape = $derived(entries.map((tab) => tab.id).join(" "));
  $effect.pre(() => {
    void shape;
    untrack(() => {
      motion.capture(list);
      drag.landed();
    });
  });
  $effect(() => {
    void shape;
    untrack(() => motion.play(list));
  });
</script>

<!--
  The rail reads top-down like the expanded list it stands in for, and ends
  the same way, with the row that opens a new tab. `title` gives a real OS
  tooltip, which is the only label that can appear outside the chrome
  WebView's rectangle.
-->
<div
  use:rememberScroll={`${tabs.profile()?.id}/${tabs.activeSpaceId()}/rail`}
  class="rail-scroller"
  data-glide-scroller
>
  <ul bind:this={list} class="rail-list" data-tabs-drop role="list" aria-label={m.open_tabs()}>
    {#each entries as tab, index (tab.id)}
      {@const active = tab.id === tabs.activeId()}
      <li
        data-cascade
        style:--cascade={index}
        data-motion-key={`tab:${tab.id}`}
        data-zephium-tab-id={tab.id}
        data-zephium-tab-url={tab.url ?? ""}
        data-zephium-projection-revision={tab.projection_revision}
      >
        <button
          type="button"
          class="rail-item"
          data-plate
          title={tab.title || m.untitled_tab()}
          aria-current={active ? "page" : undefined}
          aria-label={tab.title || m.untitled_tab()}
          oncontextmenu={(event) => handleContextMenu(event, tab)}
          onpointerdown={(event) => drag.down(event, tab.id)}
          onpointermove={(event) => drag.move(event)}
          onpointerup={(event) => drag.up(event)}
          onpointercancel={(event) => drag.cancel(event)}
          onclick={() => !drag.swallowClick() && onSelect(tab.id)}
          {...closable.has(tab.id) ? closeOnMiddleClick(() => tabs.close(tab.id)) : {}}
        >
          <FavIcon
            image={favicons.image(tab.icon)}
            tone={favicons.tone(tab.icon)}
            loading={tab.loading}
            lit={active}
            size={16}
            fallback={tab.content === "extensions" || tab.content === "extension_owned"
              ? PuzzleIcon
              : Globe02Icon}
          />
          <span data-zephium-tab-label class="sr-only">{tab.title}</span>
        </button>
      </li>
    {/each}
    <li data-cascade style:--cascade={entries.length}>
      <button
        type="button"
        class="rail-item rail-new"
        title={m.new_tab()}
        aria-label={m.new_tab()}
        onclick={tabs.open}
      >
        <Icon icon={Add01Icon} size={16} />
      </button>
    </li>
  </ul>
</div>

{#if drag.ghost}{@const held = tabs.tabs().find((tab) => tab.id === drag.ghost?.id)}<DragMark
    x={drag.ghost.x}
    y={drag.ghost.y}
    image={favicons.image(held?.icon)}
    tone={favicons.tone(held?.icon)}
  />{/if}

<style>
  .rail-scroller {
    display: flex;
    flex: 1;
    flex-direction: column;
    min-height: 0;
    overflow-y: auto;
    overscroll-behavior: contain;
    padding-block: 8px;
  }

  /* The expanded list's own pitch, row for row, so a tab sits at the same
     height in both shapes of the column. */
  .rail-list {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: var(--sidebar-row-gap);
  }

  /* The expanded row with its label taken away: the same plate, corner, rim
     and fills, never a disc of its own. */
  .rail-item {
    display: grid;
    place-items: center;
    width: 40px;
    height: var(--row-sidebar);
    border: 0;
    border-radius: var(--radius-row);
    background: transparent;
    color: var(--color-faint);
    cursor: default;
    transition:
      background-color var(--motion-fast) var(--ease-out),
      box-shadow var(--motion-fast) var(--ease-out),
      color var(--motion-fast) var(--ease-out),
      scale var(--motion-slow) var(--ease-spring);
  }

  .rail-item:hover {
    background: var(--row-hover);
    color: var(--color-label-secondary);
  }

  .rail-item:active {
    scale: 0.94;
    transition-duration: var(--motion-instant);
  }

  .rail-item[aria-current="page"] {
    background: var(--row-active);
    box-shadow: var(--row-rim);
  }
</style>
