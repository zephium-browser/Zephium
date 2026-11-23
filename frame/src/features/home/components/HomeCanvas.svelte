<script lang="ts">
  import type { Node } from "@xyflow/svelte";
  import { onDestroy, setContext, tick, untrack } from "svelte";
  import { SvelteSet } from "svelte/reactivity";
  import { reveal } from "$shared/canvas";
  import { CanvasHost, type CanvasCamera, type CanvasKeyboard } from "$shared/canvas/host";
  import { duration, reducedMotion } from "$shared/lib/motion";
  import * as m from "$shared/i18n/messages";
  import { homeTop, type HomeTop } from "../lib/context";
  import { layout, SEARCH_X, SIZE, type HomeNode } from "../lib/layout";
  import { admit, type HomeIntent, type HomeModel, type Place, type Row } from "../lib/model";
  import AddressLine from "./cards/AddressLine.svelte";
  import AlsoCard from "./cards/AlsoCard.svelte";
  import CompactLine from "./cards/CompactLine.svelte";
  import DayMark from "./cards/DayMark.svelte";
  import InsideCard from "./cards/InsideCard.svelte";
  import Lines from "./cards/Lines.svelte";
  import PageCard from "./cards/PageCard.svelte";
  import PlaceCard from "./cards/PlaceCard.svelte";
  import SearchCard from "./cards/SearchCard.svelte";
  import SocialCard from "./cards/SocialCard.svelte";
  import ThinPlace from "./cards/ThinPlace.svelte";
  import HoverCard from "./HoverCard.svelte";

  let {
    home: given,
    onintent,
    picture = () => null,
    mark = () => null,
    inset = { top: 0, bottom: 0 },
    onready,
  }: {
    home: HomeModel;
    onintent: (intent: HomeIntent) => void;
    /** Where a kept picture is drawn from. */
    picture?: (key: string) => string | null;
    mark?: (place: Place) => ImageData | null;
    /** How much of the canvas's top and foot Home's own frame covers. */
    inset?: { top: number; bottom: number };
    /** The way in from the field: the keyboard goes to the newest search. */
    onready?: (enter: () => void) => void;
  } = $props();

  const nodeTypes = {
    search: SearchCard,
    compact: CompactLine,
    place: PlaceCard,
    thin: ThinPlace,
    inside: InsideCard,
    also: AlsoCard,
    social: SocialCard,
    page: PageCard,
    address: AddressLine,
    day: DayMark,
    lines: Lines,
  };

  let home = $derived(admit(given));
  const expanded = new SvelteSet<string>();
  const shown = new SvelteSet<string>();
  // The newest search is in front until the person brings another there; a
  // new search comes to the front of itself.
  let newest = $derived(home.today.entries?.find((entry) => entry.kind !== "address")?.id ?? null);
  let chosen = $state<{ search: string; newest: string | null } | null>(null);
  let front = $derived.by(() => {
    if (!chosen || chosen.newest !== newest) return newest;
    const wanted = chosen.search;
    const held = [home.today, ...home.earlier].some((day) =>
      day.entries?.some((entry) => entry.id === wanted && entry.kind !== "address"),
    );
    return held ? wanted : newest;
  });
  let laid = $derived(
    layout([home.today, ...home.earlier], home.today.key, { expanded, front, shown }),
  );
  // Only the search that came to the front and the one that left it arrive; nothing else moves of itself.
  let moved = $state<{ to: string; from: string | null } | null>(null);
  let arriving = $derived(
    new Set(
      moved
        ? [moved.to, moved.from].flatMap((lane) => (lane ? (laid.lanes[lane]?.nodes ?? []) : []))
        : [],
    ),
  );
  const flowOf = (node: HomeNode): Node<{ item: HomeNode }> => ({
    id: node.id,
    type: node.kind,
    position: { x: node.box.x, y: node.box.y },
    width: node.box.width,
    height: node.box.height,
    data: { item: node },
    draggable: false,
    selectable: false,
    focusable: false,
    zIndex: node.kind === "lines" ? 0 : 1,
    class: arriving.has(node.id) ? "home-arrive" : undefined,
  });
  let nodes = $state.raw<Node[]>([]);
  $effect.pre(() => {
    nodes = laid.nodes.map(flowOf);
  });
  let byStop = $derived(new Map(laid.stops.map((stop) => [stop.id, stop])));
  // The keyboard starts on the asked row of the newest search.
  let stops = $derived.by(() => {
    const first = laid.first ? byStop.get(laid.first) : undefined;
    return first ? [first, ...laid.stops.filter((stop) => stop !== first)] : laid.stops;
  });

  const EDGE = 24;
  const LEAST = 0.8;
  let width = $state(0);
  let height = $state(0);
  let view = $state({ x: 0, y: 0, zoom: 1 });
  let placed = $state(false);
  $effect.pre(() => {
    if (placed || width <= 0) return;
    view = firstView(laid.head ?? { x: SEARCH_X, y: 0, width: -SEARCH_X * 2, height: 0 });
    placed = true;
  });
  let camera: CanvasCamera | undefined;
  // The way in is handed over once the camera can move and the first items are drawn to land on.
  let drawn = false;
  let handed = false;
  function handOver() {
    if (handed || !camera) return;
    handed = true;
    onready?.(async () => {
      // Just drawn, the flow may not take focus yet; it is tried for a few frames.
      for (let tries = 0; tries < 30; tries++) {
        const to = laid.first ?? stops[0]?.id;
        if (!to) return;
        camera?.focus(to);
        await new Promise((done) => requestAnimationFrame(done));
        if (root?.contains(document.activeElement)) return;
      }
    });
  }
  let root = $state<HTMLDivElement>();

  async function open(search: string) {
    if (search === front) return;
    const keyed = !!root?.contains(document.activeElement);
    moved = { to: search, from: front };
    chosen = { search, newest };
    await tick();
    const lane = laid.lanes[search];
    if (!lane || !camera) return;
    if (keyed) camera.focus(`s:${search}`);
    const next = reveal(lane.box, view, { width, height, ...inset }, 24);
    if (next) camera.glide(next, reducedMotion() ? 0 : duration("base"));
  }
  // The place stands under the field; when the newest entry reaches past the
  // page it moves aside, and only when it cannot fit at all does the view step back.
  function firstView(head: { x: number; width: number }) {
    const room = width - EDGE * 2;
    const zoom = Math.min(1, Math.max(LEAST, room / head.width));
    const x =
      head.width * zoom > room
        ? EDGE - head.x * zoom
        : Math.min(
            Math.max(width / 2, EDGE - head.x * zoom),
            width - EDGE - (head.x + head.width) * zoom,
          );
    return { x, y: inset.top + 24, zoom };
  }

  const intent = (next: HomeIntent) => onintent(next);
  const keyboard: CanvasKeyboard = {
    stops: () => stops,
    label: m.home_canvas(),
    role: m.home_canvas_role(),
    get inset() {
      return inset;
    },
    onkey(id, direction) {
      const stop = byStop.get(id);
      if (!stop) return false;
      if (direction === "right" && stop.row) {
        const { inside, first, search, row, menu } = stop.row;
        if (!inside) {
          if (menu) top.menu(search, row);
          else intent({ kind: "inside", search, row });
        } else if (first) camera?.focus(first);
        return true;
      }
      if (direction === "left" && stop.parent) {
        camera?.focus(stop.parent);
        return true;
      }
      if (direction === "left" && stop.row?.inside) {
        if (stop.row.menu) top.menu(stop.row.search, stop.row.row);
        else intent({ kind: "fold", search: stop.row.search, row: stop.row.row });
        return true;
      }
      return false;
    },
    onactivate(id) {
      const action = byStop.get(id)?.activate;
      if (action?.kind === "more") top.toggle(action.place);
      else if (action?.kind === "open") void open(action.search);
      else if (action?.kind === "menu") top.menu(action.search, action.row);
      else if (action) intent(action);
    },
    onleave: () => intent({ kind: "field" }),
  };

  // What is known of a row shows once the pointer rests on it: from the model, never asked for.
  let pointed: { row: Row; stop: string } | null = null;
  let hovered = $state<{ row: Row; stop: string } | null>(null);
  let resting: ReturnType<typeof setTimeout> | undefined;
  function hover(row: Row | null, stop?: string) {
    clearTimeout(resting);
    hovered = null;
    pointed = row && stop ? { row, stop } : null;
    if (!row || row.menu || row.inside || (!row.note && !row.known.length && !row.host)) return;
    resting = setTimeout(() => (hovered = pointed), 360);
  }
  onDestroy(() => clearTimeout(resting));
  let tip = $derived.by(() => {
    const rect = hovered ? byStop.get(hovered.stop)?.rect : undefined;
    if (!hovered || !rect) return null;
    const left = rect.x * view.zoom + view.x;
    const right = left + rect.width * view.zoom + 12;
    return {
      row: hovered.row,
      x: right + 260 < width ? right : Math.max(8, left - 272),
      y: rect.y * view.zoom + view.y - 6,
    };
  });

  const top: HomeTop = {
    intent,
    open: (search) => void open(search),
    menu(search, row) {
      const key = `${search}/${row}`;
      if (!shown.delete(key)) shown.add(key);
    },
    shown: (search, row) => shown.has(`${search}/${row}`),
    toggle(key) {
      if (!expanded.delete(key)) expanded.add(key);
    },
    expanded: (key) => expanded.has(key),
    hover,
    picture: (key) => picture(key),
    mark: (place) => mark(place),
  };
  setContext(homeTop, top);

  // An earlier day is asked for once it comes within a screen of the view.
  const reached: string[] = [];
  $effect(() => {
    const { y, zoom } = view;
    const floor = height * 2;
    for (const node of laid.nodes) {
      if (node.kind !== "day" || node.day.entries !== null) continue;
      if (node.box.y * zoom + y > floor) continue;
      untrack(() => {
        if (reached.includes(node.day.key)) return;
        reached.push(node.day.key);
        intent({ kind: "reach", day: node.day.key });
      });
    }
  });
</script>

<div
  class="home-canvas"
  bind:this={root}
  bind:clientWidth={width}
  bind:clientHeight={height}
  style:--home-pad={`${SIZE.pad}px`}
>
  {#if placed}
    <CanvasHost
      bind:nodes
      edges={[]}
      {nodeTypes}
      bind:viewport={view}
      zoom={{ min: 0.5, max: 1.5 }}
      virtualize
      {keyboard}
      onmovestart={() => hover(null)}
      oncamera={(ready) => {
        camera = ready;
        if (drawn) handOver();
      }}
      oninit={() => {
        drawn = true;
        if (camera) handOver();
      }}
    />
  {/if}
  {#if tip}<HoverCard row={tip.row} x={tip.x} y={tip.y} />{/if}
</div>

<style>
  .home-canvas {
    position: relative;
    inline-size: 100%;
    block-size: 100%;
  }

  /* A card that moves because something before it changed slides there;
     nothing moves of itself. */
  /* stylelint-disable-next-line selector-class-pattern */
  .home-canvas :global(.svelte-flow__node) {
    transition: transform var(--motion-base) var(--ease-emphasized);
  }

  /* A search coming to the front opens where its line was; the one leaving
     closes to a line. */
  /* stylelint-disable-next-line selector-class-pattern */
  .home-canvas :global(.svelte-flow__node.home-arrive) {
    animation: lane-in var(--motion-base) var(--ease-out) backwards;
  }

  @keyframes lane-in {
    from {
      opacity: 0;
    }
  }

  @media (prefers-reduced-motion: reduce) {
    /* stylelint-disable-next-line selector-class-pattern */
    .home-canvas :global(.svelte-flow__node) {
      transition: none;
      animation: none;
    }
  }

  /* stylelint-disable-next-line selector-class-pattern */
  :global(:root[data-reduce-motion="true"]) .home-canvas :global(.svelte-flow__node) {
    transition: none;
    animation: none;
  }
</style>
