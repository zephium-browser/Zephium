<script lang="ts">
  import { tick, untrack } from "svelte";
  import { commands } from "$shared/ipc/bindings";
  import type { TaskContext, TaskRow, TaskSession } from "$domain/resources";
  import EmptyState from "$shared/ui/EmptyState";
  import Icon from "$shared/ui/Icon";
  import Button from "$shared/ui/Button";
  import { CheckListIcon, Delete02Icon } from "@hugeicons/core-free-icons";
  import * as m from "$shared/i18n/messages";
  import type { TaskScope } from "../lib/task-sections";
  import { today, watchToday } from "../lib/today.svelte";
  import TaskComposer from "./TaskComposer.svelte";
  import TaskList from "./TaskList.svelte";
  import TaskNotice from "./TaskNotice.svelte";
  import TaskFailure from "./TaskFailure.svelte";
  import LazyView from "$shared/ui/LazyView";
  const loadDetail = () => import("./TaskDetail.svelte");

  let {
    session,
    scope = "today",
    density = "panel",
    query = "",
    listId = null,
    page = null,
    compact = false,
    composing = false,
    inlineDetail = true,
    notices = true,
    onselected,
    oncomposed,
  }: {
    session: TaskSession;
    scope?: TaskScope;
    density?: "panel" | "rail" | "page";
    query?: string;
    listId?: string | null;
    /** The page in front of the reader, which a new task can link to. */
    page?: TaskContext | null;
    /** Bare property marks in the composer, for the narrow sidebar column. */
    compact?: boolean;
    composing?: boolean;
    inlineDetail?: boolean;
    /** False where the host draws the undo notice itself. */
    notices?: boolean;
    onselected?: (id: string | null) => void;
    oncomposed?: () => void;
  } = $props();
  let composer = $state<ReturnType<typeof TaskComposer>>();
  let root = $state<HTMLElement>();
  let spoken = $state("");
  let opened = $state(false);
  let selected = $derived(session.rows.find((row) => row.id === session.selectedId) ?? null);
  let searching = $derived(query.trim().length > 0);
  let concealed = $derived(inlineDetail && opened && selected !== null);
  // Hidden behind the open task, the list keeps the rows it last drew rather
  // than grouping them again for every key typed into that task.
  let drawn: readonly TaskRow[] = [];
  let listed = $derived.by(() => {
    if (!concealed) drawn = session.rows;
    return drawn;
  });

  $effect(() => watchToday());
  $effect(() => {
    const view = scope;
    const day = today();
    const list = listId;
    untrack(() => session.setView(view, day, list));
  });
  $effect(() => {
    if (!composing) return;
    untrack(() => {
      opened = false;
      void tick().then(() => composer?.focus());
      oncomposed?.();
    });
  });

  const open = (url: string) => void commands.browserOpenUrl(url, false);
  function select(id: string | null, activate = true) {
    session.selectedId = id;
    if (activate) opened = id !== null;
    if (id) void session.load(id);
    onselected?.(id);
  }
  async function closeDetail() {
    const id = session.selectedId;
    opened = false;
    await tick();
    root
      ?.querySelector<HTMLElement>(`[data-task-id="${CSS.escape(id ?? "")}"] .task-title`)
      ?.focus();
  }
  function announce(message: string) {
    spoken = "";
    queueMicrotask(() => (spoken = message));
  }
  function keydown(event: KeyboardEvent) {
    const target = event.target as HTMLElement;
    if (event.defaultPrevented || target.closest("input,textarea,select,[contenteditable=true]"))
      return;
    if (event.key === "Escape" && opened && inlineDetail) {
      event.preventDefault();
      void closeDetail();
      return;
    }
    if (
      !(event.metaKey || event.ctrlKey) ||
      event.shiftKey ||
      event.key.toLowerCase() !== "z" ||
      !session.undoable
    )
      return;
    event.preventDefault();
    void session.undo();
  }
</script>

<!-- svelte-ignore a11y_no_noninteractive_element_interactions, a11y_no_noninteractive_tabindex -->
<div
  bind:this={root}
  class="tasks"
  data-density={density}
  data-tasks-root
  data-tauri-drag-region="false"
  role="group"
  aria-label={m.tool_tasks()}
  tabindex={-1}
  onkeydown={keydown}
>
  <TaskFailure {session} />
  <div class="task-list-view" class:concealed>
    {#if !session.trash && scope !== "completed"}<TaskComposer
        bind:this={composer}
        bind:value={session.captureDraft}
        bind:context={session.captureContext}
        {page}
        {compact}
        {density}
        lists={session.lists}
        initialList={listId}
        defaultDate={scope === "today" ? today() : null}
        today={today()}
        oncreate={(input) => session.create(input)}
        onexit={() => root?.querySelector<HTMLElement>(".task-title")?.focus()}
      />{/if}
    {#if session.loading && session.rows.length === 0}<p class="task-loading" role="status">
        {m.task_loading()}
      </p>{:else if session.rows.length === 0 && !session.failure}
      <EmptyState
        title={session.trash
          ? m.task_trash_empty()
          : searching
            ? m.task_empty_search()
            : scope === "inbox"
              ? m.task_empty_inbox()
              : listId
                ? m.task_empty_list()
                : scope === "all"
                  ? m.task_empty_all()
                  : scope === "completed"
                    ? m.task_empty_completed()
                    : scope === "upcoming"
                      ? m.task_empty_upcoming()
                      : m.task_empty_title()}
        description={session.trash
          ? m.task_trash_empty_help()
          : searching
            ? m.task_empty_search_help()
            : scope === "inbox"
              ? m.task_inbox_help()
              : listId
                ? m.task_list_help()
                : m.task_empty_help()}
      >
        {#snippet icon()}<Icon
            icon={session.trash ? Delete02Icon : CheckListIcon}
            size={24}
          />{/snippet}
      </EmptyState>
    {:else}
      <TaskList
        rows={listed}
        {scope}
        {density}
        {query}
        {announce}
        {listId}
        lists={session.lists}
        selectedId={session.selectedId}
        onselected={select}
        trashed={session.trash}
        ontoggle={(id, status) => session.setStatus(id, status)}
        onschedule={(id, day, time) => void session.schedule(id, day, time)}
        onrename={(id, title) => {
          // A row commits its new title once, on Enter or leaving it.
          session.rename(id, title);
          session.commitText(id);
        }}
        onpin={(id, pinned) => void session.setPinned(id, pinned)}
        ondelete={(id) => session.setTrashed(id, true)}
        onrestore={(id) => session.setTrashed(id, false)}
        onopenpage={open}
        onposition={(id, key) => void session.setPosition(id, key)}
      />
      {#if session.next}<div class="task-more">
          <Button
            size="compact"
            variant="ghost"
            disabled={session.loading}
            onclick={() => void session.reload(true)}>{m.task_more_tasks()}</Button
          >
        </div>{/if}
    {/if}
  </div>
  {#if inlineDetail && opened && selected}<LazyView
      loader={loadDetail}
      loadingLabel={m.task_loading()}
      failureLabel={m.task_read_failed()}
      retryLabel={m.surface_retry()}
      >{#snippet children(TaskDetail)}<TaskDetail
          task={selected}
          saving={session.saving(selected.id)}
          today={today()}
          lists={session.lists}
          compact
          trashed={session.trash}
          onclose={() => void closeDetail()}
          ontoggle={(id, status) => void session.setStatus(id, status)}
          onschedule={(id, day, time) => void session.schedule(id, day, time)}
          ondeadline={(id, day) => void session.setDeadline(id, day)}
          onduration={(id, minutes) => void session.setDuration(id, minutes)}
          onorganize={(id, list, inbox) => void session.organize(id, list, inbox)}
          onpriority={(id, priority) => void session.prioritize(id, priority)}
          onsteps={(id, steps) => session.updateSteps(id, steps)}
          onsteprename={(id, step, title) => session.renameStep(id, step, title)}
          onrename={(id, title) => session.rename(id, title)}
          ondescribe={(id, text) => session.describe(id, text)}
          oncommit={(id) => session.commitText(id)}
          onpin={(id, pinned) => void session.setPinned(id, pinned)}
          onremove={(id) => {
            opened = false;
            void session.setTrashed(id, !session.trash);
          }}
          onopenpage={open}
        />{/snippet}</LazyView
    >{/if}
  {#if notices}<TaskNotice
      notice={session.notice}
      onundo={() => void session.undo()}
      ondismiss={() => session.dismissNotice()}
    />{/if}
  <p class="task-live" role="status" aria-live="polite">{spoken}</p>
</div>

<style>
  .tasks,
  .task-list-view {
    display: flex;
    flex: 1;
    flex-direction: column;
    min-height: 0;
    min-width: 0;
    outline: none;
  }

  .tasks {
    position: relative;
    padding-inline: 8px;
  }

  .tasks[data-density="page"] {
    padding-inline: 0;
  }

  .concealed {
    display: none;
  }

  .task-loading {
    padding: 16px;
    color: var(--color-muted);
    font-size: var(--text-body);
  }

  .task-more {
    display: flex;
    justify-content: center;
    padding-block: 12px;
  }

  .task-live {
    position: absolute;
    width: 1px;
    height: 1px;
    margin: -1px;
    padding: 0;
    overflow: hidden;
    clip-path: inset(50%);
    white-space: nowrap;
  }
</style>
