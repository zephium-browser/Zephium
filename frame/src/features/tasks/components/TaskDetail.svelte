<script lang="ts">
  import type { Snippet } from "svelte";
  import Icon from "$shared/ui/Icon";
  import IconButton from "$shared/ui/IconButton";
  import Menu, { type MenuEntry } from "$shared/ui/Menu";
  import type { IconSvgElement } from "@hugeicons/svelte";
  import {
    Alert02Icon,
    ArrowLeft02Icon,
    ArrowTurnBackwardIcon,
    ArrowUpRight01Icon,
    Calendar03Icon,
    Cancel01Icon,
    CheckmarkCircle02Icon,
    CircleIcon,
    DashboardCircleIcon,
    Clock01Icon,
    Delete02Icon,
    Flag02Icon,
    Folder01Icon,
    InboxIcon,
    PinIcon,
    PlayCircleIcon,
    SparklesIcon,
  } from "@hugeicons/core-free-icons";
  import * as m from "$shared/i18n/messages";
  import { lagging } from "$shared/lib/lag.svelte";
  import { fitTextarea, scrollParent } from "$shared/lib/fit";
  import type { TaskRow, TaskStatus, TaskList, TaskPriority, TaskStep } from "$domain/resources";
  import { dueLabel, dueTone, durationLabel, hostOf } from "../lib/task-sections";
  import { PRIORITY_ICON } from "../lib/priority";
  import TaskSubtasks from "./TaskSubtasks.svelte";
  import TaskCheck from "./TaskCheck.svelte";
  import SiteMark from "./SiteMark.svelte";
  import { DUE_LABELS, DURATION_LABELS } from "../lib/labels";
  import DueMenu from "./DueMenu.svelte";

  let {
    task,
    today,
    lists = [],
    compact = false,
    trashed = false,
    saving = false,
    onclose,
    ontoggle,
    onschedule,
    ondeadline,
    onduration,
    onorganize,
    onpriority,
    onsteps,
    onsteprename,
    onrename,
    ondescribe,
    oncommit,
    onpin,
    onremove,
    onopenpage,
  }: {
    task: TaskRow | null;
    today: string;
    lists?: readonly TaskList[];
    compact?: boolean;
    trashed?: boolean;
    /** A write to the task is on its way to native. */
    saving?: boolean;
    onclose?: () => void;
    ontoggle: (id: string, status: TaskStatus) => void;
    onschedule: (id: string, day: string | null, time: string | null) => void;
    ondeadline: (id: string, day: string | null) => void;
    onduration: (id: string, minutes: number | null) => void;
    onorganize: (id: string, list: string | null, inbox: boolean) => void;
    onpriority: (id: string, priority: TaskPriority) => void;
    onsteps: (id: string, steps: TaskStep[]) => Promise<boolean>;
    onsteprename: (id: string, step: string, title: string) => void;
    onrename: (id: string, title: string) => void;
    ondescribe: (id: string, description: string) => void;
    /** Saves what is being typed into the task now, as leaving it does. */
    oncommit?: (id: string) => void;
    onpin: (id: string, pinned: boolean) => void;
    onremove: (id: string) => void;
    onopenpage: (url: string) => void;
  } = $props();

  const STATES: { id: TaskStatus; icon: IconSvgElement; label: () => string }[] = [
    { id: "open", icon: CircleIcon, label: m.task_state_open },
    { id: "active", icon: PlayCircleIcon, label: m.task_state_active },
    { id: "blocked", icon: Alert02Icon, label: m.task_state_blocked },
    { id: "done", icon: CheckmarkCircle02Icon, label: m.task_state_done },
  ];
  const PRIORITIES: { id: TaskPriority; label: () => string }[] = [
    { id: "high", label: m.task_priority_high },
    { id: "medium", label: m.task_priority_medium },
    { id: "low", label: m.task_priority_low },
    { id: "none", label: m.task_priority_none },
  ];
  const ESTIMATES = [15, 30, 45, 60, 90, 120, 180, 240, 480];
  const dateFormat = new Intl.DateTimeFormat(undefined, { dateStyle: "medium" });

  const slow = lagging(() => saving);
  // Each property is read out of the task on its own, so typing a title
  // rebuilds none of the menus, labels or dates below it.
  let status = $derived<TaskStatus>(task?.status ?? "open");
  let priority = $derived<TaskPriority>(task?.priority ?? "none");
  let duration = $derived(task?.duration ?? null);
  let inbox = $derived(task?.inbox ?? false);
  let list = $derived(task?.list ?? null);
  let createdAt = $derived(task?.createdAt ?? null);
  let completedAt = $derived(task?.completedAt ?? null);

  let current = $derived(STATES.find((entry) => entry.id === status) ?? STATES[0]!);
  let listTitle = $derived(
    inbox
      ? m.task_scope_inbox()
      : (lists.find((entry) => entry.id === list)?.title ?? m.task_no_list()),
  );
  let footnote = $derived.by(() => {
    const parts: string[] = [];
    if (createdAt) parts.push(m.task_added({ date: dateFormat.format(Number(createdAt) * 1000) }));
    if (status === "done" && completedAt)
      parts.push(m.task_completed_on({ date: dateFormat.format(Number(completedAt) * 1000) }));
    return parts.join(" · ");
  });

  let statusMenu: MenuEntry[] = $derived(
    STATES.map((entry) => ({
      kind: "item",
      id: entry.id,
      label: entry.label(),
      icon: entry.icon,
      checked: entry.id === status,
    })),
  );
  let listMenu: MenuEntry[] = $derived([
    {
      kind: "item",
      id: "inbox",
      label: m.task_scope_inbox(),
      icon: InboxIcon,
      checked: inbox,
    },
    {
      kind: "item",
      id: "none",
      label: m.task_no_list(),
      checked: !inbox && list === null,
    },
    ...(lists.length ? [{ kind: "separator" as const }] : []),
    ...lists.map((entry) => ({
      kind: "item" as const,
      id: entry.id,
      label: entry.title,
      icon: Folder01Icon,
      checked: list === entry.id,
    })),
  ]);
  let priorityMenu: MenuEntry[] = $derived(
    PRIORITIES.map((entry) => ({
      kind: "item",
      id: entry.id,
      label: entry.label(),
      icon: PRIORITY_ICON[entry.id],
      checked: entry.id === priority,
    })),
  );
  let estimateMenu: MenuEntry[] = $derived([
    ...ESTIMATES.map((minutes) => ({
      kind: "item" as const,
      id: String(minutes),
      label: durationLabel(minutes, DURATION_LABELS),
      checked: duration === minutes,
    })),
    { kind: "separator" },
    { kind: "item", id: "none", label: m.task_duration_clear(), checked: duration === null },
  ]);

  // Unmounting a focused field fires no blur, so closing the task, or moving
  // to another, settles its typing here.
  let taskId = $derived(task?.id ?? null);
  $effect(() => {
    const id = taskId;
    return () => {
      if (id) oncommit?.(id);
    };
  });

  function fit(element: HTMLTextAreaElement, value: string | null) {
    let scroller: HTMLElement | null | undefined;
    function size(_value: string | null) {
      queueMicrotask(() => {
        if (!element.isConnected) return;
        scroller ??= scrollParent(element);
        fitTextarea(element, scroller);
      });
    }
    size(value);
    return { update: size };
  }
</script>

{#snippet property(icon: IconSvgElement, label: string, value: Snippet)}
  <div class="property">
    <span class="property-label"><Icon {icon} size={15} />{label}</span>
    {@render value()}
  </div>
{/snippet}

{#snippet body(row: TaskRow)}
  <div class="detail-body">
    <div class="detail-title-row">
      <TaskCheck
        status={row.status}
        label={`${current.label()} — ${row.title}`}
        tabindex={0}
        disabled={trashed}
        ontoggle={(status) => ontoggle(row.id, status)}
      />
      <textarea
        class="detail-title"
        data-status={row.status}
        use:fit={row.title}
        aria-label={m.task_rename()}
        aria-invalid={!row.title.trim() || undefined}
        rows="1"
        maxlength="256"
        readonly={trashed}
        value={row.title}
        onkeydown={(event) => {
          if (event.key === "Enter") event.preventDefault();
        }}
        oninput={(event) => onrename(row.id, event.currentTarget.value)}
        onblur={() => oncommit?.(row.id)}></textarea>
    </div>

    <div class="detail-properties">
      {#snippet statusValue()}<Menu
          label={m.task_status_label()}
          triggerClass="property-value"
          entries={statusMenu}
          onselect={(id) => ontoggle(row.id, id as TaskStatus)}
          >{#snippet trigger()}<span class="state-value" data-status={row.status}
              ><Icon icon={current.icon} size={14} />{current.label()}</span
            >{/snippet}</Menu
        >{/snippet}
      {@render property(DashboardCircleIcon, m.task_status_label(), statusValue)}

      {#snippet dateValue()}<DueMenu
          due={row.dueDate}
          time={row.dueTime}
          {today}
          label={m.task_date()}
          onselect={(day, time) => onschedule(row.id, day, time)}
        >
          {#snippet trigger({ props })}<button
              {...props}
              type="button"
              class="property-value"
              data-empty={row.dueDate === null}
              data-tone={row.status === "done" ? null : dueTone(row.dueDate, today)}
              disabled={trashed}
              >{row.dueDate
                ? dueLabel(row.dueDate, today, DUE_LABELS, row.dueTime)
                : m.task_due_none()}</button
            >{/snippet}
        </DueMenu>{/snippet}
      {@render property(Calendar03Icon, m.task_date(), dateValue)}

      {#snippet deadlineValue()}<DueMenu
          due={row.deadline}
          {today}
          timeless
          label={m.task_deadline()}
          clearLabel={m.task_deadline_clear()}
          onselect={(day) => ondeadline(row.id, day)}
        >
          {#snippet trigger({ props })}<button
              {...props}
              type="button"
              class="property-value"
              data-empty={row.deadline === null}
              data-tone={row.status === "done" ? null : dueTone(row.deadline, today)}
              data-deadline
              disabled={trashed}
              >{row.deadline
                ? dueLabel(row.deadline, today, DUE_LABELS)
                : m.task_deadline_none()}</button
            >{/snippet}
        </DueMenu>{/snippet}
      {@render property(Flag02Icon, m.task_deadline(), deadlineValue)}

      {#snippet durationValue()}<Menu
          label={m.task_duration()}
          triggerClass="property-value"
          entries={estimateMenu}
          onselect={(id) => onduration(row.id, id === "none" ? null : Number(id))}
          >{#snippet trigger()}<span data-empty={row.duration === null}
              >{row.duration
                ? durationLabel(row.duration, DURATION_LABELS)
                : m.task_duration_none()}</span
            >{/snippet}</Menu
        >{/snippet}
      {@render property(Clock01Icon, m.task_duration(), durationValue)}

      {#snippet listValue()}<Menu
          label={m.task_organization()}
          triggerClass="property-value"
          entries={listMenu}
          onselect={(id) =>
            onorganize(row.id, id === "inbox" || id === "none" ? null : id, id === "inbox")}
          >{#snippet trigger()}<span>{listTitle}</span>{/snippet}</Menu
        >{/snippet}
      {@render property(row.inbox ? InboxIcon : Folder01Icon, m.task_organization(), listValue)}

      {#snippet priorityValue()}<Menu
          label={m.task_priority()}
          triggerClass="property-value"
          entries={priorityMenu}
          onselect={(id) => onpriority(row.id, id as TaskPriority)}
          >{#snippet trigger()}<span data-empty={row.priority === "none"}
              >{PRIORITIES.find((entry) => entry.id === row.priority)!.label()}</span
            >{/snippet}</Menu
        >{/snippet}
      {@render property(PRIORITY_ICON[row.priority], m.task_priority(), priorityValue)}
    </div>

    {#if row.origin === "agent" || row.assignee === "agent"}<p class="detail-agent">
        <Icon icon={SparklesIcon} size={13} />{[
          row.origin === "agent" ? m.task_from_agent() : "",
          row.assignee === "agent" ? m.task_assigned_agent() : "",
        ]
          .filter(Boolean)
          .join(" · ")}
      </p>{/if}

    <textarea
      class="detail-notes"
      use:fit={row.description}
      aria-label={m.task_description()}
      placeholder={m.task_notes_placeholder()}
      maxlength="4096"
      rows="2"
      readonly={trashed || row.description === null}
      value={row.description ?? ""}
      oninput={(event) => ondescribe(row.id, event.currentTarget.value)}
      onblur={() => oncommit?.(row.id)}></textarea>

    <TaskSubtasks
      steps={row.steps ?? []}
      disabled={trashed || row.description === null}
      onchange={(steps) => onsteps(row.id, steps)}
      onrename={(step, title) => onsteprename(row.id, step, title)}
      onsettle={() => oncommit?.(row.id)}
    />

    {#if row.context}<button
        type="button"
        class="context-link"
        title={row.context.url}
        onclick={() => onopenpage(row.context!.url)}
        ><span class="context-mark"><SiteMark url={row.context.url} size={16} /></span><span
          class="context-copy"
          ><strong>{row.context.title || hostOf(row.context.url)}</strong><small
            >{hostOf(row.context.url)}</small
          ></span
        ><Icon icon={ArrowUpRight01Icon} size={14} /></button
      >{/if}

    {#if footnote}<p class="detail-footnote">{footnote}</p>{/if}
  </div>
{/snippet}

<aside class="detail" data-compact={compact} aria-label={m.task_detail()}>
  <header class="detail-toolbar">
    {#if onclose}<IconButton
        icon={compact ? ArrowLeft02Icon : Cancel01Icon}
        label={compact ? m.task_back_list() : m.task_close_detail()}
        onclick={onclose}
      />{/if}
    <span class="detail-save" role="status">{slow.current ? m.task_saving() : ""}</span>
    <!-- The few things a task can have done to it, shown rather than folded
         into a menu that would hold only them. -->
    {#if task}
      {#if task.context}<IconButton
          icon={ArrowUpRight01Icon}
          label={m.task_open_page()}
          onclick={() => task?.context && onopenpage(task.context.url)}
        />{/if}
      {#if !trashed}<IconButton
          icon={PinIcon}
          label={task.pinned ? m.task_unpin() : m.resource_pin()}
          active={task.pinned}
          onclick={() => task && onpin(task.id, !task.pinned)}
        />{/if}
      <IconButton
        icon={trashed ? ArrowTurnBackwardIcon : Delete02Icon}
        label={trashed ? m.task_restore() : m.task_delete()}
        onclick={() => task && onremove(task.id)}
      />
    {/if}
  </header>
  {#if task}
    {#key task.id}
      {@render body(task)}
    {/key}
  {/if}
</aside>

<style>
  .detail {
    display: flex;
    flex: 1;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
    color: var(--color-text);
    animation: detail-in var(--motion-base) var(--ease-out);
  }

  /* Moving between tasks is the same pane showing another one, so the
     content only cross-fades; arriving from the side is the pane's own
     motion when it first opens (detail-motion.ts), never repeated per task. */
  @keyframes detail-in {
    from {
      opacity: 0;
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .detail {
      animation: none;
    }
  }

  .detail-toolbar {
    display: flex;
    align-items: center;
    gap: 8px;
    flex: none;
    height: 48px;
    padding-inline: 12px;
  }

  .detail-save {
    flex: 1;
    color: var(--color-faint);
    font-size: var(--text-caption);
  }

  .detail-body {
    display: flex;
    flex: 1;
    flex-direction: column;
    gap: 18px;
    padding: 4px 24px 20px;
    overflow-y: auto;
    overscroll-behavior: contain;
  }

  .detail[data-compact="true"] .detail-body {
    padding-inline: 14px;
  }

  .detail-body > * {
    flex: none;
  }

  .detail-title-row {
    display: flex;
    align-items: flex-start;
    gap: 12px;
  }

  .detail-title-row :global(.task-check) {
    margin-block-start: 5px;
  }

  .detail-title {
    width: 100%;
    min-width: 0;
    padding: 0;
    border: 0;
    border-radius: 2px;
    background: transparent;
    color: var(--color-text);
    font: inherit;
    font-size: 20px;
    font-weight: 600;
    line-height: 28px;
    letter-spacing: -0.022em;
    resize: none;
    outline: none;
  }

  .detail-title[data-status="done"] {
    color: var(--color-muted);
  }

  .detail-properties {
    display: grid;
    gap: 2px;
    margin-inline: -6px;
  }

  .property {
    display: grid;
    grid-template-columns: 112px minmax(0, 1fr);
    align-items: center;
    min-height: 30px;
    font-size: var(--text-body);
  }

  .property-label {
    display: flex;
    align-items: center;
    gap: 8px;
    padding-inline-start: 6px;
    color: var(--color-faint);
  }

  .property :global(.property-value) {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    justify-self: start;
    max-width: 100%;
    min-height: 28px;
    padding: 0 8px;
    border: 0;
    border-radius: var(--radius-inset);
    background: transparent;
    color: var(--color-text);
    font: inherit;
    text-align: start;
    cursor: default;
    outline: none;
    transition: background-color var(--motion-instant) var(--ease-smooth);
  }

  .property :global(.property-value > span) {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .property :global(.property-value:hover:not(:disabled)),
  .property :global(.property-value[data-state="open"]) {
    background: var(--row-hover);
  }

  .property :global(.property-value:focus-visible) {
    outline: 2px solid var(--color-ring);
    outline-offset: -2px;
  }

  .property :global([data-empty="true"]) {
    color: var(--color-faint);
  }

  .property :global(.property-value[data-tone="overdue"]) {
    color: var(--color-danger);
  }

  .property :global(.property-value[data-deadline][data-tone="today"]) {
    color: var(--color-warning);
  }

  .state-value[data-status="blocked"] {
    color: var(--color-warning);
  }

  .state-value[data-status="done"] {
    color: var(--color-muted);
  }

  .detail-agent {
    display: flex;
    align-items: center;
    gap: 6px;
    margin: -6px 0 0;
    color: var(--color-muted);
    font-size: var(--text-label);
  }

  .detail-notes {
    box-sizing: border-box;
    width: 100%;
    min-height: 48px;
    padding: 12px 0 0;
    border: 0;
    border-block-start: 1px solid var(--color-border);
    border-radius: 0;
    background: transparent;
    color: var(--color-text);
    font: inherit;
    font-size: 14px;
    line-height: 1.6;
    resize: none;
    outline: none;
  }

  .detail-notes::placeholder {
    color: var(--color-faint);
  }

  .context-link {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px 10px 8px 8px;
    border: 1px solid var(--color-border);
    border-radius: var(--radius-row);
    background: transparent;
    color: var(--color-faint);
    font: inherit;
    text-align: start;
    cursor: default;
    outline: none;
    transition: background-color var(--motion-instant) var(--ease-smooth);
  }

  .context-link:hover {
    background: var(--row-hover);
  }

  .context-link:focus-visible {
    outline: 2px solid var(--color-ring);
    outline-offset: 2px;
  }

  .context-mark {
    display: grid;
    place-items: center;
    flex: none;
    width: 28px;
    height: 28px;
    border-radius: var(--radius-inset);
    background: var(--color-fill);
    color: var(--color-muted);
  }

  .context-copy {
    display: grid;
    gap: 2px;
    flex: 1;
    min-width: 0;
  }

  .context-copy strong {
    overflow: hidden;
    color: var(--color-text);
    font-size: var(--text-label);
    font-weight: 500;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .context-copy small {
    font-size: var(--text-caption);
  }

  /* When it was made rests at the foot of the pane, clear of the content. */
  .detail-footnote {
    margin: auto 0 0;
    padding-block-start: 24px;
    color: var(--color-faint);
    font-size: var(--text-caption);
    font-variant-numeric: tabular-nums;
    text-align: center;
  }

  /* The chrome around it is not selectable; the words being written are. */
  .detail-title,
  textarea {
    /* stylelint-disable-next-line property-no-vendor-prefix */
    -webkit-user-select: text;
    user-select: text;
  }
</style>
