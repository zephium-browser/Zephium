<script lang="ts">
  import Icon from "$shared/ui/Icon";
  import { fitTextarea, scrollParent } from "$shared/lib/fit";
  import { untrack } from "svelte";
  import { duration, easing, reducedMotion } from "$shared/lib/motion";
  import Menu, { type MenuEntry } from "$shared/ui/Menu";
  import {
    ArrowUpRight01Icon,
    Calendar03Icon,
    CheckListIcon,
    Clock01Icon,
    Delete02Icon,
    Flag02Icon,
    Folder01Icon,
    MoreHorizontalIcon,
    PinIcon,
    SparklesIcon,
    TextIcon,
  } from "@hugeicons/core-free-icons";
  import * as m from "$shared/i18n/messages";
  import type { TaskRow, TaskStatus } from "$domain/resources";
  import {
    dueLabel,
    dueTone,
    durationLabel,
    hostOf,
    matchRange,
    showsDue,
    type SectionKey,
  } from "../lib/task-sections";
  import { PRIORITY_ICON } from "../lib/priority";
  import TaskCheck from "./TaskCheck.svelte";
  import SiteMark from "./SiteMark.svelte";
  import { DUE_LABELS, DURATION_LABELS, PRIORITY_LABEL, STATE_LABEL } from "../lib/labels";
  import DueMenu from "./DueMenu.svelte";

  let {
    task,
    section,
    today,
    density = "panel",
    listName = null,
    query = "",
    selected = false,
    marked = false,
    tabbable = false,
    leaving = false,
    trashed = false,
    onselect,
    ontoggle,
    onschedule,
    onrename,
    onpin,
    onremove,
    onopenpage,
  }: {
    task: TaskRow;
    /** Where the row is drawn, which decides whether its date has news. */
    section: SectionKey;
    today: string;
    density?: "panel" | "rail" | "page";
    /** The task's list, when the view is not already that list. */
    listName?: string | null;
    query?: string;
    selected?: boolean;
    /** Part of a multiple selection, which is a different thing from being the
     *  row the keyboard is on. */
    marked?: boolean;
    /** Carries the list's single tab stop. */
    tabbable?: boolean;
    /** Completed a moment ago and now leaving the section it was completed in. */
    leaving?: boolean;
    trashed?: boolean;
    onselect: (id: string, event?: MouseEvent) => void;
    ontoggle: (id: string, status: TaskStatus) => void;
    onschedule: (id: string, day: string | null, time: string | null) => void;
    onrename: (id: string, title: string) => void;
    onpin: (id: string, pinned: boolean) => void;
    onremove: (id: string) => void;
    onopenpage: (url: string) => void;
  } = $props();

  let editing = $state(false);
  let draft = $state("");
  let field = $state<HTMLTextAreaElement>();
  /** Pickers are built once the pointer or the keyboard reaches the row, so a
   *  long list costs rows rather than rows times their menus. */
  let armed = $state(false);
  let renaming = false;
  let open = $derived(task.status !== "done");
  let dated = $derived(showsDue(task, section, today));
  let dueToneValue = $derived(dueTone(task.dueDate, today));
  let deadlineTone = $derived(dueTone(task.deadline, today));
  let range = $derived(matchRange(task.title, query));
  let meta = $derived(
    task.pinned ||
      dated ||
      task.deadline !== null ||
      task.duration !== null ||
      task.stepCount > 0 ||
      listName !== null ||
      task.context !== null ||
      task.status === "active" ||
      task.status === "blocked",
  );
  // The check's own name carries the state, so a screen reader hears what
  // pressing it would change rather than just the task's title.
  let checkLabel = $derived(`${STATE_LABEL[task.status]()} — ${task.title}`);

  function beginRename() {
    if (trashed) return;
    draft = task.title;
    editing = true;
    // After the menu that asked for it has closed, so its focus return is not
    // what the field sees first.
    requestAnimationFrame(() => {
      renaming = false;
      field?.focus();
      field?.select();
    });
  }

  function grow(node: HTMLTextAreaElement) {
    let scroller: HTMLElement | null | undefined;
    const size = () => fitTextarea(node, (scroller ??= scrollParent(node)));
    size();
    node.addEventListener("input", size);
    return { destroy: () => node.removeEventListener("input", size) };
  }

  /** The whole row selects, not only its title: the padding and the metadata
   *  line are part of the target a pointer aims at. */
  function rowClick(event: MouseEvent) {
    if (editing || (event.target as Element).closest("button, input, textarea, a")) return;
    onselect(task.id, event);
  }

  function commitRename() {
    if (!editing) return;
    editing = false;
    const value = draft.trim();
    if (value && value !== task.title) onrename(task.id, value);
  }

  function titleKeydown(event: KeyboardEvent) {
    if (event.isComposing) return;
    if (event.key === "Enter") {
      event.preventDefault();
      commitRename();
    } else if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      editing = false;
    }
  }

  let entries: MenuEntry[] = $derived(
    trashed
      ? [{ kind: "item", id: "remove", label: m.task_restore() }]
      : [
          { kind: "item", id: "rename", label: m.task_rename(), icon: TextIcon },
          {
            kind: "item",
            id: "pin",
            label: task.pinned ? m.task_unpin() : m.resource_pin(),
            icon: PinIcon,
          },
          ...(task.context
            ? [
                {
                  kind: "item" as const,
                  id: "open",
                  label: m.task_open_page(),
                  icon: ArrowUpRight01Icon,
                },
              ]
            : []),
          { kind: "separator" },
          { kind: "item", id: "remove", label: m.task_delete(), icon: Delete02Icon, danger: true },
        ],
  );

  function act(id: string) {
    if (id === "rename") {
      renaming = true;
      beginRename();
    } else if (id === "pin") onpin(task.id, !task.pinned);
    else if (id === "open" && task.context) onopenpage(task.context.url);
    else if (id === "remove") onremove(task.id);
  }

  // A task just written is drawn from intent before native confirms it; that
  // first drawing rises into place. A row merely scrolled into view is not
  // pending, so the list's windowing never replays this.
  function arrive(node: HTMLElement) {
    if (!untrack(() => task.pending) || reducedMotion()) return;
    node.animate(
      [
        { opacity: 0, translate: "0 -6px" },
        { opacity: 1, translate: "0 0" },
      ],
      { duration: duration("slow"), easing: easing("emphasized") },
    );
  }
</script>

{#snippet dueTrigger(props: Record<string, unknown>, empty: boolean)}
  <button
    {...props}
    type="button"
    class={empty ? "task-action" : "task-meta-item task-due"}
    data-tone={dueToneValue}
    tabindex={tabbable ? 0 : -1}
    aria-label={m.task_due_schedule()}
    ><Icon icon={Calendar03Icon} size={empty ? 15 : 12} />{#if !empty && task.dueDate}{dueLabel(
        task.dueDate,
        today,
        DUE_LABELS,
        task.dueTime,
      )}{/if}</button
  >
{/snippet}

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div
  {@attach arrive}
  class="task"
  onclick={rowClick}
  onpointerenter={() => (armed = true)}
  onfocusin={() => (armed = true)}
  data-task-id={task.id}
  data-density={density}
  data-status={task.status}
  data-selected={selected}
  data-marked={marked}
  data-pending={task.pending}
  data-priority={task.priority}
  data-leaving={leaving}
>
  <TaskCheck
    status={task.status}
    label={checkLabel}
    disabled={trashed || task.revision === "0"}
    tabindex={tabbable ? 0 : -1}
    ontoggle={(next) => ontoggle(task.id, next)}
  />
  <div class="task-main">
    {#if editing}
      <textarea
        bind:this={field}
        use:grow
        class="task-rename"
        rows="1"
        aria-label={m.task_rename()}
        value={draft}
        maxlength="256"
        oninput={(event) => (draft = event.currentTarget.value.replace(/\n/gu, " "))}
        onkeydown={titleKeydown}
        onblur={commitRename}></textarea>
    {:else}
      <button
        type="button"
        class="task-title"
        tabindex={tabbable ? 0 : -1}
        aria-current={selected ? "true" : undefined}
        onclick={(event) => onselect(task.id, event)}
        ondblclick={beginRename}
      >
        {#if task.origin === "agent"}<Icon
            icon={SparklesIcon}
            size={12}
            label={m.task_from_agent()}
          />{/if}<span class="task-label"
          >{#if range}{task.title.slice(0, range[0])}<mark
              >{task.title.slice(range[0], range[1])}</mark
            >{task.title.slice(range[1])}{:else}{task.title}{/if}</span
        >
      </button>
    {/if}
    {#if meta}
      <div class="task-meta">
        {#if task.pinned}<span class="task-meta-item" aria-label={m.resource_pinned()}
            ><Icon icon={PinIcon} size={12} /></span
          >{/if}
        {#if task.status === "blocked" || task.status === "active"}<span
            class="task-meta-item task-state"
            data-status={task.status}>{STATE_LABEL[task.status]()}</span
          >{/if}
        {#if dated && !trashed && !armed}{@render dueTrigger({}, false)}{:else if dated && !trashed}
          <DueMenu
            due={task.dueDate}
            time={task.dueTime}
            {today}
            label={m.task_due_schedule()}
            onselect={(day, time) => onschedule(task.id, day, time)}
          >
            {#snippet trigger({ props })}{@render dueTrigger(props, false)}{/snippet}
          </DueMenu>
        {:else if dated && task.dueDate}<span class="task-meta-item" data-tone={dueToneValue}
            ><Icon icon={Calendar03Icon} size={12} />{dueLabel(
              task.dueDate,
              today,
              DUE_LABELS,
              task.dueTime,
            )}</span
          >{/if}
        {#if task.deadline}<span
            class="task-meta-item task-deadline"
            data-tone={open ? deadlineTone : null}
            title={m.task_deadline()}
            aria-label={m.task_deadline_label({ date: dueLabel(task.deadline, today, DUE_LABELS) })}
            ><Icon icon={Flag02Icon} size={12} />{dueLabel(task.deadline, today, DUE_LABELS)}</span
          >{/if}
        {#if task.duration}<span class="task-meta-item" title={m.task_duration()}
            ><Icon icon={Clock01Icon} size={12} />{durationLabel(
              task.duration,
              DURATION_LABELS,
            )}</span
          >{/if}
        {#if task.stepCount}<span
            class="task-meta-item"
            aria-label={m.task_subtask_progress({ done: task.stepDone, total: task.stepCount })}
            ><Icon icon={CheckListIcon} size={12} />{task.stepDone}/{task.stepCount}</span
          >{/if}
        {#if listName}<span class="task-meta-item task-list-name"
            ><Icon icon={Folder01Icon} size={12} /><span>{listName}</span></span
          >{/if}
        {#if task.context}<button
            type="button"
            class="task-meta-item task-site"
            tabindex={tabbable ? 0 : -1}
            title={task.context.url}
            onclick={() => onopenpage(task.context!.url)}
            ><SiteMark url={task.context.url} /><span>{hostOf(task.context.url)}</span></button
          >{/if}
      </div>
    {/if}
  </div>
  {#if task.priority !== "none"}<span
      class="task-priority"
      data-priority={task.priority}
      role="img"
      aria-label={`${m.task_priority()}: ${PRIORITY_LABEL[task.priority]()}`}
      title={PRIORITY_LABEL[task.priority]()}
      ><Icon icon={PRIORITY_ICON[task.priority]} size={14} /></span
    >{/if}
  <div class="task-trailing">
    {#if !armed}
      {#if !trashed && !dated && open}{@render dueTrigger({}, true)}{/if}<button
        type="button"
        class="task-action task-menu"
        aria-label={m.task_more()}
        tabindex={tabbable ? 0 : -1}><Icon icon={MoreHorizontalIcon} size={15} /></button
      >
    {:else if !trashed && !dated && open}
      <DueMenu
        due={task.dueDate}
        time={task.dueTime}
        {today}
        label={m.task_due_schedule()}
        onselect={(day, time) => onschedule(task.id, day, time)}
      >
        {#snippet trigger({ props })}{@render dueTrigger(props, true)}{/snippet}
      </DueMenu>
    {/if}
    {#if armed}<Menu
        label={m.task_more()}
        {entries}
        side="bottom"
        align="end"
        onselect={act}
        returnFocus={() => !renaming}
        triggerClass="task-action task-menu"
      >
        {#snippet trigger()}<Icon icon={MoreHorizontalIcon} size={15} />{/snippet}
      </Menu>{/if}
  </div>
</div>

<style>
  .task {
    position: relative;
    display: flex;
    /* stylelint-disable-next-line property-no-vendor-prefix */
    -webkit-user-select: none;
    user-select: none;
    align-items: flex-start;
    gap: 10px;
    box-sizing: border-box;
    min-width: 0;
    min-height: 38px;
    padding: 9px 6px 9px 10px;
    border-radius: var(--radius-row);
    color: var(--color-text);
    transition:
      background-color var(--motion-fast) var(--ease-out),
      opacity var(--motion-fast) var(--ease-out);
  }

  .task[data-density="rail"] {
    gap: 8px;
    padding-inline: 6px 4px;
  }

  .task[data-density="page"] {
    gap: 12px;
    min-height: 40px;
    padding-block: 10px;
  }

  /* The check sits on the title's first line, however many lines follow. */
  .task > :global(.task-check) {
    margin-block-start: 1px;
  }

  .task:not([data-selected="true"]):hover {
    background: var(--row-hover);
  }

  .task[data-selected="true"] {
    background: var(--row-active);
  }

  /* Part of a selection is a quieter state than being the focused row: it marks
     what an action would reach, not where the keyboard is. */
  .task[data-marked="true"] {
    background: var(--color-accent-soft);
  }

  .task-main {
    display: flex;
    flex: 1;
    flex-direction: column;
    gap: 4px;
    min-width: 0;
  }

  .task-title {
    display: flex;
    align-items: baseline;
    gap: 6px;
    width: 100%;
    min-width: 0;
    padding: 0;
    border: 0;
    border-radius: 2px;
    background: transparent;
    color: inherit;
    font: inherit;
    font-size: 14px;
    line-height: 20px;
    font-weight: var(--sidebar-row-weight);
    letter-spacing: -0.006em;
    text-align: start;
    cursor: default;
    outline: none;
  }

  .task-title :global(svg) {
    flex: none;
    align-self: center;
    color: var(--color-muted);
  }

  .task-title:focus-visible {
    outline: 2px solid var(--color-ring);
    outline-offset: 3px;
  }

  .task-label {
    display: -webkit-box;
    flex: 1;
    min-width: 0;
    overflow: hidden;
    overflow-wrap: anywhere;
    -webkit-box-orient: vertical;
    -webkit-line-clamp: 2;
    line-clamp: 2;
  }

  mark {
    border-radius: 3px;
    background: var(--color-accent-soft);
    color: inherit;
    font-weight: 600;
  }

  .task-rename {
    display: block;
    box-sizing: border-box;
    width: calc(100% + 8px);
    min-width: 0;
    margin: -2px 0 -2px -4px;
    padding: 2px 4px;
    overflow: hidden;
    border: 0;
    border-radius: var(--radius-inset);
    background: var(--color-field);
    box-shadow: var(--shadow-field-focus);
    color: var(--color-text);
    font: inherit;
    font-size: 14px;
    line-height: 20px;
    resize: none;
    /* stylelint-disable-next-line property-no-vendor-prefix */
    -webkit-user-select: text;
    user-select: text;
    outline: none;
  }

  .task-meta {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 2px 12px;
    min-width: 0;
    color: var(--color-faint);
    font-size: var(--text-caption);
    line-height: 16px;
  }

  .task-meta-item {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    min-width: 0;
    max-width: 100%;
    padding: 0;
    border: 0;
    border-radius: 2px;
    background: transparent;
    color: inherit;
    font: inherit;
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
    cursor: default;
  }

  .task-meta-item > span {
    overflow: hidden;
    text-overflow: ellipsis;
  }

  button.task-meta-item:hover {
    color: var(--color-text);
  }

  button.task-meta-item:focus-visible {
    outline: 2px solid var(--color-ring);
    outline-offset: 2px;
  }

  /* Priority rests at the end of the title line and makes way for the row's
     actions when they appear in the same place. */
  .task-priority {
    position: absolute;
    inset-block-start: 10px;
    inset-inline-end: 12px;
    display: grid;
    place-items: center;
    color: var(--color-muted);
    transition: opacity var(--motion-fast) var(--ease-out);
  }

  .task[data-density="page"] .task-priority {
    inset-block-start: 11px;
  }

  .task-priority[data-priority="high"] {
    color: var(--color-text);
  }

  .task:is(:hover, :focus-within, [data-selected="true"]) .task-priority {
    opacity: 0;
  }

  .task:not([data-priority="none"]) .task-main {
    padding-inline-end: 20px;
  }

  .task-list-name {
    max-width: 160px;
  }

  .task-site {
    max-width: 180px;
  }

  /* Colour is spent only on time and on a stop: late, due now, blocked. */
  .task-meta-item[data-tone="overdue"] {
    color: var(--color-danger);
  }

  .task-meta-item[data-tone="today"] {
    color: var(--color-text);
  }

  .task-deadline[data-tone="today"] {
    color: var(--color-warning);
  }

  .task-state[data-status="blocked"] {
    color: var(--color-warning);
    font-weight: 550;
  }

  .task-state[data-status="active"] {
    color: var(--color-muted);
    font-weight: 550;
  }

  .task[data-status="done"] .task-label {
    color: var(--color-faint);
    text-decoration: line-through;
    text-decoration-color: var(--color-faint);
  }

  .task[data-status="done"] .task-meta {
    opacity: 0.8;
  }

  .task[data-selected="true"] .task-title {
    font-weight: var(--sidebar-row-weight-current);
  }

  /* Actions float over the row's end instead of reserving a column, so a narrow
     panel spends its width on the title. While they show, the title dissolves
     beneath them rather than colliding. */
  .task-trailing {
    position: absolute;
    inset-block-start: 7px;
    inset-inline-end: 6px;
    display: flex;
    align-items: center;
    gap: 2px;
  }

  .task[data-density="page"] .task-trailing {
    inset-block-start: 8px;
  }

  .task:is(:hover, :focus-within, [data-selected="true"]) .task-title {
    mask-image: linear-gradient(to right, black calc(100% - 64px), transparent calc(100% - 44px));
  }

  .task:is(:hover, :focus-within, [data-selected="true"]) .task-title:dir(rtl) {
    mask-image: linear-gradient(to left, black calc(100% - 64px), transparent calc(100% - 44px));
  }

  .task :global(.task-action) {
    display: grid;
    place-items: center;
    width: 24px;
    height: 24px;
    margin-block: -2px;
    padding: 0;
    border: 0;
    border-radius: var(--radius-inset);
    background: transparent;
    color: var(--color-muted);
    opacity: 0;
    cursor: default;
    outline: none;
    transition:
      opacity var(--motion-fast) var(--ease-out),
      background-color var(--motion-fast) var(--ease-out),
      color var(--motion-fast) var(--ease-out);
  }

  /* Actions surface where the pointer or keyboard already is, so a long list is
     titles rather than a wall of buttons. */
  .task:hover :global(.task-action),
  .task:focus-within :global(.task-action),
  .task[data-selected="true"] :global(.task-action),
  .task :global(.task-action[data-state="open"]) {
    opacity: 1;
  }

  .task :global(.task-action:hover),
  .task :global(.task-action[data-state="open"]) {
    background: var(--row-pressed);
    color: var(--color-text);
  }

  .task :global(.task-action:focus-visible) {
    outline: 2px solid var(--color-ring);
    outline-offset: -2px;
  }

  /* Opacity and transform only: collapsing the row's own box would animate
     layout, which this interface does not do. */
  .task[data-leaving="true"] {
    animation: task-leave var(--motion-base) var(--ease-exit) forwards;
    pointer-events: none;
  }

  @keyframes task-leave {
    to {
      opacity: 0;
      transform: translateX(6px);
    }
  }

  .task[data-leaving="true"]:dir(rtl) {
    animation-name: task-leave-rtl;
  }

  @keyframes task-leave-rtl {
    to {
      opacity: 0;
      transform: translateX(-6px);
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .task[data-leaving="true"] {
      animation-duration: 1ms;
    }
  }

  @media (forced-colors: active) {
    .task[data-selected="true"] {
      outline: 1px solid Highlight;
    }
  }
</style>
