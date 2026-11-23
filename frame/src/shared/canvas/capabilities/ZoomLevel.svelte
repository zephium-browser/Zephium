<script lang="ts">
  import { onMount } from "svelte";
  import { useSvelteFlow, useViewport, Panel } from "@xyflow/svelte";
  import { duration, reducedMotion } from "$shared/lib/motion";
  let {
    label,
    fitLabel,
    dock,
    bottomInset = 0,
    onfit,
  }: {
    label: string;
    fitLabel: string;
    /** A selector for an edge slot its owner keeps for the level; without one it stands in the flow's corner. */
    dock?: string;
    bottomInset?: number;
    /** The person asked for the whole picture. */
    onfit?: () => void;
  } = $props();
  const flow = useSvelteFlow();
  const viewport = useViewport();
  const percent = $derived(Math.round(viewport.current.zoom * 100));
  /** A mark inside the flow: the level itself is drawn in the canvas's edge slot. */
  let anchor = $state<HTMLElement>();
  /** The edge slot its owner keeps for the level, when there is one. */
  let docked = $state<HTMLElement | null>(null);
  onMount(() => {
    const selector = dock;
    if (!selector) return;
    const find = () => {
      for (let at = anchor?.parentElement; at; at = at.parentElement) {
        const found = at.querySelector<HTMLElement>(selector);
        if (found) return found;
      }
      return null;
    };
    docked = find();
    if (docked) return;
    // The slot can arrive a frame after the canvas.
    const frame = requestAnimationFrame(() => (docked = find()));
    return () => cancelAnimationFrame(frame);
  });
  /** Draws the level in the edge slot. */
  function into(node: HTMLElement, target: HTMLElement) {
    // One element moves, so Svelte's removal of the block still finds it.
    target.append(node);
    return { destroy: () => node.remove() };
  }
  /** The canvas the pointer last pressed in, so a key reaches it while nothing else has focus. */
  let engaged = false;
  const travel = (name: "fast" | "slow") => (reducedMotion() ? 0 : duration(name));
  function fit() {
    onfit?.();
    void flow.fitView({
      duration: travel("slow"),
      padding:
        bottomInset > 0
          ? { top: "110px", bottom: `${bottomInset + 32}px`, left: "40px", right: "40px" }
          : 0.2,
    });
  }
  /** 0 fits the view and 1 shows it at actual size, while the canvas holds the keyboard. */
  function keys(event: KeyboardEvent) {
    if (event.defaultPrevented || event.metaKey || event.ctrlKey || event.altKey) return;
    if (event.key !== "0" && event.key !== "1") return;
    const canvas = anchor?.closest(".svelte-flow");
    const active = document.activeElement;
    const inside = !!active && !!canvas?.contains(active);
    if (!inside && !(engaged && (!active || active === document.body))) return;
    if ((event.target as Element | null)?.closest?.("input, textarea, select, [contenteditable]"))
      return;
    event.preventDefault();
    if (event.key === "0") fit();
    else void flow.setZoom(1, { duration: travel("slow") });
  }
</script>

<svelte:window
  onpointerdowncapture={(event) =>
    (engaged = !!anchor?.closest(".svelte-flow")?.contains(event.target as Node))}
  onkeydown={keys}
/>

<span class="anchor" hidden bind:this={anchor}></span>
{#snippet capsule()}
  <div class="zoom" role="group" aria-label={label}>
    <button
      type="button"
      class="level"
      aria-label={fitLabel}
      title={`${fitLabel} (0)`}
      onclick={fit}>{percent}%</button
    >
  </div>
{/snippet}
{#if docked}<div class="docked-host" use:into={docked}>{@render capsule()}</div>
{:else}<Panel position="bottom-right">{@render capsule()}</Panel>{/if}

<style>
  .docked-host {
    display: flex;
  }

  /* A reading more than a control: the level, quiet at the canvas's edge,
     and a click fits the view to it. */
  .zoom {
    display: inline-flex;
    align-items: center;
  }

  .level {
    min-inline-size: 48px;
    block-size: 28px;
    padding: 0 8px;
    border: 0;
    border-radius: var(--radius-capsule);
    background: transparent;
    color: var(--color-muted);
    font: inherit;
    font-size: var(--text-label);
    font-weight: 500;
    font-variant-numeric: tabular-nums;
    cursor: default;
    transition:
      background-color var(--motion-fast) var(--ease-out),
      color var(--motion-fast) var(--ease-out);
  }

  .level:hover {
    background: var(--color-fill-hover);
    color: var(--color-text);
  }

  .level:active {
    background: var(--color-fill-active);
  }

  .level:focus-visible {
    outline: 2px solid var(--color-ring);
    outline-offset: -2px;
  }
</style>
