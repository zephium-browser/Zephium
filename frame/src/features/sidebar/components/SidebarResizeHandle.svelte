<script lang="ts">
  import {
    applyDragWidth,
    claimSidebarResizeOwner,
    hideFromDrag,
    isHidden,
    previewDragWidth,
    sidebarResizeRevision,
    beginSidebarResize,
    finishSidebarResize,
    cancelSidebarResize,
    sidebarResizeActive,
    resolveDragWidth,
    COMPACT_WIDTH,
    MAX_EXPANDED_WIDTH,
    MIN_EXPANDED_WIDTH,
    SNAP_THRESHOLD,
  } from "$session/sidebar-mode.svelte";
  import { IS_MAC } from "$shared/platform";
  import { commands } from "$shared/ipc/bindings";
  import { onDestroy } from "svelte";

  let { width, disabled = false }: { width: number; disabled?: boolean } = $props();
  // The core layout's gap between the column and the page.
  const PAGE_GAP = 8;
  let configuredRevision = claimSidebarResizeOwner();
  let nativeCapture = $state(false);
  let resizing = $state(false);
  let guide = $state<number | null>(null);
  let startX = 0;
  let startWidth = $state(0);
  let requested = 0;
  let frame = 0;
  let pending = 0;
  let generation = 0;
  let guideRequested = false;
  // Native draws the guide over pages; the renderer line is only a fallback.
  let nativeGuide = $state(false);
  let previousCursor = "";

  type Configuration = { width: number; enabled: boolean; revision: number; generation: number };
  let pendingConfiguration: Configuration | null = null;
  let configuring = false;
  function queueConfiguration(request: Configuration) {
    pendingConfiguration = request;
    if (!configuring) void configure();
  }
  async function configure() {
    configuring = true;
    try {
      while (pendingConfiguration) {
        const request = pendingConfiguration;
        pendingConfiguration = null;
        let installed = false;
        try {
          installed = await commands.sidebarResize(
            request.width,
            request.enabled,
            request.revision,
          );
        } catch {
          /* The DOM capture remains available when native setup fails. */
        }
        if (request.generation === generation) nativeCapture = request.enabled && installed;
      }
    } finally {
      configuring = false;
    }
  }
  $effect(() => {
    const current = ++generation;
    const value = width;
    const enabled = !disabled;
    configuredRevision = sidebarResizeRevision();
    queueConfiguration({
      width: value,
      enabled,
      revision: configuredRevision,
      generation: current,
    });
  });

  function hideGuide() {
    if (frame !== 0) cancelAnimationFrame(frame);
    frame = 0;
    guide = null;
    if (guideRequested) void commands.sidebarResizeGuide(null).catch(() => {});
    guideRequested = false;
    nativeGuide = false;
  }
  function clearGuide() {
    hideGuide();
    document.body.style.cursor = previousCursor;
  }
  function handlePointerDown(event: PointerEvent) {
    if (disabled || nativeCapture || event.button !== 0) return;
    const target = event.currentTarget;
    if (!(target instanceof HTMLElement)) return;
    event.preventDefault();
    resizing = true;
    startX = event.clientX;
    startWidth = width;
    pending = width;
    target.focus();
    target.setPointerCapture(event.pointerId);
    // Held for the whole drag, over tab rows and pages alike.
    previousCursor = document.body.style.cursor;
    document.body.style.cursor = "col-resize";
    beginSidebarResize();
    updateGuide(width);
  }
  function updateGuide(value: number) {
    requested = value;
    // An expanded column follows the pointer and stops at its narrowest; it
    // never folds into the rail.
    if (startWidth >= MIN_EXPANDED_WIDTH) {
      pending = Math.round(Math.max(MIN_EXPANDED_WIDTH, Math.min(MAX_EXPANDED_WIDTH, value)));
      previewDragWidth(pending);
      hideGuide();
      return;
    }
    // The rail's drag out lands on release; the guide shows where.
    pending = Math.max(COMPACT_WIDTH, Math.min(MAX_EXPANDED_WIDTH, value));
    const landing = resolveDragWidth(pending);
    pending = landing.mode === "compact" ? COMPACT_WIDTH : landing.expanded;
    guide = pending;
    if (frame !== 0) return;
    frame = requestAnimationFrame(() => {
      frame = 0;
      if (!resizing || !sidebarResizeActive()) return;
      guideRequested = true;
      void commands.sidebarResizeGuide(pending).then(
        (drawn) => {
          if (resizing) nativeGuide = drawn;
        },
        () => {},
      );
    });
  }
  function handlePointerMove(event: PointerEvent) {
    if (resizing && sidebarResizeActive()) updateGuide(startWidth + event.clientX - startX);
  }
  function endPointerResize(event: PointerEvent) {
    if (!resizing) return;
    resizing = false;
    const target = event.currentTarget;
    if (target instanceof HTMLElement && target.hasPointerCapture(event.pointerId))
      target.releasePointerCapture(event.pointerId);
    const landing = pending;
    clearGuide();
    // Released past the narrowest, an expanded column hides, as on the native path.
    if (IS_MAC && !isHidden() && startWidth >= MIN_EXPANDED_WIDTH && requested < SNAP_THRESHOLD) {
      hideFromDrag(startWidth);
      return;
    }
    // The guide showed where release lands, snap and bounds included.
    finishSidebarResize(landing);
  }
  function cancelPointerResize(event?: PointerEvent) {
    if (!resizing) return;
    resizing = false;
    const target = event?.currentTarget;
    if (target instanceof HTMLElement && event && target.hasPointerCapture(event.pointerId))
      target.releasePointerCapture(event.pointerId);
    clearGuide();
    cancelSidebarResize();
  }
  function handleKeydown(event: KeyboardEvent) {
    if (event.key === "Escape" && resizing) {
      event.preventDefault();
      cancelPointerResize();
      return;
    }
    if (disabled) return;
    const delta = event.shiftKey ? 24 : 8;
    if (event.key === "ArrowLeft") {
      event.preventDefault();
      applyDragWidth(width - delta);
    } else if (event.key === "ArrowRight") {
      event.preventDefault();
      applyDragWidth(width + delta);
    } else if (event.key === "Home") {
      event.preventDefault();
      applyDragWidth(MIN_EXPANDED_WIDTH);
    } else if (event.key === "End") {
      event.preventDefault();
      applyDragWidth(MAX_EXPANDED_WIDTH);
    }
  }
  onDestroy(() => {
    generation++;
    clearGuide();
    if (resizing) cancelSidebarResize();
    // The latest revision, not the one this handle last configured: opening a
    // tool advances it as the handle leaves, and native refuses a stale one,
    // which left the guide live at the old edge, inside the tool's panel.
    queueConfiguration({ width, enabled: false, revision: sidebarResizeRevision(), generation });
  });
</script>

<svelte:window onblur={() => cancelPointerResize()} onresize={() => cancelPointerResize()} />
<!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
<div
  role="separator"
  aria-label="Resize sidebar"
  aria-orientation="vertical"
  aria-valuemin={COMPACT_WIDTH}
  aria-valuemax={MAX_EXPANDED_WIDTH}
  aria-valuenow={width}
  aria-disabled={disabled}
  tabindex={disabled ? -1 : 0}
  class={[
    // Fills the gap and reaches a little over the page's edge, as the native
    // target on macOS, so the tab list's edge and scrollbar stay usable.
    "group absolute inset-y-0 -right-3.5 z-20 w-4 cursor-col-resize touch-none outline-none",
    // The native control owns the pointer; the tab list's edge stays usable.
    nativeCapture && "pointer-events-none",
  ]}
  onpointerdown={handlePointerDown}
  onpointermove={handlePointerMove}
  onpointerup={endPointerResize}
  onpointercancel={cancelPointerResize}
  onlostpointercapture={cancelPointerResize}
  onkeydown={handleKeydown}
>
  <!-- No mark on hover: only keyboard focus shows where the handle is. -->
  {#if !disabled}<span
      aria-hidden="true"
      class="absolute top-1/2 right-3.5 h-7 w-0.5 -translate-y-1/2 rounded-full bg-transparent group-focus-visible:bg-accent"
    ></span>{/if}
</div>
{#if resizing && guide !== null && !nativeGuide}
  <!-- A renderer-only fallback stays inside chrome; page-overlapping guides are native. -->
  <span
    aria-hidden="true"
    class="pointer-events-none absolute inset-y-0 z-20 w-0.5 bg-accent"
    style:left={`${Math.min(startWidth, guide) + PAGE_GAP - 2}px`}
  ></span>
{/if}
