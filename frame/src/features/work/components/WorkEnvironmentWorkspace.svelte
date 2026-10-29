<script lang="ts">
  import { untrack, onMount, setContext } from "svelte";
  import { provideLeadLook } from "$shared/ui/presence/shapes";
  import { SvelteMap, SvelteSet } from "svelte/reactivity";
  import { WorkEnvironmentContext, type WorkEnvironmentSession } from "$domain/work-environment";
  import { commandId, workSession, type WorkSession } from "$domain/work";
  import { taskSession } from "$domain/resources";
  import { noteSession } from "$domain/notes";
  import * as toolHost from "$session/tools.svelte";
  import { tabRequest } from "$session/work-tab.svelte";
  import { pointerTool, setPointerTool } from "../lib/pointer-tool.svelte";
  import { ulidTime } from "../lib/ulid-time";
  import { defaultTitle, firstRunName } from "../lib/work-name";
  import { needWords } from "../lib/run/need";
  import { editTool, toolSession } from "$session/tool-drafts.svelte";
  import type {
    TabView,
    WorkHumanPageIdV1,
    WorkHumanRegionV1,
    WorkEnvironmentReference,
    WorkFileEvidenceV1,
    WorkExecutionFact,
    WorkRuntimeProjection,
  } from "$shared/ipc/bindings";
  import { commands } from "$shared/ipc/bindings";
  import { layout } from "$domain/layout";
  import { workPane, type WorkPaneRect, type WorkPaneTarget } from "$domain/work-pane";
  import { WorkHumanSession, type WorkHumanFailure } from "$domain/work-human";
  import { loadNoteHost } from "$features/notes";
  import Button from "$shared/ui/Button";
  import LazyView from "$shared/ui/LazyView";
  import {
    ComputerTerminal01Icon,
    File01Icon,
    FolderAddIcon,
    GlobalIcon,
    Image01Icon,
    Pdf01Icon,
    PlusSignIcon,
    ShapeCollectionIcon,
    Cursor01Icon,
    HandIcon,
    CheckListIcon,
  } from "../lib/icons";
  import WorkBar from "./bar/WorkBar.svelte";
  import { composerEdge, composerHole } from "../lib/composer-hole";
  import { centreSize, picksSheet } from "./objects/centre";
  import { partAsks } from "./asks/actions";
  import {
    ASK_WORDS,
    asksOf,
    exchangeOf,
    folderChoice,
    openAsks,
    registrableSite,
  } from "./asks/asks";
  import BarTool from "./bar/BarTool.svelte";
  import { preferences } from "$domain/preferences";
  import NotePanel from "./bar/NotePanel.svelte";
  import TasksPanel from "./bar/TasksPanel.svelte";
  import BlocksPanel, { type BlockKind } from "./bar/BlocksPanel.svelte";
  import AttachPanel, { type AttachKind } from "./bar/AttachPanel.svelte";
  import WorksMenu from "./top/WorksMenu.svelte";
  import ContextManifest from "./composer/ContextManifest.svelte";
  import OpenTabsChip from "./composer/OpenTabsChip.svelte";
  import { contextSelection, referencesCanvasBlock } from "../lib/context-selection";
  import WorkTabPicker from "./WorkTabPicker.svelte";
  import WorkMediaPicker from "./WorkMediaPicker.svelte";
  import { mediaSize, mediaUrl } from "$domain/resources";
  import BrowserPane from "./pane/BrowserPane.svelte";
  import Lift from "./Lift.svelte";
  import LiftHeader from "./LiftHeader.svelte";
  import { commandRecord } from "../lib/project-environment-stage";
  import {
    elementPictures,
    environmentStages,
    fetchedPictures,
    laneElement,
    measurer,
    ownPlaces,
  } from "../lib/project-environment-board";
  import type { BoardActions, DraftEdit } from "../lib/canvas-context";
  import { OwnedObjects } from "$domain/resources";
  import { ownedView } from "../lib/board/objects";
  import { objectHeight, objectWidth } from "../lib/board/object-size";
  import type { DraftView, ObjectActions, ObjectView, OwnedData } from "../lib/board/types";
  import { objectIcon, objectLabel } from "../lib/object-kind";
  import { clipText, defaultSize } from "../lib/canvas-model";
  import { homePath } from "../lib/work-files";
  import { youtubeId } from "../lib/link-media";
  import { resultPlan } from "../lib/plan-steps";
  import { stepPlan, WorkTasks, workTasksKey, type StepPlan } from "../lib/work-tasks";
  import { documentMarkdown, resultKey, WorkNotes } from "../lib/work-notes";
  import type { LiftAction } from "./LiftHeader.svelte";
  import { isAgentExecution, isLive } from "../lib/agent-steps";
  import { draftSend } from "../lib/draft-send";
  import {
    environmentAgents,
    environmentBoards,
    environmentItems,
    environmentLinks,
    environmentInputs,
    environmentSources,
    environmentParts,
    environmentPictures,
    environmentView,
    fileEvidence,
    viewPlacements,
  } from "../lib/project-environment";
  import { canvasOpenTasks, canvasProbe } from "../lib/canvas-context";
  import {
    environmentRequests,
    requestDress,
    runFolders,
    stageMade,
  } from "../lib/project-environment-thread";
  import { artifactView } from "../lib/project-work";
  import RequestLift from "./lift/RequestLift.svelte";
  import SourcesLift from "./lift/SourcesLift.svelte";
  import { failureLine, humanPage, regionOf, type HumanPage } from "../lib/work-human";
  import { openOver, type PaneRect } from "../lib/pane-geometry";
  import { organizeExecution, pendingOrganize, elementFor } from "../lib/organize";
  import { CanvasPlacing } from "../lib/workspace/placing.svelte";
  import { dropSpot, freeSpot } from "../lib/free-space";
  import { PictureQueue } from "../lib/workspace/pictures.svelte";
  import { environmentResults, type ResultReference } from "../lib/project-environment-results";
  import type { EvidenceReference } from "$shared/ui/data/Artifact";
  import type { CanvasView, CanvasItem, CanvasPosition } from "../lib/canvas-model";
  import { currentModel } from "../lib/model-name";
  import * as m from "$shared/i18n/messages";
  let {
    session,
    tabs,
    spaceName,
    currentTabId = null,
    aiEnabled = true,
    onopen,
    onnewtab,
  }: {
    session: WorkEnvironmentSession;
    tabs: readonly TabView[];
    spaceName: string;
    /** The Space's current tab, so the picker reads like its tab strip. */
    currentTabId?: string | null;
    aiEnabled?: boolean;
    /** Explicit Browse handoff for one Space tab; the pane is the default way to look at a page. */
    onopen: (id: string) => void;
    onnewtab: () => void;
  } = $props();
  const context = untrack(() => new WorkEnvironmentContext(session.profile));
  onMount(() => {
    void context.start();
    return () => context.dispose();
  });
  // The pages the runs on this canvas are holding open for a person.
  const human = untrack(() => new WorkHumanSession(session.profile));
  /** The person's own tables, checklists and calendars on this canvas. */
  const owned = untrack(() => new OwnedObjects(session.profile));
  onMount(() => {
    void owned.start().catch(() => {
      blockSaveFailed = true;
    });
    return () => owned.stop();
  });
  onMount(() => {
    void human.start();
    return () => human.dispose();
  });
  $effect(() => {
    human.update([...context.objectives.keys()]);
  });
  const notesHost = $derived(`environment:${session.space}`);
  const notes = untrack(() => noteSession(session.profile, notesHost));
  onMount(() => {
    void notes?.start();
    return () => notes?.stop();
  });
  // Steps read their tasks from a session of their own, whose scope never changes.
  const stepTaskSession = untrack(() => taskSession(session.profile, `work:${session.space}`));
  onMount(() => {
    let live = true;
    void stepTaskSession.start().then(async () => {
      // A lane's tasks may sit past the first page; a few pages cover a working list.
      while (live && stepTaskSession.next && stepTaskSession.items.length < 500)
        await stepTaskSession.reload(true);
    });
    return () => {
      live = false;
      stepTaskSession.stop();
    };
  });
  let objectiveSession = $state.raw<WorkSession | null>(null);
  let inspected = $state<string | null>(null);
  let lifted = $state.raw<{
    id: string;
    origin: DOMRect | null;
    /** A change the run proposed: the lift opens on the step, not on a card. */
    proposal?: string;
    /** Picks opened side by side as a sheet. */
    compare?: boolean;
  } | null>(null);
  let canvasRef = $state<{
    screenRect: (id: string) => DOMRect | null;
    flowPosition: (clientX: number, clientY: number) => CanvasPosition | null;
    selectionBounds: (only?: readonly string[]) => {
      x: number;
      y: number;
      width: number;
      height: number;
      ids: string[];
    } | null;
    placeArea: (
      area: string,
      rect: { x: number; y: number; width: number; height: number },
    ) => void;
    center: (id: string) => void;
    focusCard: (id: string) => void;
    settle: (id: string, span?: number) => void;
    followAgent: () => boolean;
  }>();
  let selectedIds = $state.raw<string[]>([]);
  /** The person's consent to list their open tabs, for the next request only. */
  let openTabs = $state(false);
  const contextSel = $derived(
    contextSelection(
      session.snapshot,
      selectedIds,
      tabs,
      {
        notes: context.notes,
        objectives: context.objectives,
        media: context.mediaRevisions,
        objects: owned.revisions(),
        urls: context.urlRevisions,
      },
      openTabs,
      referencesCanvasBlock(session.composer),
    ),
  );
  let cardHost = $state<HTMLElement>();
  let cardBounds = $state.raw<DOMRect | null>(null);
  $effect(() => {
    const element = cardHost;
    if (!element) return;
    const measure = () => (cardBounds = element.getBoundingClientRect());
    measure();
    // Size alone misses a move: the column beside the canvas can change width
    // while the canvas keeps its own, and a pane placed on stale bounds lands
    // off the window.
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    window.addEventListener("resize", measure);
    return () => {
      observer.disconnect();
      window.removeEventListener("resize", measure);
    };
  });
  function openLift(id: string) {
    const item = items.find((item) => item.id === id);
    if (!item) return;
    // A block opens where it stands; its board makes room.
    if (item.type === "block" || item.type === "object") {
      // A reviewed plan's result keeps its review: it opens in the lift, on its run.
      if (results.references.has(id) && !agentBlock(id)) void openResult(id);
      else if (item.type === "object") lift(id);
      else toggleBlock(id);
      return;
    }
    if (item.type === "head" || item.type === "trail") return;
    if (item.type === "objective" || item.type === "request") {
      lift(id);
      return;
    }
    if (results.references.has(id)) {
      void openResult(id);
      return;
    }
    if (item.type === "page" && item.page) {
      // A page the run is waiting on opens as a takeover, not as a copy in the
      // person's own profile: a sign-in there never reaches the agent.
      if (item.page.human?.phase === "waiting_for_human") openTakeover(id);
      else openPane({ kind: "url", url: item.page.url }, id, item.page.account);
      return;
    }
    const reference = session.snapshot?.elements.find((element) => element.id === id)?.reference;
    if (reference?.kind === "link") {
      if (youtubeId(reference.url)) playHere(id);
      else openPane({ kind: "url", url: reference.url }, id);
      return;
    }
    if (reference?.kind === "browser") {
      if (tabs.some((tab) => tab.id === reference.tab))
        openPane({ kind: "tab", id: reference.tab }, id);
      return;
    }
    lift(id);
  }
  /** A page in a branch: the person's turn opens the run's own view; any other opens as a page. */
  function openBranchPage(page: string) {
    const entry = pageEntries.get(page);
    if (!entry) return;
    if (
      entry.page.human &&
      entry.page.human.phase !== "reading" &&
      entry.page.human.phase !== "released"
    ) {
      openTakeover(page);
      return;
    }
    openPane({ kind: "url", url: entry.page.url }, null, entry.page.account);
  }
  /**
   * The page an ask is about, opened in the centre: a page held for the
   * person opens its takeover; any other opens as itself.
   */
  function openStep(step: string) {
    for (const [id, entry] of pageEntries)
      if (id.endsWith(`:${step}`)) {
        if (
          entry.page.human &&
          entry.page.human.phase !== "reading" &&
          entry.page.human.phase !== "released"
        )
          openTakeover(id);
        else openPane({ kind: "url", url: entry.page.url }, entry.part, entry.page.account);
        return;
      }
  }
  /** A lifted tab card's one action: the tab opens in the pane, over its card. */
  function openLiftedTab() {
    const element = liftedElement;
    const reference = element?.reference;
    lifted = null;
    if (reference?.kind === "browser" && tabs.some((tab) => tab.id === reference.tab))
      openPane({ kind: "tab", id: reference.tab }, element!.id);
  }
  /** A video plays in the pane, opened over its card and at least the pane's minimum. */
  function playHere(id: string) {
    const reference = snapshot?.elements.find((element) => element.id === id)?.reference;
    if (reference?.kind !== "link" && reference?.kind !== "browser") return;
    const card = canvasRef?.screenRect(id);
    if (card && !pane) openOver(card);
    openPane(
      reference.kind === "link"
        ? { kind: "url", url: reference.url }
        : { kind: "tab", id: reference.tab },
      id,
    );
  }
  function lift(id: string) {
    panel = null;
    inspected = null;
    liftRecord = null;
    lifted = { id, origin: canvasRef?.screenRect(id) ?? null };
    if (snapshot?.elements.find((element) => element.id === id)?.reference.kind === "subject")
      void pictureQueue.gallery(id, picturesOf(id));
  }
  // The floating browser pane: Rust owns the native hole, this owns the frame.
  let pane = $state.raw<{
    target: WorkPaneTarget;
    origin: DOMRect | null;
    phase: "opening" | "shown" | "failed";
    restore: HTMLElement | null;
    /** The page was read with the person's session on this host. */
    account?: string;
  } | null>(null);
  let paneRequest: Promise<void> | null = null;
  let paneRect: WorkPaneRect | null = null;
  let paneSent: WorkPaneTarget | null = null;
  let paneDeadline: ReturnType<typeof setTimeout> | undefined;
  let paneSeen = false;
  const paneLayout = $derived(layout.workPane());
  const paneTab = $derived.by(() => {
    const current = pane;
    const id = paneLayout?.tab ?? (current?.target.kind === "tab" ? current.target.id : null);
    return id ? tabs.find((tab) => tab.id === id) : undefined;
  });
  const paneAdded = $derived(
    !!paneTab &&
      !!session.snapshot?.elements.some(
        (element) => element.reference.kind === "browser" && element.reference.tab === paneTab.id,
      ),
  );
  onMount(() => void layout.init());
  $effect(() => {
    const applied = paneLayout;
    const current = pane;
    if (applied) {
      paneSeen = true;
      clearTimeout(paneDeadline);
      if (current && current.phase === "opening") pane = { ...current, phase: "shown" };
      return;
    }
    if (paneSeen && current?.phase === "shown") {
      // Rust cleared the pane (return, tab close, scope change): drop the frame with it.
      paneSeen = false;
      paneSent = null;
      pane = null;
      current.restore?.focus({ preventScroll: true });
    }
  });
  // A tab chosen in the rail opens here, over the canvas.
  let tabAnswered = tabRequest()?.sequence ?? 0;
  $effect(() => {
    const request = tabRequest();
    if (!request || request.sequence === tabAnswered) return;
    tabAnswered = request.sequence;
    untrack(() => {
      if (tabs.some((tab) => tab.id === request.tab))
        openPane({ kind: "tab", id: request.tab }, null);
    });
  });
  function openPane(target: WorkPaneTarget, originId: string | null, account?: string) {
    if (cardHost) cardBounds = cardHost.getBoundingClientRect();
    panel = null;
    lifted = null;
    endTakeover(true);
    const restore =
      pane?.restore ??
      (document.activeElement instanceof HTMLElement ? document.activeElement : null);
    const origin = originId ? (canvasRef?.screenRect(originId) ?? null) : null;
    const rect = paneRect;
    pane = {
      target,
      origin: pane ? pane.origin : origin,
      phase: "opening",
      restore,
      ...(account ? { account } : {}),
    };
    if (rect && paneLayout) void requestPane(target, rect);
  }
  async function requestPane(target: WorkPaneTarget, rect: WorkPaneRect) {
    if (paneRequest || paneSent === target) return;
    paneSent = target;
    const request = (async () => {
      const okay = await workPane.show(target, rect).then(
        (result) =>
          result.outcome === "applied" ||
          result.outcome === "deferred" ||
          result.outcome === "no_op",
        () => false,
      );
      const current = pane;
      if (!current || current.target !== target) return;
      if (!okay) failPane(current);
      else {
        // Admission is not presentation: the layout projection confirms the hole.
        clearTimeout(paneDeadline);
        paneDeadline = setTimeout(() => {
          if (pane?.target === target && pane.phase === "opening") failPane(pane);
        }, 8000);
      }
    })();
    paneRequest = request;
    try {
      await request;
    } finally {
      if (paneRequest === request) paneRequest = null;
    }
  }
  function failPane(current: NonNullable<typeof pane>) {
    pane = { ...current, phase: "failed" };
    setTimeout(() => {
      if (pane?.phase === "failed") closePane();
    }, 1600);
  }
  function paneMeasured(rect: WorkPaneRect) {
    paneRect = rect;
    const current = pane;
    if (!current) return;
    if (current.phase === "opening") void requestPane(current.target, rect);
    else if (paneLayout) workPane.setRect(rect, paneLayout.generation);
  }
  function closePane() {
    const current = pane;
    pane = null;
    paneSeen = false;
    paneSent = null;
    clearTimeout(paneDeadline);
    if (paneLayout) void workPane.hide();
    current?.restore?.focus({ preventScroll: true });
  }
  function openCitation(url: string) {
    openPane({ kind: "url", url }, null);
  }
  /** Sites the person allowed from a part's row: the next run's entry question for each is answered once. */
  const preallowed = new SvelteSet<string>();
  /** A part's need met from its row: a page to sign in on, a site or folder allowed, the part run again. */
  function meetNeed(id: string, how: string) {
    const part = parts.find((item) => item.id === id)?.part;
    const need = part?.need;
    const current = objectiveSession;
    if (!part || !need || !current || current.projection?.work.id !== part.objective) return;
    // After a check nobody solved in time, the next try names the site the person will solve it on.
    const checked =
      need.kind === "check" ||
      need.kind === "sign_in" ||
      (need.kind === "retry" && need.reason === "blocked_by_check" && !!need.target);
    const again = (
      text = checked
        ? m.work_need_again_site_request({ part: part.title, site: need.target })
        : m.work_need_again_request({ part: part.title }),
    ) => {
      // A closed wall requires a fresh execution; steering the blocked one
      // cannot recreate the retained page or grant a retry.
      if (
        activeExecution &&
        (need.kind === "sign_in" || need.kind === "check" || need.kind === "choose_folder")
      )
        void current.enqueue(text);
      else if (activeExecution)
        void current.steer(text).then((taken) => taken || current.enqueue(text));
      else void current.continueWith(text);
    };
    if (how === "again") return again();
    switch (need.kind) {
      case "sign_in":
      case "check": {
        const site = need.address ? registrableSite(host(need.address)) : null;
        const waiting = part.pages?.find(
          (page) =>
            (!site || registrableSite(host(page.url)) === site) &&
            page.human &&
            page.human.phase !== "reading" &&
            page.human.phase !== "released" &&
            page.human.reason === (need.kind === "sign_in" ? "sign_in" : "challenge"),
        );
        if (waiting) openTakeover(waiting.id);
        else again();
        return;
      }
      case "allow_site":
        if (need.address) preallowed.add(registrableSite(new URL(need.address).hostname));
        return again();
      case "choose_folder": {
        const projection = current.projection;
        const waiting = projection?.executions.flatMap((execution) =>
          (execution.steps ?? [])
            .filter(
              (step) =>
                step.status === "running" &&
                step.kind.kind === "ask" &&
                step.kind.purpose === "folder" &&
                !!step.local &&
                !step.local.folder,
            )
            .map((step) => ({ execution: execution.id, step: step.id })),
        )[0];
        void commands
          .workChooseFolder(session.profile, null)
          .then(async (picked) => {
            if (objectiveSession !== current || current.projection?.work.id !== part.objective)
              return;
            if (picked.status !== "ok" || picked.data?.kind === "refused") return;
            const choice =
              picked.status === "ok" && picked.data?.kind === "admitted" ? picked.data.path : null;
            if (waiting) {
              await current.answerStep(waiting.execution, waiting.step, choice ?? ASK_WORDS.notNow);
              return;
            }
            if (!choice || !(await placing.addFolder(choice))) return;
            if (objectiveSession !== current || current.projection?.work.id !== part.objective)
              return;
            current.folders = [...new Set([...current.folders, choice])];
            again();
          })
          .catch(() => undefined);
        return;
      }
      case "allow_folder":
        if (need.address) void placing.addFolder(need.address).then((added) => added && again());
        return;
      case "use_connection":
        return again(m.work_need_use_request({ service: need.target, part: part.title }));
      case "connect":
        void commands.runCommand(`settings.connections.${encodeURIComponent(need.target)}`);
        return;
      case "retry":
        return again();
    }
  }
  /** Folder questions seen open here: once allowed, the folder stands on the canvas. */
  const folderAsks: Record<string, true> = {};
  $effect(() => {
    const execution = objectiveSession?.projection?.executions.at(-1);
    for (const step of execution?.steps ?? []) {
      const kind = step.kind;
      if (kind.kind !== "ask" || kind.purpose !== "folder" || !step.local) continue;
      if (!kind.answer) {
        if (step.status === "running") folderAsks[step.id] = true;
        continue;
      }
      if (!folderAsks[step.id]) continue;
      delete folderAsks[step.id];
      if (step.status !== "succeeded") continue;
      const folder =
        folderChoice(kind.answer) ??
        (kind.answer === kind.options[0] && kind.options[0] !== ASK_WORDS.chooseFolder
          ? step.local.folder
          : null);
      if (!folder) continue;
      const placed = snapshot?.elements.some(
        (element) => element.reference.kind === "folder" && element.reference.path === folder,
      );
      if (!placed) untrack(() => void placing.addFolder(folder));
    }
  });
  $effect(() => {
    const current = objectiveSession;
    const execution = current?.projection?.executions.at(-1);
    if (!current || !execution || !preallowed.size) return;
    for (const ask of openAsks(asksOf(execution))) {
      if (ask.kind !== "entry" || !ask.host) continue;
      const site = registrableSite(ask.host);
      if (!preallowed.has(site)) continue;
      untrack(() => {
        preallowed.delete(site);
        void current.answerStep(execution.id, ask.step, ASK_WORDS.allow);
      });
    }
  });
  // The takeover: Rust presents the agent's own view inside the well this pane
  // reserves. There is no resize command, so a moved well is released and
  // presented again.
  let takeover = $state.raw<{
    work: string;
    card: string;
    page: WorkHumanPageIdV1;
    host: string;
    url: string;
    restore: HTMLElement | null;
  } | null>(null);
  let takeoverError = $state<string | null>(null);
  let takeoverSent: WorkHumanRegionV1 | null = null;
  let takeoverSeen = false;
  let takeoverQueue: Promise<unknown> = Promise.resolve();
  const takeoverPage = $derived.by(() => {
    const current = takeover;
    if (!current) return null;
    const found = (human.pages.get(current.work) ?? []).find(
      (candidate) =>
        candidate.id.attempt === current.page.attempt &&
        candidate.id.step === current.page.step &&
        candidate.id.generation === current.page.generation,
    );
    return found ? humanPage(found) : null;
  });
  const takeoverView = $derived.by(() => {
    const current = takeover;
    const page = takeoverPage;
    const bounds = cardBounds;
    return current && page && bounds ? { current, page, bounds } : null;
  });
  /** One command at a time: a release and its re-presentation never interleave. */
  function applyTakeover(job: () => Promise<WorkHumanFailure | null>, report: boolean) {
    const next = takeoverQueue.then(async () => {
      const failure = await job();
      if (report) takeoverError = failure ? failureLine(failure) : null;
    });
    takeoverQueue = next.catch(() => undefined);
  }
  /** The work a held page belongs to, by its id. */
  function heldWork(state: HumanPage) {
    return [...human.pages].find(([, pages]) =>
      pages.some(
        (candidate) =>
          candidate.id.attempt === state.attempt &&
          candidate.id.step === state.step &&
          candidate.id.generation === state.generation,
      ),
    )?.[0];
  }
  /** A page the run is waiting on keeps waiting a while longer, at the person's word. */
  function extendWait(page: string) {
    const state =
      pageEntries.get(page)?.page.human ??
      items.find((candidate) => candidate.id === page)?.page?.human;
    if (!state || state.phase !== "waiting_for_human" || !state.canExtend) return;
    const work = heldWork(state);
    if (work)
      void human.extend(work, {
        attempt: state.attempt,
        step: state.step,
        generation: state.generation,
      });
  }
  function openTakeover(page: string) {
    const entry = pageEntries.get(page);
    const item = entry ? undefined : items.find((candidate) => candidate.id === page);
    const state = entry?.page.human ?? item?.page?.human;
    // The well stands over the part the page is in.
    const card = entry?.part ?? page;
    const url = entry?.page.url ?? item?.page?.url ?? "";
    if (
      state &&
      takeover?.page.attempt === state.attempt &&
      takeover.page.step === state.step &&
      takeover.page.generation === state.generation
    )
      return;
    if (!state || state.phase !== "waiting_for_human") return;
    const work = heldWork(state);
    if (!work) return;
    panel = null;
    lifted = null;
    inspected = null;
    closePane();
    const restore =
      takeover?.restore ??
      (document.activeElement instanceof HTMLElement ? document.activeElement : null);
    takeoverSeen = false;
    takeoverSent = null;
    takeoverError = null;
    takeover = {
      work,
      card,
      page: { attempt: state.attempt, step: state.step, generation: state.generation },
      host: host(url),
      url,
      restore,
    };
    canvasRef?.center(card);
  }
  function endTakeover(release: boolean) {
    const current = takeover;
    takeover = null;
    takeoverError = null;
    takeoverSeen = false;
    takeoverSent = null;
    if (current && release) applyTakeover(() => human.release(current.work, current.page), false);
    current?.restore?.focus({ preventScroll: true });
  }
  // Once the page is the agent's again the pane has nothing left to hold.
  $effect(() => {
    if (!takeover) return;
    const page = takeoverPage;
    if (page && page.phase !== "reading" && page.phase !== "released") {
      takeoverSeen = true;
      return;
    }
    // A settled phase means the agent already has it; a page that vanished
    // from the projection is unknown, so its view is released to be sure.
    if (page) untrack(() => endTakeover(false));
    else if (takeoverSeen) untrack(() => endTakeover(true));
  });
  function takeoverRegion(box: PaneRect | null) {
    const current = takeover;
    if (!current) return;
    const next = box
      ? regionOf(box, { width: window.innerWidth, height: window.innerHeight })
      : null;
    // A well Rust would refuse says so rather than waiting on a view forever.
    if (box && !next) takeoverError = failureLine("invalid");
    // The page is presented once. Releasing it ends the hand-over, so a well
    // that moves later is left alone: Rust keeps the page fitted to the window.
    if (!next || takeoverSent) return;
    takeoverSent = next;
    applyTakeover(async () => {
      const failure = await human.present(current.work, current.page, next);
      if (failure && takeover === current) takeoverSent = null;
      return failure;
    }, true);
  }
  /** The person hands the page back; the run goes on without their session. */
  function continueTakeover() {
    const current = takeover;
    if (!current) return;
    takeoverSent = null;
    applyTakeover(() => human.continue(current.work, current.page, "anonymous"), true);
  }
  /** A window that goes away hands the page back; the command is already sent. */
  function abandonTakeover() {
    const current = takeover;
    if (current) void human.release(current.work, current.page);
  }
  onMount(() => abandonTakeover);
  function liftSize(item: CanvasItem | undefined) {
    if (!item) return { width: 720, height: 520 };
    if (liftedObject && item.object) return centreSize(liftedObject);
    // A product compare wants every column at once, not a scrollbar.
    if (item.artifact?.content.kind === "matrix")
      return {
        width: Math.min(1180, 320 + item.artifact.content.subjects.length * 200),
        height: 640,
      };
    if (item.artifact?.content.kind === "table" || item.artifact?.content.kind === "comparison")
      return { width: 960, height: 640 };
    switch (item.type) {
      case "note":
        return { width: 760, height: 620 };
      case "tab":
        return { width: 520, height: 260 };
      case "objective":
      case "request":
        return { width: 640, height: 600 };
      case "sources":
      case "part":
        return { width: 560, height: 560 };
      case "subject":
        // The picture and its facts at 640, the sources at 320.
        return { width: 1040, height: 720 };
      default: {
        const base = defaultSize(item);
        return { width: Math.max(720, base.width + 200), height: Math.max(520, base.height + 160) };
      }
    }
  }
  /** The bar's one open panel: a tool's, or the field's own attach. */
  let panel = $state<"tabs" | "media" | "attach" | "tasks" | "blocks" | null>(null);
  /** The blocks panel opens on the kinds of block, or on the person's notes (N). */
  let blocksView = $state<"blocks" | "notes">("blocks");
  setContext(canvasOpenTasks, () => openPanel("tasks", true));
  /** Work keeps its rail compact: a tool asked for from the app's menus opens here instead. */
  function toolAsked(event: Event) {
    if ((event as CustomEvent<string>).detail === "tasks") openPanel("tasks", true);
  }
  $effect(() => {
    window.addEventListener("zephium:work-tool", toolAsked);
    return () => window.removeEventListener("zephium:work-tool", toolAsked);
  });
  let attachKind = $state<AttachKind>("tabs");
  let objectivePending = $state(false);
  let composerElement = $state<HTMLElement>();
  let composerSurface = $state<HTMLElement>();
  let composerHeight = $state(0);
  // Over a window material the composer paints nothing, so the canvas has a hole the shape of it.
  // The hole is read from the sheet itself on every frame it changes, so the two never part.
  $effect(() => {
    const card = cardHost;
    const sheet = composerSurface;
    if (!card || !sheet) return;
    const root = document.documentElement;
    // The hole follows the bar on every frame of its width change. On a card that paints with the
    // canvas, each new clip repaints the whole card, which at a large window is the animation's
    // whole cost. A layer of its own for the moment the hole moves turns the clip into a mask the
    // compositor reapplies, and it is let go again once the hole has held still.
    let settle = 0;
    let shape = "";
    const cut = () => {
      const area = card.getBoundingClientRect();
      const bar = sheet.getBoundingClientRect();
      if (!bar.width || !area.width) return;
      const style = getComputedStyle(sheet);
      const radius = parseFloat(style.borderTopLeftRadius) || 0;
      const flare = parseFloat(style.getPropertyValue("--flare")) || 0;
      // The fillets land on the card's own foot, whole and tangent to it, as the new tab's
      // notch does at its page's edge; below it the window's material simply continues.
      const reach = { width: area.width, height: Math.min(area.height, bar.bottom - area.top) };
      const place = { left: bar.left - area.left, top: bar.top - area.top, width: bar.width };
      const edge = sheet.querySelector<SVGSVGElement>(".edge");
      if (edge) {
        edge.setAttribute("viewBox", `${place.left} ${place.top} ${bar.width} ${bar.height}`);
        const d = composerEdge(reach, place, radius, flare);
        for (const path of edge.querySelectorAll("path")) path.setAttribute("d", d);
      }
      const material = root.dataset.material;
      const clip =
        !material || material === "none" ? "" : composerHole(reach, place, radius, flare);
      if (clip === shape) return;
      shape = clip;
      if (clip) {
        card.style.willChange = "transform";
        clearTimeout(settle);
        settle = window.setTimeout(() => (card.style.willChange = ""), 180);
      }
      card.style.clipPath = clip;
    };
    const observer = new ResizeObserver(cut);
    observer.observe(card);
    observer.observe(sheet);
    observer.observe(sheet.parentElement ?? sheet);
    const materials = new MutationObserver(cut);
    materials.observe(root, { attributes: true, attributeFilter: ["data-material"] });
    window.addEventListener("resize", cut);
    cut();
    return () => {
      observer.disconnect();
      materials.disconnect();
      window.removeEventListener("resize", cut);
      clearTimeout(settle);
      card.style.clipPath = "";
      card.style.willChange = "";
    };
  });
  $effect(() => {
    const element = composerElement;
    if (!element) {
      composerHeight = 0;
      return;
    }
    const observer = new ResizeObserver(() => {
      composerHeight = element.getBoundingClientRect().height;
    });
    observer.observe(element);
    return () => observer.disconnect();
  });
  function placingCentre() {
    const bounds = cardBounds;
    return bounds
      ? (canvasRef?.flowPosition(bounds.left + bounds.width / 2, bounds.top + bounds.height / 2) ??
          null)
      : null;
  }
  const placing = new CanvasPlacing({
    session: () => session,
    busy: () => busy,
    spot: (at, size, id) => {
      if (!at) return clearSpot(id, size);
      const corner = dropPlace(null, {
        x: at.x - size.width / 2,
        y: at.y - size.height / 2,
        ...size,
      });
      return corner ? { x: corner.x + size.width / 2, y: corner.y + size.height / 2 } : at;
    },
    at: (x, y) => canvasRef?.flowPosition(x, y) ?? null,
    file: (path) => {
      const draft = session.composer.trimEnd();
      if (draft.split(/\s+/u).includes(path)) return;
      session.composer = draft ? `${draft} ${path}` : path;
    },
  });
  onMount(() => placing.listen());
  function host(url: string): string {
    try {
      return new URL(url).host.replace(/^www\./u, "");
    } catch {
      return "";
    }
  }
  const snapshot = $derived(session.snapshot);
  provideLeadLook(() => session.snapshot?.id);
  // A resource that is neither a note nor a file is one of the person's objects: read it.
  $effect(() => {
    const current = snapshot;
    if (!current) return;
    const notesHeld = new Set(context.notes.map((note) => note.id));
    void owned.load(
      current.elements.flatMap((element) =>
        element.reference.kind === "resource" &&
        !notesHeld.has(element.reference.resource) &&
        !context.media.has(element.reference.resource)
          ? [element.reference.resource]
          : [],
      ),
    );
  });
  const baseItems = $derived(
    snapshot
      ? environmentItems(snapshot, tabs, context.notes, context.objectives, context.media, owned)
      : [],
  );
  let planGeometry = $state.raw<CanvasView>({
    positions: {},
    sizes: {},
    viewport: { x: 0, y: 0, zoom: 1 },
  });
  const results = $derived(
    snapshot
      ? environmentResults(snapshot, baseItems, objectiveSession?.projection ?? null)
      : { items: baseItems, references: new Map<string, ResultReference>(), remaining: null },
  );
  /** What the run this canvas is watching is doing right now. */
  const signalOf = (objective: string) =>
    objectiveSession?.selected === objective
      ? objectiveSession.activity.at(-1)?.activity
      : undefined;
  const pictures = $derived(
    snapshot ? environmentPictures(snapshot, context.objectives, context.media) : new Map(),
  );
  /** The pages each work recorded: the live session has the newest frames for
   * the work it is on; every other work keeps the frames the context read. */
  const recordedPages = (objective: string) =>
    objectiveSession?.selected === objective && objectiveSession.pages.length
      ? objectiveSession.pages
      : (context.pages.get(objective) ?? []);
  /** The block opened in place, and every block's height as it measured itself. */
  let openBlock = $state<string | null>(null);
  const measured = new SvelteMap<string, number>();
  const takeMeasure = measurer(measured);
  /** The bar's height, context included, so the canvas fades under all of it. */
  let dockHeight = $state(64);
  /** Blocks the person dragged: only those can leave their board's flow. */
  const moved = new SvelteSet<string>();
  function toggleBlock(id: string) {
    inspected = null;
    openBlock = openBlock === id ? null : id;
  }
  // Escape closes the block that is open, when nothing sits over the canvas.
  $effect(() => {
    if (!openBlock) return;
    const close = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || event.defaultPrevented || lifted || pane || takeover) return;
      openBlock = null;
    };
    window.addEventListener("keydown", close);
    return () => window.removeEventListener("keydown", close);
  });
  const chosen = $derived(
    new Set(
      (snapshot?.decisions ?? []).flatMap((decision) =>
        decision.choice ? [decision.element] : [],
      ),
    ),
  );
  /** Requests the person opened to read whole. */
  const openRequests = new SvelteSet<string>();
  /**
   * The person's own arrangement as it stands now: the unsaved one while a
   * save is pending, so a save that lands late never puts a card back.
   */
  const ownView = $derived(
    snapshot
      ? environmentView(
          session.viewDraft?.id === snapshot.id
            ? { ...snapshot, view: session.viewDraft.view }
            : snapshot,
        )
      : undefined,
  );
  /** What the person placed and the canvas draws, as the runs must leave it: their cards and areas. */
  let ownRectsKey = "";
  let ownRectsLast: Record<string, CanvasPosition & { width: number; height: number }> = {};
  const ownRects = $derived.by(() => {
    const next = ownView
      ? {
          ...Object.fromEntries(
            results.items.flatMap((item) => {
              const at = ownView.positions[item.id];
              if (!at || item.type === "objective" || item.type === "request") return [];
              return [[item.id, { ...at, ...(ownView.sizes?.[item.id] ?? defaultSize(item)) }]];
            }),
          ),
          ...Object.fromEntries(
            Object.entries(ownView.areas ?? {}).map(([id, rect]) => [`area:${id}`, rect]),
          ),
        }
      : {};
    // A live run re-derives its items many times a second; the runs re-lay only when these move.
    const key = JSON.stringify(next);
    if (key !== ownRectsKey) {
      ownRectsKey = key;
      ownRectsLast = next;
    }
    return ownRectsLast;
  });
  /** Every request of the canvas as a band: its words, its branches, its result. */
  const stages = $derived(
    snapshot
      ? environmentStages(snapshot, context.objectives, {
          around: ownRects,
          recorded: recordedPages,
          pictures: elementPictures(snapshot, context.media),
          fetched: fetchedPictures(snapshot, context.media),
          chosen,
          measured,
          open: openBlock,
          asks: (objective: string) =>
            objectiveSession?.projection?.work.id === objective
              ? partAsks(objectiveSession, { session: human, work: objective }, openStep)
              : [],
          requests: openRequests,
          exchange: exchangeOf,
          held: (objective: string) =>
            (human.pages.get(objective) ?? []).some((page) => page.phase === "waiting_for_human"),
        })
      : [],
  );
  const clock = new Intl.DateTimeFormat(undefined, { hour: "numeric", minute: "2-digit" });
  /** When each band's request was asked: its first run carries the time in its id. */
  const askedAt = $derived(
    new Map(
      stages.flatMap((stage) => {
        const first = stage.executions[0];
        const at = first ? ulidTime(first) : null;
        return at ? [[stage.card, clock.format(at)] as const] : [];
      }),
    ),
  );
  /** Where the camera rested last, so the island speaks for the run in view. */
  let camera = $state<CanvasView["viewport"] | undefined>(undefined);
  /** The run that takes most of the view; the island speaks for it unless another is live. */
  const viewedExecution = $derived.by(() => {
    const view = camera;
    const width = cardHost?.clientWidth ?? 0;
    const height = cardHost?.clientHeight ?? 0;
    if (!view || !width || !height) return undefined;
    const seen = {
      x: -view.x / view.zoom,
      y: -view.y / view.zoom,
      width: width / view.zoom,
      height: height / view.zoom,
    };
    let best: { area: number; execution: string } | undefined;
    for (const stage of stages) {
      const box = stage.lane.box;
      const across = Math.min(box.x + box.width, seen.x + seen.width) - Math.max(box.x, seen.x);
      const down = Math.min(box.y + box.height, seen.y + seen.height) - Math.max(box.y, seen.y);
      const execution = stage.executions.at(-1);
      if (across <= 0 || down <= 0 || !execution) continue;
      if (!best || across * down > best.area) best = { area: across * down, execution };
    }
    return best?.execution;
  });
  /** What the runs in view placed: their pictures are admitted first. */
  const nearElements = $derived.by((): ReadonlySet<string> => {
    const view = camera;
    const width = cardHost?.clientWidth ?? 0;
    const height = cardHost?.clientHeight ?? 0;
    if (!view || !width || !height) return new Set();
    const left = -view.x / view.zoom;
    const top = -view.y / view.zoom;
    const right = left + width / view.zoom;
    const bottom = top + height / view.zoom;
    return new Set(
      stages.flatMap((stage) => {
        const box = stage.lane.box;
        const off =
          box.x > right || box.x + box.width < left || box.y > bottom || box.y + box.height < top;
        return off ? [] : stage.objects.map((object) => object.id);
      }),
    );
  });
  /** From a run's result to its right end: what must read whole when the run is shown. */
  const resultSpan = (stage: (typeof stages)[number]) =>
    stage.lane.box.x + stage.lane.box.width - stage.lane.corner.x;
  /** A work opened with no camera of its own opens on its newest run. */
  const home = $derived.by(() => {
    const box = stages.at(-1)?.lane.box;
    return box ? { x: box.x, y: box.y, width: box.width } : undefined;
  });
  // A run that ends while the person follows it shows them its result's top if it is out of view.
  let wasLive = new Set<string>();
  $effect(() => {
    const now = new Set(stages.filter((stage) => stage.live).map((stage) => stage.card));
    untrack(() => {
      for (const card of wasLive) {
        if (now.has(card)) continue;
        const stage = stages.find((entry) => entry.card === card);
        if (stage && canvasRef?.followAgent())
          canvasRef.settle(stage.reply?.id ?? stage.column.head ?? card, resultSpan(stage));
      }
      wasLive = now;
    });
  });
  // A work names itself once its first run ends, unless the person already has; an
  // older unnamed work takes its name as it is opened.
  const named: Record<string, true> = {};
  $effect(() => {
    const current = snapshot;
    const first = stages[0];
    const title = first
      ? firstRunName(context.objectives.get(first.objective), first.request, first.live)
      : "";
    if (!current || !title || !defaultTitle(current.title, m.work_env_default_title())) return;
    if (current.lifecycle !== "active" || named[current.id]) return;
    named[current.id] = true;
    untrack(() => void session.edit({ kind: "rename", title: title.slice(0, 64) }));
  });
  const agents = $derived(
    snapshot
      ? environmentAgents(snapshot, context.objectives, signalOf, stages)
      : { items: [], links: [], positions: {} },
  );
  const parts = $derived(
    environmentParts(
      context.objectives,
      stages,
      recordedPages,
      signalOf,
      (objective) => human.pages.get(objective) ?? [],
    ),
  );
  /** Every page a run shows, by id, with the part it stands in. */
  const pageEntries = $derived(
    new Map(
      parts.flatMap((item) =>
        (item.part?.pages ?? []).map((page) => [page.id, { part: item.id, page }] as const),
      ),
    ),
  );
  const boards = $derived(environmentBoards(stages, blockArtifact));
  /** The page this canvas's run is held on, so the line can point at its card. */
  const agentWaiting = $derived.by(() => {
    const work = objectiveSession?.projection?.work.id;
    const opened = work ? (human.pages.get(work) ?? []) : [];
    if (!opened.length) return null;
    for (const { page } of pageEntries.values()) {
      const state = page.human;
      if (state?.phase !== "waiting_for_human") continue;
      if (
        !opened.some(
          (candidate) =>
            candidate.id.attempt === state.attempt &&
            candidate.id.step === state.step &&
            candidate.id.generation === state.generation,
        )
      )
        continue;
      return {
        card: page.id,
        host: host(page.url),
        reason: state.reason,
        remaining: state.remaining,
        canExtend: state.canExtend,
      };
    }
    return null;
  });
  const requests = $derived(environmentRequests(stages));
  /** The parts of the island's run, by the run's request. */
  const islandParts = $derived.by(() => {
    const latest = objectiveSession?.projection?.executions.at(-1)?.id;
    const stage = latest ? stages.find((entry) => entry.executions.includes(latest)) : undefined;
    return stage ? parts.filter((item) => item.id.startsWith(`part:${stage.card}:`)) : [];
  });
  /** The parts at work now, each saying what it does, for the island's list. */
  const workingParts = $derived(
    islandParts.flatMap((item) =>
      item.part?.state === "running"
        ? [
            {
              id: item.id,
              title: item.part.title,
              now: item.part.now ?? item.part.summary ?? "",
              helper: item.part.helper,
              ...(item.part.host ? { host: item.part.host } : {}),
            },
          ]
        : [],
    ),
  );
  /** Why the island's run ended short, and the one action that helps, from its first part in need. */
  const runEnded = $derived.by(() => {
    const item = islandParts.find((entry) => entry.part?.need && entry.part.state !== "running");
    const need = item?.part?.need;
    if (!item || !need) return null;
    const said = needWords(need, item.part!.title, "island");
    const how = need.kind === "retry" || need.past ? "again" : "meet";
    return { text: said.text, action: said.action, onact: () => meetNeed(item.id, how) };
  });
  const dressOf = $derived(requestDress(stages));
  /** What each run remembered, shown beside its request with Undo. */
  const remembering = $derived(
    new Map(
      stages.flatMap((stage) => {
        const last = stage.executions.at(-1);
        if (!last) return [];
        const run = context.objectives
          .get(stage.objective)
          ?.executions.find((entry) => entry.id === last);
        return [
          [
            stage.card,
            {
              profile: session.profile,
              work: stage.objective,
              execution: last,
              version: run?.steps?.length ?? 0,
            },
          ] as const,
        ];
      }),
    ),
  );
  const claimedFolders = $derived(runFolders(stages));
  const items = $derived([
    ...[
      ...results.items.filter((item) => !(item.type === "folder" && claimedFolders.has(item.id))),
      ...requests.items,
    ].map((item) =>
      item.type === "objective" || item.type === "request"
        ? {
            ...item,
            expanded: openRequests.has(item.id),
            ...(askedAt.get(item.id) ? { when: askedAt.get(item.id) } : {}),
            ...(remembering.get(item.id) ? { remember: remembering.get(item.id) } : {}),
            ...dressOf.get(item.id),
          }
        : item,
    ),
    ...boards,
    ...parts,
    ...environmentInputs(stages),
    ...environmentSources(stages),
    ...agents.items,
  ]);
  const links = $derived([...(snapshot ? environmentLinks(snapshot) : []), ...requests.links]);
  const organizing = new SvelteSet<string>();
  /** A run whose placing was refused, by the revision it was refused at: tried again only after a change. */
  const stalled: Record<string, string> = {};
  // Planned placements render new elements where organize intends them before
  // their saved placement lands, so a canvas checkpoint never persists the
  // fallback grid over them.
  let planned = $state.raw<Record<string, { x: number; y: number; width: number; height: number }>>(
    {},
  );
  const plannedGeometry = $derived.by(() => {
    const positions: Record<string, { x: number; y: number }> = {};
    const sizes: Record<string, { width: number; height: number }> = {};
    if (!snapshot) return { positions, sizes };
    const saved = new Set(snapshot.view.placements.map((place) => place.element));
    for (const element of snapshot.elements) {
      if (saved.has(element.id) || laneElement(snapshot, element)) continue;
      const place = planned[JSON.stringify(element.reference)];
      if (!place) continue;
      positions[element.id] = { x: place.x, y: place.y };
      sizes[element.id] = { width: place.width, height: place.height };
    }
    return { positions, sizes };
  });
  $effect(() => {
    const current = snapshot;
    const objectives = context.objectives;
    if (!current || session.pending || session.loading) return;
    for (const element of current.elements) {
      if (element.reference.kind !== "objective") continue;
      const projection = objectives.get(element.reference.objective);
      if (!projection) continue;
      const execution = pendingOrganize(current, projection);
      if (!execution || organizing.has(execution.id)) continue;
      if (stalled[execution.id] === current.revision) continue;
      organizing.add(execution.id);
      const stage = stages.find((stage) => stage.executions.includes(execution.id));
      const place =
        stage?.place ?? current.view.placements.find((place) => place.element === element.id);
      // What a run places lands on its request's board; the anchor is only a record.
      const anchor = place ? { x: place.x, y: place.y } : { x: 80, y: 120 };
      void untrack(() =>
        organize(projection, execution, anchor)
          .then((through) => {
            if (through) delete stalled[execution.id];
            else stalled[execution.id] = session.snapshot?.revision ?? current.revision;
          })
          .finally(() => organizing.delete(execution.id)),
      );
    }
  });
  async function organize(
    projection: WorkRuntimeProjection,
    execution: WorkExecutionFact,
    anchor: { x: number; y: number },
  ): Promise<boolean> {
    if (!session.snapshot) return true;
    const plan = organizeExecution(projection, execution, anchor, session.snapshot);
    if (!plan.adds.length) return true;
    if (!(await session.flushView())) return false;
    planned = {
      ...planned,
      ...Object.fromEntries(
        plan.adds.map((add) => [JSON.stringify(add.reference), add.placement] as const),
      ),
    };
    for (const add of plan.adds) {
      if (session.snapshot && elementFor(session.snapshot, add.reference)) continue;
      if (!(await session.edit({ kind: "add", reference: add.reference, area: null })))
        return false;
    }
    const latest = session.snapshot;
    if (!latest) return false;
    for (const relation of plan.relations) {
      const from = elementFor(latest, relation.from);
      const to = elementFor(latest, relation.to);
      if (!from || !to) continue;
      if (
        (latest.relations ?? []).some(
          (existing) =>
            existing.from === from.id && existing.to === to.id && existing.kind === relation.kind,
        )
      )
        continue;
      if (
        !(await session.edit({
          kind: "relate",
          from: from.id,
          to: to.id,
          relation: relation.kind,
        }))
      )
        return false;
    }
    const placed = session.snapshot;
    if (!placed) return false;
    const placements = plan.adds.flatMap((add) => {
      const element = elementFor(placed, add.reference);
      return element
        ? [
            {
              element: element.id,
              x: Math.round(add.placement.x),
              y: Math.round(add.placement.y),
              width: Math.round(add.placement.width),
              height: Math.round(add.placement.height),
            },
          ]
        : [];
    });
    const ids = new Set(placements.map((place) => place.element));
    session.checkpoint({
      ...placed.view,
      placements: [
        ...placed.view.placements.filter((place) => !ids.has(place.element)),
        ...placements,
      ],
    });
    void pictureQueue.run();
    return true;
  }
  const pictureQueue = new PictureQueue({
    session: () => session,
    media: () => context.media,
    objectives: () => context.objectives,
    near: () => nearElements,
  });
  $effect(() => {
    const current = snapshot;
    // A projection update can bring candidates for subjects already placed.
    const runs = [...context.objectives.values()];
    const links = current?.elements.some((element) => element.reference.kind === "link");
    if (!current || (!runs.length && !links) || session.pending || session.loading) return;
    untrack(() => void pictureQueue.run());
  });
  $effect(() => () => pictureQueue.dispose());
  const liftedItem = $derived(items.find((item) => item.id === lifted?.id));
  /** Every result with steps, as tasks would carry them. */
  const stepPlans = $derived(
    new Map(
      items.flatMap((item): [string, StepPlan][] => {
        const view = item.object?.view;
        // A checkable plan or a to-do list on the canvas makes its steps into tasks.
        if (
          view &&
          item.artifact &&
          (view.kind === "plan" ? view.checkable : view.kind === "list" && view.style === "todo")
        ) {
          const objective = stages.find((stage) =>
            stage.objects.some((object) => object.id === item.id),
          )?.objective;
          const steps =
            view.kind === "plan"
              ? view.steps.map((step) => ({
                  text: step.title,
                  ...(step.detail ? { detail: step.detail } : {}),
                }))
              : view.kind === "list"
                ? view.items.map((entry) => ({
                    text: entry.title,
                    ...(entry.detail ? { detail: entry.detail } : {}),
                  }))
                : [];
          return objective && steps.length
            ? [[item.id, stepPlan(item.id, objective, item.artifact, steps)]]
            : [];
        }
        const reference = results.references.get(item.id);
        const steps = item.artifact ? resultPlan(item.artifact.content) : [];
        return reference && item.artifact && steps.length
          ? [[item.id, stepPlan(item.id, reference.objective, item.artifact, steps)]]
          : [];
      }),
    ),
  );
  const workTasks = untrack(
    () => new WorkTasks(session.profile, stepTaskSession, () => stepPlans, openTask),
  );
  setContext(workTasksKey, workTasks);
  // The lift asks for a site's icon as the canvas does: once per origin.
  const probedOrigins: Record<string, true> = {};
  setContext(canvasProbe, (origin: string) => {
    if (probedOrigins[origin]) return;
    probedOrigins[origin] = true;
    void commands.faviconProbe(session.profile, [origin]).catch(() => false);
  });
  /** A step's task opens in the sidebar's Tasks, listed whatever its day. */
  function openTask(id: string) {
    panel = null;
    lifted = null;
    editTool(toolSession(session.profile, "tasks"), { filter: "all" });
    const tasks = taskSession(session.profile, "sidebar");
    tasks.selectedId = id;
    toolHost.open("tasks");
    void tasks.load(id);
  }
  const workNotes = new WorkNotes();
  /** A document result written as the person's note, only when they ask. */
  async function saveNote(id: string) {
    const reference = results.references.get(id);
    const view = items.find((item) => item.id === id)?.artifact;
    const markdown = view ? documentMarkdown(view) : null;
    if (!reference || !markdown) return;
    await workNotes.save(resultKey(reference), markdown, notes);
  }
  /** A kept note opens in the sidebar's Notes, beside the canvas. */
  function openNote(id: string) {
    panel = null;
    lifted = null;
    toolHost.open("notes");
    void noteSession(session.profile, "sidebar")?.requestOpen(id);
  }
  /** Save as note until the note exists, then Open note. */
  function noteAction(id: string): LiftAction | undefined {
    const reference = results.references.get(id);
    // A document or an answer: whatever has Markdown to keep.
    const view = items.find((item) => item.id === id)?.artifact;
    if (!reference || !view || !documentMarkdown(view)) return undefined;
    const key = resultKey(reference);
    const note = workNotes.note(key);
    if (note) return { label: m.work_open_note(), onclick: () => openNote(note) };
    const saving = workNotes.saving(key);
    return {
      label: saving ? m.work_saving_note() : m.work_save_note(),
      disabled: saving,
      onclick: () => void saveNote(id),
    };
  }
  /** A subject written up as a note, once; then its note opens. */
  function subjectNote(
    subject: Extract<WorkEnvironmentReference, { kind: "subject" }>,
    markdown: string,
  ): LiftAction {
    const key = `subject:${subject.objective}:${subject.execution}:${subject.artifact}:${subject.index}`;
    const note = workNotes.note(key);
    if (note) return { label: m.work_open_note(), onclick: () => openNote(note) };
    const saving = workNotes.saving(key);
    return {
      label: saving ? m.work_saving_note() : m.work_save_note(),
      disabled: saving,
      onclick: () => void workNotes.save(key, markdown, notes),
    };
  }
  let liftRef = $state<Lift>();
  /** Ask about this: the composer takes the subject's name, and the lift steps back. */
  function askAbout(name: string) {
    session.composer = m.work_lift_ask_prefix({ name });
    liftRef?.close();
    requestAnimationFrame(() => {
      const field = composerElement?.querySelector<HTMLTextAreaElement>("textarea");
      field?.focus();
      field?.setSelectionRange(field.value.length, field.value.length);
    });
  }
  /** The document the agent line's run just wrote, if it wrote one. */
  const lineDocument = $derived.by(() => {
    const projection = objectiveSession?.projection;
    const execution = projection?.executions.at(-1);
    if (!projection || !execution) return null;
    for (const [id, reference] of results.references)
      if (
        reference.objective === projection.work.id &&
        reference.execution === execution.id &&
        items.find((item) => item.id === id)?.artifact?.content.kind === "document"
      )
        return { id, key: resultKey(reference) };
    return null;
  });
  const writeup = $derived(
    lineDocument && !workNotes.note(lineDocument.key) && !workNotes.saving(lineDocument.key)
      ? () => void saveNote(lineDocument.id)
      : undefined,
  );
  /** The lift's one action for a result: Make tasks for a plan, Save as note for a document. */
  function resultAction(id: string): LiftAction | undefined {
    const state = workTasks.state(id);
    // Once the steps are tasks, the result's action is its note, never a spent "made" label.
    if (state === "none" || state === "made") return noteAction(id);
    return {
      label: state === "making" ? m.work_making_tasks() : m.work_make_tasks(),
      title: m.work_make_tasks_hint(),
      disabled: state !== "ready",
      onclick: () => void workTasks.make(id),
    };
  }
  /** A trail's command opened whole, as the run recorded it. */
  let liftRecord = $state<string | null>(null);
  const liftedCommand = $derived(
    liftRecord ? commandRecord(context.objectives, liftRecord) : undefined,
  );
  /** Whether a block came from an agent's run rather than a reviewed plan. */
  function agentBlock(id: string) {
    const reference = results.references.get(id);
    const execution = reference
      ? context.objectives
          .get(reference.objective)
          ?.executions.find((entry) => entry.id === reference.execution)
      : undefined;
    return !!execution && isAgentExecution(execution);
  }
  /** What a block is when it is one published result: its note and its tasks come from it. */
  function blockArtifact(id: string) {
    const reference = snapshot?.elements.find((element) => element.id === id)?.reference;
    if (reference?.kind !== "artifact") return undefined;
    const execution = context.objectives
      .get(reference.objective)
      ?.executions.find((entry) => entry.id === reference.execution);
    const artifact = execution?.artifacts.find((entry) => entry.id === reference.artifact);
    return execution && artifact ? artifactView(artifact, execution) : undefined;
  }
  /** What the person made of each draft before it went: kept while the work is open. */
  const drafts = new SvelteMap<string, DraftEdit>();
  const editDraft = (id: string, edit: DraftEdit) => drafts.set(id, { ...drafts.get(id), ...edit });
  /** The draft's artifact and the run that made it, from the element that shows it. */
  function draftArtifact(id: string) {
    const reference = snapshot?.elements.find((element) => element.id === id)?.reference;
    return reference?.kind === "artifact" ? reference : undefined;
  }
  /** What the person made of a draft, with where its send stands read from the run it started. */
  function drafted(id: string): DraftEdit | undefined {
    const edit = drafts.get(id);
    const projection = objectiveSession?.projection;
    if (!edit?.run || !projection) return edit;
    const run = projection.executions.find((entry) => entry.id === edit.run);
    return { ...edit, ...draftSend(projection, run) };
  }
  /** The words that ask the run to send a draft, so the app's Confirm stands before it goes. */
  function draftRequest(view: DraftView): string {
    const target = view.targetUrl ?? view.to ?? "";
    const review = drafts.get(view.id)?.review ?? view.review ?? "comment";
    const lead =
      view.destination === "github"
        ? review === "approve"
          ? m.work_draft_ask_approve({ target })
          : review === "request_changes"
            ? m.work_draft_ask_changes({ target })
            : m.work_draft_ask_comment({ target })
        : view.destination === "email"
          ? view.subject
            ? m.work_draft_ask_email_subject({ target, subject: view.subject })
            : m.work_draft_ask_email({ target })
          : view.destination === "linkedin" || view.destination === "x"
            ? m.work_draft_ask_post({ target: view.destination === "x" ? "X" : "LinkedIn" })
            : m.work_draft_ask_message({ target });
    return lead;
  }
  /** Send binds a request to the draft object; its words reach the page from the object itself. */
  async function sendDraft(id: string) {
    const view = items.find((item) => item.id === id)?.object?.view;
    const reference = draftArtifact(id);
    const current = objectiveSession;
    const projection = current?.projection;
    if (view?.kind !== "draft" || !reference || !current || !projection) return;
    if (busy || activeExecution || drafted(id)?.send === "sending") return;
    const body = (drafts.get(id)?.body ?? view.body).trim();
    if (!body) return;
    const original = projection.executions
      .find((entry) => entry.id === reference.execution)
      ?.artifacts.find((entry) => entry.id === reference.artifact);
    if (original?.data.kind !== "draft") return;
    if (body !== view.body) {
      current.editArtifact(reference.execution, reference.artifact, { ...original.data, body });
      if (!(await current.saveArtifact(reference.artifact))) return;
    }
    const before = new Set(current.projection?.executions.map((entry) => entry.id));
    const started = await current.continueWith(draftRequest(view), null, {
      draft: reference.artifact,
    });
    const run = current.projection?.executions.find((entry) => !before.has(entry.id));
    if (started && run) editDraft(id, { run: run.id });
  }
  const boardActions: BoardActions = {
    send: (id) => void sendDraft(id),
    write: (id, text) => editDraft(id, { body: text }),
    review: (id, review) => editDraft(id, { review }),
    drafted,
    edit(id, change) {
      const resource = items.find((item) => item.id === id)?.owned;
      const current = resource ? owned.get(resource) : undefined;
      if (resource && current) owned.update(resource, change(current.data as OwnedData));
    },
    measure: takeMeasure,
    toggle: toggleBlock,
    ask: (name) => askAbout(name),
    choose(element, choose) {
      void session.edit(
        choose
          ? { kind: "decide", element, choice: m.work_env_chosen() }
          : { kind: "undecide", element },
      );
    },
    evidence(reference) {
      if (reference.file) {
        liftFile = fileEvidence(context.objectives, reference.file.record) ?? null;
        if (liftFile) lifted = { id: "", origin: null };
        return;
      }
      if (reference.url) openCitation(reference.url);
    },
    entity: (element) => lift(element),
    command(record) {
      panel = null;
      liftFile = null;
      liftRecord = record;
      lifted = { id: "", origin: null };
    },
    page: (url) => openCitation(url),
    note: (id) => noteAction(id),
    compare(id) {
      panel = null;
      inspected = null;
      lifted = { id, origin: canvasRef?.screenRect(id) ?? null, compare: true };
    },
  };
  /** What an object opened in the centre asks of the canvas. */
  const centreActions: ObjectActions = {
    send: (id) => void sendDraft(id),
    write: (id, text) => editDraft(id, { body: text }),
    review: (id, review) => editDraft(id, { review }),
    choose: (element, chosen) => boardActions.choose(element, chosen),
    compare: (id) => boardActions.compare?.(id),
    ask: (subject) => askAbout(subject),
    evidence: (reference) => boardActions.evidence(reference),
    link: (url) => openCitation(url),
    check(id, index) {
      const task = workTasks.tasks(id)[index];
      if (task) openTask(task.id);
    },
  };
  /** The object the centre shows: the lifted one, or its picks laid side by side. */
  const liftedObject = $derived.by((): ObjectView | undefined => {
    const view = liftedItem?.object?.view;
    if (!view) return undefined;
    if (view.kind === "draft") {
      const edit = drafted(view.id);
      return edit
        ? {
            ...view,
            ...(edit.body !== undefined ? { body: edit.body } : {}),
            ...(edit.review ? { review: edit.review } : {}),
            ...(edit.send ? { send: edit.send } : {}),
            ...(edit.note ? { note: edit.note } : {}),
          }
        : view;
    }
    return lifted?.compare && view.kind === "picks" ? picksSheet(view) : view;
  });
  const liftedElement = $derived(snapshot?.elements.find((element) => element.id === lifted?.id));
  /** A request opens its run: what it did, its steps, its sources. */
  const liftedStage = $derived(stages.find((stage) => stage.card === lifted?.id));

  /** Every admitted picture of one subject element, in the order it admitted them. */
  function picturesOf(element: string) {
    const current = snapshot;
    if (!current) return [];
    const subject = baseItems.find((item) => item.id === element)?.subject;
    if (subject?.recordMedia) {
      const record = subject.recordMedia;
      return [{ profile: record.profile, digest: record.digest, record, name: subject.name }];
    }
    if (subject?.mediaAttempted) return [];
    const resources = new Map(
      current.elements.flatMap((entry) =>
        entry.reference.kind === "resource" ? [[entry.id, entry.reference.resource]] : [],
      ),
    );
    return (current.relations ?? []).flatMap((relation) => {
      if (relation.kind !== "uses" || relation.from !== element) return [];
      const resource = resources.get(relation.to);
      const asset = resource ? context.media.get(resource) : undefined;
      return asset?.kind === "image"
        ? [{ profile: current.profile, digest: asset.digest, name: asset.name }]
        : [];
    });
  }
  const liftedPictures = $derived(
    liftedElement?.reference.kind === "subject" ? picturesOf(liftedElement.id) : [],
  );
  /** An opened object's line beside its kind: how many sources it rests on, when it changed. */
  function objectMeta(view: ObjectView) {
    const sources = Object.keys(view.sources ?? {}).length;
    return [sources ? m.work_env_sources_count({ count: sources }) : "", view.updated ?? ""]
      .filter(Boolean)
      .join(" · ");
  }
  /** The one meta line of a media lift: what it is, how large, where it came from. */
  function mediaMeta(asset: NonNullable<CanvasItem["media"]>["asset"]) {
    const origin = asset.origin.kind === "fetched" ? host(asset.origin.url) : "";
    return [asset.mime, mediaSize(asset.bytes), origin].filter(Boolean).join(" · ");
  }
  const authoritative = $derived(new Set(snapshot?.elements.map((element) => element.id) ?? []));
  let liftSource = $state.raw<EvidenceReference | null>(null);
  const loadResult = () => import("./WorkResultInspector.svelte");
  const loadFile = () => import("./WorkFileInspector.svelte");
  /** A new work's first screen, fetched only when a work is empty. */
  const loadStart = () => import("./start/WorkStart.svelte");
  const loadCentre = () => import("./objects/ObjectCentre.svelte");
  const loadModels = () => import("./bar/ModelPicker.svelte");
  const loadSubject = () => import("./WorkSubjectInspector.svelte");
  /** The file a source row opened, shown as the run recorded it. */
  let liftFile = $state.raw<WorkFileEvidenceV1 | null>(null);
  /** One surface: a result opens in the lift, on the run that produced it. */
  async function openResult(id: string, source: EvidenceReference | null = null) {
    const reference = results.references.get(id);
    if (!reference) return;
    const current = workSession(session.profile);
    if (!current) return;
    objectiveSession = current;
    await current.start();
    if (!(await current.open(reference.objective))) return;
    liftSource = source;
    lift(id);
  }
  const savedResultPositions = $derived(
    Object.fromEntries(
      snapshot?.elements.flatMap((element) => {
        if (element.reference.kind !== "artifact") return [];
        const position =
          planGeometry.positions[
            `result:${element.reference.execution}:${element.reference.artifact}`
          ];
        return position ? [[element.id, position]] : [];
      }) ?? [],
    ),
  );
  const savedResultSizes = $derived(
    Object.fromEntries(
      snapshot?.elements.flatMap((element) => {
        if (element.reference.kind !== "artifact") return [];
        const size =
          planGeometry.sizes?.[
            `result:${element.reference.execution}:${element.reference.artifact}`
          ];
        return size ? [[element.id, size]] : [];
      }) ?? [],
    ),
  );
  const canvasView = $derived.by(() => {
    if (!snapshot || !ownView) return undefined;
    const own = ownView;
    const placed = {
      ...plannedGeometry.positions,
      ...planGeometry.positions,
      ...savedResultPositions,
      ...own.positions,
    };
    // Something of the person's with no place yet stands in free room in view, clear of the runs.
    const loose = results.items.flatMap((item) =>
      item.type === "objective" || item.type === "request" || placed[item.id] ? [] : [item.id],
    );
    const near = loose.length ? untrack(placingCentre) : null;
    return {
      ...own,
      positions: {
        ...plannedGeometry.positions,
        ...planGeometry.positions,
        ...savedResultPositions,
        ...ownPlaces(
          own.positions,
          {
            ...Object.fromEntries(results.items.map((item) => [item.id, defaultSize(item)])),
            ...own.sizes,
          },
          stages,
          loose,
          near,
        ),
        ...requests.positions,
        ...agents.positions,
      },
      sizes: {
        ...plannedGeometry.sizes,
        ...planGeometry.sizes,
        ...savedResultSizes,
        ...own.sizes,
      },
    };
  });
  const remoteView = $derived(
    session.remoteView
      ? {
          sequence: session.remoteView.sequence,
          view: environmentView({ ...snapshot!, view: session.remoteView.view }),
        }
      : undefined,
  );
  async function saveResult(id: string) {
    const reference = results.references.get(id);
    const position = planGeometry.positions[id];
    const size = planGeometry.sizes?.[id];
    if (!reference || !(await session.flushView())) return;
    if (!(await session.edit({ kind: "add", reference, area: null }))) return;
    const current = session.snapshot;
    const element = current?.elements.find(
      (element) =>
        element.reference.kind === "artifact" &&
        element.reference.objective === reference.objective &&
        element.reference.execution === reference.execution &&
        element.reference.artifact === reference.artifact,
    );
    if (current && element && position)
      session.checkpoint({
        ...current.view,
        placements: [
          ...current.view.placements.filter((place) => place.element !== element.id),
          {
            element: element.id,
            x: Math.round(position.x),
            y: Math.round(position.y),
            width: size?.width ?? 480,
            height: size?.height ?? 360,
          },
        ],
      });
  }
  /** Every folder on this canvas is granted to the runs it starts. */
  const grantedFolders = $derived(
    snapshot?.elements.flatMap((element) =>
      element.reference.kind === "folder" ? [element.reference.path] : [],
    ) ?? [],
  );
  $effect(() => {
    const current = objectiveSession;
    const folders = grantedFolders;
    if (current) current.folders = folders;
  });
  // A quiet save of the arrangement is not the person waiting on anything: it never disables a control.
  const busy = $derived(
    (!!session.pending && session.pending.kind !== "checkpoint") ||
      session.loading ||
      objectivePending,
  );
  const attachedTabs = $derived(
    snapshot?.elements.flatMap((element) =>
      element.reference.kind === "browser" ? [element.reference.tab] : [],
    ) ?? [],
  );
  const loadCanvas = () => import("./WorkCanvas.svelte");
  const loadAgentLine = () => import("./AgentLine.svelte");
  const loadTakeover = () => import("./pane/TakeoverPane.svelte");
  $effect(() => {
    const current = snapshot;
    const focused = inspected;
    if (current) untrack(() => context.update(current, focused ?? undefined));
  });
  $effect(() => {
    const urls = (snapshot?.elements ?? []).flatMap((element) => {
      const reference = element.reference;
      if (reference.kind === "link") return [reference.url];
      if (reference.kind !== "browser") return [];
      const tab = tabs.find((tab) => tab.id === reference.tab);
      return tab ? [tab.url ?? ""] : [];
    });
    untrack(() => context.trackUrls(urls));
  });
  $effect(() => {
    const current = session;
    untrack(() => current.snapshot?.id);
    return () => objectiveSession?.stopObserving();
  });
  // Leaving Work unmounts this workspace, so a return starts with no objective
  // session and the canvas loses the agent's line and its page frames. Reattach
  // the profile's session to the objective it was last on.
  $effect(() => {
    const current = snapshot;
    if (!current || objectiveSession) return;
    const objectives = current.elements.flatMap((element) =>
      element.reference.kind === "objective" ? [element.reference.objective] : [],
    );
    if (!objectives.length) return;
    untrack(() => {
      let resumed;
      try {
        resumed = workSession(session.profile);
      } catch {
        return;
      }
      const wanted = objectives.includes(resumed.selected ?? "")
        ? resumed.selected!
        : objectives.at(-1)!;
      objectiveSession = resumed;
      void resumed.start().then(() => resumed.open(wanted));
    });
  });
  async function attach(ids: string[]) {
    for (const tab of ids) {
      if (!tabs.some((candidate) => candidate.id === tab)) continue;
      if (!(await session.edit({ kind: "add", reference: { kind: "browser", tab }, area: null })))
        break;
    }
  }
  async function attachResources(ids: string[]) {
    for (const resource of ids) {
      if (
        snapshot?.elements.some(
          (e) => e.reference.kind === "resource" && e.reference.resource === resource,
        )
      )
        continue;
      if (
        !(await session.edit({
          kind: "add",
          reference: { kind: "resource", resource },
          area: null,
        }))
      )
        break;
    }
    panel = null;
  }
  /** A new note joins the canvas and opens over it, ready to write in. */
  async function createNote() {
    const current = notes;
    const environment = session.snapshot?.id;
    if (!current || busy) return;
    await current.start();
    // A note has no file, and so no identity to attach, until it has text.
    await current.create(`# ${m.note_untitled()}\n`);
    const id = current.note?.id;
    if (!id || !(await current.flush()) || environment !== session.snapshot?.id) return;
    const reference = { kind: "resource" as const, resource: id };
    if (!(await placing.place(reference, "note", clearSpot(null, NOTE_SIZE)))) return;
    const element = session.snapshot ? elementFor(session.snapshot, reference) : undefined;
    if (element) lift(element.id);
  }
  const NOTE_SIZE = { width: 360, height: 304 } as const;
  /** A block the person puts down: an empty table, checklist or calendar of their own, ready to fill. */
  async function createBlock(kind: BlockKind) {
    panel = null;
    if (kind === "note") return void createNote();
    if (busy || !session.snapshot) return;
    const environment = session.snapshot.id;
    const [title, data]: [string, OwnedData] =
      kind === "checklist"
        ? [m.work_block_checklist(), { kind: "list", style: "todo", items: [] }]
        : kind === "table"
          ? [
              m.work_block_table(),
              {
                kind: "sheet",
                columns: [
                  { label: m.work_block_name_column(), kind: "text" },
                  { label: m.work_block_notes_column(), kind: "text" },
                ],
                rows: [],
              },
            ]
          : [
              m.work_block_calendar(),
              { kind: "plan", steps: [], checkable: false, calendar: true },
            ];
    const id = await owned.create(title, data);
    if (!id || environment !== session.snapshot?.id) return;
    const view = ownedView(id, title, data);
    const width = view ? objectWidth(view).ideal : 400;
    const size = { width, height: view ? Math.round(objectHeight(view, width)) + 48 : 240 };
    await placing.place({ kind: "resource", resource: id }, "object", clearSpot(null, size), size);
  }
  /** What stands on the canvas now, one thing left out: the runs and every placed item. */
  function taken(except: string | null) {
    const view = canvasView;
    const rects = stages.map((stage) => stage.lane.box);
    if (!view) return rects;
    for (const item of items) {
      if (item.id === except || item.type === "agent") continue;
      const at = view.positions[item.id];
      if (!at) continue;
      rects.push({ ...at, ...(view.sizes[item.id] ?? defaultSize(item)) });
    }
    return rects;
  }
  /** A drop the canvas put clear of a run says so, briefly. */
  let putClear = $state(false);
  let putClearTimer: ReturnType<typeof setTimeout> | undefined;
  $effect(() => () => clearTimeout(putClearTimer));
  /**
   * Where something of the person's dropped at `rect` must stand instead:
   * only on or against a run's card is a drop refused, and then it goes to
   * the nearest free spot. Null, and it stays exactly where it was put. A
   * run's own cards are the run's to place.
   */
  function dropPlace(id: string | null, rect: CanvasPosition & { width: number; height: number }) {
    const current = snapshot;
    const element = id ? current?.elements.find((entry) => entry.id === id) : undefined;
    if (!current || (id && (!element || laneElement(current, element)))) return null;
    const spot = dropSpot(
      rect,
      stages.flatMap((stage) => Object.values(stage.lane.rects)),
    );
    if (!spot) return null;
    putClear = true;
    clearTimeout(putClearTimer);
    putClearTimer = setTimeout(() => (putClear = false), 4000);
    return spot;
  }
  /** The centre of the nearest free room to the view's middle, or to where a thing already stands. */
  function clearSpot(except: string | null, size: { width: number; height: number }) {
    const view = canvasView;
    const at = except ? view?.positions[except] : null;
    const want = at ? { x: at.x + size.width / 2, y: at.y + size.height / 2 } : placingCentre();
    return want ? freeSpot(want, size, taken(except)) : null;
  }
  /** A note put down after writing it lands clear of everything else, near where it was, and stays. */
  function settleNote(id: string) {
    const current = session.snapshot;
    const place = current?.view.placements.find((entry) => entry.element === id);
    const element = current?.elements.find((entry) => entry.id === id);
    if (!current || !place || element?.reference.kind !== "resource") return;
    const item = items.find((entry) => entry.id === id);
    if (item?.type !== "note") return;
    const size = { width: place.width, height: place.height };
    const spot = clearSpot(id, size);
    if (!spot) return;
    const x = Math.round(spot.x - size.width / 2);
    const y = Math.round(spot.y - size.height / 2);
    if (x === place.x && y === place.y) return;
    session.checkpoint({
      ...current.view,
      placements: current.view.placements.map((entry) =>
        entry.element === id ? { ...entry, x, y } : entry,
      ),
    });
  }
  /** The bar's keys: V and H pick the pointer, N and A open their panels, / goes to the ask. */
  function toolKeys(event: KeyboardEvent) {
    if (event.defaultPrevented || event.metaKey || event.ctrlKey || event.altKey) return;
    const target = event.target instanceof HTMLElement ? event.target : null;
    if (target?.closest("input, textarea, select, [contenteditable], [role='dialog']")) return;
    if (!snapshot || pane || lifted || takeover) return;
    const key = event.key.toLowerCase();
    if (key === "v" || key === "h") setPointerTool(key === "v" ? "select" : "hand");
    else if (key === "n" && preferences.enabled("notes")) {
      blocksView = "notes";
      openPanel("blocks", true);
    } else if (key === "a") openPanel("media", true);
    else if (key === "b") {
      blocksView = "blocks";
      openPanel("blocks", true);
    } else if (key === "t" && preferences.enabled("tasks")) openPanel("tasks", true);
    else if (key === "/" && aiEnabled)
      composerElement?.querySelector<HTMLElement>("textarea")?.focus();
    else return;
    event.preventDefault();
  }
  /** One of the person's own notes, placed on the canvas as it is. */
  async function placeNote(id: string) {
    const reference = { kind: "resource" as const, resource: id };
    if (await placing.place(reference, "note", clearSpot(null, NOTE_SIZE))) panel = null;
  }
  /** The work this canvas is already talking to, if its request card is here. */
  const runningObjective = $derived(
    !!objectiveSession?.projection &&
      !!snapshot?.elements.some(
        (element) =>
          element.reference.kind === "objective" &&
          element.reference.objective === objectiveSession?.selected,
      ),
  );
  /** One field, one meaning: the first message starts the work, the rest continue it. */
  let blockSaveFailed = $state(false);
  async function send() {
    const text = session.composer.trim();
    if (!text || busy) return;
    blockSaveFailed = !(await owned.flushAll());
    if (blockSaveFailed) return;
    const current = objectiveSession;
    if (runningObjective && current) {
      const context = contextSel;
      session.composer = "";
      openTabs = false;
      // Words typed while the agent works steer it now; if it cannot take
      // them mid-step, they go next.
      if (activeExecution) {
        if (!(await current.steer(text))) current.enqueue(text);
      } else await current.continueWith(text, context);
      return;
    }
    await createObjective();
  }
  /** The workflow a start tile put in the ask bar, loaded first by the run it sends. */
  let startSkill = $state<string | null>(null);
  async function createObjective() {
    if (!session.composer.trim() || objectivePending || busy) return;
    const current = workSession(session.profile);
    if (!current) return;
    const submission = session.objectiveSubmission ?? {
      objective: session.composer.trim(),
      command: commandId(),
      attached: false,
      context: contextSel,
    };
    session.objectiveSubmission = submission;
    objectivePending = true;
    objectiveSession = current;
    try {
      await current.start();
      if (!session.objectiveToAttach) {
        const pending = current.pending;
        const created = pending
          ? pending.kind === "author" &&
            pending.command.intent.kind === "create" &&
            pending.command.command === submission.command &&
            pending.command.intent.objective === submission.objective &&
            (await current.retry())
          : await current.create(submission.objective, submission.command);
        if (!created) {
          if (!current.pending) session.objectiveSubmission = null;
          return;
        }
        session.objectiveToAttach = current.selected;
      }
      const objectiveId = session.objectiveToAttach;
      if (!objectiveId) return;
      if (!submission.attached) {
        const attached = session.snapshot?.elements.some(
          (element) =>
            element.reference.kind === "objective" && element.reference.objective === objectiveId,
        );
        // A quiet save of the arrangement may be in flight: it lands first.
        if (!attached && !(await session.flushView())) return;
        if (
          !attached &&
          !(await session.edit({
            kind: "add",
            reference: { kind: "objective", objective: objectiveId },
            area: null,
          }))
        )
          return;
        submission.attached = true;
      }
      if (submission.context && !submission.related) {
        const goal = session.snapshot?.elements.find(
          (element) =>
            element.reference.kind === "objective" && element.reference.objective === objectiveId,
        );
        for (const item of submission.context.items) {
          if (!goal || item.element === goal.id) continue;
          const known = (session.snapshot?.relations ?? []).some(
            (relation) => relation.from === goal.id && relation.to === item.element,
          );
          if (known) continue;
          if (
            !(await session.edit({
              kind: "relate",
              from: goal.id,
              to: item.element,
              relation: "uses",
            }))
          )
            return;
        }
        submission.related = true;
      }
      if (!(await current.open(objectiveId))) return;
      const basis = current.projection?.work;
      if (current.selected !== objectiveId || basis?.id !== objectiveId) return;
      session.composer = "";
      session.objectiveToAttach = null;
      session.objectiveSubmission = null;
      openTabs = false;
      const skill = startSkill;
      startSkill = null;
      await current.run(submission.context, { skill });
    } finally {
      objectivePending = false;
    }
  }
  function checkpoint(view: CanvasView) {
    camera = view.viewport ?? camera;
    if (!snapshot || session.loading) return;
    const ids = Object.keys(view.positions).filter(
      (id) => results.references.has(id) && !snapshot.elements.some((element) => element.id === id),
    );
    const positions = Object.fromEntries(
      [
        ...Object.entries(planGeometry.positions),
        ...ids.map((id) => [id, view.positions[id]!] as const),
      ].slice(-128),
    );
    const sizes = Object.fromEntries(
      [
        ...Object.entries(planGeometry.sizes ?? {}),
        ...ids.flatMap((id) => (view.sizes?.[id] ? [[id, view.sizes[id]] as const] : [])),
      ].slice(-128),
    );
    if (
      JSON.stringify(positions) !== JSON.stringify(planGeometry.positions) ||
      JSON.stringify(sizes) !== JSON.stringify(planGeometry.sizes)
    )
      planGeometry = { ...view, positions, sizes };
    const next = {
      revision: snapshot.view.revision,
      x: Math.round(view.viewport.x),
      y: Math.round(view.viewport.y),
      zoom_milli: Math.round(view.viewport.zoom * 1000),
      areas: snapshot.areas.flatMap((area) => {
        const place = view.areas?.[area.id];
        const previous = snapshot.view.areas?.find((entry) => entry.area === area.id);
        const source = place ?? previous;
        return source
          ? [
              {
                area: area.id,
                x: Math.round(source.x),
                y: Math.round(source.y),
                width: Math.round(source.width),
                height: Math.round(source.height),
              },
            ]
          : [];
      }),
      placements: viewPlacements(snapshot, view, stages, moved),
    };
    if (
      JSON.stringify({ ...next, revision: "" }) !==
      JSON.stringify({ ...(session.viewDraft?.view ?? snapshot.view), revision: "" })
    )
      session.checkpoint(next);
  }
  const activeExecution = $derived.by(() => {
    const projection = objectiveSession?.projection;
    return !!projection?.executions.some((execution) => isLive(projection, execution));
  });
  // The sidebar marks the project whose run is live; a run keeps its project when the canvas moves on.
  $effect(() => {
    const live = activeExecution;
    const here = runningObjective ? snapshot?.id : undefined;
    untrack(() => {
      if (!live) session.running = null;
      else if (here) session.running = here;
    });
  });
  const needsDecision = $derived(
    !!objectiveSession?.projection?.work.questions.some((question) => question.state === "active"),
  );
  /** One click makes an Area: around the selection when there is one. */
  const ownedElements = (ids: readonly string[]) =>
    ids.filter((id) => snapshot?.elements.some((element) => element.id === id));
  /** A new area around the person's selected elements; the canvas places it before they move in. */
  async function groupSelection(title: string, ids: readonly string[] = selectedIds) {
    const bounds = canvasRef?.selectionBounds(ownedElements(ids));
    const current = snapshot;
    if (!bounds || !current || !title.trim()) return false;
    if (!(await session.flushView())) return false;
    if (!(await session.edit({ kind: "create_area", title: title.trim() }))) return false;
    const created = session.snapshot?.areas.find(
      (area) => !current.areas.some((known) => known.id === area.id),
    );
    if (!created) return false;
    const { ids: members, ...rect } = bounds;
    canvasRef?.placeArea(created.id, rect);
    for (const id of members)
      if (!(await session.edit({ kind: "assign_area", element: id, area: created.id })))
        return false;
    panel = null;
    return true;
  }
  async function selectionAction(action: "area" | "ask" | "remove", ids: string[]) {
    const owned = ownedElements(ids);
    if (action === "ask") {
      composerElement?.querySelector<HTMLElement>("textarea")?.focus();
      return;
    }
    if (action === "area") {
      const first = items.find((item) => item.id === owned[0])?.title.trim();
      await groupSelection(first ? clipText(first, 24).trimEnd() : m.work_env_area(), owned);
      return;
    }
    for (const id of owned) {
      if (!(await session.edit({ kind: "remove", element: id }))) return;
      if (inspected === id) inspected = null;
    }
  }
  /** Opens one of the bar's panels; the tab and media tools open the attach panel on their kind. */
  function openPanel(next: typeof panel, open: boolean) {
    if (!open) {
      if (panel === next) panel = null;
      return;
    }
    if (next === "tabs") attachKind = "tabs";
    panel = next;
  }
  const model = $derived(currentModel(objectiveSession?.projection));
  /** The run on this canvas is going, or waits on the person: the bar is its line. */
  const lineRunning = $derived(
    runningObjective && (activeExecution || needsDecision || !!agentWaiting),
  );
  // Saving is silent; only a write that did not land says so.
  const troubled = $derived(
    !!session.failure ||
      placing.folderRefused ||
      session.delivery === "unknown" ||
      session.delivery === "conflict" ||
      session.delivery === "rejected",
  );
  const runStatus = $derived(
    runningObjective && needsDecision
      ? m.work_env_needs_you()
      : runningObjective && activeExecution
        ? m.work_env_working()
        : null,
  );
  // A different project on the canvas starts with nothing lifted or open over it.
  let shownProject: string | undefined;
  $effect(() => {
    const id = snapshot?.id;
    untrack(() => {
      if (shownProject !== undefined && id !== shownProject) {
        inspected = null;
        lifted = null;
        panel = null;
      }
      shownProject = id;
    });
  });
</script>

{#snippet attachPanel()}<AttachPanel
    bind:kind={attachKind}
    {busy}
    folderPending={placing.folderPending}
    onaddlink={(raw: string) =>
      placing.addLink(raw).then((placed) => {
        if (placed) panel = null;
        return placed;
      })}
    onaddfolder={(path: string) =>
      placing.addFolder(path).then((placed) => {
        if (placed) panel = null;
        return placed;
      })}
    onchoosefolder={() =>
      void placing.chooseFolder().then((placed) => {
        if (placed) panel = null;
      })}
    >{#snippet picker()}<WorkTabPicker
        {tabs}
        {openTabs}
        onopentabs={(on: boolean) => (openTabs = on)}
        {spaceName}
        {currentTabId}
        attachedTabIds={attachedTabs}
        pending={objectivePending || session.pending?.kind === "command"}
        onattach={(ids) => void attach(ids)}
        onopen={(id) => openPane({ kind: "tab", id }, null)}
        {onnewtab}
      />{/snippet}{#snippet media(kind: "document" | "image")}<WorkMediaPicker
        profile={session.profile}
        {kind}
        attachedIds={snapshot?.elements.flatMap((element) =>
          element.reference.kind === "resource" ? [element.reference.resource] : [],
        ) ?? []}
        pending={busy}
        onattach={(ids) => void attachResources(ids)}
      />{/snippet}</AttachPanel
  >{/snippet}
{#snippet notesList(back: () => void)}{#if notes}<NotePanel
      {notes}
      placed={snapshot?.elements.flatMap((element) =>
        element.reference.kind === "resource" ? [element.reference.resource] : [],
      ) ?? []}
      {busy}
      oncreate={() => {
        panel = null;
        void createNote();
      }}
      onpick={(id) => void placeNote(id)}
      onback={back}
    />{/if}{/snippet}
{#snippet barTools()}
  {@const hand = pointerTool() === "hand"}
  <BarTool
    icon={hand ? HandIcon : Cursor01Icon}
    label={hand ? m.work_tool_hand() : m.work_tool_select()}
    keys={[hand ? "H" : "V"]}
    also={{ label: hand ? m.work_tool_select() : m.work_tool_hand(), keys: [hand ? "V" : "H"] }}
    onclick={() => setPointerTool(hand ? "select" : "hand")}
  />
  <span class="tool-rule" aria-hidden="true"></span>
  <BarTool
    icon={ShapeCollectionIcon}
    label={m.work_tool_blocks()}
    keys={["B"]}
    disabled={!snapshot}
    open={panel === "blocks"}
    onopenchange={(open) => {
      if (open && panel !== "blocks") blocksView = "blocks";
      openPanel("blocks", open);
    }}
    >{#snippet content()}<BlocksPanel
        {busy}
        bind:view={blocksView}
        notes={preferences.enabled("notes") && notes ? notesList : undefined}
        onpick={(kind) => void createBlock(kind)}
      />{/snippet}</BarTool
  >
  {#if preferences.enabled("tasks")}<BarTool
      icon={CheckListIcon}
      label={m.panel_tasks()}
      keys={["T"]}
      disabled={!snapshot}
      wide
      open={panel === "tasks"}
      onopenchange={(open) => openPanel("tasks", open)}
      >{#snippet content()}<TasksPanel profile={session.profile} />{/snippet}</BarTool
    >{/if}
  <BarTool
    icon={PlusSignIcon}
    label={m.work_tool_attach()}
    keys={["A"]}
    disabled={!snapshot}
    wide
    open={panel === "media"}
    onopenchange={(open) => openPanel("media", open)}
    content={attachPanel}
  />
{/snippet}
{#snippet barAttach()}<BarTool
    icon={PlusSignIcon}
    label={m.work_tool_attach()}
    small
    wide
    disabled={!snapshot}
    open={panel === "attach"}
    onopenchange={(open) => openPanel("attach", open)}
    content={attachPanel}
  />{/snippet}
{#snippet agentLine()}
  <LazyView
    loader={loadAgentLine}
    loadingLabel=""
    failureLabel={m.surface_render_failed()}
    retryLabel={m.surface_retry()}
    >{#snippet children(Line)}
      <Line
        session={objectiveSession!}
        viewed={viewedExecution}
        agents={agents.items}
        working={workingParts}
        ended={runEnded}
        draft={session.composer}
        onreview={(step: string) => {
          panel = null;
          inspected = null;
          liftFile = null;
          lifted = { id: "", origin: null, proposal: step };
        }}
        waiting={agentWaiting}
        problem={workTasks.failed
          ? m.work_tasks_failed()
          : workNotes.failed
            ? m.work_note_failed()
            : null}
        ondismissproblem={() => {
          workTasks.dismiss();
          workNotes.dismiss();
        }}
        {writeup}
        onwaitingpage={(card: string) => {
          panel = null;
          canvasRef?.focusCard(pageEntries.get(card)?.part ?? card);
        }}
        onfocusagent={(id) => canvasRef?.center(id)}
        onsteered={() => (session.composer = "")}
        {human}
        onopenstep={openStep}
      />{/snippet}</LazyView
  >
{/snippet}
{#snippet composerContext()}
  {#if openTabs}<OpenTabsChip
      disabled={objectivePending || !!session.objectiveSubmission}
      onremove={() => (openTabs = false)}
    />{/if}
  {#if contextSel?.items.length}
    <ContextManifest profile={session.profile} selection={contextSel} purpose="agent" />
  {/if}
{/snippet}
<svelte:window onbeforeunload={abandonTakeover} onkeydown={toolKeys} />
<div class="environment">
  <div class="canvas-card" bind:this={cardHost}>
    {#if snapshot}{#key snapshot.id}
        <LazyView
          loader={loadCanvas}
          loadingLabel={m.surface_loading()}
          failureLabel={m.surface_render_failed()}
          retryLabel={m.surface_retry()}
          >{#snippet children(Canvas)}<Canvas
              {items}
              {links}
              areas={snapshot.areas}
              {pictures}
              initialView={canvasView}
              {remoteView}
              {authoritative}
              expose={(api) => (canvasRef = api)}
              board={boardActions}
              work={(objective: string) =>
                objectiveSession?.projection?.work.id === objective
                  ? objectiveSession.projection
                  : context.objectives.get(objective)}
              onmoved={(ids: string[]) => {
                for (const id of ids) moved.add(id);
              }}
              placeDrop={dropPlace}
              onprobe={(origin: string) =>
                void commands.faviconProbe(session.profile, [origin]).catch(() => false)}
              fitBottomInset={composerHeight}
              fitTopInset={56}
              {home}
              still={!!lifted || !!pane || !!takeover}
              oninspect={(id: string) => (inspected = id)}
              onopen={openLift}
              onopenlink={openCitation}
              onselectionchange={(ids: string[]) => {
                selectedIds = ids.filter((id) => authoritative.has(id));
                if (!ids.length) inspected = null;
              }}
              onareachange={(id: string, area: string | null) =>
                void session.edit({ kind: "assign_area", element: id, area })}
              onselectionaction={(action: "area" | "ask" | "remove", ids: string[]) =>
                void selectionAction(action, ids)}
              onareaedit={(
                area: string,
                edit: { kind: "rename"; title: string } | { kind: "remove" },
              ) =>
                void session.edit(
                  edit.kind === "rename"
                    ? { kind: "rename_area", area, title: edit.title }
                    : { kind: "remove_area", area },
                )}
              onevidence={(id: string, source: EvidenceReference) => void openResult(id, source)}
              onaction={(id: string, action?: string) => {
                if (action === "remove") {
                  const element = snapshot?.elements.find((element) => element.id === id);
                  if (element)
                    void session.edit({ kind: "remove", element: element.id }).then((okay) => {
                      if (okay && inspected === id) inspected = null;
                    });
                  return;
                }
                if (action === "choose" || action === "unchoose") {
                  void session.edit(
                    action === "choose"
                      ? { kind: "decide", element: id, choice: m.work_env_chosen() }
                      : { kind: "undecide", element: id },
                  );
                  return;
                }
                if (action?.startsWith("area:")) {
                  void session.edit({
                    kind: "assign_area",
                    element: id,
                    area: action.slice(5) || null,
                  });
                  return;
                }
                if (action === "help") {
                  openTakeover(id);
                  return;
                }
                if (action === "wait" || action?.startsWith("wait:")) {
                  extendWait(action === "wait" ? id : action.slice("wait:".length));
                  return;
                }
                if (action === "expand-request") {
                  if (openRequests.has(id)) openRequests.delete(id);
                  else openRequests.add(id);
                  return;
                }
                if (action?.startsWith("need:")) {
                  meetNeed(id, action.slice("need:".length));
                  return;
                }
                if (action?.startsWith("page:")) {
                  openBranchPage(action.slice("page:".length));
                  return;
                }
                if (action === "play") {
                  playHere(id);
                  return;
                }
                if (action === "ask") {
                  composerElement?.querySelector<HTMLElement>("textarea")?.focus();
                  return;
                }
                if (results.references.has(id)) void saveResult(id);
              }}
              onviewchange={checkpoint}
            />{/snippet}</LazyView
        >
        {#if snapshot.elements.length === 0}{#if aiEnabled}<LazyView
              loader={loadStart}
              loadingLabel=""
              failureLabel={m.surface_render_failed()}
              retryLabel={m.surface_retry()}
              >{#snippet children(Start)}<Start
                  bind:value={session.composer}
                  onskill={(skill: string | null) => (startSkill = skill)}
                  field={composerElement}
                  works={session.works}
                  profile={session.profile}
                  disabled={busy || objectivePending}
                  inset={composerHeight}
                />{/snippet}</LazyView
            >{:else}<div class="welcome"><p>{m.work_env_ai_off_hint()}</p></div>{/if}{/if}
      {/key}{:else}<div class="welcome">
        <p>{session.loading ? m.surface_loading() : m.work_env_preparing()}</p>
      </div>{/if}
    <!-- The canvas never ends in a hard edge: it fades under what sits at its
         top and bottom, and costs nothing while nothing moves. -->
    <div class="edge-scrim top" aria-hidden="true"></div>
    <div class="edge-scrim bottom" style:--dock="{dockHeight}px" aria-hidden="true"></div>
    <div class="canvas-top">
      <WorksMenu {session} untitled={m.work_env_default_title()} live={!!runStatus} />
      <span class="canvas-top-gap"></span>
      <div class="island" class:gone={!(runningObjective && objectiveSession)}>
        {#if runningObjective && objectiveSession}{@render agentLine()}{/if}
      </div>
      {#if putClear && !troubled}<div class="status" role="status">
          <span>{m.work_env_put_clear()}</span>
        </div>{/if}
      {#if troubled}<div class="status" role="status">
          <span
            >{session.failure
              ? m.work_request_failed()
              : placing.folderRefused
                ? m.work_env_folder_refused()
                : m.work_env_view_unsaved()}</span
          >{#if session.delivery === "unknown"}<Button
              size="compact"
              onclick={() => void session.retry()}>{m.work_reconcile()}</Button
            >{:else if session.delivery === "conflict" && session.viewDraft}<Button
              size="compact"
              onclick={() => void session.discardView()}>{m.work_env_discard_view()}</Button
            >{:else if session.failure}<Button size="compact" onclick={() => void session.refresh()}
              >{m.work_env_reload()}</Button
            >{/if}
        </div>{/if}
      <!-- The zoom level reads in the top-right corner, a quiet counterpart to the work's name. -->
      <div class="zoom-slot" data-work-zoom-slot></div>
    </div>
  </div>
  {#if lifted && cardBounds}
    <Lift
      bind:this={liftRef}
      origin={lifted.origin}
      source={lifted.id || null}
      bounds={cardBounds}
      preferred={liftSize(liftedItem)}
      hug={!lifted.proposal && !liftFile && liftedElement?.reference.kind !== "resource"}
      title={lifted.proposal ? m.work_line_review() : (liftedItem?.title ?? "")}
      onclose={() => {
        const closing = lifted?.id;
        lifted = null;
        if (closing) settleNote(closing);
        liftSource = null;
        liftFile = null;
        liftRecord = null;
      }}
    >
      {#if lifted.proposal && objectiveSession}
        <LazyView
          loader={loadFile}
          loadingLabel={m.surface_loading()}
          failureLabel={m.surface_render_failed()}
          retryLabel={m.surface_retry()}
          >{#snippet children(FileView)}<FileView
              proposal={{ session: objectiveSession!, step: lifted!.proposal! }}
              ondecided={() => (lifted = null)}
            />{/snippet}</LazyView
        >
      {:else if liftFile}
        <LazyView
          loader={loadFile}
          loadingLabel={m.surface_loading()}
          failureLabel={m.surface_render_failed()}
          retryLabel={m.surface_retry()}
          >{#snippet children(FileView)}<FileView
              file={liftFile!}
              onreveal={(path: string) => placing.reveal(path)}
              onback={() => (liftFile = null)}
            />{/snippet}</LazyView
        >
      {:else if liftedCommand}
        <div class="lift-body">
          <LiftHeader
            kind={m.work_env_command()}
            title={liftedCommand.command.command}
            icon={ComputerTerminal01Icon}
          />
          {#await import("./local/CommandRecord.svelte") then module}
            <module.default record={liftedCommand} />
          {/await}
        </div>
      {:else if liftedItem?.sources || liftedItem?.drawn}
        {@const part = liftedItem.type === "part"}
        <SourcesLift
          kind={part ? m.work_part() : m.work_sources()}
          title={part ? liftedItem.title : undefined}
          rows={liftedItem.sources
            ? liftedItem.sources.map((row) => ({
                key: row.key,
                title: row.title,
                host: row.where,
                url: row.url,
                ...(row.note ? { note: row.note } : {}),
                ...(row.file ? { file: row.file.record } : {}),
              }))
            : (liftedItem.drawn?.rows ?? []).map((row) => ({
                key: row.key,
                title: row.title,
                host: row.host,
                url: row.url,
                ...(row.file ? { file: row.file.record } : {}),
              }))}
          unread={liftedItem.drawn?.unread ?? []}
          onopen={(row) => {
            if (row.file) {
              liftFile = fileEvidence(context.objectives, row.file) ?? null;
              return;
            }
            lifted = null;
            openPane({ kind: "url", url: row.url }, null);
          }}
        >
          {#snippet notes()}{#if liftedItem?.part?.notes?.length}{#await import("./objects/ObjectView.svelte") then module}
                {#each liftedItem.part.notes as note (note.id)}<module.default
                    object={note}
                    centre
                  />{/each}
              {/await}{/if}{/snippet}
        </SourcesLift>
      {:else if liftedElement?.reference.kind === "resource" && liftedItem?.media}
        {@const asset = liftedItem.media.asset}
        {@const image =
          asset.kind === "image" ? mediaUrl(liftedItem.media.profile, asset.digest, 1600) : null}
        <div class="lift-body lift-media">
          <LiftHeader
            kind={liftedItem.kind}
            title={liftedItem.title}
            meta={mediaMeta(asset)}
            icon={asset.kind === "image"
              ? Image01Icon
              : asset.kind === "pdf"
                ? Pdf01Icon
                : File01Icon}
          >
            {#snippet actions()}{#if !image}<Button
                  size="compact"
                  onclick={() => {
                    const id =
                      liftedElement?.reference.kind === "resource"
                        ? liftedElement.reference.resource
                        : null;
                    if (id) void commands.mediaOpen(session.profile, id);
                  }}>{m.work_media_open_file()}</Button
                >{/if}{/snippet}
          </LiftHeader>
          {#if image}<div class="lift-picture"><img src={image} alt={liftedItem.title} /></div>{/if}
        </div>
      {:else if liftedElement?.reference.kind === "resource"}
        <LazyView
          loader={loadNoteHost}
          loadingLabel={m.surface_loading()}
          failureLabel={m.surface_render_failed()}
          retryLabel={m.surface_retry()}
          >{#snippet children(Host)}<Host
              profile={session.profile}
              host={`${notesHost}:lift`}
              id={liftedElement.reference.kind === "resource"
                ? liftedElement.reference.resource
                : ""}
            />{/snippet}</LazyView
        >
      {:else if liftedElement?.reference.kind === "folder"}
        {@const folder = liftedElement.reference.path}
        <div class="lift-body lift-plain">
          <LiftHeader
            kind={liftedItem?.kind ?? m.work_env_folder()}
            title={liftedItem?.title ?? ""}
            meta={homePath(folder)}
            icon={FolderAddIcon}
          >
            {#snippet actions()}<Button size="compact" onclick={() => placing.reveal(folder)}
                >{m.work_env_reveal()}</Button
              >{/snippet}
          </LiftHeader>
          <p>{m.work_env_folder_grant_note()}</p>
        </div>
      {:else if liftedElement?.reference.kind === "subject"}
        {@const subject = liftedElement.reference}
        <LazyView
          loader={loadSubject}
          loadingLabel={m.surface_loading()}
          failureLabel={m.surface_render_failed()}
          retryLabel={m.surface_retry()}
          >{#snippet children(Product)}<Product
              reference={subject}
              objectives={context.objectives}
              pictures={liftedPictures}
              pages={recordedPages(subject.objective)}
              note={(markdown: string) => subjectNote(subject, markdown)}
              onask={askAbout}
              onopen={openCitation}
              onfile={(record: string) =>
                (liftFile = fileEvidence(context.objectives, record) ?? null)}
            />{/snippet}</LazyView
        >
      {:else if liftedStage}
        <RequestLift
          request={liftedStage.request}
          trail={liftedStage.trail}
          runs={context.objectives
            .get(liftedStage.objective)
            ?.executions.filter((run) => liftedStage.executions.includes(run.id)) ?? []}
          made={stageMade(liftedStage)}
          oncommand={(record) => boardActions.command(record)}
          onsource={(row) => {
            if (row.file) {
              liftFile = fileEvidence(context.objectives, row.file.record) ?? null;
              return;
            }
            lifted = null;
            openPane({ kind: "url", url: row.url }, null);
          }}
          onmade={(id) => {
            lifted = null;
            canvasRef?.focusCard(id);
          }}
        />
      {:else if liftedObject && !(liftedItem && results.references.has(liftedItem.id) && !agentBlock(liftedItem.id))}
        {@const named = liftedObject.kind === "reply" ? liftedObject.headline : liftedObject.title}
        <div class="lift-body">
          <LiftHeader
            kind={objectLabel(liftedObject.kind)}
            meta={objectMeta(liftedObject)}
            icon={objectIcon(liftedObject.kind)}
            primary={liftedItem ? resultAction(liftedItem.id) : undefined}
          >
            {#snippet actions()}{#if named}<Button size="compact" onclick={() => askAbout(named)}
                  >{m.work_lift_ask()}</Button
                >{/if}{/snippet}
          </LiftHeader>
          <LazyView
            loader={loadCentre}
            loadingLabel={m.surface_loading()}
            failureLabel={m.surface_render_failed()}
            retryLabel={m.surface_retry()}
            >{#snippet children(Centre)}<Centre
                object={liftedObject!}
                actions={centreActions}
              />{/snippet}</LazyView
          >
        </div>
      {:else if liftedItem?.artifact}
        <div class="lift-result">
          {#if results.references.get(liftedItem.id) && objectiveSession}
            <LazyView
              loader={loadResult}
              loadingLabel={m.surface_loading()}
              failureLabel={m.surface_render_failed()}
              retryLabel={m.surface_retry()}
              >{#snippet children(Result)}<Result
                  session={objectiveSession!}
                  reference={results.references.get(liftedItem!.id)!}
                  source={liftSource}
                  primary={resultAction(liftedItem!.id)}
                  secondary={workTasks.state(liftedItem!.id) === "none"
                    ? undefined
                    : noteAction(liftedItem!.id)}
                  {pictures}
                  onopen={openCitation}
                  onfile={(record: string) =>
                    (liftFile = fileEvidence(context.objectives, record) ?? null)}
                />{/snippet}</LazyView
            >
          {:else}<LiftHeader kind={liftedItem.kind} title={liftedItem.title} />{/if}
        </div>
      {:else if liftedItem}
        <div class="lift-body lift-plain">
          <LiftHeader
            kind={liftedItem.kind}
            title={liftedItem.title}
            meta={[liftedItem.detail, liftedItem.status].filter(Boolean).join(" · ")}
            icon={liftedElement?.reference.kind === "browser" ? GlobalIcon : undefined}
            primary={liftedElement?.reference.kind === "browser"
              ? { label: m.work_env_open_page(), onclick: openLiftedTab }
              : undefined}
          />
        </div>
      {/if}
    </Lift>
  {/if}
  {#if pane && cardBounds}
    <BrowserPane
      tab={paneTab}
      account={pane.account}
      applied={paneLayout}
      bounds={cardBounds}
      origin={pane.origin}
      phase={pane.phase}
      added={paneAdded}
      onmeasure={paneMeasured}
      onnavigate={(input) => {
        if (paneTab) void commands.tabsNavigate(paneTab.id, input);
      }}
      onback={() => {
        if (paneTab) void commands.tabsBack(paneTab.id);
      }}
      onforward={() => {
        if (paneTab) void commands.tabsForward(paneTab.id);
      }}
      onreload={() => {
        if (paneTab) void commands.tabsReload(paneTab.id);
      }}
      onopenbrowse={() => {
        if (paneTab) onopen(paneTab.id);
      }}
      onadd={() => {
        if (paneTab) void attach([paneTab.id]);
      }}
      onclose={closePane}
    />
  {/if}
  {#if takeoverView}
    <LazyView
      loader={loadTakeover}
      loadingLabel={m.surface_loading()}
      failureLabel={m.surface_render_failed()}
      retryLabel={m.surface_retry()}
      >{#snippet children(Takeover)}<Takeover
          host={takeoverView.current.host}
          url={takeoverView.current.url}
          bounds={takeoverView.bounds}
          page={takeoverView.page}
          error={takeoverError}
          onregion={takeoverRegion}
          oncontinue={continueTakeover}
          onclose={() => endTakeover(true)}
        />{/snippet}</LazyView
    >
  {/if}
  <div class="bar-dock" bind:clientHeight={dockHeight}>
    {#if blockSaveFailed}<p role="alert">{m.resource_conflict()}</p>{/if}
    {#if aiEnabled}<WorkBar
        bind:ref={composerElement}
        bind:surface={composerSurface}
        bind:value={session.composer}
        placeholder={lineRunning
          ? m.work_composer_steer()
          : runningObjective
            ? m.work_composer_continue()
            : m.work_composer_start()}
        disabled={objectivePending || !!session.objectiveSubmission}
        {busy}
        holding={panel === "attach"}
        {model}
        tools={barTools}
        attach={barAttach}
        context={openTabs || contextSel ? composerContext : undefined}
        onsubmit={() => void send()}
      >
        {#snippet trailing()}{#await loadModels() then picker}<picker.default
              profile={session.profile}
            />{/await}{/snippet}
      </WorkBar>{:else}<div class="tools-only" role="toolbar" aria-label={m.work_env_toolbar()}>
        {@render barTools()}
      </div>{/if}
  </div>
</div>

<style>
  .environment {
    position: relative;
    inline-size: 100%;
    block-size: 100%;
    min-inline-size: 0;
    min-block-size: 0;
    overflow: hidden;
    color: var(--color-text);
    font-size: var(--text-body);
  }

  /* Framed like a Browse page, 8px off every window edge; the bar below it
     stands on the window's own bottom edge, across that gap. */
  .canvas-card {
    position: absolute;
    inset: 0 0 8px;
    border-radius: var(--content-radius);
    background: var(--color-canvas);
    box-shadow: inset 0 0 0 1px var(--color-border);
    overflow: hidden;
  }

  /* The canvas keeps its cards' stacking to itself, so the edges can lie over them. */
  .canvas-card :global(.work-canvas) {
    isolation: isolate;
  }

  /* The canvas's edge fades to its own ground under the title and the bar. */
  .edge-scrim {
    position: absolute;
    inset-inline: 0;
    z-index: 4;
    block-size: 72px;
    pointer-events: none;
  }

  /* Solid only behind the title and the island, then a short, soft ease out. */
  .edge-scrim.top {
    inset-block-start: 0;
    block-size: 72px;
    background: linear-gradient(
      to bottom,
      var(--color-canvas) 0 34px,
      color-mix(in srgb, var(--color-canvas) 62%, transparent) 46px,
      color-mix(in srgb, var(--color-canvas) 20%, transparent) 60px,
      transparent 72px
    );
  }

  /* Under the bar, however tall its context makes it: solid where the bar stands, then a short ease out. */
  .edge-scrim.bottom {
    inset-block-end: 0;
    block-size: calc(var(--dock, 64px) + 40px);
    background: linear-gradient(
      to top,
      var(--color-canvas) 0 calc(var(--dock, 64px) * 0.5),
      color-mix(in srgb, var(--color-canvas) 62%, transparent) calc(var(--dock, 64px) + 8px),
      color-mix(in srgb, var(--color-canvas) 20%, transparent) calc(var(--dock, 64px) + 24px),
      transparent calc(var(--dock, 64px) + 40px)
    );
  }

  .canvas-top {
    position: absolute;
    inset-block-start: 12px;
    inset-inline: 12px 16px;
    z-index: 5;
    display: flex;
    align-items: center;
    gap: 16px;
    min-inline-size: 0;
    pointer-events: none;
  }

  .canvas-top-gap {
    flex: 1;
  }

  .tool-rule {
    flex: none;
    inline-size: 1px;
    block-size: 18px;
    margin-inline: 4px;
    background: var(--color-border);
  }

  /* The run's line stands at the top centre, where the eye goes first, and
     opens downward over the canvas. */
  .island {
    position: absolute;
    inset-block-start: -2px;
    inset-inline-start: 50%;
    inline-size: max-content;
    min-inline-size: 240px;
    max-inline-size: min(520px, calc(100% - 560px));
    translate: -50% 0;
    pointer-events: auto;
    transition:
      opacity var(--motion-base) var(--ease-out),
      visibility var(--motion-base);
  }

  /* The island stays mounted across a run's waits and resumes, so it fades
     rather than replaying an entrance each time its contents return. */
  .island.gone {
    opacity: 0;
    visibility: hidden;
    pointer-events: none;
  }

  @media (prefers-reduced-motion: reduce) {
    .island {
      transition: none;
    }
  }

  .zoom-slot {
    display: flex;
    flex: none;
    pointer-events: auto;
  }

  .bar-dock {
    position: absolute;
    inset-block-end: 0;
    inset-inline: 0;
    z-index: 30;
    display: flex;
    justify-content: center;
    pointer-events: none;
  }

  /* With AI off the bar is its tools alone. */
  .tools-only {
    display: flex;
    gap: 2px;
    padding: 8px;
    border-radius: var(--radius-panel) var(--radius-panel) 0 0;
    background: color-mix(in srgb, var(--color-float) var(--wash-raised), var(--color-canvas));
    box-shadow: var(--shadow-popover);
    pointer-events: auto;
  }

  .welcome {
    position: absolute;
    inset: 36% 24px auto;
    pointer-events: none;
    text-align: center;
  }

  .welcome p {
    margin: 0;
    color: var(--color-muted);
  }

  .status {
    display: flex;
    align-items: center;
    gap: 10px;
    min-inline-size: 0;
    color: var(--color-muted);
    font-size: var(--text-label);
    pointer-events: auto;
  }

  .lift-body {
    display: flex;
    flex-direction: column;
    gap: 16px;
    min-block-size: 0;
  }

  .lift-media {
    block-size: 100%;
  }

  .lift-picture {
    display: grid;
    flex: 1;
    place-items: center;
    min-block-size: 0;
  }

  .lift-picture img {
    max-inline-size: 100%;
    max-block-size: 100%;
    object-fit: contain;
    border-radius: var(--radius-row);
  }

  .lift-plain p {
    margin: 0;
    color: var(--color-muted);
  }
</style>
