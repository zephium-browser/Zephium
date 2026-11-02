<script lang="ts">
  import * as m from "$shared/i18n/messages";
  import { rememberScroll } from "$shared/ui/scroll-memory";
  import { Add01Icon } from "@hugeicons/core-free-icons";
  import { tabs } from "$domain/tabs";
  import Icon from "$shared/ui/Icon";
  import type { SidebarEntry } from "../lib/sidebar-model";
  import TabList from "./TabList.svelte";
  import SpaceBar from "./SpaceBar.svelte";
  import SpacePeek from "./SpacePeek.svelte";
  import { remember } from "../lib/space-memory";
  import { arrive, spaceSwipe, stepSpace } from "../lib/space-swipe";
  import { untrack, type Snippet } from "svelte";

  let {
    pinned,
    today,
    splitting,
    onSelect,
    extras,
  }: {
    pinned: SidebarEntry[];
    today: SidebarEntry[];
    splitting: boolean;
    onSelect: (id: string) => void;
    /** Set beside the way to open a tab: pinned extensions. */
    extras?: Snippet;
  } = $props();

  // Private browsing keeps one space, so it has no switcher.
  const spaced = $derived(tabs.profile()?.kind !== "incognito" && tabs.activeSpace() !== undefined);
  const order = $derived(tabs.spaces().map((space) => space.id));
  let stage = $state<HTMLElement>();
  let sheet = $state<HTMLElement>();
  // The neighbours are drawn only while a swipe has them in view.
  let peeking = $state(false);
  const at = $derived(order.indexOf(tabs.activeSpaceId() ?? ""));
  const before = $derived(at > 0 ? tabs.spaces()[at - 1] : undefined);
  const after = $derived(at !== -1 ? tabs.spaces()[at + 1] : undefined);
  // What a space shows is kept, so that it can be drawn beside the next one.
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
    return spaced && at !== -1 && at + step >= 0 && at + step < order.length;
  };
  // Whatever moved the window to another space, the list arrives from the
  // side that space stands on.
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
</script>

<!--
  The only scrolling region in the chrome. Everything above it is pinned so a
  long tab list can never push a scrollbar up alongside the navigation row.

  Pinned tabs are separated by a rule rather than a heading: the rule says
  "these stay" without spending a row on a word, and the rows themselves carry
  the difference by having no close control.
-->
<!-- Opening a tab never scrolls away with the tabs it adds to. -->
{#if spaced}<SpaceBar {extras} />{:else}<div class="new-tab-slot" data-cascade style:--cascade={0}>
    <button type="button" class="new-tab" onclick={tabs.open}>
      <Icon icon={Add01Icon} size={16} />
      <span>{m.new_tab()}</span>
    </button>
    {@render extras?.()}
  </div>{/if}

<div
  bind:this={stage}
  class="space-stage"
  use:spaceSwipe={{ canStep, step: stepSpace, sheet: () => sheet, peek: (on) => (peeking = on) }}
>
  <div bind:this={sheet} class="space-sheet">
    {#if peeking && before}<SpacePeek space={before} side="prev" />{/if}
    <div
      data-sheet="current"
      data-tabs-drop
      data-under-panel
      use:rememberScroll={`${tabs.profile()?.id}/${tabs.activeSpaceId()}/tabs`}
      class="tab-scroller scroll-edges"
      data-glide-scroller
    >
      <TabList
        entries={pinned}
        section="pinned"
        label={m.pinned_tabs()}
        cascadeFrom={1}
        {splitting}
        {onSelect}
      />

      {#if pinned.length > 0}
        <div class="tab-divider" aria-hidden="true"></div>
      {/if}

      <TabList
        entries={today}
        section="today"
        label={m.open_tabs()}
        cascadeFrom={pinned.length + 1}
        {splitting}
        {onSelect}
      />
    </div>
    {#if peeking && after}<SpacePeek space={after} side="next" />{/if}
  </div>
</div>

<style>
  /* The spaces side by side as one sheet; only the middle one is the list. */
  .space-stage {
    position: relative;
    display: flex;
    flex: 1;
    flex-direction: column;
    min-height: 0;
    overflow: clip;
  }

  .space-sheet {
    position: relative;
    display: flex;
    flex: 1;
    flex-direction: column;
    min-height: 0;
  }

  .tab-scroller {
    min-height: 0;
    flex: 1;
    overflow-y: auto;
    overscroll-behavior: contain;
    padding-block-end: 4px;
  }

  .tab-divider {
    height: 1px;
    margin: 5px calc(var(--sidebar-inset) + 8px);
    background: var(--color-border);
  }

  .new-tab-slot {
    display: flex;
    flex: none;
    align-items: center;
    gap: 2px;
    padding-inline: var(--sidebar-inset);
    padding-block: 2px calc(var(--sidebar-row-gap) + 2px);
  }

  .new-tab {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    height: var(--row-sidebar);
    padding-inline: 8px;
    border: 0;
    border-radius: var(--radius-row);
    background: transparent;
    color: var(--color-muted);
    font: inherit;
    font-size: var(--sidebar-row-text);
    font-weight: var(--sidebar-row-weight);
    letter-spacing: -0.005em;
    text-align: start;
    cursor: default;
    outline: none;
    transition:
      background-color var(--motion-fast) var(--ease-out),
      color var(--motion-fast) var(--ease-out);
  }

  .new-tab:hover {
    background: var(--row-hover);
    color: var(--color-label-secondary);
  }
</style>
