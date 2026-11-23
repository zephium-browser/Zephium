import type { WorkPicture } from "$domain/resources";
import type { MediaAssetV1 } from "$domain/resources";
import type { IconRef } from "$shared/ipc/bindings";
import type { ArtifactView, EvidenceReference, SubjectView } from "$shared/ui/data/Artifact";
import type { Node } from "@xyflow/svelte";
import { absolutePosition } from "$shared/canvas";
import type { HumanPage } from "./work-human";
import { defaultSize as cardSize } from "./card-size";
import type { Block, ObjectView } from "./board/types";
import type { TrailLine } from "./board/trail";
import type { RunSources } from "./run/sources";

export { absolutePosition };

/** The Sources card's line for pages that could not be read. */
const UNREAD_FOOTER = 24;
/** A card's size before it renders; Sources grows by its unread line. */
export function defaultSize(item: CanvasItem): CanvasSize {
  if (item.size) return item.size;
  const size = cardSize(item);
  return item.unread?.length ? { ...size, height: size.height + UNREAD_FOOTER } : size;
}

type CanvasKind =
  | "tab"
  | "note"
  | "media"
  | "objective"
  | "request"
  | "result"
  | "subject"
  | "sources"
  | "folder"
  | "link"
  | "page"
  | "agent"
  | "block"
  | "head"
  | "trail"
  | "part"
  | "input"
  | "object";
type RelationKind = "supports" | "uses" | "depends_on" | "same_as" | "contradicts";
/** Display values only; deliberately independent from the generated Work wire contract. */
export type CanvasItem = {
  id: string;
  title: string;
  kind: string;
  type?: CanvasKind;
  detail: string;
  status: string;
  area?: string | null;
  /** A cached native site icon by origin; never a remote image URL. */
  icon?: IconRef | null;
  artifact?: ArtifactView;
  layout?: "artifact";
  actionLabel?: string;
  subject?: SubjectView;
  /** What the run established about a subject, price first. */
  facts?: { label: string; value: string }[];
  /** One block of a request's board, with the sources its chips name. */
  block?: {
    data: Block;
    sources: Readonly<Record<string, EvidenceReference>>;
    open: boolean;
    /** The request's run is still going. */
    live: boolean;
  };
  /** What a request's runs did, as the process column tells it. */
  trail?: readonly TrailLine[];
  /** A lane card's size, set by its lane rather than saved. */
  size?: CanvasSize;
  /** The person's own object this card draws: its resource, which edits write to. */
  owned?: string;
  /** Transient agent presence: its orb's seed, what it does, where it stands, what it says. */
  agent?: {
    seed: number;
    activity: string;
    objective: string;
    doing?: string;
    stand?: CanvasPosition;
    /** One word or a host, while the activity lasts. */
    caption?: string;
  };
  /** The pages one fetch stage cited; opening a row goes through the pane. */
  sources?: readonly {
    key: string;
    /** Empty for a file: a granted folder is not a place the pane can open. */
    url: string;
    where: string;
    title: string;
    /** Why the read gave up, when it did; the row says so instead of the title. */
    note?: string;
    /** A file a step disclosed; the lift shows what the run recorded of it. */
    file?: { record: string; path: string; kind: string };
  }[];
  /** What a run drew on, under its result: the pages and files it cites, and what it could not open. */
  drawn?: RunSources;
  /** Frames of the pages the request read, kept once its run is done. */
  frames?: readonly { key: string; url: string; host: string; frame: string }[];
  /** Pages a run opened and could not read: the Sources card lists them, never as cards. */
  unread?: readonly { key: string; url: string; host: string; note: string }[];
  /** A page a browser step opened: its newest frame while the agent works there. */
  page?: {
    url: string;
    host: string;
    frame: string | null;
    live: boolean;
    /** Set while the run is holding this page open for a person. */
    human?: HumanPage;
    /** Read with the person's signed-in session: the host it was granted on. */
    account?: string;
    /** An open tab the request was shown, not read: no frame, its title and a caption. */
    tab?: boolean;
  };
  /** Where the facts a request's run remembered are read from, shown with Undo beside it. */
  remember?: { profile: string; work: string; execution: string; version: number };
  /** A request opened to show every line of the person's words. */
  expanded?: boolean;
  /** When the person asked, as a short clock time. */
  when?: string;
  /** One of a run's objects, drawn at the canvas's detail. */
  object?: { view: ObjectView; live: boolean };
  /** A part of a run: its name and mark, and the pages it worked on. */
  part?: PartView;
  /** Something a run drew on before it began: memory, a skill, notes, tabs, files. */
  input?: {
    kind: "memory" | "skill" | "history" | "notes" | "tabs" | "files" | "connection" | "work";
    label: string;
    /** A folder the run read, by its own name. */
    folder?: boolean;
    count?: number;
    lit?: boolean;
    /** The canvas element it stands for, opened from its mark. */
    element?: string;
  };
  /** Under a request: what the lead read on this Mac itself, and its questions and their answers. */
  turns?: {
    local: readonly {
      kind: "folder" | "file" | "search" | "command" | "change";
      text: string;
    }[];
    exchange: readonly (
      { kind: "ask"; question: string; answer: string | null } | { kind: "steer"; text: string }
    )[];
  };
  unavailable?: boolean;
  /** One of the person's notes: the note it is and its first words after the title. */
  note?: { id: string; preview: string };
  /** The stage a run is working in right now; it glows while that is true. */
  active?: boolean;
  /** The user's recorded choice about this element. */
  decision?: string;
  /** An admitted media asset; the image URL is derived from profile and digest. */
  media?: { profile: string; asset: MediaAssetV1 };
  /** An admitted image related to this element, shown as its picture. */
  image?: WorkPicture;
};
/** A part's row head as the canvas shows it. */
export type PartView = {
  title: string;
  /** The site whose mark stands for the part. */
  host?: string;
  helper: "browser" | "research" | "computer" | "connection" | "lead";
  state: "planned" | "running" | "waiting" | "done" | "failed" | "stopped";
  /**
   * Its pages as windows, a list of what search cited, a helper's own view of
   * its work, an ask waiting on the person, or just its name.
   */
  shape: "pages" | "sources" | "helper" | "ask" | "label";
  /** What it came to, in a few words: "3 homes". */
  summary?: string;
  /** What the lead gave it to do. */
  goal?: string;
  /** While it works, what it is doing now, in words. */
  now?: string;
  pages: readonly PartPage[];
  /** What a search part cited, first few; a web search's results, marked when the run opened them. */
  cited?: readonly { key: string; url: string; where: string; title: string; opened?: boolean }[];
  /** The rows are one web search's results, not the sites research cited. */
  search?: boolean;
  /** Every result is on one site: its mark says so, so no row repeats its name. */
  oneSite?: boolean;
  /** What the run searched for, and how many searches the row stands for. */
  query?: string;
  queries?: number;
  /** Everything it cited, for the count past the first few. */
  citedCount?: number;
  /** The connection a connection part works through, for its mark. */
  connection?: string;
  /** The work the part belongs to and its steps, for a helper's own view. */
  objective?: string;
  steps?: readonly string[];
  /** What a computer part touched, for the view that stands in until the helper's own. */
  lines?: readonly { kind: "read" | "write" | "command" | "search"; text: string }[];
  /** A question on this part waiting on the person: the ask card's props. */
  ask?: { props: Record<string, unknown> };
  /** What the part needs from the person to do its job, and its one action. */
  need?: PartNeed;
  /** Pages of the site its waiting page is on that wait behind that one, counted with it. */
  pending?: number;
  /** Notes it handed the lead, read in its opened view. */
  notes?: readonly ObjectView[];
};
/** A part's need as its row says it: a sentence and the action that meets it. */
export type PartNeed = {
  kind:
    | "sign_in"
    | "check"
    | "allow_site"
    | "allow_folder"
    | "choose_folder"
    | "use_connection"
    | "connect"
    | "retry";
  /** The site, folder or connection it concerns, as a person names it. */
  target: string;
  /** The page to open for a sign-in, the folder to allow. */
  address?: string;
  /** Why the part could not do its job, in closed words. */
  reason?:
    "couldnt_read" | "signed_out" | "blocked_by_check" | "not_found" | "site_error" | "no_answer";
  /** The site a connection would stand in for, as a person names it. */
  site?: string;
  /** The run has settled: a check or a sign-in is told as what happened, not as what it waits on. */
  past?: true;
};
/** One page of a part, as its row shows it. */
export type PartPage = {
  id: string;
  url: string;
  title: string;
  /** How the read went, or that it waits on the person. */
  status: string;
  frame: string | null;
  live: boolean;
  /** Set while the run is holding this page open for a person. */
  human?: HumanPage;
  /** The page was read. */
  read?: boolean;
  /** Read with the person's signed-in session: the host it was granted on. */
  account?: string;
  /** An open tab the request was shown, not read. */
  tab?: boolean;
};
export type CanvasLink = {
  id: string;
  source: string;
  target: string;
  /** `thread` joins one request to the next; relation kinds show only while an end is hovered or selected. */
  kind: "dependency" | "reference" | "thread" | "flow" | RelationKind;
  label?: string;
  /** A run's line while work is moving along it. */
  live?: boolean;
  /** Drawn only while one of its ends is under the pointer or selected: it would cross a run to reach the other. */
  hover?: boolean;
  /** A run's line as its layout routed it, and where it truly meets its two nodes. */
  route?: {
    points: readonly CanvasPosition[];
    from: CanvasPosition;
    to: CanvasPosition;
    laid: { source: CanvasPosition; target: CanvasPosition };
  };
};
/** Past this many ties a focused card lights none: a fan of lines says nothing. */
const RELATION_CAP = 6;
/** Cards that never light a tie: the lane already says where they stand. */
const QUIET = new Set<CanvasKind | undefined>([
  "objective",
  "request",
  "sources",
  "result",
  "agent",
  "page",
  "block",
  "head",
  "trail",
  "object",
]);
/** Drawn at rest: the thread, a band's flow, and a plan's own structure. */
export const restLink = (link: CanvasLink) =>
  link.kind === "thread" ||
  link.kind === "flow" ||
  link.kind === "dependency" ||
  link.kind === "reference";
/**
 * The relations a focused card lights: the ties the person drew between their
 * own cards. More than six light nothing.
 */
export function relationLinks(
  links: readonly CanvasLink[],
  items: ReadonlyMap<string, CanvasItem>,
  focused: ReadonlySet<string>,
): Set<string> {
  const lit = new Set<string>();
  for (const id of focused) {
    const item = items.get(id);
    if (!item) continue;
    const type = item.type ?? (item.artifact ? "result" : "objective");
    if (QUIET.has(type) || item.artifact) continue;
    const ties = links.filter((link) => {
      if (restLink(link)) return false;
      const other = items.get(link.source === id ? link.target : link.source);
      return (
        (link.source === id || link.target === id) &&
        !!other &&
        !QUIET.has(other.type) &&
        !other.artifact
      );
    });
    if (ties.length <= RELATION_CAP) for (const link of ties) lit.add(link.id);
  }
  return lit;
}
export type CanvasPosition = { x: number; y: number };
export type CanvasSize = { width: number; height: number };
export type CanvasArea = { id: string; title: string };
export type CanvasView = {
  positions: Record<string, CanvasPosition>;
  sizes?: Record<string, CanvasSize>;
  areas?: Record<string, CanvasPosition & CanvasSize>;
  viewport: { x: number; y: number; zoom: number };
};
export type AreaData = { title: string; count: number };
export type WorkItemNode = Node<CanvasItem, "work">;
type AgentNode = Node<CanvasItem, "agent">;
export type WorkNode = WorkItemNode | Node<AreaData, "area"> | AgentNode;
const AREA_PREFIX = "area:";
const areaNodeId = (id: string) => `${AREA_PREFIX}${id}`;
export const isAreaNode = (node: WorkNode): node is Node<AreaData, "area"> => node.type === "area";
export const isItemNode = (node: WorkNode): node is WorkItemNode => node.type === "work";
export const isAgentNode = (node: WorkNode): node is AgentNode => node.type === "agent";
const DEFAULT_AREA: CanvasSize = { width: 640, height: 420 };
/** The agent's orb; its caption hangs outside the node. */
const MARK_SIZE = 24;
const validPosition = (p: CanvasPosition | undefined): p is CanvasPosition =>
  !!p &&
  Number.isFinite(p.x) &&
  Number.isFinite(p.y) &&
  Math.abs(p.x) <= 1_000_000 &&
  Math.abs(p.y) <= 1_000_000;
const validSize = (s: CanvasSize | undefined): s is CanvasSize =>
  !!s &&
  Number.isInteger(s.width) &&
  Number.isInteger(s.height) &&
  s.width >= 120 &&
  s.width <= 4096 &&
  s.height >= 80 &&
  s.height <= 4096;
const validAreaSize = (s: CanvasSize | undefined): s is CanvasSize =>
  !!s &&
  Number.isInteger(s.width) &&
  Number.isInteger(s.height) &&
  s.width >= 240 &&
  s.width <= 8192 &&
  s.height >= 160 &&
  s.height <= 8192;
const CANVAS_ITEM_LIMIT = 500;
const CANVAS_LINK_LIMIT = 2000;
export const TEXT_LIMIT = { id: 128, title: 512, detail: 2048, kind: 128, status: 256 } as const;

/** Clips to a whole character: a cut never splits a surrogate pair. */
export function clipText(value: string, max: number): string {
  if (value.length <= max) return value;
  const cut = value.slice(0, max);
  const last = cut.charCodeAt(max - 1);
  return last >= 0xd800 && last <= 0xdbff ? cut.slice(0, -1) : cut;
}

/**
 * Whatever a producer hands over becomes a scene the canvas can draw: long text
 * is clipped, repeated and empty identities are dropped, links that lead
 * nowhere are left out, and the counts stop at the limits. One bad item costs
 * that item, never the canvas.
 */
export function sanitizeScene(
  items: readonly CanvasItem[],
  links: readonly CanvasLink[],
): { items: CanvasItem[]; links: CanvasLink[] } {
  const ids = new Set<string>();
  const kept: CanvasItem[] = [];
  for (const item of items) {
    if (kept.length >= CANVAS_ITEM_LIMIT) break;
    if (!item.id || item.id.length > TEXT_LIMIT.id || ids.has(item.id)) continue;
    ids.add(item.id);
    const title = clipText(item.title, TEXT_LIMIT.title);
    const detail = clipText(item.detail, TEXT_LIMIT.detail);
    const kind = clipText(item.kind, TEXT_LIMIT.kind);
    const status = clipText(item.status, TEXT_LIMIT.status);
    kept.push(
      title === item.title && detail === item.detail && kind === item.kind && status === item.status
        ? item
        : { ...item, title, detail, kind, status },
    );
  }
  const seen = new Set<string>();
  const edges: CanvasLink[] = [];
  for (const link of links) {
    if (edges.length >= CANVAS_LINK_LIMIT) break;
    if (!link.id || link.id.length > TEXT_LIMIT.id || seen.has(link.id)) continue;
    if (link.source === link.target || !ids.has(link.source) || !ids.has(link.target)) continue;
    seen.add(link.id);
    edges.push(link);
  }
  return { items: kept, links: edges };
}

export function validScene(items: readonly CanvasItem[], links: readonly CanvasLink[]): boolean {
  if (items.length > CANVAS_ITEM_LIMIT || links.length > CANVAS_LINK_LIMIT) return false;
  const ids = new Set(items.map((item) => item.id));
  return (
    ids.size === items.length &&
    items.every(
      (item) =>
        item.id.length > 0 &&
        item.id.length <= TEXT_LIMIT.id &&
        item.title.length <= TEXT_LIMIT.title &&
        item.detail.length <= TEXT_LIMIT.detail &&
        item.kind.length <= TEXT_LIMIT.kind &&
        item.status.length <= TEXT_LIMIT.status,
    ) &&
    new Set(links.map((link) => link.id)).size === links.length &&
    links.every(
      (link) =>
        link.id.length > 0 &&
        link.id.length <= TEXT_LIMIT.id &&
        ids.has(link.source) &&
        ids.has(link.target) &&
        link.source !== link.target,
    )
  );
}

/** Position once. Subsequent projection changes preserve user arrangement and node identity. */
export function reconcileNodes(
  previous: WorkNode[],
  items: readonly CanvasItem[],
  positions: Readonly<Record<string, CanvasPosition>> = {},
  sizes: Readonly<Record<string, CanvasSize>> = {},
  areas: readonly CanvasArea[] = [],
  areaPlacements: Readonly<Record<string, CanvasPosition & CanvasSize>> = {},
  /** The positions the last call was given: a card follows only a place that changed. */
  before: Readonly<Record<string, CanvasPosition>> = positions,
): WorkNode[] {
  const existing = new Map(previous.map((node) => [node.id, node]));
  const areaNodes: Node<AreaData, "area">[] = areas.map((area, index) => {
    const id = areaNodeId(area.id);
    const count = items.filter((item) => item.area === area.id).length;
    const node = existing.get(id);
    if (node && isAreaNode(node)) {
      return node.data.title === area.title && node.data.count === count
        ? node
        : { ...node, data: { title: area.title, count } };
    }
    const placement = areaPlacements[area.id];
    const valid = placement && validPosition(placement) && validAreaSize(placement);
    return {
      id,
      type: "area",
      position: valid
        ? { x: placement.x, y: placement.y }
        : { x: 80 + index * 60, y: 80 + index * 60 },
      width: valid ? placement.width : DEFAULT_AREA.width,
      height: valid ? placement.height : DEFAULT_AREA.height,
      data: { title: area.title, count },
      dragHandle: ".area-title",
      deletable: false,
      connectable: false,
      selectable: true,
      zIndex: -1,
      ariaLabel: area.title,
    };
  });
  const areaById = new Map(areaNodes.map((node) => [node.id, node]));
  const occupied = items.flatMap((item) => {
    const node = existing.get(item.id);
    return node ? [absolutePosition(node, previous)] : [];
  });
  function nextPosition(index: number): CanvasPosition {
    let position: CanvasPosition;
    do {
      position = { x: 80 + (index % 3) * 520, y: 120 + Math.floor(index / 3) * 420 };
      index++;
    } while (
      occupied.some(
        (other) => Math.abs(other.x - position.x) < 500 && Math.abs(other.y - position.y) < 400,
      )
    );
    return position;
  }
  const next: WorkNode[] = items.map((item, index): WorkNode => {
    const node = existing.get(item.id);
    const parentId = item.area ? areaNodeId(item.area) : undefined;
    const parent = parentId ? areaById.get(parentId) : undefined;
    if (node && (isItemNode(node) || isAgentNode(node))) {
      const reparented = (node.parentId ?? undefined) !== (parent ? parentId : undefined);
      const same =
        !reparented &&
        node.data.title === item.title &&
        node.data.kind === item.kind &&
        node.data.type === item.type &&
        node.data.area === item.area &&
        node.data.unavailable === item.unavailable &&
        node.data.detail === item.detail &&
        node.data.status === item.status &&
        node.data.icon?.origin === item.icon?.origin &&
        node.data.icon?.revision === item.icon?.revision &&
        node.data.artifact === item.artifact &&
        node.data.layout === item.layout &&
        node.data.actionLabel === item.actionLabel &&
        node.data.subject === item.subject &&
        node.data.decision === item.decision &&
        node.data.active === item.active &&
        JSON.stringify(node.data.sources) === JSON.stringify(item.sources) &&
        JSON.stringify(node.data.unread) === JSON.stringify(item.unread) &&
        JSON.stringify(node.data.frames) === JSON.stringify(item.frames) &&
        node.data.media?.asset.digest === item.media?.asset.digest &&
        node.data.image?.digest === item.image?.digest &&
        JSON.stringify(node.data.agent) === JSON.stringify(item.agent) &&
        JSON.stringify(node.data.page) === JSON.stringify(item.page) &&
        JSON.stringify(node.data.part) === JSON.stringify(item.part) &&
        JSON.stringify(node.data.input) === JSON.stringify(item.input) &&
        JSON.stringify(node.data.turns) === JSON.stringify(item.turns) &&
        JSON.stringify(node.data.remember) === JSON.stringify(item.remember) &&
        node.data.expanded === item.expanded &&
        node.data.when === item.when &&
        JSON.stringify(node.data.object) === JSON.stringify(item.object) &&
        JSON.stringify(node.data.facts) === JSON.stringify(item.facts) &&
        JSON.stringify(node.data.block) === JSON.stringify(item.block) &&
        JSON.stringify(node.data.trail) === JSON.stringify(item.trail) &&
        JSON.stringify(node.data.size) === JSON.stringify(item.size);
      // The agent follows its work; a card follows its lane when the lane moves it.
      const target = positions[item.id];
      const followed =
        !!item.agent ||
        (!parent &&
          !!target &&
          (before[item.id]?.x !== target.x || before[item.id]?.y !== target.y));
      const kept = target;
      const relocated =
        followed &&
        validPosition(kept) &&
        !node.dragging &&
        (node.position.x !== kept.x || node.position.y !== kept.y);
      if (same && !relocated) return node;
      if (isAgentNode(node))
        return {
          ...node,
          data: item,
          ariaLabel: item.status,
          ...(relocated ? { position: { ...kept! } } : {}),
        };
      // A lane card takes the size its lane gives it; one nobody sized follows what it
      // says; a saved or resized one keeps its size.
      const was = defaultSize(node.data);
      const grown = item.size
        ? item.size
        : !same &&
            !Object.hasOwn(sizes, item.id) &&
            node.width === was.width &&
            node.height === was.height
          ? defaultSize(item)
          : undefined;
      const size = grown ? { width: grown.width, height: grown.height } : {};
      if (!reparented)
        return {
          ...node,
          ...size,
          data: item,
          ariaLabel: `${item.title}. ${item.status}`,
          ...(relocated ? { position: { ...kept! } } : {}),
        };
      const absolute = absolutePosition(node, previous);
      return {
        ...node,
        ...size,
        data: item,
        ariaLabel: `${item.title}. ${item.status}`,
        parentId: parent ? parentId : undefined,
        position: parent
          ? { x: absolute.x - parent.position.x, y: absolute.y - parent.position.y }
          : absolute,
      };
    }
    const restored = Object.hasOwn(positions, item.id) ? positions[item.id] : undefined;
    const absolute = validPosition(restored) ? { ...restored! } : nextPosition(index);
    if (item.agent)
      return {
        id: item.id,
        type: "agent",
        position: absolute,
        data: item,
        width: MARK_SIZE,
        height: MARK_SIZE,
        selectable: false,
        draggable: false,
        focusable: false,
        deletable: false,
        connectable: false,
        zIndex: 1000,
        ariaLabel: item.status,
      };
    occupied.push(absolute);
    const position = parent
      ? { x: absolute.x - parent.position.x, y: absolute.y - parent.position.y }
      : absolute;
    const size = item.size ?? sizes[item.id];
    const restoredSize =
      size &&
      Number.isInteger(size.width) &&
      Number.isInteger(size.height) &&
      size.width >= 120 &&
      size.width <= 4096 &&
      size.height >= 80 &&
      size.height <= 4096
        ? size
        : undefined;
    return {
      id: item.id,
      type: "work",
      position,
      ...(parent ? { parentId } : {}),
      // A run's nodes glide when the run makes room.
      ...(item.type === "block" ||
      item.type === "part" ||
      item.type === "head" ||
      item.type === "object"
        ? { class: "board-block" }
        : {}),
      data: item,
      width: restoredSize?.width ?? defaultSize(item).width,
      height: restoredSize?.height ?? defaultSize(item).height,
      dragHandle: ".work-drag-handle",
      deletable: false,
      connectable: false,
      ariaLabel: `${item.title}. ${item.status}`,
    };
  });
  const combined: WorkNode[] = [...areaNodes, ...next];
  return combined.length === previous.length &&
    combined.every((node, index) => node === previous[index])
    ? previous
    : combined;
}

/** Applies a remote view to existing nodes in place; dragging and derived nodes keep local geometry. */
export function applyRemoteView(
  previous: WorkNode[],
  view: CanvasView,
  authoritative: ReadonlySet<string>,
): WorkNode[] {
  let changed = false;
  const areaMoves = new Map<string, CanvasPosition>();
  const next = previous.map((node): WorkNode => {
    if (node.dragging) return node;
    if (isAreaNode(node)) {
      const placement = view.areas?.[node.id.slice(AREA_PREFIX.length)];
      if (!placement || !validPosition(placement) || !validAreaSize(placement)) return node;
      if (
        node.position.x === placement.x &&
        node.position.y === placement.y &&
        node.width === placement.width &&
        node.height === placement.height
      )
        return node;
      changed = true;
      areaMoves.set(node.id, { x: placement.x, y: placement.y });
      return {
        ...node,
        position: { x: placement.x, y: placement.y },
        width: placement.width,
        height: placement.height,
      };
    }
    if (!authoritative.has(node.id)) return node;
    const position = view.positions[node.id];
    const size = view.sizes?.[node.id];
    const parent = node.parentId
      ? (areaMoves.get(node.parentId) ??
        previous.find((candidate) => candidate.id === node.parentId)?.position)
      : undefined;
    const relative = validPosition(position)
      ? parent
        ? { x: position.x - parent.x, y: position.y - parent.y }
        : { ...position }
      : undefined;
    const samePosition =
      !relative || (node.position.x === relative.x && node.position.y === relative.y);
    const sameSize = !validSize(size) || (node.width === size.width && node.height === size.height);
    if (samePosition && sameSize) return node;
    changed = true;
    return {
      ...node,
      ...(samePosition ? {} : { position: relative! }),
      ...(sameSize ? {} : { width: size!.width, height: size!.height }),
    };
  });
  return changed ? next : previous;
}

/** Area containing the node's centre, if any. */
export function containingArea(node: WorkNode, nodes: readonly WorkNode[]): string | null {
  if (isAreaNode(node)) return null;
  const absolute = absolutePosition(node, nodes);
  const cx = absolute.x + (node.width ?? 280) / 2;
  const cy = absolute.y + (node.height ?? 160) / 2;
  for (const candidate of nodes) {
    if (!isAreaNode(candidate)) continue;
    const w = candidate.width ?? DEFAULT_AREA.width;
    const h = candidate.height ?? DEFAULT_AREA.height;
    if (
      cx >= candidate.position.x &&
      cx <= candidate.position.x + w &&
      cy >= candidate.position.y &&
      cy <= candidate.position.y + h
    )
      return candidate.id.slice(AREA_PREFIX.length);
  }
  return null;
}

/** Bounds around the given nodes in absolute canvas coordinates. */
export function nodesBounds(
  ids: readonly string[],
  nodes: readonly WorkNode[],
  padding = 32,
): (CanvasPosition & CanvasSize) | null {
  const selected = nodes.filter((node) => ids.includes(node.id) && isItemNode(node));
  if (!selected.length) return null;
  let minX = Infinity;
  let minY = Infinity;
  let maxX = -Infinity;
  let maxY = -Infinity;
  for (const node of selected) {
    const p = absolutePosition(node, nodes);
    minX = Math.min(minX, p.x);
    minY = Math.min(minY, p.y);
    maxX = Math.max(maxX, p.x + (node.width ?? 280));
    maxY = Math.max(maxY, p.y + (node.height ?? 160));
  }
  return {
    x: Math.round(minX - padding),
    y: Math.round(minY - padding - 36),
    width: Math.max(240, Math.round(maxX - minX + padding * 2)),
    height: Math.max(160, Math.round(maxY - minY + padding * 2 + 36)),
  };
}

/** Invalid saved geometry never reaches XYFlow. User arrangement is view state only. */
export function validViewport(
  value: CanvasView["viewport"] | undefined,
): CanvasView["viewport"] | undefined {
  return value &&
    Number.isFinite(value.x) &&
    Number.isFinite(value.y) &&
    Math.abs(value.x) <= 1_000_000 &&
    Math.abs(value.y) <= 1_000_000 &&
    Number.isFinite(value.zoom) &&
    value.zoom >= 0.2 &&
    value.zoom <= 2
    ? { ...value }
    : undefined;
}
