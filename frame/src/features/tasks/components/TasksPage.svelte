<script lang="ts">
  import { tick, untrack } from "svelte";
  import { MediaQuery } from "svelte/reactivity";
  import { commands } from "$shared/ipc/bindings";
  import { taskSession } from "$domain/resources";
  import { surface as browser } from "$domain/surface";
  import { tabs } from "$domain/tabs";
  import SegmentedControl from "$shared/ui/SegmentedControl";
  import SearchField from "$shared/ui/SearchField";
  import LazyView from "$shared/ui/LazyView";
  import * as m from "$shared/i18n/messages";
  import type { TaskScope } from "../lib/task-sections";
  import { today, watchToday } from "../lib/today.svelte";
  import { pageView, setPageView, setPageProfile } from "../lib/page-view.svelte";
  import Tasks from "./Tasks.svelte";
  import TasksNavigation from "./TasksNavigation.svelte";
  import TaskNotice from "./TaskNotice.svelte";
  import TaskFailure from "./TaskFailure.svelte";
  import * as detailMotion from "../lib/detail-motion";
  const loadBoard = () => import("./TaskBoard.svelte");
  const loadDetail = () => import("./TaskDetail.svelte");

  let profile = $derived(tabs.profile()?.id ?? "unbound");
  let session = $state.raw(untrack(() => taskSession(profile, "page")));
  let query = $state(untrack(() => session.query));
  let composing = $state(false);
  let root = $state<HTMLElement>();
  $effect(() => {
    const owner = profile;
    return untrack(() => {
      setPageProfile(owner);
      const current = taskSession(owner, "page");
      session = current;
      query = current.query;
      void current.start(current.query);
      return () => current.stop();
    });
  });
  $effect(() => watchToday());
  let view = $derived(pageView());
  $effect(() => {
    const trashed = view.trashed;
    const scope = view.scope;
    const day = today();
    const list = view.list;
    untrack(() => {
      session.showTrash(trashed);
      session.setView(scope, day, list);
    });
  });
  let page = $derived(tabs.activeTab());
  let capturable = $derived(
    page?.url && /^https?:\/\//iu.test(page.url) ? { url: page.url, title: page.title } : null,
  );
  const wide = new MediaQuery("(min-width: 1100px)");
  let selected = $derived(session.rows.find((row) => row.id === session.selectedId) ?? null);

  // Opening and closing a task beside the list is measured once before and
  // once after, and carried out by transform (see detail-motion.ts).
  let stage = $state<HTMLElement>();
  let open = $derived(selected !== null);
  let shownOpen: boolean | null = null;
  let panes: ReturnType<typeof detailMotion.capture> = null;
  $effect.pre(() => {
    const next = open;
    untrack(() => {
      if (shownOpen !== null && shownOpen !== next && wide.current) {
        panes = detailMotion.capture(stage);
      }
    });
  });
  $effect(() => {
    const next = open;
    untrack(() => {
      if (shownOpen !== null && shownOpen !== next) detailMotion.play(stage, panes);
      panes = null;
      shownOpen = next;
    });
  });
  // The first task opened must not wait on its own code.
  $effect(() => void loadDetail());
  let searching = $derived(query.trim().length > 0);
  // A status board only means something where finished work is part of the
  // view: date views hold open tasks only, so their Done column is always empty.
  let boardable = $derived(!view.trashed && !searching && (view.scope === "all" || !!view.list));
  let board = $derived(view.board && boardable);
  let list = $derived(session.lists.find((entry) => entry.id === view.list) ?? null);
  const labels: Record<TaskScope, () => string> = {
    inbox: m.task_scope_inbox,
    today: m.task_scope_today,
    upcoming: m.task_scope_upcoming,
    all: m.task_scope_all,
    completed: m.task_scope_completed,
  };
  const longDate = new Intl.DateTimeFormat(undefined, {
    weekday: "long",
    month: "long",
    day: "numeric",
    timeZone: "UTC",
  });
  let heading = $derived(
    searching
      ? m.task_search_results()
      : view.trashed
        ? m.resource_show_trash()
        : view.list
          ? (list?.title ?? m.task_scope_all())
          : labels[view.scope](),
  );
  let subtitle = $derived.by(() => {
    if (searching) return m.task_count({ count: session.rows.length });
    const counts = session.counts;
    if (view.trashed) return m.task_count({ count: counts.trash });
    if (view.list) return m.task_count({ count: list?.count ?? 0 });
    if (view.scope === "today") {
      const [year, month, day] = today().split("-").map(Number);
      const date = longDate.format(new Date(Date.UTC(year!, month! - 1, day!)));
      return counts.overdue
        ? `${date} · ${m.task_summary_overdue({ count: counts.overdue })}`
        : date;
    }
    const total = {
      inbox: counts.inbox,
      upcoming: counts.upcoming,
      all: counts.all,
      completed: counts.completed,
    }[view.scope];
    return m.task_count({ count: total });
  });

  function compose() {
    session.selectedId = null;
    if (view.trashed || view.scope === "completed") setPageView({ scope: "all", trashed: false });
    setPageView({ board: false });
    composing = true;
  }
  async function closeDetail() {
    const id = session.selectedId;
    session.selectedId = null;
    await tick();
    root
      ?.querySelector<HTMLElement>(
        `[data-task-id="${CSS.escape(id ?? "")}"] .task-title, [data-task-card="${CSS.escape(id ?? "")}"] .board-card-title`,
      )
      ?.focus();
  }
  function keydown(event: KeyboardEvent) {
    if (
      event.defaultPrevented ||
      (event.target as HTMLElement).closest("input,textarea,select,[contenteditable=true]")
    )
      return;
    const plain = !event.metaKey && !event.ctrlKey && !event.altKey;
    if (event.key === "Escape" && selected) {
      event.preventDefault();
      void closeDetail();
    } else if (plain && event.key.toLowerCase() === "n" && !view.trashed) {
      event.preventDefault();
      compose();
    } else if (plain && event.key === "/") {
      event.preventDefault();
      root?.querySelector<HTMLInputElement>(".page-tools input")?.focus();
    } else if (
      (event.metaKey || event.ctrlKey) &&
      !event.shiftKey &&
      event.key.toLowerCase() === "z" &&
      session.undoable
    ) {
      event.preventDefault();
      void session.undo();
    }
  }
</script>

<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<section
  bind:this={root}
  class="library-shell task-page"
  role="group"
  aria-label={m.tool_tasks()}
  onkeydown={keydown}
>
  <div class="page-nav">
    <TasksNavigation {session} onclose={() => void browser.open(null)} />
  </div>
  <div
    bind:this={stage}
    class="page-stage"
    class:has-detail={selected !== null}
    class:compact={!wide.current}
  >
    <div class="page-list" class:concealed={!wide.current && selected !== null}>
      <div class="page-column" class:board>
        <header class="page-head">
          <div class="page-heading">
            <h1>{heading}</h1>
            <p>{subtitle}</p>
          </div>
          <div class="page-tools">
            <SearchField
              label={m.tool_search_tasks()}
              placeholder={m.tool_search_tasks()}
              size="chrome"
              value={query}
              oninput={(value) => {
                query = value;
                session.search(value);
              }}
            />
            {#if boardable}<SegmentedControl
                label={m.task_view()}
                value={board ? "board" : "list"}
                options={[
                  { value: "list", label: m.task_list_view() },
                  { value: "board", label: m.task_board() },
                ]}
                onchange={(value) => setPageView({ board: value === "board" })}
              />{/if}
          </div>
        </header>
        {#if board}
          <TaskFailure {session} />
          <LazyView
            loader={loadBoard}
            loadingLabel={m.task_loading()}
            failureLabel={m.task_read_failed()}
            retryLabel={m.surface_retry()}
            >{#snippet children(TaskBoard)}<TaskBoard
                rows={session.rows}
                selectedId={session.selectedId}
                onmove={(id, status, key) => session.move(id, status, key)}
                onselect={(id) => void session.load(id)}
                onopenpage={(url) => void commands.browserOpenUrl(url, false)}
              />{/snippet}</LazyView
          >
        {:else}<Tasks
            {session}
            scope={view.scope}
            listId={view.list}
            density="page"
            page={capturable}
            {query}
            {composing}
            inlineDetail={false}
            notices={false}
            oncomposed={() => (composing = false)}
          />{/if}
      </div>
    </div>
    {#if selected}<div class="page-detail">
        <LazyView
          loader={loadDetail}
          loadingLabel={m.task_loading()}
          failureLabel={m.task_read_failed()}
          retryLabel={m.surface_retry()}
          >{#snippet children(TaskDetail)}{#key selected.id}
              <TaskDetail
                task={selected}
                saving={session.saving(selected.id)}
                today={today()}
                lists={session.lists}
                compact={!wide.current}
                trashed={view.trashed}
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
                  session.selectedId = null;
                  void session.setTrashed(id, !view.trashed);
                }}
                onopenpage={(url) => void commands.browserOpenUrl(url, false)}
              />{/key}{/snippet}</LazyView
        >
      </div>{/if}
    <TaskNotice
      notice={session.notice}
      onundo={() => void session.undo()}
      ondismiss={() => session.dismissNotice()}
    />
  </div>
</section>

<style>
  /* Two surfaces side by side: navigation, and the list with its inspector.
     Both are content, so both carry the content surface and its corners. */
  .task-page {
    position: relative;
    display: grid;
    grid-template-columns: 240px minmax(0, 1fr);
    gap: 8px;
    min-width: 0;
    border-radius: 0;
    background: transparent;
    /* stylelint-disable-next-line property-no-vendor-prefix */
    -webkit-user-select: none;
    user-select: none;
  }

  .page-nav {
    display: flex;
    flex-direction: column;
    min-height: 0;
    overflow: hidden;
    border-radius: var(--content-radius);
    background: var(--color-page);
  }

  .page-nav > :global(.task-navigation) {
    flex: 1;
  }

  .page-stage {
    display: grid;
    grid-template-columns: minmax(0, 1fr);
    min-width: 0;
    min-height: 0;
    position: relative;
    overflow: hidden;
    border-radius: var(--content-radius);
    background: var(--color-page);
  }

  .page-stage.has-detail:not(.compact) {
    grid-template-columns: minmax(360px, 1fr) clamp(340px, 34%, 440px);
  }

  .page-list {
    display: flex;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
    padding: 0 24px;
  }

  /* A reading column: rows stay close to their own metadata however wide the
     window grows, and the board alone takes the full width it needs. */
  .page-column {
    display: flex;
    flex: 1;
    flex-direction: column;
    width: 100%;
    max-width: 780px;
    min-height: 0;
    margin-inline: auto;
  }

  .page-column.board {
    max-width: none;
  }

  /* The tools line up with the title itself, not with the count under it. */
  .page-head {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    flex-wrap: wrap;
    gap: 12px 16px;
    flex: none;
    padding: 28px 10px 14px;
  }

  .page-heading {
    min-width: 0;
  }

  .page-heading h1 {
    margin: 0;
    overflow: hidden;
    color: var(--color-text);
    font-size: 26px;
    font-weight: 650;
    line-height: 32px;
    letter-spacing: -0.03em;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .page-heading p {
    margin: 2px 0 0;
    color: var(--color-faint);
    font-size: var(--text-body);
    font-variant-numeric: tabular-nums;
  }

  .page-tools {
    display: flex;
    align-items: center;
    gap: 8px;
    min-height: 32px;
  }

  .page-tools :global(.ui-search) {
    width: 200px;
    min-width: 120px;
  }

  .page-detail {
    display: flex;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
    border-inline-start: 1px solid var(--color-border);
  }

  .compact .page-detail {
    border-inline-start: 0;
  }

  .concealed {
    display: none;
  }

  @media (width < 900px) {
    .task-page {
      grid-template-columns: 200px minmax(0, 1fr);
    }

    .page-list {
      padding-inline: 8px;
    }
  }
</style>
