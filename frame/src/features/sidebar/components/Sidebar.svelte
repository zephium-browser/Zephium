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
    collapseAfterHover,
    effectiveWidth,
    expandForHover,
    isCompact,
    sidebarResizeSettlement,
    toggleMode,
  } from "$session/sidebar-mode.svelte";
  import { uiCommands as ui } from "$domain/ui-commands";
  import { preferences } from "$domain/preferences";
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

  // Hover to expand: a short rest opens the full column so passing over the
  // rail on the way to the page does not, and a short grace on leaving lets
  // the pointer overshoot the edge without the column snapping shut.
  const HOVER_OPEN_MS = 150;
  const HOVER_CLOSE_MS = 300;
  let hoverTimer: ReturnType<typeof setTimeout> | undefined;
  function pointerEnter() {
    pointerIn = true;
    clearTimeout(hoverTimer);
    if (preferences.value("sidebar.hover-expand") !== "true" || !isCompact()) return;
    if (railPage || tools.activeTool() !== null) return;
    hoverTimer = setTimeout(expandForHover, HOVER_OPEN_MS);
  }
  function pointerLeave() {
    pointerIn = false;
    clearTimeout(hoverTimer);
    hoverTimer = setTimeout(collapseAfterHover, HOVER_CLOSE_MS);
  }
  // Work, a tool or the preference turning off while the pointer stays put
  // gets no leave, so drop a pending or open hover expansion here.
  $effect(() => {
    const eligible =
      preferences.value("sidebar.hover-expand") === "true" &&
      !railPage &&
      tools.activeTool() === null;
    untrack(() => {
      if (eligible) return;
      clearTimeout(hoverTimer);
      collapseAfterHover();
    });
  });
  $effect(() => () => {
    clearTimeout(hoverTimer);
    collapseAfterHover();
  });

  $effect(() => {
    const out = () => pointerLeave();
    document.documentElement.addEventListener("mouseleave", out);
    window.addEventListener("blur", out);
    return () => {
      document.documentElement.removeEventListener("mouseleave", out);
      window.removeEventListener("blur", out);
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
  });

  function toggleShape() {
    if (tools.activeTool() !== null) tools.close();
    else toggleMode();
  }
</script>

<aside
  data-tauri-drag-region="deep"
  aria-label={m.ui_browser_sidebar()}
  style:width={`${width}px`}
  style:--sidebar-width={`${width}px`}
  data-menu-material={IS_WINDOWS && !settings ? "opaque" : undefined}
  data-reshaping={reshaping}
  data-sidebar-resize-settling={resizeSettling}
  data-header-fresh={headerFresh}
  data-pointer-in={pointerIn}
  onpointerenter={pointerEnter}
  onpointerleave={pointerLeave}
  class="browser-sidebar relative flex shrink-0 flex-col text-text select-none"
>
  {#if !navigating && !railPage && tools.activeTool() === null}<SidebarResizeHandle
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
</style>
