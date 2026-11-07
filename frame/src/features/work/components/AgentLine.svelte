<script lang="ts">
  import { tick, untrack } from "svelte";
  import type { WorkSession } from "$domain/work";
  import { commands } from "$shared/ipc/bindings";
  import type { WorkHumanReasonV1, WorkModelFaultV1 } from "$shared/ipc/bindings";
  import { currentActivity } from "$domain/work";
  import { agentLine, endingFault, endingNote, isLive } from "../lib/agent-steps";
  import { dailyResetTime } from "../lib/daily-reset";
  import { cardCountdown } from "../lib/work-human";
  import type { WorkHumanSession } from "$domain/work-human";
  import RunAsks from "./asks/RunAsks.svelte";
  import { askPart, asksOf, openAsks } from "./asks/asks";
  import { fileName } from "../lib/work-files";
  import { effected } from "../lib/run/parts";
  import type { CanvasItem } from "../lib/canvas-model";
  import { Character, Orb, Shimmer, type Mood } from "$shared/ui/presence";
  import HostGlyph, { siteMark } from "./cards/HostGlyph.svelte";
  import Icon from "$shared/ui/Icon";
  import {
    ArrowDown01Icon,
    ArrowRight02Icon,
    ArrowUp02Icon,
    NoteAddIcon,
    StopIcon,
  } from "../lib/icons";
  import * as m from "$shared/i18n/messages";
  let {
    session,
    viewed,
    agents = [],
    draft = "",
    waiting = null,
    problem = null,
    ondismissproblem,
    writeup,
    onfocusagent,
    onwaitingpage,
    onreview,
    onsteered,
    human = null,
    onopenstep,
    working = [],
    ended = null,
  }: {
    /** The parts at work now, each with what it is doing, for the island's list. */
    working?: readonly {
      id: string;
      title: string;
      now: string;
      host?: string;
      helper?: "browser" | "research" | "computer" | "connection" | "lead";
    }[];
    /** Why the run in view ended short, in a person's words, and the one thing that helps. */
    ended?: { text: string; action: string; onact: () => void } | null;
    session: WorkSession;
    /** The run the person is looking at: the line speaks for it while nothing runs. */
    viewed?: string | undefined;
    /** The run's agent presences, as the canvas already projects them. */
    agents?: readonly CanvasItem[];
    /** What the person is typing while the agent runs. */
    draft?: string;
    /** The page card this run is held on, while a person is needed there. */
    waiting?: {
      card: string;
      host: string;
      reason: WorkHumanReasonV1;
      remaining: number;
      /** The wait is a short one the person may keep going. */
      canExtend?: boolean;
    } | null;
    /** One line about something the person asked for that did not land. */
    problem?: string | null;
    ondismissproblem?: () => void;
    /** A document result can be written up as the person's note: an action, not a follow-up. */
    writeup?: () => void;
    onfocusagent?: (id: string) => void;
    /** Pans to the waiting page card and focuses it. */
    onwaitingpage?: (card: string) => void;
    /** Opens the change the run is proposing, so the person can read it whole. */
    onreview?: (step: string) => void;
    /** The draft was handed to the agent or queued; clear the composer. */
    onsteered?: () => void;
    /** The work's held pages, for the asks a sign-in wall puts. */
    human?: WorkHumanSession | null;
    /** Opens the page an ask is about, in the centre. */
    onopenstep?: (step: string) => void;
  } = $props();
  const id = $props.id();
  const runtime = $derived(session.projection);
  const work = $derived(runtime?.work);
  const run = $derived(work ? session.operations.latest(work.id, "run") : undefined);
  const latest = $derived(runtime?.executions.at(-1));
  /** The live run, else the one in view, else the newest: never another run's words. */
  const execution = $derived(
    latest && runtime && !isLive(runtime, latest) && viewed
      ? (runtime.executions.find((entry) => entry.id === viewed) ?? latest)
      : latest,
  );
  /** Follow-ups and the next step belong to the newest run only. */
  const newest = $derived(execution === latest);
  const interrupted = $derived(!!execution && !!runtime?.interrupted.includes(execution.id));
  const live = $derived(!!runtime && !!execution && isLive(runtime, execution));
  const blocked = $derived(!!session.pending || !["ready", "rejected"].includes(session.delivery));
  const activity = $derived(
    runtime ? currentActivity(runtime, session.activity).at(-1)?.activity : undefined,
  );
  const readingHost = $derived.by(() => {
    const step = (execution?.steps ?? []).find(
      (step) =>
        step.status === "running" && (step.kind.kind === "read" || step.kind.kind === "discover"),
    );
    if (step?.kind.kind !== "read") return "";
    try {
      return new URL(step.kind.url).host;
    } catch {
      return "";
    }
  });
  /** What the agent is doing in a granted folder, when it is doing that. */
  const fileState = $derived.by(() => {
    for (const step of execution?.steps ?? []) {
      if (step.status !== "running") continue;
      switch (step.kind.kind) {
        case "list":
          return m.work_line_listing_files({ name: fileName(step.kind.path) });
        case "read_file":
          return m.work_line_reading_file({ name: fileName(step.kind.path) });
        case "search_files":
          return m.work_line_searching_files();
        case "write_file":
        case "edit_file":
          return step.kind.decision === undefined || step.kind.decision === null
            ? m.work_line_change_waiting({ name: fileName(step.kind.path) })
            : m.work_line_writing_file({ name: fileName(step.kind.path) });
      }
    }
    return "";
  });
  /** A proposed change to a file, waiting on the person's word. */
  const proposal = $derived.by(() => {
    if (!live) return undefined;
    for (const step of execution?.steps ?? []) {
      if (step.status !== "running") continue;
      const kind = step.kind;
      if (kind.kind !== "write_file" && kind.kind !== "edit_file") continue;
      if (kind.decision === undefined || kind.decision === null) return { id: step.id };
    }
    return undefined;
  });
  const activityStates: Record<string, () => string> = {
    planning: m.work_line_thinking,
    delegating: m.work_line_thinking,
    searching: m.work_line_searching,
    reading: m.work_line_reading_web,
    interacting: m.work_line_using_page,
    verifying: m.work_line_checking,
    recovering: m.work_line_recovering,
    comparing: m.work_line_comparing,
    producing_artifact: m.work_line_writing,
    paused: m.work_line_paused,
    waiting_for_approval: m.work_line_waiting,
    waiting_for_human: m.work_line_waiting_for_you,
    cancelling: m.work_line_stopping,
    finishing: m.work_line_finishing,
  };
  const questions = $derived(
    live
      ? (execution?.steps ?? []).flatMap((step) =>
          step.kind.kind === "ask" && step.status === "running"
            ? [{ id: step.id, prompt: step.kind.prompt, options: step.kind.options }]
            : [],
        )
      : [],
  );
  /**
   * A run that stopped on its question still asks it: the answer, or any
   * next message, resumes the work as its next request.
   */
  const stoppedQuestion = $derived.by(() => {
    if (live || !execution || !["cancelled", "failed", "interrupted"].includes(execution.status))
      return undefined;
    const step = (execution.steps ?? []).at(-1);
    if (step?.kind.kind !== "ask" || step.kind.answer || step.status === "succeeded")
      return undefined;
    return { id: step.id, prompt: step.kind.prompt, options: step.kind.options };
  });
  const openNow = $derived.by(() => {
    if (!execution || !work) return [];
    const held = human ? (human.pages.get(work.id) ?? []) : [];
    return openAsks(asksOf(execution, session.pages, held));
  });
  /** An ask with no place on the canvas stands in the island as a card; the line's own question gives way. */
  const asking = $derived(openNow.some((ask) => !askPart(ask)));
  /**
   * An ask on a site's card is answered there and only there; the island just
   * says so and points at it.
   */
  const askedOnCanvas = $derived.by(() => {
    for (const ask of openNow) {
      const card = askPart(ask);
      if (card)
        return { card, host: "host" in ask ? ask.host : "site" in ask ? ask.site : undefined };
    }
    return null;
  });
  const question = $derived(asking ? undefined : (questions.at(-1) ?? stoppedQuestion));
  const failure = $derived(
    run?.state.kind === "refused"
      ? run.state.error
      : run?.state.kind === "settled" && run.state.response.reply.kind === "error"
        ? run.state.response.reply.error
        : null,
  );
  const intervention = $derived(execution?.intervention ?? null);
  /** Why the agent stopped for a person, in the person's words. */
  const interventionLabel = $derived.by(() => {
    if (!intervention) return "";
    const origin = intervention.origin ?? "";
    switch (intervention.kind) {
      case "sign_in":
        return m.work_intervention_sign_in({ origin });
      case "challenge":
        return m.work_intervention_challenge({ origin });
      case "permission":
        return m.work_intervention_permission();
      case "unsupported_interaction":
        return m.work_intervention_unsupported_interaction();
      case "review":
        return m.work_intervention_review();
      case "human_takeover":
        return m.work_intervention_human_takeover();
    }
  });
  /** A finished run's own closing sentence, else the headline of what it made. */
  const closing = $derived.by(() => {
    if (!execution) return null;
    const said = agentLine(execution);
    if (said) return said;
    for (const artifact of execution.artifacts)
      if (artifact.data.kind === "reply") return artifact.data.headline;
    return execution.artifacts.find((artifact) => artifact.title.trim())?.title.trim() ?? null;
  });
  /** A request refused before it ran says why, never an older run's words. */
  const refusals: Record<string, () => string> = {
    capacity: m.work_line_full,
    conflict: m.work_line_busy,
    unavailable: m.work_line_unavailable,
    shutdown: m.work_line_unavailable,
    profile_unavailable: m.work_line_unavailable,
    outcome_unknown: m.work_line_unknown,
  };
  /**
   * A refused request says why for a moment, then the run's own words return:
   * it never stands for good over a run that answered. With no run to tell
   * instead, it stays.
   */
  const REFUSAL_MS = 6000;
  const refusal = $derived(failure ?? session.failure);
  const refusalKey = $derived(refusal ? `${run?.id ?? ""}:${refusal}` : null);
  let refusalTold = $state<string | null>(null);
  $effect(() => {
    const key = refusalKey;
    if (!key) return;
    const timer = setTimeout(() => (refusalTold = key), REFUSAL_MS);
    return () => clearTimeout(timer);
  });
  const refused = $derived(
    refusalKey && (!execution || refusalTold !== refusalKey) ? refusal : null,
  );
  /** Two or three words while it works; one quiet sentence once it stops. */
  const headline = $derived.by(() => {
    if (waiting) return m.work_line_waiting_on({ site: waiting.host });
    if (askedOnCanvas?.host) return m.work_line_waiting_on({ site: askedOnCanvas.host });
    if (askedOnCanvas) return m.work_line_waiting_for_you();
    if (intervention) return interventionLabel;
    if (refused) return refusals[refused]?.() ?? m.work_line_failed();
    if (interrupted) return m.work_line_interrupted();
    if (stoppedQuestion) return m.work_line_waiting_for_you();
    if (live) {
      if (fileState) return fileState;
      if (activity === "reading" && readingHost) return m.work_line_reading({ host: readingHost });
      if (activity) return activityStates[activity]?.() ?? m.work_line_thinking();
      return execution?.status === "cancel_requested"
        ? m.work_line_stopping()
        : m.work_line_thinking();
    }
    // A run that ended short says what is missing and what helps, never why the machine gave up.
    if (shortOf && newest) return shortOf.text;
    switch (execution?.status) {
      case "completed":
      case "needs_review":
        return closing ?? m.work_env_status_done();
      case "cancelled":
        return ending ?? m.work_line_stopped();
      case "interrupted":
        return ending ?? m.work_line_interrupted();
      case "failed":
        return ending ?? m.work_line_ended_short();
      default:
        return run?.state.kind === "pending" ? m.work_line_thinking() : m.work_line_ready();
    }
  });
  /** A model refusal the person can fix says so in their words, with the fix. */
  const fault = $derived(execution && !live ? endingFault(execution) : null);
  const faultLines: Partial<Record<WorkModelFaultV1, () => string>> = {
    hosted_usage: m.work_fault_hosted_usage,
    plan_required: m.work_fault_plan_required,
    billing_review: m.work_fault_billing_review,
    daily_limit: () => m.work_fault_daily_limit({ time: dailyResetTime() }),
    free_usage_paused: m.work_fault_free_usage_paused,
    hosted_busy: m.work_fault_hosted_busy,
    hosted_paused: m.work_fault_hosted_paused,
    signed_out: m.work_fault_signed_out,
    missing_key: m.work_fault_missing_key,
    key_refused: m.work_fault_key_refused,
    provider_credit: m.work_fault_provider_credit,
    plan_limit: m.work_fault_plan_limit,
  };
  const faultFix = $derived.by(() => {
    switch (fault) {
      case "hosted_usage":
        return { label: m.work_fault_fix_plan(), command: "settings.account" };
      case "plan_required":
      case "free_usage_paused":
        return { label: m.work_fault_fix_plans(), command: "settings.account" };
      case "billing_review":
        return { label: m.work_fault_fix_account(), command: "settings.account" };
      case "daily_limit":
        return { label: m.work_fault_fix_limit(), command: "settings.account" };
      case "hosted_paused":
        return { label: m.work_fault_fix_key(), command: "settings.ai" };
      case "signed_out":
        return { label: m.work_fault_fix_sign_in(), command: "settings.account" };
      case "missing_key":
      case "key_refused":
      case "provider_credit":
      case "plan_limit":
        return { label: m.work_model_manage(), command: "settings.ai" };
      default:
        return null;
    }
  });
  /** Why the run ended early, in Rust's closed words: the note its last unfinished step left. */
  const ending = $derived(
    (fault && faultLines[fault]?.()) ?? (execution ? endingNote(execution) : null),
  );
  /**
   * A part's need speaks for the run only when the run gave no answer: an
   * answered run says what it found and still offers the part's one remedy.
   */
  const shortOf = $derived(
    ended &&
      !(
        closing &&
        !!execution?.artifacts.length &&
        (execution.status === "completed" || execution.status === "needs_review")
      )
      ? ended
      : null,
  );
  const settled = $derived(!live && !question && !proposal);
  /** A short run's one remedy, or Try again for one that simply stopped. */
  const remedy = $derived.by(() => {
    if (live || !newest || !execution || question) return null;
    if (faultFix)
      return {
        label: faultFix.label,
        run: () => void commands.runCommand(faultFix.command),
      };
    if (ended) return { label: ended.action, run: ended.onact };
    // Never a whole new try once an approved send or purchase may have happened.
    if (["failed", "interrupted"].includes(execution.status) && !execution.steps?.some(effected))
      return {
        label: m.work_need_again(),
        run: () => void session.continueWith(m.work_line_try_again_request()),
      };
    return null;
  });
  const mark = $derived(
    waiting || question || proposal || asking || askedOnCanvas
      ? ("waiting" as const)
      : live
        ? ("live" as const)
        : execution?.status === "completed" || execution?.status === "needs_review"
          ? shortOf
            ? ("stopped" as const)
            : ("done" as const)
          : execution && ["failed", "cancelled", "interrupted"].includes(execution.status)
            ? ("stopped" as const)
            : ("idle" as const),
  );
  /** What the lead is doing, as its face shows it. */
  const doing = $derived.by((): Mood => {
    if (fileState)
      return (execution?.steps ?? []).some(
        (step) =>
          step.status === "running" &&
          (step.kind.kind === "write_file" || step.kind.kind === "edit_file"),
      )
        ? "working"
        : "reading";
    switch (activity) {
      case "searching":
        return "searching";
      case "reading":
        return "reading";
      case "interacting":
      case "producing_artifact":
        return "working";
      default:
        return "thinking";
    }
  });
  /**
   * While it works the line says what the lead does: what it reads or thinks
   * itself, or which parts it is coordinating. Each helper says its own on
   * the canvas.
   */
  const liveWords = $derived(
    fileState || working.length === 0
      ? headline
      : m.work_line_working_on({
          parts: new Intl.ListFormat(undefined, { type: "conjunction" }).format(
            working.map((part) => part.title),
          ),
        }),
  );
  /** At work: the indicator and one shimmering line stand where the lead's face is. */
  const thinking = $derived(mark === "live" && !waiting);
  const face = $derived<Mood>(
    mark === "waiting"
      ? "waiting"
      : mark === "live"
        ? doing
        : mark === "done"
          ? "done"
          : mark === "stopped"
            ? "stopped"
            : "rest",
  );
  const helperOf = (part: (typeof working)[number]) =>
    part.helper ?? (part.host ? "browser" : "research");
  const HELPER_MOOD: Record<string, Mood> = {
    browser: "reading",
    research: "searching",
    computer: "working",
    connection: "working",
  };
  const followups = $derived(settled && newest ? session.followups.slice(0, 3) : []);
  const writeupOffer = $derived(settled && newest ? writeup : undefined);
  const nextRows = $derived(followups.length > 0 || !!writeupOffer);
  const preview = $derived(draft.trim().slice(0, 60));

  /** What the person opened the capsule for; a question takes it whenever it is open. */
  let want = $state<"agents" | "answer" | "next" | null>(null);
  /** The clipped words opened where they stand: the line itself grows to hold them. */
  let whole = $state(false);
  let answer = $state("");
  let host = $state<HTMLElement>();
  /** The headline is one line; when it is clipped its words open the whole of it. */
  let words = $state<HTMLElement>();
  let clipped = $state(false);
  const panel = $derived(
    want === null
      ? null
      : want === "agents"
        ? "agents"
        : question
          ? "question"
          : want === "next" && nextRows
            ? "next"
            : null,
  );
  const expanded = $derived(panel !== null);
  /** What the capsule holds while it closes, so it shrinks around its rows, not around nothing. */
  let held = $state<NonNullable<typeof panel> | null>(null);
  $effect(() => {
    if (panel) held = panel;
  });
  $effect(() => {
    if (want && !panel) want = null;
  });
  $effect(() => {
    void headline;
    whole = false;
  });
  $effect(() => {
    void headline;
    const element = words;
    if (!element || whole) return;
    const measure = () => (clipped = element.scrollWidth > element.clientWidth);
    void tick().then(measure);
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    return () => observer.disconnect();
  });
  $effect(() => {
    if (!live && want === "agents") untrack(() => (want = null));
  });
  // The oldest queued message rides on as soon as the run in flight closes.
  let sending = false;
  $effect(() => {
    if (live || blocked || !session.queue.length || sending) return;
    sending = true;
    void untrack(() => session.sendQueued()).finally(() => (sending = false));
  });
  async function submitAnswer() {
    const text = answer.trim();
    if (!text || !execution || !question || blocked) return;
    const sent = stoppedQuestion
      ? await session.continueWith(text)
      : await session.answerStep(execution.id, question.id, text);
    if (sent) {
      answer = "";
      want = null;
    }
  }
  async function steer() {
    const text = draft.trim();
    if (!text) return;
    if (!(await session.steer(text))) session.enqueue(text);
    onsteered?.();
  }
  async function stop() {
    if (!execution || blocked) return;
    await session.execute({ kind: "cancel", execution: execution.id });
  }
</script>

<svelte:window
  onpointerdown={(event) => {
    if (expanded && event.target instanceof Node && host && !host.contains(event.target))
      want = null;
  }}
  onkeydown={(event) => {
    if (event.key !== "Escape" || !expanded) return;
    event.stopPropagation();
    want = null;
  }}
/>

{#if work}
  <section class="agent-line" class:settled bind:this={host} aria-label={m.work_agent_line()}>
    <!-- One capsule at the canvas's top: it grows down into its list and folds back into the line. -->
    <div class="capsule" class:open={expanded}>
      <div class="expand" class:shown={expanded} aria-hidden={!expanded} inert={!expanded}>
        <div class="expand-inner">
          {#if held === "agents"}
            <ul class="rows" aria-label={m.work_line_agents()}>
              {#each working as part (part.id)}
                <li>
                  <button
                    type="button"
                    class="row"
                    onclick={() => {
                      want = null;
                      onfocusagent?.(part.id);
                    }}
                  >
                    <span class="row-mark"
                      ><Character
                        kind={helperOf(part)}
                        mood={HELPER_MOOD[helperOf(part)]}
                        size={22}
                      />{#if part.host && siteMark(part.host)}<span class="row-badge"
                          ><HostGlyph host={part.host} size={10} initial={false} /></span
                        >{/if}</span
                    >
                    <span class="said">
                      <span class="who">{part.title}</span>
                      {#if part.now}<span class="doing">{part.now}</span>{/if}
                    </span>
                  </button>
                </li>
              {:else}<li class="none">{m.work_line_no_agents()}</li>{/each}
            </ul>
          {:else if held === "next"}
            <ul class="rows" aria-label={m.work_line_next()}>
              {#each followups as followup, index (index)}
                <li>
                  <button
                    type="button"
                    class="row"
                    disabled={blocked}
                    onclick={() => {
                      want = null;
                      void session.continueWith(followup);
                    }}><span>{followup}</span><Icon icon={ArrowRight02Icon} size={13} /></button
                  >
                </li>
              {/each}
              {#if writeupOffer}{#if followups.length}<li class="rule" aria-hidden="true"></li>{/if}
                <li>
                  <button
                    type="button"
                    class="row writeup"
                    onclick={() => {
                      want = null;
                      writeupOffer?.();
                    }}
                    ><Icon icon={NoteAddIcon} size={14} /><span>{m.work_line_write_note()}</span
                    ></button
                  >
                </li>{/if}
            </ul>
          {:else if held === "question" && question}
            <form
              onsubmit={(event) => {
                event.preventDefault();
                void submitAnswer();
              }}
            >
              <label for={`${id}-answer`}>{question.prompt}</label>
              <div class="options">
                {#each question.options as option, index (index)}
                  <button
                    type="button"
                    class="chip"
                    disabled={blocked}
                    onclick={() => (answer = option)}>{option}</button
                  >
                {/each}
              </div>
              <div class="answer">
                <input
                  id={`${id}-answer`}
                  maxlength="8192"
                  placeholder={m.work_line_answer_placeholder()}
                  bind:value={answer}
                  disabled={blocked}
                />
                <button
                  type="submit"
                  class="send"
                  aria-label={m.work_line_send_answer()}
                  disabled={blocked || !answer.trim()}
                >
                  <Icon icon={ArrowUp02Icon} size={15} strokeWidth={2} />
                </button>
              </div>
            </form>
          {/if}
        </div>
      </div>
      {#if asking}<div class="ask">
          <RunAsks
            {session}
            {human}
            placement="island"
            seed={agents[0]?.agent?.seed ?? 0}
            homeless
            onopenpage={(step) => onopenstep?.(step)}
          />
        </div>{/if}
      <div class="line">
        <button
          type="button"
          class="avatar"
          class:working={thinking}
          aria-expanded={panel === "agents"}
          aria-label={m.work_line_agents()}
          onclick={() => (want = want === "agents" ? null : "agents")}
        >
          {#if thinking}<Orb size={20} label={m.work_agent_line()} />{:else}<Character
              kind="lead"
              mood={face}
              size={22}
              label={m.work_agent_line()}
            />{/if}
        </button>
        <div class="state">
          {#if waiting}
            <button
              type="button"
              class="words waiting"
              onclick={() => onwaitingpage?.(waiting.card)}
            >
              <span class="said-title">{headline}</span>{#if cardCountdown(waiting.remaining)}<span
                  class="left">{cardCountdown(waiting.remaining)}</span
                >{/if}
            </button>
          {:else if problem}
            <span class="problem" role="alert"
              ><button
                type="button"
                title={m.work_line_dismiss()}
                onclick={() => ondismissproblem?.()}>{problem}</button
              ></span
            >
          {:else}
            {#key headline}
              {#if clipped || whole}
                <button
                  type="button"
                  class="words more"
                  class:whole
                  aria-expanded={whole}
                  onclick={() => (whole = !whole)}
                  ><span class="text" bind:this={words}>{headline}</span><span
                    class="turn"
                    aria-hidden="true"><Icon icon={ArrowDown01Icon} size={12} /></span
                  ></button
                >
              {:else if thinking}
                <span class="words"
                  ><span class="text" bind:this={words}><Shimmer text={liveWords} /></span></span
                >
              {:else}
                <span class="words"><span class="text" bind:this={words}>{headline}</span></span>
              {/if}
            {/key}
          {/if}
          {#if live && preview}<span class="draft">{preview}</span>{/if}
        </div>
        <div class="controls">
          {#if live && preview}
            <button
              type="button"
              class="action"
              title={m.work_line_steer_hint()}
              onclick={() => void steer()}>{m.work_line_steer()}</button
            >
          {:else if waiting || askedOnCanvas}
            <button
              type="button"
              class="action"
              onclick={() => onwaitingpage?.(waiting?.card ?? askedOnCanvas?.card ?? "")}
              >{m.work_line_show()}</button
            >
          {:else if proposal}
            <button
              type="button"
              class="action"
              disabled={blocked}
              onclick={() => onreview?.(proposal.id)}>{m.work_line_review()}</button
            >
          {:else if question && !expanded}
            <button type="button" class="action" onclick={() => (want = "answer")}
              >{m.work_line_answer()}</button
            >
          {:else if remedy}
            <button type="button" class="action" disabled={blocked} onclick={() => remedy?.run()}
              >{remedy.label}</button
            >
          {:else if nextRows}
            <!-- The disclosure of the capsule's growth, not a second surface. -->
            <button
              type="button"
              class="action disclosure"
              aria-expanded={panel === "next"}
              onclick={() => (want = want === "next" ? null : "next")}
              >{m.work_line_next()}<Icon icon={ArrowDown01Icon} size={12} /></button
            >
          {/if}
          {#if live}
            <button
              type="button"
              class="quiet"
              aria-label={m.work_line_stop()}
              title={m.work_line_stop()}
              disabled={blocked}
              onclick={() => void stop()}
            >
              <Icon icon={StopIcon} size={14} />
            </button>
          {/if}
        </div>
      </div>
    </div>
  </section>
{/if}

<style>
  .ask {
    padding: 8px 8px 0;
  }

  .agent-line {
    display: flex;
    flex-direction: column;
    min-inline-size: 0;
  }

  /* The island: a pill while it is one line (the panel radius clamps to half
     its height), a sheet once it holds rows. Its line stays on top and what it
     opens grows down from it, the way the island at the top of a phone does.
     It is solid: a glass would blur the canvas again every frame a helper moves. */
  .capsule {
    display: flex;
    flex-direction: column-reverse;
    min-inline-size: 0;
    border-radius: var(--radius-panel);
    background: color-mix(in srgb, var(--color-float) var(--wash-raised), var(--color-canvas));
    box-shadow: var(--shadow-menu);
  }

  /* Grows on the arrival curve, folds on the exit curve; the rows fade with it. */
  .expand {
    display: grid;
    grid-template-rows: 0fr;
    transition: grid-template-rows var(--motion-base) var(--ease-exit);
  }

  .expand.shown {
    grid-template-rows: 1fr;
    transition-timing-function: var(--ease-emphasized);
  }

  .expand-inner {
    min-block-size: 0;
    overflow: hidden;
    opacity: 0;
    transition: opacity var(--motion-fast) var(--ease-exit);
  }

  .shown .expand-inner {
    opacity: 1;
    transition: opacity var(--motion-base) var(--ease-out);
  }

  .line {
    display: flex;
    align-items: center;
    gap: 10px;
    box-sizing: border-box;
    min-block-size: 36px;
    padding: 0 6px 0 7px;
  }

  .capsule.open .line {
    border-block-end: 1px solid var(--color-border);
  }

  .avatar {
    display: grid;
    place-items: center;
    flex: none;
    inline-size: 24px;
    block-size: 24px;
    padding: 0;
    border: 0;
    border-radius: var(--radius-capsule);
    background: transparent;
    cursor: default;
    transition: scale var(--motion-base) var(--ease-spring);
  }

  .avatar:focus-visible {
    outline: 2px solid var(--color-ring);
    outline-offset: 2px;
  }

  .avatar:active {
    scale: 0.94;
  }

  .state {
    display: flex;
    align-items: baseline;
    gap: 8px;
    flex: 1;
    min-inline-size: 0;
    font-size: var(--text-label);
  }

  .words {
    display: flex;
    align-items: baseline;
    min-inline-size: 0;
    max-inline-size: 100%;
    animation: line-in var(--motion-base) var(--ease-out);
  }

  .words .text {
    min-inline-size: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  button.more {
    align-items: center;
    gap: 4px;
    padding: 0;
    border: 0;
    background: transparent;
    color: inherit;
    font: inherit;
    font-size: var(--text-label);
    cursor: default;
  }

  button.more:focus-visible {
    outline: 2px solid var(--color-ring);
    outline-offset: 2px;
  }

  button.more :global(svg),
  .disclosure :global(svg) {
    flex: none;
    transition: rotate var(--motion-base) var(--ease-emphasized);
  }

  button.more :global(svg) {
    color: var(--color-muted);
  }

  button.more[aria-expanded="true"] :global(svg) {
    rotate: 180deg;
  }

  /* Opened in place: the words wrap and the island grows to hold them. */
  button.more.whole {
    align-items: flex-start;
    padding-block: 8px;
    text-align: start;
  }

  button.more.whole .text {
    overflow: visible;
    white-space: normal;
    line-height: 16px;
  }

  .turn {
    display: grid;
    flex: none;
    block-size: 16px;
    place-items: center;
  }

  .settled .words {
    color: var(--color-muted);
  }

  /* A line that points somewhere reads as a link, not as another button. */
  button.waiting {
    display: inline-flex;
    min-inline-size: 0;
    max-inline-size: 100%;
    align-items: baseline;
    gap: 6px;
    padding: 0;
    border: 0;
    background: transparent;
    color: var(--color-text);
    font: inherit;
    font-size: var(--text-label);
    cursor: default;
  }

  /* It pans to the page; the lit button beside it is the action. */
  .said-title {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    text-decoration: underline;
    text-decoration-color: transparent;
    text-underline-offset: 3px;
    transition: text-decoration-color var(--motion-fast) var(--ease-out);
  }

  button.waiting:hover .said-title {
    text-decoration-color: var(--color-border-strong);
  }

  /* What the person asked for and did not land: said once, gone when read. */
  .problem {
    display: flex;
    min-inline-size: 0;
  }

  .problem button {
    min-inline-size: 0;
    padding: 0;
    border: 0;
    background: transparent;
    color: var(--color-text);
    font: inherit;
    font-size: var(--text-label);
    text-align: start;
    cursor: default;
  }

  .problem button:focus-visible {
    outline: 2px solid var(--color-ring);
    outline-offset: 2px;
  }

  button.waiting .left {
    color: var(--color-muted);
    flex: none;
    font-variant-numeric: tabular-nums;
  }

  .draft {
    min-inline-size: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--color-faint);
  }

  .controls {
    display: flex;
    align-items: center;
    gap: 4px;
    flex: none;
  }

  .action {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    block-size: 26px;
    padding: 0 12px;
    border: 0;
    border-radius: var(--radius-capsule);
    background: var(--color-lit);
    color: var(--color-on-lit);
    font: inherit;
    font-size: var(--text-label);
    font-weight: 550;
    cursor: default;
    transition: background-color var(--motion-fast) var(--ease-out);
  }

  .action:focus-visible {
    outline: 2px solid var(--color-ring);
    outline-offset: 2px;
  }

  .action:disabled {
    background: var(--color-control);
    color: var(--color-faint);
  }

  .action:hover:not(:disabled) {
    background: var(--color-lit-hover);
  }

  /* Next opens the capsule upward: its chevron points where the rows will be. */
  .action.disclosure {
    padding-inline-end: 10px;
    background: var(--color-control);
    color: var(--color-text);
  }

  .action.disclosure:hover:not(:disabled) {
    background: var(--color-control-hover);
  }

  .disclosure :global(svg) {
    rotate: 180deg;
  }

  .disclosure[aria-expanded="true"] :global(svg) {
    rotate: 0deg;
  }

  .quiet {
    display: grid;
    place-items: center;
    inline-size: 26px;
    block-size: 26px;
    border: 0;
    border-radius: var(--radius-capsule);
    background: transparent;
    color: var(--color-muted);
    cursor: default;
    transition:
      background-color var(--motion-fast) var(--ease-out),
      color var(--motion-fast) var(--ease-out);
  }

  .quiet:focus-visible {
    outline: 2px solid var(--color-ring);
    outline-offset: 2px;
  }

  .quiet:hover:not(:disabled) {
    background: var(--color-control-hover);
    color: var(--color-text);
  }

  .rows {
    display: flex;
    flex-direction: column;
    gap: 2px;
    max-block-size: 220px;
    margin: 0;
    padding: 6px;
    overflow: auto;
    list-style: none;
  }

  .row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 10px;
    inline-size: 100%;
    box-sizing: border-box;
    min-block-size: 32px;
    padding: 6px 10px;
    border: 0;
    border-radius: var(--radius-row);
    background: transparent;
    color: var(--color-text);
    font: inherit;
    font-size: var(--text-label);
    text-align: start;
    cursor: default;
    transition: background-color var(--motion-fast) var(--ease-out);
  }

  .row > :global(svg) {
    flex: none;
    color: var(--color-muted);
  }

  .row:focus-visible {
    outline: 2px solid var(--color-ring);
    outline-offset: -2px;
  }

  .row:disabled {
    color: var(--color-faint);
  }

  .rule {
    block-size: 1px;
    margin: 3px 10px;
    background: var(--color-border);
  }

  /* An action of the frontend's own, not words for the agent: its glyph leads. */
  .writeup {
    justify-content: flex-start;
  }

  .writeup span {
    flex: 1;
    text-align: start;
  }

  .row:hover:not(:disabled) {
    background: var(--row-hover);
  }

  .row:active:not(:disabled) {
    background: var(--row-pressed);
  }

  /* A helper at work, the way a colleague is listed: who, then what it is doing, whole. */
  .said {
    display: flex;
    flex: 1;
    flex-direction: column;
    gap: 1px;
    min-inline-size: 0;
  }

  .who {
    font-weight: 550;
  }

  .row-mark {
    position: relative;
    display: grid;
    flex: none;
    place-items: center;
    inline-size: 24px;
    block-size: 24px;
  }

  .row-badge {
    position: absolute;
    inset-block-end: -2px;
    inset-inline-end: -3px;
    display: grid;
    place-items: center;
    inline-size: 13px;
    block-size: 13px;
    border-radius: 50%;
    background: var(--color-float);
  }

  .doing {
    color: var(--color-muted);
    line-height: 16px;
  }

  .none {
    padding: 6px 10px;
    overflow: hidden;
    color: var(--color-muted);
    font-size: var(--text-label);
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  form {
    display: flex;
    flex-direction: column;
    gap: 8px;
    margin: 0;
    padding: 12px 14px;
  }

  label {
    font-size: var(--text-label);
  }

  .options {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    min-inline-size: 0;
  }

  .chip {
    max-inline-size: 100%;
    block-size: 26px;
    padding: 0 12px;
    border: 0;
    border-radius: var(--radius-capsule);
    background: var(--color-control);
    color: var(--color-text);
    font: inherit;
    font-size: var(--text-label);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    cursor: default;
    transition: background-color var(--motion-fast) var(--ease-out);
  }

  .chip:focus-visible {
    outline: 2px solid var(--color-ring);
    outline-offset: 2px;
  }

  .chip:disabled {
    color: var(--color-faint);
  }

  .chip:hover:not(:disabled) {
    background: var(--color-control-hover);
  }

  .answer {
    display: flex;
    align-items: center;
    gap: 6px;
  }

  input {
    flex: 1;
    min-inline-size: 0;
    box-sizing: border-box;
    block-size: 30px;
    padding: 0 10px;
    border: 0;
    border-radius: var(--radius-control);
    background: var(--color-field);
    color: var(--color-text);
    font: inherit;
    font-size: var(--text-label);
    outline: none;
  }

  input::placeholder {
    color: var(--color-faint);
  }

  input:focus-visible {
    box-shadow: var(--shadow-field-focus);
  }

  .send {
    display: grid;
    place-items: center;
    flex: none;
    inline-size: 28px;
    block-size: 28px;
    border: 0;
    border-radius: var(--radius-capsule);
    background: var(--color-lit);
    color: var(--color-on-lit);
    cursor: default;
  }

  .send:focus-visible {
    outline: 2px solid var(--color-ring);
    outline-offset: 2px;
  }

  .send:disabled {
    background: var(--color-control);
    color: var(--color-faint);
  }

  @keyframes line-in {
    from {
      opacity: 0;
    }
  }
</style>
