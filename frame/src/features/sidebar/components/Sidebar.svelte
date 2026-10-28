<script lang="ts">
  import * as m from "$shared/i18n/messages";
  import type { Snippet } from "svelte";
  import { IS_MAC, IS_WINDOWS } from "$shared/platform";
  import * as motion from "$session/motion.svelte";
  import { surface as browserPage } from "$domain/surface";
  import * as tools from "$session/tools.svelte";
  import { untrack } from "svelte";
  import {
    COMPACT_WIDTH,
    PANEL_GAP,
    effectiveWidth,
    holdHidden,
    isCompact,
    isHidden,
    isPeeking,
    peek,
    sidebarResizeActive,
    sidebarResizeSettlement,
    toggleHidden,
    toggleMode,
    unpeek,
  } from "$session/sidebar-mode.svelte";
  import { uiCommands as ui } from "$domain/ui-commands";
  import SidebarHeader from "./SidebarHeader.svelte";
  import * as shapeMorph from "../lib/shape-morph";
  import { duration, reducedMotion } from "$shared/lib/motion";
  import SidebarResizeHandle from "./SidebarResizeHandle.svelte";

  let {
    settingsNavigation,
    toolPanel,
    browserBody,
    dock,
  }: {
    settingsNavigation: Snippet;
    toolPanel: Snippet<[tools.ToolKind]>;
    browserBody: Snippet<[boolean]>;
    dock: Snippet<[boolean]>;
  } = $props();
  let settings = $derived(browserPage.currentPage() === "settings");
  const SETTINGS_PANEL = 264;
  let taskPage = $derived(
    browserPage.currentPage() === "tasks" || browserPage.currentPage() === "notes",
  );
  // Work gives the window to its canvas: the column is always the rail there.
  let inWork = $derived(browserPage.currentPage() === "work");
  let railPage = $derived(taskPage || inWork);
  // Work keeps the column as its compact rail, always: a panel open in Browse
  // closes on the way in, and a tool asked for while in Work opens over its
  // canvas instead of beside it.
  let settledInWork = false;
  $effect(() => {
    const tool = tools.activeTool();
    const work = inWork;
    untrack(() => {
      if (work && tool !== null) {
        tools.close();
        if (settledInWork)
          window.dispatchEvent(new CustomEvent("zephium:work-tool", { detail: tool }));
      }
      settledInWork = work;
    });
  });
  // Settings opens like a tool: the column keeps its tab rail, and its sections
  // stand in the panel beside it while the page shows the section. Tasks and
  // Notes are part of browsing too: a tab is one click away and choosing it
  // returns to that page.
  let navigating = $derived(settings);
  let paneled = $derived(settings || tools.activeTool() !== null);
  let compact = $derived(navigating || railPage || isCompact() || tools.activeTool() !== null);
  // Settings, a tool or a rail page stands beside the column, so it cannot hide.
  $effect(() => {
    const held = navigating || railPage || tools.activeTool() !== null;
    untrack(() => holdHidden(held));
  });
  let hidden = $derived(isHidden());
  let peeking = $derived(isPeeking());

  // A change of shape is one continuous change: every mark travels from the
  // old shape into the new one while native slides the page to match. Not
  // on the first shape, which launch brings in with its own cascade.
  let columns = $state<HTMLElement>();
  let shown: boolean | null = null;
  let morph: ReturnType<typeof shapeMorph.capture> = null;
  // While the shape changes, the column's width travels with the page
  // instead of jumping, and its contents hold their final width so only the
  // space beside them moves. A resize guide commits its width immediately on release.
  let reshaping = $state(false);
  let resizeSettling = $state(false);
  let observedResizeSettlement = sidebarResizeSettlement();
  // The header only fades in when it is the other header, not merely beside
  // a column that changed (a tool opening keeps the same header).
  let headerCompact = $derived(compact && !paneled);
  let headerFresh = $state(false);
  let shownHeader: boolean | null = null;
  let reshaped: ReturnType<typeof setTimeout> | undefined;
  $effect(() => () => clearTimeout(reshaped));
  $effect.pre(() => {
    const next = compact;
    const head = headerCompact;
    const settlement = sidebarResizeSettlement();
    untrack(() => {
      // A drag that only changes the width lands at once; one that snaps the
      // column to the other shape travels like any change of shape, since
      // native slides the page with it.
      if (settlement !== observedResizeSettlement && (shown === null || shown === next)) {
        observedResizeSettlement = settlement;
        resizeSettling = true;
        queueMicrotask(() => {
          if (observedResizeSettlement === settlement) resizeSettling = false;
        });
        morph = null;
        reshaping = false;
        headerFresh = false;
        clearTimeout(reshaped);
        shown = next;
        shownHeader = head;
        return;
      }
      observedResizeSettlement = settlement;
      const headChanged = shownHeader !== null && shownHeader !== head;
      shownHeader = head;
      if (shown === null || shown === next) return;
      morph = shapeMorph.capture(columns);
      if (reducedMotion()) return;
      reshaping = true;
      headerFresh = headChanged;
      clearTimeout(reshaped);
      reshaped = setTimeout(
        () => {
          reshaping = false;
          headerFresh = false;
        },
        duration("page") + 40,
      );
    });
  });
  $effect(() => {
    const next = compact;
    untrack(() => {
      if (shown !== null && shown !== next) shapeMorph.play(morph);
      morph = null;
      shown = next;
    });
  });
  let width = $derived(
    navigating
      ? COMPACT_WIDTH + PANEL_GAP + SETTINGS_PANEL
      : railPage && tools.activeTool() === null
        ? COMPACT_WIDTH
        : effectiveWidth(),
  );

  // Dispatch runs untracked and behind a sequence guard. Handlers read the
  // state they mutate (the sidebar shape, the tab projection), so a tracked
  // effect would subscribe to its own writes and re-fire on the same command,
  // which made the collapsed menu's Compact Mode toggle immediately undo
  // itself.
  // Whether the pointer is over the column, for what shows only while it is.
  // Tracked rather than left to :hover, which a move straight onto a page's
  // own view can leave standing; leaving the document or the window losing
  // focus ends it too.
  let pointerIn = $state(false);
  // A hidden column floats in as the pointer reaches the window's left edge,
  // or passes beyond it, and leaves as soon as the pointer has gone, unless
  // something in it still holds the person: an open menu, the field they are
  // typing in, or a resize. Native says where the pointer is; the column's
  // side spans it and an allowance over the page beside it, so a pointer that
  // strays a little past its edge keeps it, as in Arc.
  let aside = $state<HTMLElement>();
  let leaving: ReturnType<typeof setTimeout> | undefined;
  let over = false;
  // Counts what native says about the pointer, and window blurs. Native only
  // reports a departure once the column is up, so anything counted while it
  // rises is newer than the answer raising gives, and wins over it.
  let heard = 0;
  const LEAVE_DELAY = 0;
  // The slide, both ways; matches --sidebar-slide below.
  const LEAVE_SLIDE = 200;
  function stay() {
    clearTimeout(leaving);
    leaving = undefined;
  }
  function holding() {
    if (sidebarResizeActive()) return true;
    // An open panel or popup, which would vanish with the column. Not an
    // expanded folder: it stays open, and would hold the column up for good.
    if (
      aside?.querySelector(
        '.sidebar-softens[data-open="true"], [aria-haspopup][aria-expanded="true"]',
      )
    )
      return true;
    // Only while this document has the keyboard: a field keeps being the
    // active element after the person has moved on to a page, which would
    // otherwise hold the column up for good.
    const active = document.activeElement;
    return (
      document.hasFocus() &&
      active instanceof HTMLInputElement &&
      active.type !== "button" &&
      !!aside?.contains(active)
    );
  }
  // Scheduled even before the column is up: the pointer can leave while it
  // is still being raised.
  function leave() {
    if (!hidden) return;
    stay();
    leaving = setTimeout(() => {
      leaving = undefined;
      if (!peeking || over || holding()) return;
      unpeek(LEAVE_SLIDE);
    }, LEAVE_DELAY);
  }
  $effect(() => () => clearTimeout(leaving));
  // Native watches the window's edge while the column is hidden, and the
  // floating column while it is up: this page cannot see the pointer reach
  // the edge from over a page.
  $effect(() => {
    const edge = (event: Event) => {
      if (!(event instanceof CustomEvent) || typeof event.detail?.inside !== "boolean") return;
      heard++;
      over = event.detail.inside;
      if (over) {
        stay();
        const asked = heard;
        void peek().then((onSide) => {
          if (onSide === null) return;
          if (heard === asked) over = onSide;
          if (!over) leave();
        });
      } else {
        pointerIn = false;
        leave();
      }
    };
    window.addEventListener("zephium:sidebar-edge", edge);
    return () => window.removeEventListener("zephium:sidebar-edge", edge);
  });
  $effect(() => {
    if (!hidden) stay();
  });
  $effect(() => {
    const out = () => (pointerIn = false);
    // Native watches only the key window, so leaving it ends the watch.
    const blur = () => {
      out();
      heard++;
      over = false;
      leave();
    };
    document.documentElement.addEventListener("mouseleave", out);
    window.addEventListener("blur", blur);
    return () => {
      document.documentElement.removeEventListener("mouseleave", out);
      window.removeEventListener("blur", blur);
    };
  });

  let handledCommand = 0;
  $effect(() => {
    const command = ui.uiCommand();
    if (command.seq === 0 || command.seq === handledCommand) return;
    handledCommand = command.seq;

    // Changed after this pass rather than inside it, as a click would: a
    // shape changed mid-pass re-renders before the old shape is measured,
    // so nothing could travel from it.
    if (command.id === "sidebar.toggleCompact") queueMicrotask(toggleShape);
    if (command.id === "sidebar.toggleHidden") queueMicrotask(toggleHidden);
  });

  function toggleShape() {
    if (tools.activeTool() !== null) tools.close();
    else toggleMode();
  }
</script>

<aside
  bind:this={aside}
  data-tauri-drag-region="deep"
  aria-label={m.ui_browser_sidebar()}
  inert={hidden && !peeking}
  style:width={`${width}px`}
  style:--sidebar-width={`${width}px`}
  data-menu-material={IS_WINDOWS && !settings ? "opaque" : undefined}
  data-reshaping={reshaping}
  data-sidebar-resize-settling={resizeSettling}
  data-header-fresh={headerFresh}
  data-pointer-in={pointerIn}
  data-hidden={hidden}
  data-peek={hidden && peeking}
  onpointerenter={() => {
    pointerIn = true;
    stay();
  }}
  onpointerleave={() => (pointerIn = false)}
  onfocusout={leave}
  class="browser-sidebar relative flex shrink-0 flex-col text-text select-none"
>
  {#if (!hidden || peeking) && !navigating && !railPage && tools.activeTool() === null}<SidebarResizeHandle
      {width}
      disabled={reshaping}
    />{/if}
  <!-- Settings stands beside the rail with no head: its panel runs the full
       height and the rail keeps only the lights' row. A tool keeps the head,
       with search and the page's own controls. -->
  {#if !settings}<SidebarHeader
      compact={headerCompact}
      launcher={paneled}
      ontoggle={toggleShape}
      pageControls={!inWork}
    />{/if}
  <div
    bind:this={columns}
    class="sidebar-columns"
    data-glide-host
    data-launch={motion.launchState()}
  >
    {#key compact}<div class="sidebar-browser-column" class:sidebar-tool-rail={paneled}>
        {#if settings && IS_MAC}<div class="sidebar-lights" aria-hidden="true"></div>{/if}
        {@render browserBody(compact)}
        {@render dock(compact)}
      </div>{/key}
    {#if settings}<div class="sidebar-tool-host">
        {@render settingsNavigation()}
      </div>{:else if tools.activeTool() !== null}<div class="sidebar-tool-host">
        {@render toolPanel(tools.activeTool()!)}
      </div>{/if}
  </div>
</aside>

<style>
  /* A panel open in the column (the space switcher, the tray on the address,
     the profile's stack) is the one thing to read: whatever it lies over
     steps back under it, and comes forward again when it closes. */
  :global([data-under-panel]) {
    transition: opacity var(--motion-base) var(--ease-out);
  }

  :global(
    .browser-sidebar:has(.sidebar-softens[data-open="true"])
      [data-under-panel]:not(:has(.sidebar-softens[data-open="true"]))
  ) {
    opacity: 0.18;
  }

  @media (prefers-reduced-motion: reduce) {
    :global([data-under-panel]) {
      transition: none;
    }
  }

  /* Hidden, the column leaves the row to the page and waits off the window's
     edge. Called up, it floats over the page on a solid card, its edge drawn
     out into the gutter around it. */
  .browser-sidebar[data-hidden="true"] {
    --sidebar-slide: 200ms;

    position: fixed;
    inset-block: 8px;
    inset-inline-start: 8px;
    z-index: 30;
    border-radius: var(--content-radius);
    background: var(--color-float);
    box-shadow:
      0 0 0 8px var(--color-float),
      var(--shadow-menu);
    translate: calc(-100% - 24px) 0;

    /* Leaving keeps one even pace: an arriving curve would cover most of
       the way in its first beat. */
    transition: translate var(--sidebar-slide) cubic-bezier(0.37, 0, 0.63, 1);

    /* The slide is where the column is, not decoration. Like the page's own
       slide as the column hides or shows, which native draws whatever the
       motion setting, it stays under reduced motion, which otherwise
       collapses every transition to a frame. */
    transition-duration: var(--sidebar-slide) !important;
  }

  /* Arriving settles in on the curve of the page's own slide. */
  .browser-sidebar[data-peek="true"] {
    translate: 0 0;
    transition: translate var(--sidebar-slide) var(--ease-emphasized);
    transition-duration: var(--sidebar-slide) !important;
  }

  /* The new tab fills the window, flush with its edge as a page would be. */
  :global(.shell:has(> .browser-sidebar[data-hidden="true"]) > main) {
    padding-inline-start: 0;
  }
</style>
