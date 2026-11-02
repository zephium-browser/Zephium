<script lang="ts">
  import { rememberScroll } from "$shared/ui/scroll-memory";
  import type { TabView } from "$shared/ipc/bindings";
  import { tabs } from "$domain/tabs";
  import FavIcon from "$shared/ui/FavIcon";
  import { favicons } from "$domain/favicons";
  import { Add01Icon, Cancel01Icon } from "@hugeicons/core-free-icons";
  import { tabFallbackIcon } from "../lib/tab-icon";
  import Icon from "$shared/ui/Icon";
  import * as m from "$shared/i18n/messages";
  import { untrack } from "svelte";
  import { ListMotion } from "$shared/lib/list-motion";
  import * as tabDrag from "$session/tab-drag.svelte";
  import { RowDrag } from "$session/row-drag.svelte";
  import DragMark from "$shared/ui/DragMark";
  import AudioControl from "$shared/ui/AudioControl";
  import CaptureControl from "$shared/ui/CaptureControl";
  import SpacePeek from "./SpacePeek.svelte";
  import RailSpaces from "./RailSpaces.svelte";
  import { uiCommands as ui } from "$domain/ui-commands";
  import { remember } from "../lib/space-memory";
  import * as requests from "../lib/space-requests.svelte";
  import { arrive, spaceSwipe, stepSpace } from "../lib/space-swipe";
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
    region: () => scroller,
    reorder: () => ({ essential: false }),
    drop: ({ id, x, y, over, before }) => {
      const essential = !!over?.closest("[data-essentials-drop]");
      // A tab that cannot be kept goes back where it was.
      if (essential && !tabDrag.keepable(id)) return;
      if (tabs.inSplit(id) && (essential || over?.closest("[data-tabs-drop]")))
        void tabDrag.leaveSplit(id, essential || before ? { essential, before } : null);
      else if (essential) void tabDrag.move(id, true, before);
      else tabs.dropTab(id, x, y);
    },
  });
  const motion = new ListMotion({ enter: () => "rise" });

  const space = $derived(tabs.profile()?.kind === "incognito" ? undefined : tabs.activeSpace());
  const order = $derived(tabs.spaces().map((entry) => entry.id));
  let scroller = $state<HTMLElement>();
  let stage = $state<HTMLElement>();
  let sheet = $state<HTMLElement>();
  let peeking = $state(false);
  const at = $derived(order.indexOf(tabs.activeSpaceId() ?? ""));
  const before = $derived(at > 0 ? tabs.spaces()[at - 1] : undefined);
  const after = $derived(at !== -1 ? tabs.spaces()[at + 1] : undefined);
  $effect(() => {
    const id = tabs.activeSpaceId();
    const nodes = tabs.sidebarNodes();
    const all = tabs.tabs();
    const active = tabs.activeId();
    const spaces = tabs.spaces();
    if (id) untrack(() => remember(id, { nodes, tabs: all, active }, spaces));
  });
  const canStep = (step: 1 | -1) => {
    const at = order.indexOf(tabs.activeSpaceId() ?? "");
    return space !== undefined && at !== -1 && at + step >= 0 && at + step < order.length;
  };
  let shownSpace: string | null = null;
  $effect(() => {
    const next = tabs.activeSpaceId();
    untrack(() => {
      const from = shownSpace === null ? -1 : order.indexOf(shownSpace);
      const to = next === null ? -1 : order.indexOf(next);
      if (shownSpace !== null && next !== shownSpace && from !== -1 && to !== -1)
        arrive(stage, to > from ? 1 : -1);
      shownSpace = next;
    });
  });
  // Making or editing a space from a menu opens the column in place for it.
  $effect(() => {
    const command = ui.uiCommand();
    if (command.id !== "space.new" && command.id !== "space.edit") return;
    untrack(() => {
      if (!requests.claim(command.seq) || space === undefined) return;
      requests.openInColumn(command.id === "space.new" ? "new" : "edit");
    });
  });
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
  The rail reads top-down like the expanded list it stands in for. It opens
  with the spaces, which switch from a short stack and take the column in place
  to be made or edited, and ends
  the way the list does, with the row that opens a new tab after the last one.
  `title` gives a real OS tooltip, which is the only label that can appear
  outside the chrome WebView's rectangle.
-->
{#if space}<div class="rail-spaces-slot" data-cascade style:--cascade={0}>
    <RailSpaces active={space} />
  </div>{/if}
<div
  bind:this={stage}
  class="rail-stage"
  use:spaceSwipe={{ canStep, step: stepSpace, sheet: () => sheet, peek: (on) => (peeking = on) }}
>
  <div bind:this={sheet} class="rail-sheet">
    {#if peeking && before}<SpacePeek space={before} side="prev" rail />{/if}
    <div
      bind:this={scroller}
      data-sheet="current"
      use:rememberScroll={`${tabs.profile()?.id}/${tabs.activeSpaceId()}/rail`}
      class="rail-scroller scroll-edges"
      data-under-panel
      data-glide-scroller
    >
      <ul bind:this={list} class="rail-list" data-tabs-drop role="list" aria-label={m.open_tabs()}>
        {#each entries as tab, index (tab.id)}
          {@const active = tab.id === tabs.activeId()}
          <li
            data-cascade
            style:--cascade={index + 1}
            data-motion-key={`tab:${tab.id}`}
            data-zephium-tab-id={tab.id}
            data-zephium-tab-url={tab.url ?? ""}
            data-zephium-projection-revision={tab.projection_revision}
            data-audio-host
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
                fallback={tabFallbackIcon(tab.content)}
                notched={!!tab.audio}
              />
              <span data-zephium-tab-label class="sr-only">{tab.title}</span>
            </button>
            {#if tab.capture}<span class="rail-capture"
                ><CaptureControl
                  badge
                  site={tab.url ?? tab.title}
                  capture={tab.capture}
                  onOpen={(x, y) => tabs.openMediaMenu(tab.id, x, y)}
                /></span
              >{/if}
            {#if tab.audio}<AudioControl
                audio={tab.audio}
                size={16}
                onToggle={(muted) => void tabs.setMuted(tab.id, muted)}
              />{/if}
            <!-- A rail has no row to put a close control in, so it rides the
             mark's corner, the way a notification's does. -->
            {#if !tabs.loneNewTab(tab.id)}<button
                type="button"
                class="rail-close"
                title={m.close_tab()}
                aria-label={m.close_named_tab({ title: tab.title || m.untitled_tab() })}
                onclick={() => tabs.close(tab.id)}
                ><Icon icon={Cancel01Icon} size={8} strokeWidth={2.4} /></button
              >{/if}
          </li>
        {/each}
        <li data-cascade style:--cascade={entries.length + 1}>
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
    {#if peeking && after}<SpacePeek space={after} side="next" rail />{/if}
  </div>
</div>

{#if drag.ghost}{@const held = tabs.tabs().find((tab) => tab.id === drag.ghost?.id)}<DragMark
    x={drag.ghost.x}
    y={drag.ghost.y}
    image={favicons.image(held?.icon)}
    tone={favicons.tone(held?.icon)}
    title={held?.title}
  />{/if}

<style>
  /* Close under the head's rule, so the spaces read as the first thing of
     the column rather than a thing apart from it. */
  .rail-spaces-slot {
    display: flex;
    flex: none;
    flex-direction: column;
    align-items: center;
    gap: var(--sidebar-row-gap);
    padding-block: 0 2px;
  }

  .rail-stage {
    position: relative;
    display: flex;
    flex: 1;
    flex-direction: column;
    min-height: 0;
    overflow: clip;
  }

  .rail-sheet {
    position: relative;
    display: flex;
    flex: 1;
    flex-direction: column;
    min-height: 0;
  }

  .rail-scroller {
    display: flex;
    flex: 1;
    flex-direction: column;
    min-height: 0;
    overflow-y: auto;
    overscroll-behavior: contain;
    padding-block: 4px 8px;
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
     and fills, never a disc of its own, and square at the row's height. */
  :global(.rail-item) {
    display: grid;
    place-items: center;
    width: var(--row-sidebar);
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

  :global(.rail-item:hover),
  :global(.rail-item[data-state="open"]) {
    background: var(--row-hover);
    color: var(--color-label-secondary);
  }

  :global(.rail-item:active) {
    scale: 0.94;
    transition-duration: var(--motion-instant);
  }

  :global(.rail-item[aria-current="page"]) {
    background: var(--row-active);
    box-shadow: var(--row-rim);
  }

  .rail-list > li {
    position: relative;
  }

  .rail-capture {
    position: absolute;
    inset-block-end: -3px;
    inset-inline-start: -3px;
  }

  /* A small raised disc on the mark's corner: it waits a beat before it
     appears, so running the pointer down the rail does not set every one off. */
  .rail-close {
    position: absolute;
    inset-block-start: -3px;
    inset-inline-end: -3px;
    display: grid;
    place-items: center;
    width: 16px;
    height: 16px;
    padding: 0;
    border: 0;
    border-radius: var(--radius-capsule);
    background: var(--color-float);
    box-shadow: var(--shadow-badge);
    color: var(--color-muted);
    opacity: 0;
    scale: 0.6;
    pointer-events: none;
    cursor: default;
    transition:
      opacity var(--motion-fast) var(--ease-out),
      scale var(--motion-base) var(--ease-spring),
      color var(--motion-instant) var(--ease-out);
  }

  .rail-close:focus-visible {
    opacity: 1;
    scale: 1;
    pointer-events: auto;
  }

  .rail-close:hover {
    color: var(--color-text);
  }

  .rail-list > li:hover .rail-close {
    opacity: 1;
    scale: 1;
    pointer-events: auto;
    transition-delay: 160ms, 160ms, 0ms;
  }

  .rail-list > li:hover .rail-close:active {
    scale: 0.9;
  }
</style>
