<script lang="ts">
  import Icon from "$shared/ui/Icon";
  import type { CanvasItem, PartPage } from "../../lib/canvas-model";
  import { PART, partLead, rowKey, stackSize } from "../../lib/run/part-size";
  import {
    ArrowUpRight01Icon,
    CommandLineIcon,
    FileEditIcon,
    File01Icon,
    Search01Icon,
  } from "../../lib/icons";
  import { askCard, loadedView, loadView, partContent } from "./slots";
  import { needWords } from "../../lib/run/need";
  import { getContext, untrack } from "svelte";
  import { canvasBoard, type BoardActions } from "../../lib/canvas-context";
  import HostGlyph from "../cards/HostGlyph.svelte";
  import PageFace from "./PageFace.svelte";
  import NeedsYou from "./NeedsYou.svelte";
  import PartMark from "../presence/PartMark.svelte";
  import PartAgent from "../presence/PartAgent.svelte";
  import { resultLabel } from "../../lib/run/result-label";
  import * as m from "$shared/i18n/messages";

  let {
    item,
    selected,
    onopen,
    onlist,
    onneed,
    onwait,
  }: {
    item: CanvasItem;
    selected: boolean;
    /** Opens one of the part's pages in the centre. */
    onopen: (page: string) => void;
    /** Opens everything the part read or cited. */
    onlist: () => void;
    /** Meets the part's need: `sign_in` opens the site, `again` runs the part again. */
    onneed?: (how: "meet" | "again") => void;
    /** Keeps the run waiting on one of the part's pages a while longer. */
    onwait?: (page: string) => void;
  } = $props();

  const part = $derived(item.part!);
  const shape = $derived(part.shape);
  const waits = (page: PartPage) => page.human?.phase === "waiting_for_human";
  /** A page waiting on the person leads, then the page it is on now, then the newest. */
  const ordered = $derived(
    [...part.pages]
      .reverse()
      .sort((a, b) => Number(waits(b)) - Number(waits(a)) || Number(b.live) - Number(a.live)),
  );
  /** The front window, when the run waits on the person there. */
  const waiting = $derived(ordered[0] && waits(ordered[0]) ? ordered[0] : null);
  /** A middle dot stays with the words before it, so no line starts with one. */
  const keepDots = (text: string) => text.replaceAll(" · ", "\u00a0· ");
  const siteOf = (url: string) => {
    try {
      return new URL(url).hostname.replace(/^www\./u, "");
    } catch {
      return (part.host ?? "").replace(/^www\./u, "");
    }
  };
  const shown = $derived(ordered.slice(0, PART.shown));
  const stack = $derived(stackSize(part.pages.length));
  const working = $derived(part.state === "running" || part.state === "waiting");
  const cited = $derived(part.cited ?? []);
  const more = $derived(Math.max(0, (part.citedCount ?? 0) - cited.length));
  /** Results the run opened already stand as pages beside the list: it names only the rest, and counts those. */
  const opened = $derived(cited.filter((row) => row.opened).length);
  const passed = $derived(cited.filter((row) => !row.opened));
  /** Folded, each window behind the front one peeks out right and below it. */
  const placeOf = (index: number) =>
    `translate(${index * PART.behindX}px, ${index * PART.behindY}px)`;
  /** Fanned out on hover, the windows stand side by side along the row. */
  const fanOf = (index: number) => `translate(${index * (PART.window + 16)}px, 0)`;
  /** A helper's own view of its work, when its stream has built one. */
  const content = $derived(shape === "helper" ? partContent(part.helper) : null);
  const asking = $derived(part.ask ? askCard() : null);
  // Each view loads once and is drawn as soon as it is there, so a measure of the
  // row never takes it down.
  const Content = $derived(loadedView(content));
  const Asking = $derived(loadedView(asking));
  $effect(() => {
    loadView(content);
    loadView(asking);
  });
  const LINE = {
    read: File01Icon,
    write: FileEditIcon,
    command: CommandLineIcon,
    search: Search01Icon,
  } as const;
  const lines = $derived(part.lines ?? []);
  const need = $derived(part.need);
  /** A sign-in opened: what is left is to run the part again. */
  let signing = $state(false);
  const needSaid = $derived.by(() => {
    if (!need) return null;
    // A sign-in opened: what is left is to run the part again.
    if (need.kind === "sign_in" && !need.past && signing)
      return { text: m.work_need_signed_in({ site: need.target }), action: m.work_need_again() };
    return needWords(need, part.title, "row");
  });
  /** A check or a sign-in the part still needs the person for, as the same card a waiting page gets. */
  const asked = $derived(
    (need?.kind === "check" || need?.kind === "sign_in") &&
      !need.past &&
      !(need.kind === "sign_in" && signing),
  );
  function meet() {
    if (need?.kind === "sign_in" && !need.past && !signing) {
      signing = true;
      onneed?.("meet");
    } else
      onneed?.(need?.kind === "sign_in" || need?.kind === "retry" || need?.past ? "again" : "meet");
  }
  const board = getContext<BoardActions | undefined>(canvasBoard);
  // The row's own height: the taller of its name and its work, so rows never overlap.
  let labelBody = $state<HTMLElement>();
  let workBody = $state<HTMLElement>();
  $effect(() => {
    const label = labelBody;
    const work = workBody;
    if (!label) return;
    let reported = 0;
    const report = () => {
      const height = Math.ceil(Math.max(label.offsetHeight, work?.offsetHeight ?? 0));
      if (!height || height === reported) return;
      reported = height;
      untrack(() => board?.measure(rowKey(item.id), 0, false, height));
    };
    report();
    const observer = new ResizeObserver(() => requestAnimationFrame(report));
    observer.observe(label);
    if (work) observer.observe(work);
    return () => observer.disconnect();
  });
  /** A one-word name longer than its column is set smaller rather than cut. */
  const fitted = $derived.by(() => {
    const longest = Math.max(...part.title.split(/\s+/u).map((word) => word.length), 1);
    return Math.max(11, Math.min(13, Math.floor((PART.label - 23) / (longest * 0.56))));
  });
</script>

<section
  class="part work-drag-handle {shape}"
  class:label-only={shape === "label"}
  class:selected
  class:working
  aria-label={part.title}
  data-part={item.id}
>
  <div class="agent"><PartAgent {part} /></div>
  <div class="label" bind:this={labelBody}>
    <button
      type="button"
      class="name nodrag"
      style:--fitted="{fitted}px"
      onclick={(event) => {
        event.stopPropagation();
        onlist();
      }}
    >
      <PartMark {part} />
      <strong>{part.title}</strong>
    </button>
    {#if waiting?.human}<NeedsYou
        inline
        page={waiting.human}
        host={siteOf(waiting.url)}
        url={waiting.url}
        pending={part.pending}
        onopen={() => onopen(waiting.id)}
        onwait={onwait ? () => onwait?.(waiting.id) : undefined}
      />
    {:else if asked && need}<NeedsYou
        inline
        reason={need.kind === "sign_in" ? "sign_in" : "challenge"}
        host={siteOf(need.address ?? "")}
        url={need.address ?? ""}
        onopen={meet}
      />
    {:else if needSaid}<p class="summary need-text">{needSaid.text}</p>
      <button type="button" class="need nodrag nopan" onclick={meet}>{needSaid.action}</button>
    {:else if part.summary}<p class="summary" class:turn={part.state === "waiting"}>
        {keepDots(part.summary)}
      </p>{/if}
  </div>

  {#if shape === "ask" && part.ask}
    <div class="slot" style:inset-inline-start="{partLead}px" data-part-ask={item.id}>
      <div bind:this={workBody}>
        {#if Asking}<Asking {...part.ask.props} />{/if}
      </div>
    </div>
  {:else if shape === "helper"}
    <div class="slot" style:inset-inline-start="{partLead}px">
      <div bind:this={workBody}>
        {#if content}{#if Content}<Content
              id={item.id}
              {part}
              objective={part.objective ?? ""}
              steps={part.steps ?? []}
            />{/if}
        {:else}<ul class="lines">
            {#each lines.slice(0, PART.helperLines) as line, index (index)}<li class={line.kind}>
                <Icon icon={LINE[line.kind]} size={13} /><span>{line.text}</span>
              </li>{/each}
            {#if lines.length > PART.helperLines}<li class="rest">
                {m.work_part_lines_more({ count: lines.length - PART.helperLines })}
              </li>{/if}
          </ul>{/if}
      </div>
    </div>
  {:else if shape === "pages"}
    <div
      class="stack"
      class:fans={shown.length > 1}
      class:holding={!!waiting?.human}
      style:inset-inline-start="{partLead}px"
      style:inline-size="{stack.width}px"
      style:block-size="{stack.height}px"
    >
      {#each shown as page, index (page.id)}
        {@render window(page, index)}
      {/each}
      {#if part.pages.length > 1}<span class="count" style:inset-inline-start="{stack.width}px"
          >{part.pages.length}</span
        >{/if}
    </div>
  {:else if shape === "sources" && part.search}
    <div class="slot" style:inset-inline-start="{partLead}px">
      <ul class="results" bind:this={workBody}>
        {#if part.query}<li class="query" title={part.query}>
            <Icon icon={Search01Icon} size={13} /><span class="words">{part.query}</span>
            {#if part.queries}<span class="again">+{part.queries - 1}</span>{/if}
          </li>{/if}
        {#if opened}<li class="rest opened-count">
            <Icon icon={ArrowUpRight01Icon} size={12} />{m.work_part_opened_count({
              count: opened,
            })}
          </li>{/if}
        {#each passed as row (row.key)}{@const label = resultLabel(row, part.oneSite)}
          <li>
            <button
              type="button"
              class="row nodrag"
              title={row.title || row.url}
              onclick={(event) => {
                event.stopPropagation();
                onlist();
              }}
            >
              <span class="site-mark"
                ><HostGlyph host={row.where} url={row.url} size={14} initial={false} /></span
              >
              <span class="result">{label.text}</span>
              {#if label.site}<span class="where">{label.site}</span>{/if}
            </button>
          </li>{:else}{#if working && !opened}{#each [0, 1, 2] as index (index)}<li
                class="pending"
                style:inline-size="{100 - index * 18}%"
                style:--delay="{index * 120}ms"
                aria-hidden="true"
              ></li>{/each}{/if}{/each}
        {#if more}<li class="rest">{m.work_part_more_results({ count: more })}</li>{/if}
      </ul>
    </div>
  {:else if shape === "sources"}
    <div class="slot" style:inset-inline-start="{partLead}px">
      <ul class="cited" bind:this={workBody}>
        {#each cited as row (row.key)}<li>
            <button
              type="button"
              class="nodrag"
              title={row.title}
              onclick={(event) => {
                event.stopPropagation();
                onlist();
              }}
            >
              <span class="site-mark"
                ><HostGlyph host={row.where} url={row.url} size={14} initial={false} /></span
              >
              <span class="site">{row.where}</span>
              {#if row.title}<span class="read">{row.title}</span>{/if}
            </button>
          </li>{/each}
        {#if more}<li class="rest">{m.work_part_more_cited({ count: more })}</li>{/if}
      </ul>
    </div>
  {/if}
</section>

{#snippet window(page: PartPage, index: number)}
  <button
    type="button"
    class="page nodrag"
    class:behind={index > 0}
    style:--place={placeOf(index)}
    style:--fan={fanOf(index)}
    style:z-index={PART.shown - index}
    style:inline-size="{PART.window}px"
    style:block-size="{PART.bar + PART.frame}px"
    title={page.title || page.url}
    aria-label={page.title || page.url}
    onclick={(event) => {
      event.stopPropagation();
      onopen(page.id);
    }}
  >
    <PageFace
      url={page.url}
      title={page.title}
      frame={page.frame}
      host={part.host ?? ""}
      live={page.live}
      still={waits(page)}
    />
  </button>
{/snippet}

<style>
  /* The row's box is larger than what it draws: only what is drawn takes the pointer, so
     the empty space around the windows pans the canvas like any other. */
  .part {
    position: relative;
    box-sizing: border-box;
    inline-size: 100%;
    block-size: 100%;
    color: var(--color-text);
    pointer-events: none;
  }

  .label,
  .page,
  .slot > :global(*) {
    pointer-events: auto;
  }

  .label {
    position: absolute;
    inset-block-start: 0;
    inset-inline-start: 0;
    display: flex;
    flex-direction: column;
    gap: 3px;
    box-sizing: border-box;
    inline-size: 144px;
  }

  .label-only .label {
    inline-size: 100%;
  }

  .name {
    display: flex;
    align-items: flex-start;
    gap: 7px;
    min-inline-size: 0;
    padding: 3px 0;
    border: 0;
    border-radius: var(--radius-inset);
    background: transparent;
    color: inherit;
    font: inherit;
    text-align: start;
    cursor: default;
  }

  .name:focus-visible {
    outline: 2px solid var(--color-ring);
    outline-offset: 2px;
  }

  strong {
    font-size: var(--fitted, var(--text-body));
    font-weight: 600;
    line-height: 18px;
    letter-spacing: -0.005em;
    text-wrap: balance;
  }

  .summary {
    margin: 0;
    padding-inline-start: 23px;
    color: var(--color-muted);
    font-size: var(--text-label);
    line-height: 16px;
    text-wrap: pretty;

    /* A few lines at most: a long goal or query never runs down the canvas. */
    display: -webkit-box;
    overflow: hidden;
    overflow-wrap: anywhere;
    -webkit-box-orient: vertical;
    -webkit-line-clamp: 3;
    line-clamp: 3;
  }

  /* The helper that owns the part, at its top-right, over its work. */
  .agent {
    position: absolute;
    inset-block-end: calc(100% + 10px);
    inset-inline-end: 0;
    display: flex;
    justify-content: flex-end;
    max-inline-size: 100%;
  }

  .summary.turn {
    color: var(--color-text);
    font-weight: 600;
  }

  /* What the part needs, in a sentence, and the one control that meets it. */
  .need-text {
    color: var(--color-text);
  }

  /* The remedy is quiet: a control of the row, not a call to action over the canvas. */
  .need {
    align-self: flex-start;
    margin: 6px 0 0 23px;
    padding: 3px 11px;
    border: 0;
    border-radius: var(--radius-capsule);
    background: var(--color-control);
    color: var(--color-text);
    font: inherit;
    font-size: var(--text-label);
    font-weight: 600;
    line-height: 16px;
    cursor: default;
    transition: background-color var(--motion-fast) var(--ease-out);
  }

  .need:hover {
    background: var(--color-control-hover);
  }

  .need:focus-visible {
    outline: 2px solid var(--color-ring);
    outline-offset: 2px;
  }

  .part.selected .name {
    box-shadow: 0 0 0 1.5px var(--color-ring);
  }

  .slot {
    position: absolute;
    inset-block-start: 0;
    inset-inline-end: 0;
  }

  /* The part's pages as windows: the front one whole, the ones behind it peeking out. */
  .stack {
    position: absolute;
    inset-block-start: 0;
  }

  .page {
    position: absolute;
    inset-block-start: 0;
    inset-inline-start: 0;
    display: block;
    padding: 0;
    border: 0;
    border-radius: var(--radius-row);
    background: transparent;
    color: inherit;
    font: inherit;
    text-align: start;
    cursor: default;
    transform: var(--place);
    transition: transform var(--motion-slow) var(--ease-emphasized);
  }

  .page:focus-visible {
    outline: 2px solid var(--color-ring);
    outline-offset: 2px;
  }

  /* Hovering the stack fans its windows out along the row, over what follows. */
  .fans:hover .page {
    transform: var(--fan);
  }

  .fans .page:hover {
    translate: 0 -3px;
    transition:
      transform var(--motion-slow) var(--ease-emphasized),
      translate var(--motion-base) var(--ease-spring);
  }

  /* A lifted window keeps the strip it rose from: at its lower edge the pointer
     would otherwise fall off it, drop it, meet it again, and flicker. */
  .fans .page:hover::after {
    position: absolute;
    inset: 100% 0 -3px;
    content: "";
  }

  .count {
    position: absolute;
    inset-block-start: 0;
    z-index: 4;
    translate: -60% -40%;
    padding: 1px 7px;
    border-radius: var(--radius-capsule);
    background: var(--color-float);
    box-shadow: var(--shadow-raised);
    color: var(--color-text);
    font-size: var(--text-caption);
    font-variant-numeric: tabular-nums;
    font-weight: 600;
    line-height: 16px;
    transition: opacity var(--motion-fast) var(--ease-out);
  }

  .fans:hover .count {
    opacity: 0;
  }

  /* The windows stay folded while the run waits: the need in the row speaks for the front one. */
  .stack.holding .page,
  .stack.holding .page:hover {
    transform: var(--place);
    translate: none;
  }

  .stack.holding .count {
    opacity: 1;
  }

  .lines {
    display: flex;
    flex-direction: column;
    margin: 0;
    padding: 0;
    list-style: none;
  }

  .lines li {
    display: flex;
    align-items: center;
    gap: 8px;
    block-size: 24px;
    min-inline-size: 0;
    color: var(--color-muted);
    font-size: var(--text-label);
  }

  .lines span {
    overflow: hidden;
    color: var(--color-text);
    white-space: nowrap;
    mask-image: linear-gradient(to right, black calc(100% - 20px), transparent);
  }

  .lines .command span {
    font-family: var(--font-mono);
  }

  .lines .rest {
    padding-inline-start: 21px;
  }

  /* A search's results: the ones the run opened are pages beside it, so only the rest are listed. */
  .results {
    display: flex;
    flex-direction: column;
    inline-size: 320px;
    margin: 0;
    padding: 0;
    list-style: none;
  }

  /* What it searched for, set like the field it was typed into. */
  .query {
    display: flex;
    align-items: center;
    gap: 8px;
    block-size: 30px;
    margin-block-end: 4px;
    padding: 0 10px;
    border-radius: var(--radius-capsule);
    background: var(--color-fill);
    color: var(--color-muted);
  }

  .words {
    flex: 1;
    min-inline-size: 0;
    overflow: hidden;
    color: var(--color-text);
    font-size: var(--text-body);
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .query .again {
    flex: none;
    color: var(--color-muted);
    font-size: var(--text-label);
    font-variant-numeric: tabular-nums;
  }

  .row {
    display: flex;
    align-items: center;
    gap: 8px;
    inline-size: 100%;
    block-size: 28px;
    padding: 0 6px;
    border: 0;
    border-radius: var(--radius-inset);
    background: transparent;
    color: var(--color-text);
    font: inherit;
    text-align: start;
    cursor: default;
    transition: opacity var(--motion-base) var(--ease-out);
  }

  .row:hover {
    background: var(--color-control-hover);
  }

  .result {
    flex: 1;
    min-inline-size: 0;
    overflow: hidden;
    font-size: var(--text-body);
    white-space: nowrap;
    mask-image: linear-gradient(to right, black calc(100% - 24px), transparent);
  }

  .where {
    flex: none;
    max-inline-size: 40%;
    overflow: hidden;
    color: var(--color-muted);
    font-size: var(--text-label);
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  /* Results on their way: quiet bars that breathe until the engine answers. */
  .pending {
    block-size: 12px;
    margin: 8px 6px;
    border-radius: var(--radius-capsule);
    background: var(--color-fill);
    animation: breathe 1.4s var(--ease-in-out) var(--delay) infinite;
  }

  @keyframes breathe {
    50% {
      opacity: 0.45;
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .pending {
      animation: none;
    }
  }

  .cited {
    display: flex;
    flex-direction: column;
    inline-size: 320px;
    margin: 0;
    padding: 0;
    list-style: none;
  }

  .cited button {
    display: flex;
    align-items: center;
    gap: 8px;
    inline-size: 100%;
    block-size: 28px;
    padding: 0 6px;
    border: 0;
    border-radius: var(--radius-inset);
    background: transparent;
    color: inherit;
    font: inherit;
    text-align: start;
    cursor: default;
  }

  .cited button:hover {
    background: var(--color-control-hover);
  }

  .site-mark {
    display: grid;
    flex: none;
    place-items: center;
    inline-size: 16px;
    block-size: 16px;
  }

  .site {
    flex: none;
    max-inline-size: 60%;
    overflow: hidden;
    color: var(--color-text);
    font-size: var(--text-body);
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  /* The page it read there, quieter, fading at the column's edge. */
  .read {
    flex: 1;
    min-inline-size: 0;
    overflow: hidden;
    color: var(--color-muted);
    font-size: var(--text-label);
    white-space: nowrap;
    mask-image: linear-gradient(to right, black calc(100% - 24px), transparent);
  }

  .results .rest,
  .cited .rest {
    padding: 4px 6px 0 28px;
    color: var(--color-muted);
    font-size: var(--text-label);
  }

  .results .opened-count {
    display: flex;
    align-items: center;
    gap: 6px;
    margin-block-end: 2px;
    padding-inline-start: 6px;
  }
</style>
