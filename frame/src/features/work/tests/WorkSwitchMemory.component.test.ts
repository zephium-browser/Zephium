import "$styles/global.css";
import { expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { commands as files, page } from "vitest/browser";
import type {
  WorkEnvironmentSnapshot,
  WorkEnvironmentSummary,
  WorkRuntimeProjection,
} from "$shared/ipc/bindings";
import type { MediaAssetV1 } from "$domain/resources";
import { WorkEnvironmentSession } from "$domain/work-environment";
import type { BoardScene } from "./board-fixtures";

/**
 * A day of switching works: the QA profile's real works (see WorkBandLook) and
 * an all-day work of thirty runs, opened in turn on the real Work screen three
 * times over, then left at rest. Each step reports what the page holds: nodes,
 * canvases, pictures, workers, timers, animations; and, when
 * a footprint sampler (answering in node_modules/.work-memory) runs beside it, each WebKit process's
 * footprint and CPU time.
 */
const LOOK = "/node_modules/.work-look";
const OUT = "node_modules/.work-memory";
const frames = new Map<string, string>();
const pictures = new Map<string, string>();
vi.mock("$domain/resources", async (original) => ({
  ...(await original<typeof import("$domain/resources")>()),
  pageFrameUrl: (attempt: string, step: string) => frames.get(`${attempt}-${step}`) ?? null,
  mediaUrl: (_profile: string, digest: string) => pictures.get(digest) ?? "",
}));
const native = vi.hoisted(() => ({
  works: new Map<string, unknown>(),
  objectives: new Map<string, unknown>(),
  pages: new Map<string, unknown[]>(),
  media: new Map<string, unknown>(),
  summaries: [] as unknown[],
  unmocked: new Set<string>(),
  minted: 0,
}));
vi.mock("$shared/ipc/bindings", async () => {
  const { composerCall } = await import("./composer-fixtures");
  const reply = (profile: string, body: unknown) => ({ version: 1, profile, reply: body });
  const known: Record<string, (...args: never[]) => Promise<unknown>> = {
    faviconProbe: async () => true,
    workPaneShow: async () => ({ accepted: true, operation_id: null }),
    workPaneHide: async () => ({ accepted: true }),
    workPaneSetRect: async () => ({ accepted: true }),
    mediaAdmitRemote: async () => ({ status: "error", error: "unavailable" }),
    workActivity: async (profile: string, work: string) => ({
      version: 1,
      profile,
      work,
      signals: [],
      pages: native.pages.get(work) ?? [],
      error: null,
    }),
    noteCall: async (profile: string) => ({
      profile,
      response: { kind: "error", error: "not_found" },
    }),
    resourceCall: async (profile: string, call: { kind: string; id?: string }) => {
      const asset = call.id ? native.media.get(call.id) : undefined;
      return {
        profile,
        response: asset
          ? {
              kind: "record",
              record: {
                id: call.id,
                revision: "1",
                created_at: "0",
                updated_at: "0",
                trashed: false,
                draft: { content: { kind: "media", asset } },
              },
            }
          : { kind: "error", error: "not_found" },
      };
    },
    workCall: async (profile: string, call: { kind: string; request: unknown }) => {
      const request = call.request as {
        kind: string;
        id?: string;
        query?: { kind: string; work?: string };
        view?: { revision: string };
        expected?: string;
      };
      if (call.kind === "environment") {
        if (request.kind === "list")
          return reply(profile, {
            kind: "environment",
            reply: { kind: "page", works: native.summaries, selected: null, next: null },
          });
        if ((request.kind === "open" || request.kind === "read") && native.works.has(request.id!))
          return reply(profile, {
            kind: "environment",
            reply: { kind: "snapshot", snapshot: native.works.get(request.id!) },
          });
        if (request.kind === "checkpoint") {
          const current = native.works.get(request.id!) as WorkEnvironmentSnapshot;
          const applied = String(BigInt(request.expected!) + 1n);
          const snapshot = {
            ...current,
            view: { ...(request.view as object), revision: applied },
          };
          native.works.set(request.id!, snapshot);
          return reply(profile, {
            kind: "environment",
            reply: {
              kind: "checkpointed",
              expected: request.expected,
              applied_view_revision: applied,
              replayed: false,
              snapshot,
            },
          });
        }
      }
      if (call.kind === "environment" && request.kind === "command") {
        // Rust accepts what the canvas places: an added element and a relation stand.
        const { command, intent } = request as unknown as {
          command: string;
          intent: { kind: string; id: string; edit: Record<string, unknown> };
        };
        const current = native.works.get(intent.id) as WorkEnvironmentSnapshot | undefined;
        const edit = intent.edit;
        if (intent.kind === "edit" && current && (edit.kind === "add" || edit.kind === "relate")) {
          native.minted += 1;
          const id = `01M3ZZZZZZZZZZZZZZZZ${String(native.minted).padStart(6, "0")}`;
          const revision = String(BigInt(current.revision) + 1n);
          const snapshot = {
            ...current,
            revision,
            ...(edit.kind === "add"
              ? { elements: [...current.elements, { id, area: null, reference: edit.reference }] }
              : {
                  relations: [
                    ...(current.relations ?? []),
                    { id, from: edit.from, to: edit.to, kind: edit.relation, origin: "agent" },
                  ],
                }),
          } as WorkEnvironmentSnapshot;
          native.works.set(intent.id, snapshot);
          return reply(profile, {
            kind: "environment",
            reply: {
              kind: "applied",
              command,
              applied_revision: revision,
              applied_view_revision: snapshot.view.revision,
              replayed: false,
              snapshot,
            },
          });
        }
      }
      if (call.kind === "query" && request.query?.kind === "projection") {
        const projection = native.objectives.get(request.query.work!);
        if (projection) return reply(profile, { kind: "projection", projection });
      }
      return reply(profile, { kind: "error", error: "not_found" });
    },
  };
  known.workCall = composerCall(known.workCall as Parameters<typeof composerCall>[0]);
  return {
    commands: new Proxy(known, {
      get(target, key) {
        if (typeof key === "symbol" || key === "then") return undefined;
        if (key in target) return target[key];
        native.unmocked.add(key);
        return async () => {
          throw new Error(`unmocked ${key}`);
        };
      },
    }),
  };
});

type Raw = {
  snapshot: BoardScene["snapshot"];
  objectives: Record<string, WorkRuntimeProjection>;
  pages: BoardScene["pages"];
  media?: Record<string, MediaAssetV1>;
};
const EXTENSION: Record<string, string> = {
  "image/png": "png",
  "image/jpeg": "jpg",
  "image/webp": "webp",
  "image/gif": "gif",
};
const CROCKFORD = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";
const ULID = /\b[0-9A-HJKMNP-TV-Z]{26}\b/gu;
const WORKS = [
  "trip",
  "jobs",
  "db",
  "learning",
  "compare",
  "compilers",
  "lego",
  "trip3",
  "yctrip",
  "r4browsers",
];
const ALL_DAY = [
  "trip3",
  "compare",
  "compilers",
  "lunios",
  "day",
  "yctrip",
  "aisaas",
  "lego",
  "learning",
  "jobs",
];

/** A work's copy with every id its own (the last ULID character set by the copy), its files mapped. */
function copy(raw: Raw, mark: number | null, name: string): Raw {
  const renamed = (text: string) =>
    mark === null
      ? text
      : text.replace(ULID, (id) =>
          id === PROFILE || id === raw.snapshot.space ? id : `${id.slice(0, 25)}${CROCKFORD[mark]}`,
        );
  const out = JSON.parse(renamed(JSON.stringify(raw))) as Raw;
  raw.pages.forEach((original, at) => {
    const one = out.pages[at]!;
    frames.set(
      `${one.attempt}-${one.step}`,
      `${LOOK}/${name}/frames/${original.attempt}-${original.step}.png${mark === null ? "" : `?v=${mark}`}`,
    );
  });
  for (const asset of Object.values(raw.media ?? {}))
    pictures.set(
      asset.digest,
      `${LOOK}/${name}/media/${asset.digest}.${EXTENSION[asset.mime] ?? "png"}${mark === null ? "" : `?v=${mark}`}`,
    );
  return out;
}

async function load(name: string): Promise<Raw | null> {
  const response = await fetch(`${LOOK}/${name}/scene.json`);
  return response.ok ? ((await response.json()) as Raw) : null;
}

const PROFILE = "01M3CTWDG20GFZPACD7905RSRJ";
const SPACE = "01M3CTWDG2AVF2G6YPBVJHJK5X";
const DRAFT = "01M3CTWDG2AVF2G6YPBVJHJK60";

/** Registers a work with the fake native side: its snapshot, runs, pages and pictures. */
function admit(part: Raw, id: string, title: string, touched: number) {
  const snapshot: WorkEnvironmentSnapshot = {
    ...part.snapshot,
    id,
    profile: PROFILE,
    space: SPACE,
    title,
    lifecycle: "active",
  };
  native.works.set(id, snapshot);
  for (const [objective, projection] of Object.entries(part.objectives))
    native.objectives.set(objective, projection);
  const byObjective = new Map<string, unknown[]>();
  const executionOf = new Map(
    Object.entries(part.objectives).flatMap(([objective, projection]) =>
      projection.executions.map((execution) => [execution.id, objective] as const),
    ),
  );
  for (const one of part.pages) {
    const objective = executionOf.get(one.execution);
    if (objective) byObjective.set(objective, [...(byObjective.get(objective) ?? []), one]);
  }
  for (const [objective, list] of byObjective) native.pages.set(objective, list);
  for (const element of snapshot.elements) {
    const reference = element.reference;
    if (reference.kind === "resource")
      native.media.set(reference.resource, Object.values(part.media ?? {})[0]);
  }
  const summary: WorkEnvironmentSummary = {
    id,
    space: SPACE,
    title,
    lifecycle: "active",
    revision: snapshot.revision,
    name: title,
    requests: [],
    touched_ms: String(touched),
    empty: !snapshot.elements.length,
  };
  native.summaries.push(summary);
}

let touched = 1_700_000_000_000;
/** The same works again as new ones: every id their own, so nothing cached for the first set is theirs. */
async function freshWorks(mark: number): Promise<string[]> {
  const ids: string[] = [];
  for (const [at, name] of WORKS.entries()) {
    const raw = await load(name);
    if (!raw) continue;
    const id = `01M3CTWDG2AVF2G6YPBVJH${CROCKFORD[mark]}${CROCKFORD[at]}${CROCKFORD[at]}${CROCKFORD[at]}`;
    admit(copy(raw, mark, name), id, `${name}-${mark}`, touched++);
    ids.push(id);
  }
  return ids;
}

async function buildWorks(): Promise<string[]> {
  const ids: string[] = [];
  for (const [at, name] of WORKS.entries()) {
    const raw = await load(name);
    if (!raw) continue;
    const id = `01M3CTWDG2AVF2G6YPBVJHJK${CROCKFORD[at]}${CROCKFORD[at]}`;
    admit(copy(raw, null, name), id, name, touched++);
    ids.push(id);
  }
  const loaded: [string, Raw][] = [];
  for (const name of ALL_DAY) {
    const raw = await load(name);
    if (raw) loaded.push([name, raw]);
  }
  const parts: Raw[] = [];
  for (let index = 0; index < 30 && loaded.length; index++) {
    const [name, raw] = loaded[index % loaded.length]!;
    parts.push(copy(raw, 10 + Math.floor(index / loaded.length), name));
  }
  if (parts.length) {
    const first = parts[0]!;
    const merged: Raw = {
      snapshot: {
        ...first.snapshot,
        elements: parts.flatMap((part) => part.snapshot.elements),
        relations: parts.flatMap((part) => part.snapshot.relations ?? []),
        view: { ...first.snapshot.view, placements: [] },
      },
      objectives: Object.fromEntries(parts.flatMap((part) => Object.entries(part.objectives))),
      pages: parts.flatMap((part) => part.pages),
      media: Object.fromEntries(parts.flatMap((part) => Object.entries(part.media ?? {}))),
    };
    const id = "01M3CTWDG2AVF2G6YPBVJHJKZZ";
    admit(merged, id, "all day", touched++);
    ids.push(id);
  }
  native.works.set(DRAFT, {
    version: 1,
    id: DRAFT,
    profile: PROFILE,
    space: SPACE,
    title: "New work",
    lifecycle: "active",
    revision: "1",
    elements: [],
    areas: [],
    view: { revision: "1", x: 0, y: 0, zoom_milli: 1000, placements: [] },
  });
  native.summaries.unshift({
    id: DRAFT,
    space: SPACE,
    title: "New work",
    lifecycle: "active",
    revision: "1",
    name: null,
    requests: [],
    touched_ms: String(touched),
    empty: true,
  });
  return ids;
}

const intervals = new Set<number>();
const timeouts = new Set<number>();
const workers = { made: 0, ended: 0 };
function watchTimers() {
  const setI = window.setInterval.bind(window);
  const clearI = window.clearInterval.bind(window);
  const setT = window.setTimeout.bind(window);
  const clearT = window.clearTimeout.bind(window);
  window.setInterval = ((handler: TimerHandler, ms?: number, ...rest: unknown[]) => {
    const id = setI(handler, ms, ...rest);
    intervals.add(id);
    return id;
  }) as typeof window.setInterval;
  window.clearInterval = ((id?: number) => {
    if (id !== undefined) intervals.delete(id);
    clearI(id);
  }) as typeof window.clearInterval;
  window.setTimeout = ((handler: TimerHandler, ms?: number, ...rest: unknown[]) => {
    const id = setT(
      (...args: unknown[]) => {
        timeouts.delete(id);
        if (typeof handler === "function") (handler as (...a: unknown[]) => void)(...args);
      },
      ms,
      ...rest,
    );
    timeouts.add(id);
    return id;
  }) as typeof window.setTimeout;
  window.clearTimeout = ((id?: number) => {
    if (id !== undefined) timeouts.delete(id);
    clearT(id);
  }) as typeof window.clearTimeout;
  const Native = window.Worker;
  window.Worker = class extends Native {
    constructor(url: string | URL, options?: WorkerOptions) {
      super(url, options);
      workers.made += 1;
    }
    override terminate() {
      workers.ended += 1;
      super.terminate();
    }
  };
}

/** Allocation pressure until the collector has run: WebKit gives a page no gc(). */
async function collect() {
  for (let round = 0; round < 6; round++) {
    let held: ArrayBuffer[] = [];
    for (let at = 0; at < 64; at++) held.push(new ArrayBuffer(4 << 20));
    held = [];
    void held;
    await wait(250);
  }
  await wait(3000);
}

const wait = (ms: number) => new Promise((done) => setTimeout(done, ms));
const frame = () => new Promise((done) => requestAnimationFrame(() => done(null)));

function inPage() {
  let canvasBytes = 0;
  const canvases = document.querySelectorAll("canvas");
  for (const canvas of canvases) canvasBytes += canvas.width * canvas.height * 4;
  let imageBytes = 0;
  const images = document.querySelectorAll("img");
  for (const image of images)
    if (image.complete) imageBytes += image.naturalWidth * image.naturalHeight * 4;
  const oversized = [...images]
    .filter((image) => image.complete && image.naturalWidth > 2 * image.clientWidth + 1)
    .map(
      (image) =>
        `${image.className || image.parentElement?.className}:${image.naturalWidth}x${image.naturalHeight}@${image.clientWidth}x${image.clientHeight}`,
    );
  return {
    oversized,
    nodes: document.getElementsByTagName("*").length,
    flowNodes: document.querySelectorAll(".svelte-flow__node").length,
    canvases: canvases.length,
    canvasMB: +(canvasBytes / 1048576).toFixed(1),
    images: images.length,
    imageMB: +(imageBytes / 1048576).toFixed(1),
    workersLive: workers.made - workers.ended,
    intervals: intervals.size,
    timeouts: timeouts.size,
    animations: document.getAnimations().filter((a) => a.playState === "running").length,
  };
}

let sampler = true;
const RUN = Date.now().toString(36);
/** The native sampler's reading of every WebKit process, when it runs. */
async function footprint(label: string): Promise<string> {
  if (!sampler) return "";
  label = `${RUN}-${label}`;
  await files.writeFile(`${OUT}/ask`, label);
  for (let tries = 0; tries < 50; tries++) {
    await wait(200);
    try {
      return await files.readFile(`${OUT}/answer-${label}`);
    } catch {
      /* not yet */
    }
  }
  sampler = false;
  return "";
}

const ROUNDS = 2;
const report: Record<string, unknown> = {};
async function measure(label: string) {
  const here = inPage();
  const processes = (await footprint(label)).trim();
  report[label] = { ...here, processes };
  console.warn("work-switch-memory", label, JSON.stringify(here), `\n${processes}`);
}

test(
  "switching through a day of works holds memory flat and rests at zero",
  { timeout: 600_000 },
  async () => {
    await page.viewport(1440, 900);
    const ids = await buildWorks();
    if (ids.length < 2) return;
    watchTimers();
    const { default: WorkEnvironmentWorkspace } =
      await import("../components/WorkEnvironmentWorkspace.svelte");
    await measure("before");
    const session = new WorkEnvironmentSession(PROFILE, SPACE);
    const screen = await render(WorkEnvironmentWorkspace, {
      session,
      tabs: [],
      spaceName: "Personal",
      aiEnabled: true,
      onopen: () => {},
      onnewtab: () => {},
    });
    const root = screen.container.querySelector<HTMLElement>(".environment");
    if (root) {
      root.style.height = "900px";
      root.style.width = "1440px";
    }
    await session.start("New work");
    await expect.poll(() => session.snapshot?.id).toBe(DRAFT);
    await wait(1000);
    await measure("empty");
    const settle = async () => {
      let last = -1;
      let still = 0;
      for (let tick = 0; tick < 600 && still < 20; tick++) {
        await frame();
        const count = screen.container.querySelectorAll(".svelte-flow__node, canvas, img").length;
        still = count === last ? still + 1 : 0;
        last = count;
      }
      await wait(800);
    };
    for (const id of ids) {
      expect(await session.open(id)).toBe(true);
      await settle();
    }
    expect(await session.open(DRAFT)).toBe(true);
    await settle();
    await measure("warm");
    await collect();
    await measure("warm-collected");
    for (let round = 1; round <= ROUNDS; round++) {
      for (const id of ids) {
        expect(await session.open(id)).toBe(true);
        await settle();
        if (round === 1) {
          const name = native.summaries.find((w) => (w as { id: string }).id === id) as {
            title: string;
          };
          report[`open-${name.title}`] = inPage();
        }
      }
      await measure(`round${round}`);
      await collect();
      await measure(`round${round}-collected`);
    }
    for (const mark of [20, 21, 22]) {
      await session.reload();
      for (const id of await freshWorks(mark)) {
        expect(await session.open(id)).toBe(true);
        await settle();
      }
      await collect();
      await measure(`fresh${mark}-collected`);
    }
    expect(await session.open(DRAFT)).toBe(true);
    await settle();
    await measure("left-on-empty");
    await collect();
    await measure("left-collected");
    await wait(20_000);
    await measure("rest-20s");
    await wait(5_000);
    await measure("rest-25s");
    // At rest nothing ticks, nothing moves, and the layout worker has been let go.
    const rest = inPage();
    expect(rest.intervals).toBe(0);
    expect(rest.animations).toBe(0);
    expect(rest.workersLive).toBe(0);
    console.warn(
      "work-switch-memory-summary",
      JSON.stringify({ ...report, unmocked: [...native.unmocked] }),
    );
    session.dispose();
    await screen.unmount();
    screen.container.remove();
    await collect();
    await measure("unmounted-collected");
    console.warn("work-switch-memory-unmounted", JSON.stringify(report["unmounted-collected"]));
  },
);
