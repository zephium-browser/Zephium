<script lang="ts">
  import { tick, untrack } from "svelte";
  import { SvelteMap, SvelteSet } from "svelte/reactivity";
  import * as m from "$shared/i18n/messages";
  import type { TaskList, TaskRow as Row, TaskStatus } from "$domain/resources";
  import { createVirtualWindow } from "$shared/lib/virtual-window.svelte";
  import { createPointerDrag } from "$shared/lib/pointer-drag.svelte";
  import { reorderKeys } from "../lib/task-order";
  import { createSections, type SectionKey, type TaskScope } from "../lib/task-sections";
  import { today as currentDay, watchToday } from "../lib/today.svelte";
  import Icon from "$shared/ui/Icon";
  import { ArrowRight01Icon } from "@hugeicons/core-free-icons";
  import TaskRow from "./TaskRow.svelte";
  import TaskBulkBar from "./TaskBulkBar.svelte";

  let {
    rows,
    scope = "all",
    density = "panel",
    trashed = false,
    query = "",
    label = m.task_list(),
    lists = [],
    listId = null,
    onselected,
    selectedId = null,
    announce,
    ontoggle,
    onschedule,
    onrename,
    onpin,
    ondelete,
    onrestore,
    onopenpage,
    onposition,
  }: {
    rows: readonly Row[];
    scope?: TaskScope;
    density?: "panel" | "rail" | "page";
    trashed?: boolean;
    /** The active search. A match outside the current scope must still be
     *  findable, so a search widens the list to everything. */
    query?: string;
    label?: string;
    lists?: readonly TaskList[];
    /** The list being viewed, whose name its rows need not repeat. */
    listId?: string | null;
    onselected?: (id: string | null, open?: boolean) => void;
    selectedId?: string | null;
    announce?: (message: string) => void;
    ontoggle: (id: string, status: TaskStatus) => Promise<boolean>;
    onschedule: (id: string, day: string | null, time: string | null) => void;
    onrename: (id: string, title: string) => void;
    onpin: (id: string, pinned: boolean) => void;
    ondelete: (id: string) => Promise<boolean>;
    onrestore: (id: string) => Promise<boolean>;
    onopenpage: (url: string) => void;
    /** Writes a manual position; absent where the view cannot be reordered. */
    onposition?: (id: string, sortKey: string) => void;
  } = $props();

  /** A task completed here keeps its place for a moment before it leaves, so the
   *  row does not vanish from under the pointer that just ticked it. */
  const HOLD_MS = 1100;
  const LEAVE_MS = 200;
  const COMPLETED_LIMIT = Number.POSITIVE_INFINITY;
  let showCompleted = $state(false);

  /** First-paint estimates; a drawn row reports its real height. */
  const ROW_HEIGHT = { panel: 38, rail: 36, page: 40 } as const;
  const META_HEIGHT = 20;
  const HEADING_HEIGHT = { panel: 30, rail: 26, page: 38 } as const;
  const measured = new SvelteMap<string, number>();

  let holding = new SvelteSet<string>();
  let leaving = new SvelteSet<string>();
  const timers = new SvelteMap<string, ReturnType<typeof setTimeout>[]>();

  let today = $derived(currentDay());
  let selected = $derived(selectedId);
  let listNames = $derived(new Map(lists.map((list) => [list.id, list.title])));
  let marked = new SvelteSet<string>();
  let scroller = $state<HTMLElement>();

  $effect(() => watchToday());

  let searching = $derived(query.trim().length > 0);
  const sections = createSections();
  let grouped = $derived(
    sections(rows, {
      // A search answers "where is it", which a scope must not veto.
      scope: trashed || searching ? "all" : scope,
      today,
      holding,
      completedLimit: COMPLETED_LIMIT,
      labels: {
        overdue: m.task_section_overdue(),
        today: m.task_section_today(),
        tomorrow: m.task_section_tomorrow(),
        upcoming: m.task_section_upcoming(),
        anytime: m.task_section_anytime(),
        completed: m.task_section_completed(),
      },
    }),
  );

  type Entry =
    | { kind: "heading"; id: string; label: string; count: number; clipped: boolean }
    | { kind: "task"; id: string; task: Row; section: SectionKey };

  // A lone section that only repeats the view's own name is not a heading.
  let headless = $derived(
    grouped.length === 1 &&
      (grouped[0]!.key === "anytime" || (scope === "today" && grouped[0]!.key === "today")),
  );

  let entries = $derived(
    grouped.flatMap((section): Entry[] => [
      ...(headless
        ? []
        : [
            {
              kind: "heading" as const,
              id: `heading:${section.key}`,
              label: section.label,
              count: section.total,
              clipped: section.rows.length < section.total,
            },
          ]),
      ...(section.key === "completed" &&
      !showCompleted &&
      scope !== "completed" &&
      !searching &&
      !trashed
        ? []
        : section.rows
      ).map((task): Entry => ({
        kind: "task",
        id: task.id,
        task,
        section: section.key,
      })),
    ]),
  );
  let order = $derived(entries.flatMap((entry) => (entry.kind === "task" ? [entry.id] : [])));

  const virtual = createVirtualWindow({
    threshold: 40,
    heights: () =>
      entries.map((entry) =>
        entry.kind === "heading"
          ? HEADING_HEIGHT[density]
          : (measured.get(entry.id) ?? estimate(entry.task)),
      ),
  });

  function estimate(task: Row): number {
    const meta =
      task.dueDate !== null ||
      task.deadline !== null ||
      task.duration !== null ||
      task.stepCount > 0 ||
      task.context !== null;
    return ROW_HEIGHT[density] + (meta ? META_HEIGHT : 0);
  }

  /** Rows size to their content, so the window learns each one's real height
   *  from the row itself instead of assuming a line count. One observer serves
   *  the whole list. */
  const observed = new WeakMap<Element, string>();
  const observer = new ResizeObserver((changes) => {
    for (const change of changes) {
      const id = observed.get(change.target);
      const height = Math.round(change.borderBoxSize[0]?.blockSize ?? 0);
      if (id && height && measured.get(id) !== height) measured.set(id, height);
    }
  });
  $effect(() => () => observer.disconnect());
  // Heights of rows that have left the list would otherwise pile up for as
  // long as the list stays open.
  $effect(() => {
    const live = rows;
    untrack(() => {
      if (measured.size <= live.length) return;
      const ids = new Set(live.map((row) => row.id));
      for (const id of [...measured.keys()]) if (!ids.has(id)) measured.delete(id);
    });
  });

  function measure(node: HTMLElement, id: string) {
    observed.set(node, id);
    observer.observe(node);
    return {
      update(next: string) {
        observed.set(node, next);
      },
      destroy() {
        observer.unobserve(node);
        observed.delete(node);
      },
    };
  }

  let slice = $derived(virtual.window);
  let visible = $derived(entries.slice(slice.first, slice.last));

  $effect(() => () => {
    for (const handles of timers.values()) for (const handle of handles) clearTimeout(handle);
    timers.clear();
  });

  function release(id: string) {
    for (const handle of timers.get(id) ?? []) clearTimeout(handle);
    timers.delete(id);
    holding.delete(id);
    leaving.delete(id);
  }

  async function toggle(id: string, status: TaskStatus) {
    release(id);
    if (status === "done") {
      holding.add(id);
      timers.set(id, [
        setTimeout(() => {
          leaving.add(id);
          timers.set(id, [...(timers.get(id) ?? []), setTimeout(() => release(id), LEAVE_MS)]);
        }, HOLD_MS),
      ]);
    }
    const title = rows.find((row) => row.id === id)?.title ?? "";
    const saved = await ontoggle(id, status);
    if (!saved) {
      release(id);
      return false;
    }
    announce?.(
      status === "done"
        ? m.task_announce_completed({ title })
        : m.task_announce_reopened({ title }),
    );
    return true;
  }

  function focusRow(id: string | null) {
    if (!id) return;
    const at = entries.findIndex((entry) => entry.id === id);
    if (at >= 0) virtual.scrollToIndex(at);
    void tick().then(() =>
      scroller
        ?.querySelector<HTMLElement>(`[data-task-id="${CSS.escape(id)}"] .task-title`)
        ?.focus(),
    );
  }

  function move(offset: number) {
    if (!order.length) return;
    const focused =
      scroller?.ownerDocument.activeElement?.closest<HTMLElement>("[data-task-id]")?.dataset.taskId;
    const at = order.indexOf(selected ?? focused ?? "");
    const next = at < 0 ? (offset > 0 ? 0 : order.length - 1) : at + offset;
    const id = order[Math.min(order.length - 1, Math.max(0, next))];
    if (!id) return;
    selected = id;
    onselected?.(id, false);
    focusRow(id);
  }

  /** Removing the focused row must not drop focus onto the document, or the very
   *  next key — an undo, usually — would go nowhere. */
  function remove(id: string) {
    const at = order.indexOf(id);
    const next = order[at + 1] ?? order[at - 1] ?? null;
    selected = next;
    onselected?.(next, false);
    marked.delete(id);
    const title = rows.find((row) => row.id === id)?.title ?? "";
    if (trashed) {
      void onrestore(id).then((saved) => {
        if (saved) announce?.(m.task_announce_restored({ title }));
      });
    } else {
      void ondelete(id).then((saved) => {
        if (saved) announce?.(m.task_announce_deleted({ title }));
      });
    }
    if (next) {
      focusRow(next);
      return;
    }
    // Resolved now, not in the microtask: removing the last row can unmount this
    // list before then, and the binding would already be gone.
    const panel = scroller?.closest<HTMLElement>("[data-tasks-root]") ?? scroller;
    queueMicrotask(() => panel?.focus());
  }

  // Reordering happens within what the list already groups by: a task moves
  // among the tasks sharing its section, state and pin, because those still
  // decide the order above any manual position.
  const RANK: Record<TaskStatus, number> = { blocked: 0, active: 1, open: 2, done: 3 };
  let reorderable = $derived(
    !trashed && !searching && scope !== "completed" && onposition !== undefined,
  );
  let drop = $state<{ id: string; after: boolean } | null>(null);

  function groupOf(id: string): Row[] {
    const home = entries.find((entry) => entry.kind === "task" && entry.id === id);
    if (!home || home.kind !== "task") return [];
    return entries.flatMap((entry) =>
      entry.kind === "task" &&
      entry.section === home.section &&
      RANK[entry.task.status] === RANK[home.task.status] &&
      entry.task.pinned === home.task.pinned
        ? [entry.task]
        : [],
    );
  }

  function place(moving: string, over: string, after: boolean) {
    const group = groupOf(moving);
    if (!onposition || moving === over || !group.some((row) => row.id === over)) return;
    const without = group.filter((row) => row.id !== moving);
    const at = without.findIndex((row) => row.id === over) + (after ? 1 : 0);
    const mover = group.find((row) => row.id === moving)!;
    const ordered = [...without.slice(0, at), mover, ...without.slice(at)];
    for (const [id, key] of Object.entries(reorderKeys(ordered, moving))) onposition(id, key);
  }

  function landing(moving: string, target: Element | null, y: number) {
    const row = target?.closest<HTMLElement>("[data-task-id]");
    const id = row?.dataset.taskId;
    if (!row || !id || id === moving) return null;
    if (!groupOf(moving).some((task) => task.id === id)) return null;
    const box = row.getBoundingClientRect();
    return { id, after: y > box.top + box.height / 2 };
  }

  const drag = createPointerDrag<Row>({
    ondrop(task, target, at) {
      drop = null;
      const spot = landing(task.id, target, at.y);
      if (spot) place(task.id, spot.id, spot.after);
    },
  });

  function press(event: PointerEvent, task: Row) {
    if (!reorderable) return;
    // The title carries the row and may be dragged by; controls may not.
    const control = (event.target as Element).closest("button, input, textarea, a");
    if (control && !control.classList.contains("task-title")) return;
    drag.begin(event, task);
  }

  function carry(event: PointerEvent) {
    drag.move(event);
    if (drag.item)
      drop = landing(
        drag.item.id,
        document.elementFromPoint(event.clientX, event.clientY),
        event.clientY,
      );
  }

  /** The keyboard's equal of dragging: one place up or down within the group. */
  function shift(offset: number) {
    if (!reorderable || selected === null) return;
    const group = groupOf(selected);
    const at = group.findIndex((row) => row.id === selected);
    const neighbour = group[at + offset];
    if (!neighbour) return;
    place(selected, neighbour.id, offset > 0);
    focusRow(selected);
  }

  function select(id: string, event?: MouseEvent) {
    if (drag.absorbClick()) return;
    if (event && (event.metaKey || event.ctrlKey)) {
      if (marked.has(id)) marked.delete(id);
      else marked.add(id);
      selected = id;
      return;
    }
    if (event?.shiftKey && selected !== null) {
      // Everything between the anchor and here, which is what a reader means by
      // shift-clicking a list.
      const from = order.indexOf(selected);
      const to = order.indexOf(id);
      if (from >= 0 && to >= 0)
        for (const between of order.slice(Math.min(from, to), Math.max(from, to) + 1))
          marked.add(between);
      selected = id;
      return;
    }
    if (marked.size) marked.clear();
    selected = id;
    onselected?.(id);
  }

  async function bulk(run: (id: string) => Promise<boolean>, message: (count: number) => string) {
    const chosen = [...marked];
    marked.clear();
    const results = await Promise.allSettled(chosen.map(run));
    const succeeded = results.filter(
      (result) => result.status === "fulfilled" && result.value,
    ).length;
    if (succeeded) announce?.(message(succeeded));
  }

  function keydown(event: KeyboardEvent) {
    const target = event.target as HTMLElement | null;
    if (target?.tagName === "INPUT" || target?.tagName === "TEXTAREA" || target?.isContentEditable)
      return;
    const row = selected === null ? null : rows.find((entry) => entry.id === selected);
    if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "a") {
      event.preventDefault();
      for (const id of order) marked.add(id);
      return;
    }
    if (event.altKey && (event.metaKey || event.ctrlKey) && /^Arrow(Up|Down)$/u.test(event.key)) {
      event.preventDefault();
      shift(event.key === "ArrowDown" ? 1 : -1);
      return;
    }
    switch (event.key) {
      case "ArrowDown":
        event.preventDefault();
        move(1);
        return;
      case "ArrowUp":
        event.preventDefault();
        move(-1);
        return;
      case "Home":
        event.preventDefault();
        move(-order.length);
        return;
      case "End":
        event.preventDefault();
        move(order.length);
        return;
      case "Enter":
        if (!row) return;
        event.preventDefault();
        onselected?.(row.id);
        return;
      case " ":
        if (trashed) return;
        event.preventDefault();
        if (marked.size)
          bulk(
            (id) => toggle(id, "done"),
            (count) => m.task_announce_bulk_completed({ count }),
          );
        else if (row) toggle(row.id, row.status === "done" ? "open" : "done");
        return;
      case "Escape":
        if (marked.size) {
          event.preventDefault();
          marked.clear();
          return;
        }
        if (selected === null) return;
        event.preventDefault();
        selected = null;
        onselected?.(null);
        return;
      case "Backspace":
      case "Delete":
        if (!(event.metaKey || event.ctrlKey)) return;
        event.preventDefault();
        if (marked.size)
          bulk(
            (id) => (trashed ? onrestore(id) : ondelete(id)),
            (count) => m.task_announce_bulk_deleted({ count }),
          );
        else if (row) remove(row.id);
        return;
      default:
    }
  }
</script>

<!--
  Keys are handled once for the whole list rather than on every row. Focus lives
  on the row's own controls; this element only listens on their behalf.
-->
<!-- svelte-ignore a11y_no_noninteractive_element_interactions, a11y_no_noninteractive_tabindex -->
<div
  class="task-scroller"
  bind:this={scroller}
  data-density={density}
  role="group"
  aria-label={label}
  tabindex={-1}
  use:virtual.attach
  onkeydown={keydown}
>
  {#if marked.size}
    <TaskBulkBar
      count={marked.size}
      {trashed}
      onclear={() => marked.clear()}
      oncomplete={() =>
        bulk(
          (id) => toggle(id, "done"),
          (count) => m.task_announce_bulk_completed({ count }),
        )}
      onremove={() =>
        bulk(
          (id) => (trashed ? onrestore(id) : ondelete(id)),
          (count) => m.task_announce_bulk_deleted({ count }),
        )}
    />
  {/if}
  <div class="task-spacer" style:block-size={`${slice.before}px`}></div>
  {#each visible as entry, index (entry.id)}
    {#if entry.kind === "heading"}
      <h3
        class="task-heading"
        data-key={entry.id.slice(8)}
        style:block-size={`${HEADING_HEIGHT[density]}px`}
      >
        {#if entry.id === "heading:completed" && scope !== "completed" && !searching && !trashed}<button
            class="completed-disclosure"
            type="button"
            aria-expanded={showCompleted}
            onclick={() => (showCompleted = !showCompleted)}
            ><Icon icon={ArrowRight01Icon} size={12} />{entry.label}</button
          >{:else}{entry.label}{/if}<span class="task-count"
          >{entry.clipped ? m.task_section_clipped({ count: entry.count }) : entry.count}</span
        >
      </h3>
    {:else}
      <!-- svelte-ignore a11y_no_static_element_interactions (Option-Command-Arrow is the keyboard's move) -->
      <div
        class="task-slot"
        data-virtual-index={slice.first + index}
        data-drop={drop?.id === entry.id ? (drop.after ? "after" : "before") : undefined}
        data-carrying={drag.item?.id === entry.id}
        use:measure={entry.id}
        onpointerdown={(event) => press(event, entry.task)}
        onpointermove={carry}
        onpointerup={drag.end}
        onpointercancel={(event) => {
          drop = null;
          drag.cancel(event);
        }}
      >
        <TaskRow
          task={entry.task}
          section={entry.section}
          {today}
          {density}
          {trashed}
          {query}
          listName={entry.task.list && entry.task.list !== listId
            ? (listNames.get(entry.task.list) ?? null)
            : null}
          leaving={leaving.has(entry.id)}
          selected={selected === entry.id}
          marked={marked.has(entry.id)}
          tabbable={(selected ?? order[0]) === entry.id}
          onselect={select}
          ontoggle={toggle}
          onremove={remove}
          {onschedule}
          {onrename}
          {onpin}
          {onopenpage}
        />
      </div>
    {/if}
  {/each}
  <div class="task-spacer" style:block-size={`${slice.after}px`}></div>
</div>

{#if drag.item}
  <div class="task-ghost" style:translate={`${drag.at.x + 12}px ${drag.at.y + 8}px`}>
    {drag.item.title}
  </div>
{/if}

<style>
  /* This element is the scroller, not merely a column inside one: the window
     reads its own scrollTop, so a parent scrolling instead would leave the
     window frozen at the top of the list. */
  .task-scroller {
    display: flex;
    flex-direction: column;
    flex: 1;
    min-height: 0;
    min-width: 0;
    overflow-y: auto;
    overscroll-behavior: contain;
    outline: none;
  }

  .task-spacer,
  .task-slot {
    flex: none;
  }

  .task-slot {
    position: relative;
  }

  .task-slot[data-carrying="true"] {
    opacity: 0.4;
  }

  /* Where the carried task would land, drawn as a line between two rows. */
  .task-slot[data-drop]::before {
    position: absolute;
    inset-inline: 10px;
    z-index: 1;
    height: 2px;
    border-radius: var(--radius-capsule);
    background: var(--color-accent);
    content: "";
    pointer-events: none;
  }

  .task-slot[data-drop="before"]::before {
    inset-block-start: -1px;
  }

  .task-slot[data-drop="after"]::before {
    inset-block-end: -1px;
  }

  /* Follows the pointer and is never hit-tested, so the row under the pointer
     is always the drop target rather than the thing being carried. */
  .task-ghost {
    position: fixed;
    inset-block-start: 0;
    inset-inline-start: 0;
    z-index: 60;
    max-width: 260px;
    padding: 6px 10px;
    overflow: hidden;
    border-radius: var(--radius-row);
    background: var(--color-raised);
    box-shadow: var(--shadow-float);
    color: var(--color-text);
    font-size: var(--text-body);
    text-overflow: ellipsis;
    white-space: nowrap;
    pointer-events: none;
  }

  .task-heading {
    display: flex;
    flex: none;
    align-items: center;
    gap: 8px;
    box-sizing: border-box;
    margin: 0;
    padding: 10px 10px 4px;
    color: var(--color-muted);
    font-size: var(--text-label);
    font-weight: 600;
    letter-spacing: -0.005em;
  }

  /* Late is the one heading that is allowed to raise its voice. */
  .task-heading[data-key="overdue"] {
    color: var(--color-danger);
  }

  .completed-disclosure {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    margin-inline-start: -2px;
    border: 0;
    padding: 0;
    background: transparent;
    color: inherit;
    font: inherit;
  }

  .completed-disclosure :global(svg) {
    transition: rotate var(--motion-fast) var(--ease-out);
  }

  .completed-disclosure[aria-expanded="true"] :global(svg) {
    rotate: 90deg;
  }

  .completed-disclosure:focus-visible {
    outline: 2px solid var(--color-ring);
    outline-offset: 3px;
  }

  .task-count {
    color: var(--color-faint);
    font-weight: 500;
    letter-spacing: 0;
    text-transform: none;
  }

  .task-scroller[data-density="page"] .task-heading {
    padding-block-start: 14px;
    font-size: var(--text-body);
  }
</style>
