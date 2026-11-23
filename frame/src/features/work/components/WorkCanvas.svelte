<script lang="ts">
  import { CanvasHost, type CanvasCamera } from "$shared/canvas/host";
  import { pointerTool as currentTool, setPointerTool } from "../lib/pointer-tool.svelte";
  const pointerTool = $derived(currentTool());
  import { type Edge } from "@xyflow/svelte";
  import { NodeToolbar, Position, SelectionMode } from "@xyflow/svelte";
  import { untrack, setContext, tick } from "svelte";
  import { fade } from "svelte/transition";
  import Icon from "$shared/ui/Icon";
  import { Navigation03Icon } from "../lib/icons";
  import type { OwnedData } from "../lib/board/types";
  import {
    canvasInspection,
    canvasResize,
    canvasAction,
    canvasEvidence,
    canvasFocusResult,
    canvasOpen,
    canvasOpenLink,
    canvasAreas,
    canvasPictures,
    canvasAreaActions,
    canvasArrival,
    canvasProbe,
    canvasBoard,
    canvasWork,
    type BoardActions,
    type DraftReview,
  } from "../lib/canvas-context";
  import CanvasNode from "./CanvasNode.svelte";
  import AreaNode from "./AreaNode.svelte";
  import AgentMark from "./AgentMark.svelte";
  import WorkEdge from "./WorkEdge.svelte";
  import { arrivals, centreOn, frameSpan, toCanvas, toScreen } from "$shared/canvas";
  import {
    followMove,
    glideMs,
    settleMove,
    ZoomLevel,
    type CameraMove,
    type CameraView,
  } from "$shared/canvas/capabilities";
  import { duration, reducedMotion } from "$shared/lib/motion";
  import { IS_MAC } from "$shared/platform";
  import SelectionBar from "./SelectionBar.svelte";
  import {
    alignTo,
    arrange,
    fitArea,
    moveTo,
    selectedPlacements,
    type Alignment,
    type Arrangement,
  } from "../lib/selection";
  import {
    reconcileNodes,
    applyRemoteView,
    absolutePosition,
    containingArea,
    defaultSize,
    isAgentNode,
    isAreaNode,
    isItemNode,
    nodesBounds,
    sanitizeScene,
    validScene,
    validViewport,
    relationLinks,
    restLink,
    type CanvasArea,
    type CanvasItem,
    type CanvasLink,
    type CanvasPosition,
    type CanvasSize,
    type CanvasView,
    type WorkNode,
  } from "../lib/canvas-model";
  import * as m from "$shared/i18n/messages";
  import type { EvidenceReference } from "$shared/ui/data/Artifact";
  import type { WorkRuntimeProjection } from "$shared/ipc/bindings";
  let {
    items,
    links,
    areas = [],
    pictures = new Map(),
    initialView,
    remoteView,
    authoritative,
    fitBottomInset = 0,
    fitTopInset = 0,
    home,
    still = false,
    virtualizeFrom = 24,
    oninspect,
    onopen,
    onopenlink,
    onaction,
    onevidence,
    onviewchange,
    onselectionchange,
    onareachange,
    onselectionaction,
    onareaedit,
    onmoved,
    placeDrop,
    board,
    work,
    onprobe,
    expose,
  }: {
    items: readonly CanvasItem[];
    links: readonly CanvasLink[];
    areas?: readonly CanvasArea[];
    /** The admitted picture of each subject, by merge key. */
    pictures?: ReadonlyMap<string, { profile: string; digest: string }>;
    /** Without a camera, the canvas opens on its home run. */
    initialView?: Omit<CanvasView, "viewport"> & { viewport?: CanvasView["viewport"] };
    remoteView?: { sequence: number; view: CanvasView };
    authoritative: ReadonlySet<string>;
    fitBottomInset?: number;
    fitTopInset?: number;
    /** The run a canvas with no saved camera opens on: framed to its width, its result whole. */
    home?: { x: number; y: number; width: number };
    /** A lift, the pane or the takeover is open: the camera does not follow the agent. */
    still?: boolean;
    virtualizeFrom?: number;
    oninspect: (id: string) => void;
    onopen?: (id: string) => void;
    /** A prose link inside a result; opens through a native intent. */
    onopenlink?: (href: string) => void;
    onaction?: (id: string, action?: string) => void;
    onevidence?: (id: string, reference: EvidenceReference) => void;
    onviewchange?: (view: CanvasView) => void;
    onselectionchange?: (ids: string[]) => void;
    onareachange?: (id: string, area: string | null) => void;
    /** A selection bar action on the selected cards. */
    onselectionaction?: (action: "area" | "ask" | "remove", ids: string[]) => void;
    onareaedit?: (
      area: string,
      edit: { kind: "rename"; title: string } | { kind: "remove" },
    ) => void;
    /** The person dragged these cards to where they stand now. */
    onmoved?: (ids: string[]) => void;
    /**
     * Where a card the person dropped must stand instead, as its top-left, when
     * it landed somewhere it cannot stay; null, and it stays exactly where it was put.
     */
    placeDrop?: (id: string, rect: CanvasPosition & CanvasSize) => CanvasPosition | null;
    /** What a board's blocks ask of the canvas's owner. */
    board?: BoardActions;
    /** The runs behind the canvas, by objective, for a helper's own view of its part. */
    work?: (objective: string) => WorkRuntimeProjection | undefined;
    /** Asks native for the icon of an origin no tab has shown. */
    onprobe?: (origin: string) => void;
    expose?: (api: CanvasApi) => void;
  } = $props();
  setContext(canvasWork, (objective: string) => work?.(objective));
  setContext(canvasBoard, {
    measure: (id: string, width: number, open: boolean, height: number) =>
      board?.measure(id, width, open, height),
    toggle: (id: string) => board?.toggle(id),
    ask: (name: string) => board?.ask(name),
    choose: (element: string, chosen: boolean) => board?.choose(element, chosen),
    evidence: (reference: EvidenceReference) => board?.evidence(reference),
    entity: (element: string) => board?.entity(element),
    command: (record: string) => board?.command(record),
    page: (url: string) => board?.page(url),
    note: (id: string) => board?.note(id),
    compare: (id: string) => board?.compare?.(id),
    send: (id: string) => board?.send?.(id),
    write: (id: string, markdown: string) => board?.write?.(id, markdown),
    drafted: (id: string) => board?.drafted?.(id),
    review: (id: string, review: DraftReview) => board?.review?.(id, review),
    edit: (id: string, change: (data: OwnedData) => OwnedData) => board?.edit?.(id, change),
  } satisfies BoardActions);
  /** Each origin is asked for once per canvas. */
  const probed: Record<string, true> = {};
  setContext(canvasProbe, (origin: string) => {
    if (probed[origin]) return;
    probed[origin] = true;
    onprobe?.(origin);
  });
  setContext(canvasEvidence, {
    get open() {
      return onevidence;
    },
  });
  setContext(canvasAction, (id: string, action?: string) => onaction?.(id, action));
  setContext(canvasOpenLink, (href: string) => onopenlink?.(href));
  setContext(canvasInspection, (id: string) => oninspect(id));
  setContext(canvasOpen, (id: string) => onopen?.(id));
  setContext(canvasAreas, {
    get list() {
      return areas;
    },
  });
  setContext(canvasPictures, {
    get map() {
      return pictures;
    },
  });
  let resizing = $state(false);
  setContext(canvasResize, (active: boolean) => (resizing = active));
  let nodes = $state.raw<WorkNode[]>([]);
  let canvasWidth = $state(0);
  let canvasHeight = $state(0);
  let host = $state<HTMLDivElement>();
  const nodeFor = (id: string) => nodes.find((node) => node.id === id);
  function center(id: string) {
    const node = nodeFor(id);
    if (!node) return;
    homed = false;
    viewport = centreOn(
      { ...absolutePosition(node, nodes), width: node.width ?? 480, height: node.height ?? 360 },
      { width: canvasWidth, height: canvasHeight, top: fitTopInset, bottom: fitBottomInset },
    );
    publishView();
  }
  setContext(canvasFocusResult, center);
  const HOME_SETTLE = 1500;
  /**
   * The view that holds a span of the canvas across its width: at 100% when
   * it fits, down to 50%; wider still, its right end stays in view, so a
   * run's result is never cut.
   */
  const framing = (rect: { x: number; y: number; width: number }) =>
    frameSpan(rect, { width: canvasWidth, top: fitTopInset });
  /**
   * A run that ends while the person follows it: the result's top comes into
   * view if it is not, with one pan at the zoom they have. Never a re-fit.
   */
  function settle(id: string, span?: number, tries = 30) {
    const node = nodeFor(id);
    // The answer lands a frame or two after its run ends: wait for it rather than for nothing.
    if (!node && tries > 0) {
      requestAnimationFrame(() => settle(id, span, tries - 1));
      return;
    }
    if (!node || !camera) return;
    homed = false;
    const position = absolutePosition(node, nodes);
    const move = settleMove({
      view: cameraView(),
      room: cameraRoom(),
      result: {
        left: position.x,
        top: position.y,
        right: position.x + (span ?? node.width ?? 480),
        bottom: position.y + (node.height ?? node.measured?.height ?? 320),
      },
      following,
      reduced: reducedMotion(),
      base: duration("page"),
    });
    if (move) glide(move);
  }
  const restoredViewport = untrack(() => validViewport(initialView?.viewport));
  let viewport = $state(restoredViewport ?? { x: 0, y: 0, zoom: 1 });
  /**
   * A canvas with no saved camera stands on its home run, and keeps standing
   * there as the run's objects measure themselves, until the person moves.
   */
  let homed = !restoredViewport;
  $effect(() => {
    const target = home;
    if (!homed || !target || !canvasWidth) return;
    untrack(() => (viewport = framing(target)));
  });
  // Homing lasts while the opened run measures itself, never through a run that goes on growing.
  $effect(() => {
    if (!homed || !canvasWidth) return;
    const settle = setTimeout(() => (homed = false), HOME_SETTLE);
    return () => clearTimeout(settle);
  });
  // One bad card never hides the canvas: the scene is repaired, then guarded.
  const scene = $derived(sanitizeScene(items, links));
  let valid = $derived(validScene(scene.items, scene.links));
  const nodeTypes = { work: CanvasNode, area: AreaNode, agent: AgentMark };
  const edgeTypes = { work: WorkEdge };
  let selection: string[] = [];
  let selectedIds = $state.raw<ReadonlySet<string>>(new Set());
  let hovered = $state<string | null>(null);
  /** Cards whose ties show: the selection and the card under the pointer. */
  const focused = $derived<ReadonlySet<string>>(
    hovered ? new Set([...selectedIds, hovered]) : selectedIds,
  );
  /** What arrived when: only what a live run adds after the canvas opened moves in. */
  const arrival = arrivals();
  setContext(canvasArrival, (id: string) => arrival.motion(id));
  const byId = $derived(new Map(scene.items.map((item) => [item.id, item])));
  const lit = $derived(relationLinks(scene.links, byId, focused));
  const title = (id: string) => byId.get(id)?.title ?? "";

  let edges = $derived.by<Edge[]>(() => {
    if (!valid) return [];
    arrival.see(scene.items);
    return scene.links.flatMap((link) => {
      if (link.hover && !focused.has(link.source) && !focused.has(link.target)) return [];
      const rest = restLink(link);
      if (!rest && !lit.has(link.id)) return [];
      return [
        {
          id: link.id,
          source: link.source,
          target: link.target,
          type: "work",
          data: {
            ...(link.route ? { route: link.route } : {}),
            tone:
              link.kind === "thread"
                ? "thread"
                : link.kind === "flow"
                  ? "flow"
                  : rest
                    ? "rest"
                    : "relation",
            ...(link.live ? { live: true } : {}),
          },
          deletable: false,
          selectable: false,
          focusable: false,
          class: `work-edge kind-${link.kind}`,
          ariaLabel:
            link.kind === "dependency" || rest
              ? m.work_env_dependency_label({
                  source: title(link.source),
                  target: title(link.target),
                })
              : link.label
                ? `${title(link.source)} ${link.label} ${title(link.target)}`
                : m.work_env_reference_label({
                    source: title(link.source),
                    target: title(link.target),
                  }),
        },
      ];
    });
  });
  /** The positions the last reconcile was given: a card follows only a place that moved. */
  let lastPositions: Readonly<Record<string, CanvasPosition>> = {};
  /** An agent whose run ended rests where it finished, then fades. */
  let resting: WorkNode[] = [];
  const REST_MS = 1200;
  function rest(previous: readonly WorkNode[], next: WorkNode[]): WorkNode[] {
    const ids = new Set(next.map((node) => node.id));
    resting = resting.filter((node) => !ids.has(node.id));
    for (const node of previous) {
      if (!isAgentNode(node) || ids.has(node.id) || resting.some((r) => r.id === node.id)) continue;
      const still = {
        ...node,
        data: { ...node.data, agent: { ...node.data.agent!, caption: undefined } },
        class: "resting",
      };
      resting.push(still);
      setTimeout(() => {
        resting = resting.map((r) => (r.id === node.id ? { ...r, class: "resting leaving" } : r));
        nodes = nodes.map((candidate) =>
          candidate.id === node.id ? { ...candidate, class: "resting leaving" } : candidate,
        );
        setTimeout(() => {
          resting = resting.filter((r) => r.id !== node.id);
          nodes = nodes.filter((candidate) => candidate.id !== node.id);
        }, duration("slow"));
      }, REST_MS);
    }
    if (!resting.length) return next;
    const kept = [...next, ...resting.filter((r) => !ids.has(r.id))];
    return kept.length === previous.length && kept.every((node, index) => node === previous[index])
      ? (previous as WorkNode[])
      : kept;
  }
  /** Cards the canvas moved itself glide there, then lose the class that let them. */
  const GLIDE = "settling";
  let glideTimer: ReturnType<typeof setTimeout> | undefined;
  const classes = (node: WorkNode) =>
    typeof node.class === "string" ? node.class.split(" ").filter(Boolean) : [];
  function glideCards(previous: readonly WorkNode[], next: WorkNode[]): WorkNode[] {
    const before = new Map(previous.map((node) => [node.id, node]));
    let moved = false;
    const glided = next.map((node) => {
      const was = before.get(node.id);
      if (
        !was ||
        !isItemNode(node) ||
        node.dragging ||
        classes(node).includes(GLIDE) ||
        (was.position.x === node.position.x && was.position.y === node.position.y)
      )
        return node;
      moved = true;
      return { ...node, class: [...classes(node), GLIDE].join(" ") };
    });
    if (!moved) return next;
    clearTimeout(glideTimer);
    glideTimer = setTimeout(
      () => {
        nodes = nodes.map((node) => {
          const names = classes(node);
          if (!names.includes(GLIDE)) return node;
          const kept = names.filter((name) => name !== GLIDE);
          return { ...node, class: kept.length ? kept.join(" ") : undefined };
        });
      },
      duration("slow") + 40,
    );
    return glided;
  }
  $effect(() => () => clearTimeout(glideTimer));
  $effect(() => {
    const next = scene.items;
    const grouping = areas;
    const ready = valid;
    const placed = initialView?.positions ?? {};
    nodes = untrack(() => {
      if (!ready) return [];
      arrival.see(next);
      const reconciled = reconcileNodes(
        nodes,
        next,
        placed,
        initialView?.sizes,
        grouping,
        { ...initialView?.areas, ...pendingAreas },
        lastPositions,
      );
      lastPositions = placed;
      return rest(nodes, glideCards(nodes, reconciled));
    });
  });
  let publishedPositions = "";
  const positionKey = (list: WorkNode[]) =>
    JSON.stringify(
      list
        .filter((node) => isItemNode(node) || isAreaNode(node))
        .map((node) => [
          node.id,
          node.parentId ?? "",
          node.position.x,
          node.position.y,
          node.width,
          node.height,
        ]),
    );
  let appliedRemote = 0;
  let deferredRemote: CanvasView | null = null;
  function applyRemote(view: CanvasView) {
    const next = applyRemoteView(nodes, view, authoritative);
    const restored = validViewport(view.viewport);
    if (next !== nodes) {
      publishedPositions = positionKey(next);
      nodes = next;
    }
    if (restored) viewport = restored;
  }
  $effect(() => {
    const remote = remoteView;
    if (!remote || remote.sequence === appliedRemote) return;
    appliedRemote = remote.sequence;
    untrack(() => {
      if (resizing || nodes.some((node) => node.dragging)) deferredRemote = remote.view;
      else applyRemote(remote.view);
    });
  });
  $effect(() => {
    if (!nodes.length || resizing || nodes.some((node) => node.dragging)) return;
    if (deferredRemote) {
      const view = deferredRemote;
      deferredRemote = null;
      untrack(() => applyRemote(view));
      return;
    }
    const key = positionKey(nodes);
    if (key !== publishedPositions) {
      publishedPositions = key;
      untrack(publishView);
    }
  });
  function publishView() {
    if (resizing) return;
    const elements = nodes.filter(isItemNode);
    const areaNodes = nodes.filter(isAreaNode);
    onviewchange?.({
      positions: Object.fromEntries(
        elements.map((node) => [node.id, absolutePosition(node, nodes)]),
      ),
      viewport: { ...viewport },
      sizes: Object.fromEntries(
        elements.map((node) => [
          node.id,
          { width: Math.round(node.width ?? 280), height: Math.round(node.height ?? 160) },
        ]),
      ),
      areas: Object.fromEntries(
        areaNodes.map((node) => [
          node.id.slice("area:".length),
          {
            x: Math.round(node.position.x),
            y: Math.round(node.position.y),
            width: Math.round(node.width ?? 640),
            height: Math.round(node.height ?? 420),
          },
        ]),
      ),
    });
  }
  type CanvasApi = {
    screenRect: (id: string) => DOMRect | null;
    flowPosition: (clientX: number, clientY: number) => CanvasPosition | null;
    selectionBounds: (
      only?: readonly string[],
    ) => (CanvasPosition & CanvasSize & { ids: string[] }) | null;
    placeArea: (area: string, rect: CanvasPosition & CanvasSize) => void;
    center: (id: string) => void;
    focusCard: (id: string) => void;
    settle: (id: string, span?: number) => void;
    followAgent: () => boolean;
    resumeFollow: () => void;
  };
  function selectionBounds(only?: readonly string[]) {
    const ids = (only ?? selection).filter((id) =>
      nodes.some((node) => node.id === id && isItemNode(node)),
    );
    const bounds = nodesBounds(ids, nodes);
    return bounds ? { ...bounds, ids } : null;
  }
  function screenRect(id: string): DOMRect | null {
    const node = nodeFor(id);
    const origin = host?.getBoundingClientRect();
    if (!node || !origin) return null;
    const z = viewport.zoom;
    const at = toScreen(absolutePosition(node, nodes), { x: origin.left, y: origin.top }, viewport);
    return new DOMRect(at.x, at.y, (node.width ?? 280) * z, (node.height ?? 160) * z);
  }
  /** Pans to a card and puts the keyboard on it; the node itself is the stop. */
  async function focusCard(id: string) {
    center(id);
    await tick();
    host
      ?.querySelector<HTMLElement>(`.svelte-flow__node[data-id="${CSS.escape(id)}"]`)
      ?.focus({ preventScroll: true });
  }
  /** Where a client point lands on the canvas, so a drop keeps its place. */
  function flowPosition(clientX: number, clientY: number): CanvasPosition | null {
    const origin = host?.getBoundingClientRect();
    if (!origin) return null;
    return toCanvas({ x: clientX, y: clientY }, { x: origin.left, y: origin.top }, viewport);
  }
  $effect(() => {
    expose?.({
      screenRect,
      flowPosition,
      selectionBounds,
      placeArea,
      center,
      focusCard: (id: string) => void focusCard(id),
      settle,
      followAgent: () => following,
      resumeFollow,
    });
  });

  // Camera follow. While a run is live and the mark walks out of sight, the
  // view pans (never zooms) just far enough to bring it back. A manual pan,
  // zoom or drag ends it: it resumes on a new request, from the Follow pill
  // or the fit control, never on its own.
  /** The orb and the widest caption it usually carries. */
  const MARK_REACH = { width: 220, height: 24 };
  let camera: CanvasCamera | undefined;
  let following = $state(true);
  /** Where the camera is bound while a glide runs, so a second move starts from it, not from mid-flight. */
  let aim: CameraView | undefined;
  const cameraView = () => aim ?? viewport;
  const cameraRoom = () => ({
    width: canvasWidth,
    top: fitTopInset,
    bottom: canvasHeight - fitBottomInset,
  });
  /** The only way the follow moves the camera: one eased glide, which replaces any still running. */
  function glide(move: CameraMove) {
    if (!camera) return;
    aim = move.view;
    camera.glide(move.view, move.ms);
  }
  let fitted = false;
  let requestsSeen = false;
  let stands: Record<string, string> = {};
  const requests: Record<string, true> = {};
  function resumeFollow() {
    following = true;
    const marks = nodes.filter(isAgentNode).filter((node) => !node.class);
    if (marks.length) followMarks(marks.map((node) => node.position));
  }
  /**
   * A request just sent stands in the middle of what can be seen: between
   * the island and the composer, at the zoom the person reads at.
   */
  function centreRequest(id: string, tries = 12) {
    const node = nodeFor(id);
    if (!node || !camera || !canvasWidth || !(node.width ?? node.measured?.width)) {
      // A just-sent card is placed and measured a few frames after it exists.
      if (tries > 0) requestAnimationFrame(() => centreRequest(id, tries - 1));
      return;
    }
    homed = false;
    const zoom = cameraView().zoom;
    const position = absolutePosition(node, nodes);
    const width = (node.width ?? node.measured?.width ?? 320) * zoom;
    const height = (node.height ?? node.measured?.height ?? 80) * zoom;
    const top = fitTopInset;
    const bottom = canvasHeight - fitBottomInset;
    glide({
      view: {
        x: (canvasWidth - width) / 2 - position.x * zoom,
        y: top + (bottom - top - height) / 2 - position.y * zoom,
        zoom,
      },
      ms: glideMs(reducedMotion(), duration("page")),
    });
  }
  /** Brings the mark that moved into view, by the shortest pan, when it has left it. */
  function followMarks(positions: readonly CanvasPosition[], lead?: CanvasPosition) {
    const at = lead ?? positions.at(-1);
    if (!at || !camera) return;
    const move = followMove({
      view: cameraView(),
      room: cameraRoom(),
      newest: {
        left: at.x,
        top: at.y,
        right: at.x + MARK_REACH.width,
        bottom: at.y + MARK_REACH.height,
      },
      following,
      reduced: reducedMotion(),
      base: duration("page"),
    });
    if (move) glide(move);
  }
  const live = $derived(nodes.some((node) => isAgentNode(node) && !node.class));
  $effect(() => {
    const agents = nodes.filter(isAgentNode).filter((node) => !node.class);
    const cards = scene.items.flatMap((item) =>
      item.type === "request" || item.type === "objective" ? [item.id] : [],
    );
    const paused = still;
    untrack(() => {
      // A new lane's request card, or the end of a run, resumes following.
      // Cards present when the canvas first looks (or arriving together, as a
      // work loads) are not new; one card appearing on its own was just sent,
      // including the first request of an empty work.
      const fresh = cards.filter((id) => !requests[id]);
      for (const id of fresh) requests[id] = true;
      const sent = requestsSeen && fresh.length === 1 ? fresh[0] : undefined;
      requestsSeen = true;
      if (sent) {
        following = true;
        requestAnimationFrame(() => centreRequest(sent));
      }
      // A run's end keeps whether the person was following: its result is
      // handed to them only then, and a new request resumes it anyway.
      if (!agents.length && Object.keys(stands).length) stands = {};
      let moved: CanvasPosition | undefined;
      for (const node of agents) {
        const position = node.position;
        const key = `${position.x},${position.y}`;
        if (stands[node.id] !== key) moved = position;
        stands[node.id] = key;
      }
      if (moved && fitted && following && !paused)
        followMarks(
          agents.map((node) => node.position),
          moved,
        );
    });
  });
  let lastClick = { id: "", at: 0 };
  // Selection, marquee, arrange and areas.
  const pendingAreas: Record<string, CanvasPosition & CanvasSize> = {};
  /** Where a new area stands: kept until its node exists, or moved in place once it does. */
  function placeArea(area: string, rect: CanvasPosition & CanvasSize) {
    const id = `area:${area}`;
    if (!nodes.some((node) => node.id === id)) {
      pendingAreas[area] = rect;
      return;
    }
    nodes = nodes.map((node) =>
      node.id === id
        ? { ...node, position: { x: rect.x, y: rect.y }, width: rect.width, height: rect.height }
        : node,
    );
  }
  let marquee = $state(false);
  let engaged = false;
  const selectedItems = $derived(
    nodes.flatMap((node) => (node.selected && isItemNode(node) ? [node.id] : [])),
  );
  const dragging = $derived(nodes.some((node) => node.dragging));
  function place(next: WorkNode[]) {
    if (next === nodes) return;
    nodes = next;
    publishedPositions = positionKey(next);
    publishView();
  }
  /**
   * A dropped card stays where it was put. One that landed where it cannot
   * stay glides to the nearest spot it can, and the owner says why.
   */
  function settleDrop(ids: readonly string[]) {
    if (!placeDrop) return;
    const spots: Record<string, CanvasPosition> = {};
    for (const id of ids) {
      const node = nodes.find((candidate) => candidate.id === id);
      if (!node || !isItemNode(node)) continue;
      const at = absolutePosition(node, nodes);
      const spot = placeDrop(id, {
        ...at,
        width: node.width ?? defaultSize(node.data).width,
        height: node.height ?? defaultSize(node.data).height,
      });
      if (spot && (spot.x !== at.x || spot.y !== at.y)) spots[id] = spot;
    }
    if (!Object.keys(spots).length) return;
    nodes = glideCards(
      nodes,
      nodes.map((node) => {
        const spot = spots[node.id];
        if (!spot) return node;
        const parent = node.parentId
          ? nodes.find((candidate) => candidate.id === node.parentId)
          : undefined;
        const position = parent
          ? { x: spot.x - parent.position.x, y: spot.y - parent.position.y }
          : spot;
        return { ...node, dragging: false, position };
      }),
    );
  }
  const arrangeSelection = (how: Arrangement) =>
    place(moveTo(nodes, arrange(selectedPlacements(selectedItems, nodes), how)));
  const alignSelection = (how: Alignment) =>
    place(moveTo(nodes, alignTo(selectedPlacements(selectedItems, nodes), how)));
  function clearSelection() {
    if (!nodes.some((node) => node.selected)) return false;
    nodes = nodes.map((node) => (node.selected ? { ...node, selected: false } : node));
    return true;
  }
  setContext(canvasAreaActions, {
    fit: (id: string) => place(fitArea(nodes, id)),
    rename: (id: string, title: string) =>
      onareaedit?.(id.slice("area:".length), { kind: "rename", title }),
    remove: (id: string) => onareaedit?.(id.slice("area:".length), { kind: "remove" }),
  });
  /** V and H pick the tool, Escape clears the selection, while the canvas holds the keyboard. */
  function canvasKeys(event: KeyboardEvent) {
    if (event.defaultPrevented || event.metaKey || event.ctrlKey || event.altKey) return;
    const active = document.activeElement;
    const inside = !!active && !!host?.contains(active);
    if (!inside && !(engaged && (!active || active === document.body))) return;
    const target = event.target instanceof HTMLElement ? event.target : null;
    if (target?.closest("input, textarea, select, [contenteditable]")) return;
    const key = event.key.toLowerCase();
    if (key === "v" || key === "h") {
      setPointerTool(key === "v" ? "select" : "hand");
      event.preventDefault();
    } else if (event.key === "Escape" && clearSelection()) event.preventDefault();
  }
</script>

<svelte:window
  onpointerdowncapture={(event) => (engaged = !!host?.contains(event.target as Node))}
  onkeydown={canvasKeys}
/>

<div
  class="work-canvas"
  class:multi={selectedItems.length > 1}
  bind:this={host}
  aria-label={m.work_canvas_label()}
>
  {#if valid}
    <CanvasHost
      bind:nodes
      {edges}
      {nodeTypes}
      {edgeTypes}
      bind:viewport
      bind:width={canvasWidth}
      bind:height={canvasHeight}
      fitView={scene.items.length > 0 && !restoredViewport && !home}
      fitViewOptions={{ padding: 0.2, duration: 0 }}
      dots={{ color: "var(--work-canvas-dot)", gap: 20, size: 1.5 }}
      panOnDrag={pointerTool === "hand"}
      edgeDescription={m.work_env_edge_readonly()}
      virtualize={scene.items.length >= virtualizeFrom}
      capabilities={{
        drag: {
          threshold: 3,
          autoPan: true,
          onstart: () => {
            following = false;
            homed = false;
            aim = undefined;
          },
          onstop: ({ targetNode, nodes: carried }) => {
            onmoved?.(carried.map((node) => node.id));
            settleDrop(carried.map((node) => node.id));
            if (!targetNode || !isItemNode(targetNode as WorkNode)) return;
            const node = nodes.find((candidate) => candidate.id === targetNode.id);
            if (!node || !isItemNode(node) || !authoritative.has(node.id)) return;
            const area = containingArea(node, nodes);
            const current = node.parentId ? node.parentId.slice("area:".length) : null;
            if (area !== current) onareachange?.(node.id, area);
          },
        },
        select: {
          multiKey: [IS_MAC ? "Meta" : "Control", "Shift"],
          elevate: true,
          onchange: ({ nodes: selected }) => {
            selection = selected.map((node) => node.id);
            selectedIds = new Set(selection);
            onselectionchange?.(selection);
          },
        },
        marquee: {
          active: pointerTool === "select",
          key: "Shift",
          mode: SelectionMode.Partial,
          onstart: () => (marquee = true),
          onend: () => {
            marquee = false;
            // A marquee that caught cards leaves the areas it touched alone.
            if (selectedItems.length && nodes.some((node) => node.selected && isAreaNode(node)))
              nodes = nodes.map((node) =>
                node.selected && isAreaNode(node) ? { ...node, selected: false } : node,
              );
          },
        },
      }}
      onnodeclick={({ node, event }) => {
        if (
          event.target instanceof Element &&
          event.target.closest(".artifact-body, button, a, input, textarea, select, summary")
        )
          return;
        const now = performance.now();
        if (lastClick.id === node.id && now - lastClick.at < 320) {
          lastClick = { id: "", at: 0 };
          // An input that stands for something on the canvas opens that thing.
          const input = isItemNode(node as WorkNode) ? (node.data as CanvasItem).input : undefined;
          onopen?.(input?.element ?? node.id);
          return;
        }
        lastClick = { id: node.id, at: now };
        oninspect(node.id);
      }}
      onnodepointerenter={({ node }) => {
        if (isItemNode(node as WorkNode)) hovered = node.id;
      }}
      onnodepointerleave={({ node }) => {
        if (hovered === node.id) hovered = null;
      }}
      onpaneclick={() => {
        lastClick = { id: "", at: 0 };
        onselectionchange?.([]);
      }}
      onmoveend={() => {
        aim = undefined;
        publishView();
      }}
      oninit={() => requestAnimationFrame(() => requestAnimationFrame(() => (fitted = true)))}
      onmovestart={(event) => {
        if (!event) return;
        following = false;
        homed = false;
        aim = undefined;
      }}
      oncamera={(ready) => (camera = ready)}
    >
      <ZoomLevel
        label={m.work_canvas_controls()}
        fitLabel={m.work_fit()}
        dock="[data-work-zoom-slot]"
        bottomInset={fitBottomInset}
        onfit={() => (following = true)}
      />
      <NodeToolbar
        nodeId={selectedItems}
        isVisible={selectedItems.length > 1 && !dragging && !marquee}
        position={Position.Top}
        offset={14}
      >
        <SelectionBar
          count={selectedItems.length}
          owned={selectedItems.filter((id) => authoritative.has(id)).length}
          onarrange={arrangeSelection}
          onalign={alignSelection}
          onarea={() => onselectionaction?.("area", selectedItems)}
          onask={() => onselectionaction?.("ask", selectedItems)}
          onremove={() => onselectionaction?.("remove", selectedItems)}
        />
      </NodeToolbar>
    </CanvasHost>
    {#if live && !following && !still}<button
        type="button"
        class="follow"
        style:inset-block-end={`${fitBottomInset + 14}px`}
        transition:fade={{ duration: duration("fast") }}
        onclick={resumeFollow}><Icon icon={Navigation03Icon} size={13} />{m.work_follow()}</button
      >{/if}
  {:else}<div class="canvas-empty" role="status">
      {valid ? m.work_canvas_empty() : m.work_canvas_unavailable()}
    </div>{/if}
</div>

<style>
  .work-canvas {
    position: relative;
    width: 100%;
    height: 100%;
    min-height: 240px;

    --work-canvas-dot: color-mix(in srgb, var(--color-text) 16%, transparent);
  }

  /* stylelint-disable-next-line selector-class-pattern */
  .work-canvas :global(.svelte-flow__node) {
    transition: none;
  }

  /* A board's block glides where its board makes room; a dragged one follows the pointer. */
  /* stylelint-disable-next-line selector-class-pattern */
  .work-canvas :global(.svelte-flow__node.board-block:not(.dragging)) {
    transition: transform var(--motion-slow) var(--ease-emphasized);
  }

  /* A card the canvas moved itself (a drop put clear, a run making room) glides there. */
  /* stylelint-disable-next-line selector-class-pattern */
  .work-canvas :global(.svelte-flow__node.settling:not(.dragging)) {
    transition: transform var(--motion-slow) var(--ease-emphasized);
  }

  /* A part's windows fan out over what follows its row. */
  /* stylelint-disable-next-line selector-class-pattern */
  .work-canvas :global(.svelte-flow__node:has(.fans:hover)) {
    z-index: 1000 !important;
  }

  /* The agent's mark walks to its work on a spring; it never takes the pointer. */
  /* stylelint-disable-next-line selector-class-pattern */
  .work-canvas :global(.svelte-flow__node-agent) {
    pointer-events: none;
    transition: opacity var(--motion-slow) var(--ease-exit);
  }

  /* stylelint-disable-next-line selector-class-pattern */
  .work-canvas :global(.svelte-flow__node-agent.leaving) {
    opacity: 0;
  }

  /* Several cards selected: one bar for all of them, not a toolbar and handles each. */
  /* stylelint-disable-next-line selector-class-pattern */
  .work-canvas.multi :global(.svelte-flow__node-toolbar:not(:has(.selection-bar))),
  .work-canvas.multi :global(.work-resize-handle) {
    display: none;
  }

  /* An area is taken by its title; its body lets the pane pan or draw a marquee. */
  /* stylelint-disable-next-line selector-class-pattern */
  .work-canvas :global(.svelte-flow__node-area) {
    pointer-events: none;
  }

  /* stylelint-disable selector-class-pattern */
  .work-canvas :global(.svelte-flow__node-area .area-title),
  .work-canvas :global(.svelte-flow__node-area .work-resize-handle) {
    pointer-events: auto;
  }
  /* stylelint-enable selector-class-pattern */

  /* A part's row is taken by its name, its windows and its work; the rest of its box lets the pane pan. */
  /* stylelint-disable-next-line selector-class-pattern */
  .work-canvas :global(.svelte-flow__node:has(.part[data-part])) {
    pointer-events: none;
  }

  /* A set of cards is taken by its cards and title; the gaps between them let the pane pan. */
  /* stylelint-disable selector-class-pattern */
  .work-canvas :global(.svelte-flow__node:has(.object.picks)) {
    pointer-events: none;
  }

  .work-canvas :global(.svelte-flow__node:has(.object.picks) .work-resize-handle) {
    pointer-events: auto;
  }
  /* stylelint-enable selector-class-pattern */

  /* While the person looks elsewhere during a run: one quiet way back to the work. */
  .follow {
    position: absolute;
    inset-inline-start: 50%;
    z-index: 4;
    display: inline-flex;
    align-items: center;
    gap: 7px;
    block-size: 28px;
    padding: 0 12px 0 10px;
    border: 0;
    border-radius: var(--radius-capsule);
    background: var(--color-float);
    box-shadow: var(--shadow-popover);
    color: var(--color-label-secondary);
    font: inherit;
    font-size: var(--text-label);
    font-weight: 550;
    translate: -50% 0;
    cursor: default;
    transition: color var(--motion-fast) var(--ease-out);
  }

  .follow:hover {
    color: var(--color-text);
  }

  .follow:focus-visible {
    outline: 2px solid var(--color-ring);
    outline-offset: 2px;
  }

  .canvas-empty {
    height: 100%;
    display: grid;
    place-items: center;
    padding: 24px;
    color: var(--color-muted);
  }
</style>
