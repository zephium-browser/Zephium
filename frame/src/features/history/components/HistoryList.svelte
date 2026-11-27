<script lang="ts">
  import { Delete02Icon, Globe02Icon } from "@hugeicons/core-free-icons";
  import FavIcon from "$shared/ui/FavIcon";
  import IconButton from "$shared/ui/IconButton";
  import { favicons } from "$domain/favicons";
  import * as m from "$shared/i18n/messages";
  import { hostOf, matchRange, timeLabel, toRows, visitedAt } from "../lib/history-model";
  import { createVirtualWindow } from "$shared/lib/virtual-window.svelte";
  import { everyWhileVisible } from "$shared/lib/visible-interval";
  import type { HistorySession } from "../lib/history-session.svelte";

  let {
    session,
    density = "page",
    onopen,
  }: {
    session: HistorySession;
    density?: "page" | "panel";
    onopen: (url: string, newTab: boolean) => void;
  } = $props();

  const heights = $derived(density === "page" ? { day: 38, visit: 44 } : { day: 32, visit: 34 });

  let scroller: HTMLElement | undefined = $state();
  let selected = $state<string | null>(null);

  let today = $state(new Date());
  let rows = $derived(
    toRows(session.visits, today, { today: m.history_today(), yesterday: m.history_yesterday() }),
  );
  const virtual = createVirtualWindow({
    heights: () => rows.map((row) => (row.kind === "day" ? heights.day : heights.visit)),
  });
  let slice = $derived(virtual.window);
  let visible = $derived(rows.slice(slice.first, slice.last));

  // A day boundary while the page is open must not leave "Today" on yesterday.
  $effect(() =>
    everyWhileVisible(60_000, () => {
      const now = new Date();
      if (now.getDate() !== today.getDate()) today = now;
    }),
  );

  function onscroll(event: Event & { currentTarget: HTMLElement }) {
    const { scrollTop, scrollHeight, clientHeight } = event.currentTarget;
    if (scrollTop + clientHeight >= scrollHeight - clientHeight) void session.reload(true);
  }

  /** Rows a page key covers, from the viewport rather than a fixed guess. */
  function pageStep() {
    const viewport = scroller?.clientHeight ?? 0;
    return Math.max(1, Math.floor(viewport / heights.visit) - 1);
  }

  function select(position: number) {
    const visits = rows.filter((row) => row.kind === "visit");
    if (!visits.length) return;
    const next = Math.min(visits.length - 1, Math.max(0, position));
    const target = visits[next];
    if (!target) return;
    selected = target.id;
    // A jump lands outside the drawn window, where the row has no element for
    // scrollIntoView to find. Carry the day heading with the row it opens, so
    // a jumped-to visit is never stranded under a heading left off-screen.
    const index = rows.indexOf(target);
    virtual.scrollToIndex(rows[index - 1]?.kind === "day" ? index - 1 : index);
  }

  function move(offset: number) {
    const visits = rows.filter((row) => row.kind === "visit");
    const at = visits.findIndex((row) => row.id === selected);
    select(at < 0 ? 0 : at + offset);
  }

  function keydown(event: KeyboardEvent) {
    if (event.target instanceof Element && event.target.closest("button")) return;
    const jumps: Record<string, () => void> = {
      ArrowDown: () => move(1),
      ArrowUp: () => move(-1),
      PageDown: () => move(pageStep()),
      PageUp: () => move(-pageStep()),
      Home: () => select(0),
      End: () => select(Number.MAX_SAFE_INTEGER),
    };
    const jump = jumps[event.key];
    if (jump) {
      event.preventDefault();
      jump();
      return;
    }
    const row = rows.find((candidate) => candidate.id === selected);
    if (!row || row.kind !== "visit") return;
    if (event.key === "Enter") {
      event.preventDefault();
      onopen(row.visit.url, event.metaKey || event.ctrlKey);
    } else if (event.key === "Backspace" || event.key === "Delete") {
      event.preventDefault();
      void session.forget([row.visit.url]);
    }
  }
</script>

<div
  class="scroller"
  class:scrolls={density === "page"}
  bind:this={scroller}
  data-density={density}
  use:virtual.attach
  {onscroll}
  onkeydown={keydown}
  role="listbox"
  aria-label={m.browser_history_title()}
  aria-activedescendant={selected ?? undefined}
  tabindex="0"
>
  <div class="spacer" style:block-size={`${slice.before}px`}></div>
  {#each visible as row, offset (row.id)}
    {#if row.kind === "day"}
      <h2
        class="day"
        style:block-size={`${heights.day}px`}
        data-virtual-index={slice.first + offset}
      >
        {row.label}<span class="count">{row.count}</span>
      </h2>
    {:else}
      {@const at = visitedAt(row.visit)}
      {@const range = matchRange(row.visit.title, session.query)}
      {@const site = favicons.mark(row.visit.icon, row.visit.url)}
      <div
        class="visit"
        class:selected={selected === row.id}
        data-row={row.id}
        data-virtual-index={slice.first + offset}
        id={row.id}
        style:block-size={`${heights.visit}px`}
        role="option"
        aria-selected={selected === row.id}
        tabindex="-1"
        onpointermove={() => (selected = row.id)}
        onclick={(event) => onopen(row.visit.url, event.metaKey || event.ctrlKey)}
        onkeydown={(event) => {
          if (event.target === event.currentTarget && event.key === " ") {
            event.preventDefault();
            onopen(row.visit.url, false);
          }
        }}
      >
        <FavIcon
          image={site?.image ?? null}
          tone={site?.tone}
          size={16}
          lit
          fallback={Globe02Icon}
        />
        <span class="title"
          >{#if range}{row.visit.title.slice(0, range[0])}<mark
              >{row.visit.title.slice(range[0], range[1])}</mark
            >{row.visit.title.slice(range[1])}{:else}{row.visit.title}{/if}</span
        >
        <span class="host">{hostOf(row.visit.url)}</span>
        <time datetime={at.toISOString()}>{timeLabel(at)}</time>
        <span class="forget"
          ><IconButton
            icon={Delete02Icon}
            label={m.history_forget()}
            size={14}
            buttonSize={22}
            onclick={(event) => {
              event.stopPropagation();
              void session.forget([row.visit.url]);
            }}
          /></span
        >
      </div>
    {/if}
  {/each}
  <div class="spacer" style:block-size={`${slice.after}px`}></div>
  {#if session.capped}
    <p class="notice">{m.history_capped()}</p>
  {:else if !session.exhausted}
    <button type="button" class="more" onclick={() => void session.reload(true)}
      >{m.history_more()}</button
    >
  {/if}
</div>

<style>
  /* The scroller is the pane, not the column inside it: when the column itself
     scrolled, the bar came up the middle of the window instead of down its
     edge. The reading column is made by padding rather than by a wrapper, so
     the virtual list keeps its rows as direct children. */
  .scroller {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    outline: none;
  }

  .scroller[data-density="page"] {
    padding-inline: max(var(--library-gutter), calc((100% - var(--library-column)) / 2));
    padding-block-end: 24px;
  }

  .scroller[data-density="panel"] {
    padding: 8px 8px 12px;
  }

  .day {
    display: flex;
    align-items: flex-end;
    gap: 8px;
    margin: 0;
    padding: 0 10px 6px;
    font-size: var(--text-label);
    font-weight: 550;
    letter-spacing: 0.01em;
    color: var(--color-muted);
    position: sticky;
    top: 0;
    z-index: 1;
    background: var(--color-page);
  }

  /* The sidebar tool does not sit on the page ground, so a sticky heading
     there would paint the wrong colour over the rows behind it. */
  .scroller[data-density="panel"] .day {
    position: static;
    background: none;
  }

  .count {
    font-weight: 400;
    color: var(--color-muted);
  }

  /* A table, not a row of floated parts: the host, the time and the delete
     button each hold a column of their own, so every title truncates at the
     same x and the three trailing columns line up down the page. As flex
     siblings the host slid left and right with its own length and a long
     title ran into it. */
  .visit {
    display: grid;
    grid-template-columns: 16px minmax(0, 1fr) auto 62px 22px;
    align-items: center;
    gap: 10px;
    padding: 0 10px;
    border-radius: var(--radius-row);
    cursor: default;
    transition: background-color var(--motion-instant) var(--ease-smooth);
  }

  /* The pointer and the caret land on the same plate every other list in the
     product uses for the row it is on. */
  .visit.selected {
    background: var(--row-active);
  }

  .title {
    min-width: 0;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
    font-size: var(--text-body);
    color: var(--color-text);
  }

  /* The part the typing accounts for carries the weight; the rest recedes. */
  mark {
    background: none;
    color: var(--color-text);
    font-weight: 550;
  }

  .host {
    max-width: 180px;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
    text-align: end;
    font-size: var(--text-label);
    color: var(--color-muted);
  }

  time {
    text-align: end;
    font-size: var(--text-label);
    font-variant-numeric: tabular-nums;
    color: var(--color-muted);
  }

  /* Its column is always there, so revealing it never shifts the row under
     the pointer. */
  .forget {
    visibility: hidden;
  }

  .forget:focus-within {
    visibility: visible;
  }

  .visit.selected .forget {
    visibility: visible;
  }

  .spacer {
    flex: none;
  }

  .notice,
  .more {
    display: block;
    width: 100%;
    margin: 12px 0 0;
    padding: 10px;
    border: 0;
    background: none;
    font-size: 12px;
    color: var(--color-muted);
    text-align: center;
  }

  .more {
    cursor: default;
    border-radius: var(--radius-row);
  }

  .more:hover {
    background: var(--color-fill-hover);
  }

  .scroller[data-density="panel"] .visit {
    grid-template-columns: 16px minmax(0, 1fr) 22px;
  }

  .scroller[data-density="panel"] .title {
    font-size: var(--text-label);
  }

  .scroller[data-density="panel"] .host,
  .scroller[data-density="panel"] time {
    display: none;
  }

  @media (prefers-reduced-motion: reduce) {
    .visit {
      transition: none;
    }
  }

  @media (forced-colors: active) {
    .visit.selected {
      outline: 1px solid Highlight;
    }
  }
</style>
