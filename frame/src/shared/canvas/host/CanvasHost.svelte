<script lang="ts" generics="N extends Node = Node, E extends Edge = Edge">
  import "@xyflow/svelte/dist/base.css";
  import {
    SvelteFlow,
    Background,
    type Edge,
    type Node,
    type NodeTypes,
    type EdgeTypes,
    type SvelteFlowProps,
    type FitViewOptions,
    type useSvelteFlow,
  } from "@xyflow/svelte";
  import { setContext, tick, untrack, type Snippet } from "svelte";
  import { duration, easing, reducedMotion } from "$shared/lib/motion";
  import { bezier, reveal, type Viewport } from "../camera";
  import { cameNear, seenFar } from "../cull";
  import { DIRECTIONS, nextStop } from "../focus";
  import { canvasFar, canvasFocus, canvasSeen, type CanvasFocusState } from "../context";
  import { absolutePosition } from "../nodes";
  import type { CanvasCamera, CanvasCapabilities, CanvasKeyboard } from "./types";
  import FlowHandle from "./FlowHandle.svelte";
  import { layerRelease } from "./layers";
  type Flow = SvelteFlowProps<N, E>;
  let {
    nodes = $bindable(),
    edges,
    nodeTypes,
    edgeTypes,
    viewport = $bindable({ x: 0, y: 0, zoom: 1 }),
    width = $bindable(0),
    height = $bindable(0),
    zoom = { min: 0.2, max: 2 },
    fitView = false,
    fitViewOptions,
    virtualize = false,
    dots,
    panOnDrag = true,
    panKey = " ",
    edgeDescription,
    capabilities = {},
    keyboard,
    onnodeclick,
    onnodepointerenter,
    onnodepointerleave,
    onpaneclick,
    onmovestart,
    onmove,
    onmoveend,
    oninit,
    oncamera,
    children,
  }: {
    nodes: N[];
    edges: E[];
    nodeTypes: NodeTypes;
    edgeTypes?: EdgeTypes;
    viewport?: Viewport;
    width?: number;
    height?: number;
    zoom?: { min: number; max: number };
    fitView?: boolean;
    fitViewOptions?: FitViewOptions<N>;
    /** Past this, the flow keeps only what is on screen in the page. */
    virtualize?: boolean;
    dots?: { color: string; gap: number; size: number };
    panOnDrag?: boolean;
    panKey?: string | null;
    edgeDescription?: string;
    capabilities?: CanvasCapabilities<N, E>;
    keyboard?: CanvasKeyboard;
    onnodeclick?: Flow["onnodeclick"];
    onnodepointerenter?: Flow["onnodepointerenter"];
    onnodepointerleave?: Flow["onnodepointerleave"];
    onpaneclick?: Flow["onpaneclick"];
    onmovestart?: Flow["onmovestart"];
    onmove?: Flow["onmove"];
    onmoveend?: Flow["onmoveend"];
    oninit?: () => void;
    /** The camera, once the flow can move it. */
    oncamera?: (camera: CanvasCamera) => void;
    /** What the top draws inside the flow: its toolbars, its zoom level. */
    children?: Snippet;
  } = $props();
  let root = $state<HTMLDivElement>();
  let flow: ReturnType<typeof useSvelteFlow> | undefined;
  let layers: ReturnType<typeof layerRelease> | undefined;
  $effect(() => {
    const element = root;
    if (!element) return;
    const release = layerRelease(element);
    layers = release;
    return () => {
      release.destroy();
      if (layers === release) layers = undefined;
    };
  });
  // Whatever arrives, leaves or settles on the canvas moves: the layer is let go once it is still.
  $effect(() => {
    void nodes;
    void edges;
    untrack(() => layers?.moved());
  });
  /**
   * The items that have come within a screen of the view. Until one has, a
   * top may draw it as an empty box of its size; once near it stays drawn
   * while the flow keeps it. The set only grows.
   */
  let near = $state.raw<ReadonlySet<string>>(new Set());
  $effect(() => {
    const view = viewport;
    // Before the canvas has measured itself, the window stands in for it.
    const w = width || window.innerWidth;
    const h = height || window.innerHeight;
    const list = nodes;
    untrack(() => {
      const added = cameNear(
        list.map((node) => ({
          id: node.id,
          rect: {
            ...absolutePosition(node, list),
            width: node.width ?? node.measured?.width ?? 0,
            height: node.height ?? node.measured?.height ?? 0,
          },
        })),
        near,
        view,
        w,
        h,
      );
      if (added.length) near = new Set([...near, ...added]);
    });
  });
  setContext(canvasSeen, (id: string) => near.has(id));
  let far = $state(false);
  $effect(() => {
    const level = viewport.zoom;
    untrack(() => (far = seenFar(far, level)));
  });
  setContext(canvasFar, {
    get far() {
      return far;
    },
  });
  const camera: CanvasCamera = {
    glide(view, ms) {
      if (!flow) return;
      void flow.setViewport(view, {
        duration: ms,
        ease: bezier(easing("smooth")),
        interpolate: "linear",
      });
    },
    focus: (id) => void arrive(id),
  };
  let current = $state<string | null>(null);
  /** The stop in the tab order: the one the keyboard was last on while it stands, else the first. */
  const standing = $derived.by(() => {
    const stops = keyboard?.stops() ?? [];
    return stops.some((stop) => stop.id === current) ? current : (stops[0]?.id ?? null);
  });
  const focus: CanvasFocusState = {
    get on() {
      return !!keyboard;
    },
    get current() {
      return standing;
    },
    arrived: (id) => (current = id),
  };
  setContext(canvasFocus, focus);
  /** The keyboard goes to a stop: into view first, then onto it once it is drawn. */
  async function arrive(id: string) {
    const stop = keyboard?.stops().find((candidate) => candidate.id === id);
    if (!stop || !root) return;
    current = id;
    keyboard?.onmove?.(id);
    const view = reveal(stop.rect, viewport, {
      width,
      height,
      top: keyboard?.inset?.top ?? 0,
      bottom: keyboard?.inset?.bottom ?? 0,
    });
    if (view) camera.glide(view, reducedMotion() ? 0 : duration("base"));
    for (let tries = 0; tries < 12; tries++) {
      await tick();
      const element = root?.querySelector<HTMLElement>(`[data-canvas-stop="${CSS.escape(id)}"]`);
      if (element) {
        element.focus({ preventScroll: true });
        return;
      }
      await new Promise((done) => requestAnimationFrame(done));
    }
  }
  $effect(() => {
    const element = root;
    if (!element || !keyboard) return;
    const keys = (event: KeyboardEvent) => {
      if (event.defaultPrevented || event.metaKey || event.ctrlKey || event.altKey) return;
      const target = event.target instanceof HTMLElement ? event.target : null;
      if (!target || target.closest("input, textarea, select, [contenteditable]")) return;
      const at = target.closest<HTMLElement>("[data-canvas-stop]")?.dataset.canvasStop;
      if (!at) return;
      if (event.key === "Escape") {
        event.preventDefault();
        keyboard.onleave?.();
        return;
      }
      if (event.key === "Enter") {
        if (target.closest("button, a[href]")) return;
        event.preventDefault();
        keyboard.onactivate?.(at);
        return;
      }
      const direction = DIRECTIONS[event.key];
      if (!direction) return;
      event.preventDefault();
      if (keyboard.onkey?.(at, direction)) return;
      const next = nextStop(keyboard.stops(), at, direction);
      if (next) void arrive(next);
    };
    element.addEventListener("keydown", keys);
    return () => element.removeEventListener("keydown", keys);
  });
  const drag = $derived(capabilities.drag);
  const marquee = $derived(capabilities.marquee);
  const select = $derived(capabilities.select);
</script>

<div
  class="canvas-host"
  class:keyed={!!keyboard}
  bind:this={root}
  bind:clientWidth={width}
  bind:clientHeight={height}
  role={keyboard ? "group" : undefined}
  aria-label={keyboard?.label}
  aria-roledescription={keyboard?.role}
>
  <SvelteFlow
    bind:nodes
    {edges}
    {nodeTypes}
    {edgeTypes}
    bind:viewport
    {fitView}
    {fitViewOptions}
    minZoom={zoom.min}
    maxZoom={zoom.max}
    nodeExtent={[
      [-1_000_000, -1_000_000],
      [1_000_000, 1_000_000],
    ]}
    proOptions={{ hideAttribution: true }}
    nodesConnectable={false}
    edgesFocusable={false}
    nodesFocusable={keyboard ? false : undefined}
    disableKeyboardA11y={keyboard ? true : undefined}
    nodesDraggable={!!drag}
    autoPanOnNodeDrag={drag?.autoPan}
    nodeDragThreshold={drag?.threshold}
    panOnScroll
    panOnScrollSpeed={1}
    zoomOnScroll={false}
    zoomOnPinch
    zoomOnDoubleClick={false}
    {panOnDrag}
    selectionOnDrag={marquee?.active ?? false}
    selectionKey={marquee ? marquee.key : null}
    selectionMode={marquee?.mode}
    onselectionstart={marquee?.onstart}
    onselectionend={marquee?.onend}
    elementsSelectable={!!select}
    multiSelectionKey={select ? select.multiKey : null}
    elevateNodesOnSelect={select?.elevate ?? false}
    onselectionchange={select?.onchange}
    panActivationKey={panKey}
    ariaLabelConfig={edgeDescription
      ? { "edge.a11yDescription.default": edgeDescription }
      : undefined}
    onlyRenderVisibleElements={virtualize}
    deleteKey={[]}
    {onnodeclick}
    {onnodepointerenter}
    {onnodepointerleave}
    {onpaneclick}
    onnodedragstart={drag?.onstart}
    onnodedragstop={drag?.onstop}
    {onmovestart}
    onmove={(event, view) => {
      layers?.engage();
      onmove?.(event, view);
    }}
    {onmoveend}
    {oninit}
  >
    <FlowHandle
      onready={(handle) => {
        flow = handle;
        oncamera?.(camera);
      }}
    />
    {#if dots}<Background patternColor={dots.color} gap={dots.gap} size={dots.size} />{/if}
    {@render children?.()}
  </SvelteFlow>
</div>

<style>
  .canvas-host {
    position: relative;
    width: 100%;
    height: 100%;
    min-height: inherit;

    --xy-edge-stroke-default: var(--color-border-strong);
    --xy-edge-stroke-width-default: 1.5;
    --xy-handle-background-color-default: transparent;
    --xy-handle-border-color-default: transparent;
    --xy-selection-background-color-default: var(--color-accent-soft);
    --xy-selection-border-default: 1px solid var(--color-accent);
    --xy-background-color-default: transparent;
  }

  .canvas-host :global(.svelte-flow) {
    min-height: inherit;
    background: transparent;
  }

  /* While a person is at the canvas each item is a layer (see layers.ts). */
  /* stylelint-disable-next-line selector-class-pattern */
  .canvas-host :global(.svelte-flow__viewport[data-held] .svelte-flow__node) {
    will-change: transform;
  }

  /* A handle is where a line meets an item, never a thing to see. */
  /* stylelint-disable-next-line selector-class-pattern */
  .canvas-host :global(.svelte-flow__handle) {
    opacity: 0;
  }

  .keyed :global([data-canvas-stop]:focus-visible) {
    outline: 2px solid var(--color-ring);
    outline-offset: 2px;
  }
</style>
