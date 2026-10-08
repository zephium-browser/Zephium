<script lang="ts">
  import * as m from "$shared/i18n/messages";
  import { untrack } from "svelte";
  import { ListMotion } from "$shared/lib/list-motion";
  import * as tabDrag from "$session/tab-drag.svelte";
  import { RowDrag } from "$session/row-drag.svelte";
  import DragMark from "$shared/ui/DragMark";
  import { Globe02Icon } from "@hugeicons/core-free-icons";
  import { tabs } from "$domain/tabs";
  import type { TabView } from "$shared/ipc/bindings";
  import FavIcon from "$shared/ui/FavIcon";
  import CaptureControl from "$shared/ui/CaptureControl";
  import { stopCaptureFor } from "$domain/capture";
  import { favicons } from "$domain/favicons";

  let {
    entries,
    onSelect,
  }: {
    entries: TabView[];
    onSelect: (id: string) => void;
  } = $props();

  function handleContextMenu(event: MouseEvent, tab: TabView) {
    event.preventDefault();
    drag.abandon();
    tabs.openTabMenu(tab.id, event.clientX, event.clientY);
  }

  // Order and membership, never content: see TabList.
  let list = $state<HTMLUListElement>();
  // Kept sites reorder among themselves; let go over the rail's tabs and the
  // site is no longer kept, anywhere else and native decides (a page).
  const drag = new RowDrag({
    list: () => list,
    reorder: () => ({ essential: true }),
    drop: ({ id, x, y, over, before }) => {
      if (over?.closest("[data-tabs-drop]")) void tabDrag.move(id, false, before);
      else tabs.dropTab(id, x, y);
    },
  });
  const motion = new ListMotion({ enter: () => "grow" });
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

<ul
  bind:this={list}
  class="rail-sites"
  data-essentials-drop
  data-over={tabDrag.overEssentials()}
  role="list"
  aria-label={m.essentials()}
  hidden={entries.length === 0 && tabDrag.draggedId() === null}
>
  {#each entries as tab (tab.id)}
    {@const active = tab.id === tabs.activeId()}
    <li
      data-motion-key={`tab:${tab.id}`}
      data-zephium-tab-id={tab.id}
      data-zephium-tab-url={tab.url ?? ""}
      data-zephium-projection-revision={tab.projection_revision}
    >
      <button
        type="button"
        class="rail-site"
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
      >
        <FavIcon
          image={favicons.image(tab.icon)}
          tone={favicons.tone(tab.icon)}
          loading={tab.loading}
          size={16}
          lit
          fallback={Globe02Icon}
        />
        <span data-zephium-tab-label class="sr-only">{tab.title}</span>
      </button>
      {#if tab.capture}
        <span class="capture-badge">
          {#key tab.capture.navigation_id}<CaptureControl
              onStop={stopCaptureFor(tab.id, tab.capture.navigation_id)}
              site={tab.url ?? tab.title}
              capture={tab.capture}
            />{/key}
        </span>
      {/if}
    </li>
  {/each}
  <!-- With nothing kept yet, a drag still needs somewhere to keep it: one
       empty disc on the kept-site ground, there only while a tab is held. -->
  {#if entries.length === 0}<li class="rail-slot" aria-hidden="true"></li>{/if}
</ul>

{#if drag.ghost}{@const held = tabs.tabs().find((tab) => tab.id === drag.ghost?.id)}<DragMark
    x={drag.ghost.x}
    y={drag.ghost.y}
    image={favicons.image(held?.icon)}
    tone={favicons.tone(held?.icon)}
  />{/if}

<style>
  .rail-sites > li {
    position: relative;
  }

  .capture-badge {
    position: absolute;
    inset-inline-end: -6px;
    inset-block-end: -4px;
    background: var(--color-raised);
    border-radius: var(--radius-capsule);
  }

  .rail-sites {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: var(--sidebar-row-gap);
  }

  /* Kept sites are the rail's one round thing: a disc on the kept-site
     ground, so at rail width they read as their own kind — places you keep —
     rather than as more tabs. Marks stay in full colour, as on the dock. */
  .rail-site {
    display: grid;
    place-items: center;
    width: 36px;
    height: 36px;
    border: 0;
    border-radius: var(--radius-capsule);
    background: var(--color-card);
    cursor: default;
    transition:
      background-color var(--motion-fast) var(--ease-out),
      box-shadow var(--motion-fast) var(--ease-out),
      scale var(--motion-slow) var(--ease-spring);
  }

  .rail-slot {
    width: 36px;
    height: 36px;
    border-radius: var(--radius-capsule);
    background: var(--color-card);
    transition: background-color var(--motion-fast) var(--ease-out);
  }

  /* A tab held over the kept sites: they brighten to say they will take it. */
  .rail-sites:global([data-over="true"]) :is(.rail-site, .rail-slot) {
    background: var(--color-fill-hover);
    box-shadow: inset 0 0 0 1px var(--color-border-strong);
  }

  .rail-site:hover {
    background: var(--color-fill-hover);
  }

  .rail-site:active {
    scale: 0.94;
    transition-duration: var(--motion-instant);
  }

  .rail-site[aria-current="page"] {
    background: var(--color-fill-active);
    box-shadow: var(--row-rim);
  }
</style>
