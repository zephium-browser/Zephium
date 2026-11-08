<script lang="ts">
  import { untrack } from "svelte";
  import type { ToolHostProps } from "$session/tool-drafts.svelte";
  import { taskSession } from "$domain/resources";
  import { surface as browser } from "$domain/surface";
  import { tabs } from "$domain/tabs";
  import { loadTasks, setTaskPageView, type TaskScope } from "$features/tasks";
  import LazyView from "$shared/ui/LazyView";
  import Icon from "$shared/ui/Icon";
  import Menu, { type MenuEntry } from "$shared/ui/Menu";
  import { ArrowDown01Icon, Delete02Icon, MoreHorizontalIcon } from "@hugeicons/core-free-icons";
  import * as m from "$shared/i18n/messages";
  import ToolFrame from "../ToolFrame.svelte";

  let props: ToolHostProps = $props();

  // The host owns the session's lifetime so the frame's search field, filters
  // and footer read the same state the list is drawn from.
  let session = $state.raw(untrack(() => taskSession(props.profile, "sidebar")));
  $effect(() => {
    const owner = props.profile;
    return untrack(() => {
      const current = taskSession(owner, "sidebar");
      session = current;
      void current.start(props.state.query);
      return () => current.stop();
    });
  });

  $effect(() => {
    const value = props.state.query;
    untrack(() => {
      if (value !== session.query) session.search(value);
    });
  });

  // The page a task can reopen. Only a committed address is worth keeping; a
  // blank tab has nothing to return to.
  let page = $derived(tabs.activeTab());
  let capturable = $derived(
    page?.url && /^https?:\/\//iu.test(page.url) ? { url: page.url, title: page.title } : null,
  );

  const SCOPES = ["inbox", "today", "upcoming", "all", "completed"];
  let scope = $derived(
    SCOPES.includes(props.state.filter) ? (props.state.filter as TaskScope) : "today",
  );

  const SCOPE_LABEL: Record<TaskScope, () => string> = {
    inbox: m.task_scope_inbox,
    today: m.task_scope_today,
    upcoming: m.task_scope_upcoming,
    all: m.task_scope_all,
    completed: m.task_scope_completed,
  };

  let scopeEntries: MenuEntry[] = $derived(
    SCOPES.map((value) => ({
      kind: "item",
      id: value,
      label: SCOPE_LABEL[value as TaskScope](),
      checked: !session.trash && scope === value,
    })),
  );

  let overflow: MenuEntry[] = $derived([
    {
      kind: "item",
      id: "trash",
      label: session.trash ? m.resource_show_active() : m.resource_show_trash(),
      icon: Delete02Icon,
    },
  ]);

  async function expand() {
    if (!(await session.flush())) return;
    const destination = taskSession(props.profile, "page");
    destination.selectedId = session.selectedId;
    destination.query = props.state.query;
    destination.captureDraft = session.captureDraft;
    destination.captureContext = session.captureContext;
    session.captureDraft = "";
    session.captureContext = null;
    setTaskPageView(
      { scope, list: session.listId, trashed: session.trash, board: false },
      props.profile,
    );
    void browser.open("tasks");
    // The full page is the same tasks, so the panel does not stay open behind it.
    props.onclose();
  }

  function act(id: string) {
    if (id === "trash") session.showTrash(!session.trash);
  }
</script>

<ToolFrame
  {...props}
  caption={false}
  scrolls={false}
  onexpand={expand}
  searchLabel={m.tool_search_tasks()}
  searchFocus={false}
  onsearchdismiss={() => props.edit({ query: "" })}
>
  {#snippet heading()}
    <!--
      The title is the scope. One control instead of a name and a band of
      segments underneath it, which in this column is most of the chrome.
    -->
    <Menu
      label={m.task_scope()}
      entries={scopeEntries}
      side="bottom"
      align="start"
      triggerClass="task-scope"
      onselect={(id) => {
        if (session.trash) session.showTrash(false);
        props.edit({ filter: id });
      }}
    >
      {#snippet trigger()}<span
          >{session.trash ? m.resource_show_trash() : SCOPE_LABEL[scope]()}</span
        ><Icon icon={ArrowDown01Icon} size={13} />{/snippet}
    </Menu>
  {/snippet}
  {#snippet actions()}
    <Menu
      label={m.task_more()}
      entries={overflow}
      side="bottom"
      align="end"
      triggerClass="task-overflow"
      onselect={act}
    >
      {#snippet trigger()}<Icon icon={MoreHorizontalIcon} size={16} />{/snippet}
    </Menu>
  {/snippet}
  <LazyView
    loader={loadTasks}
    loadingLabel={m.panel_loading()}
    failureLabel={m.panel_load_failed()}
    retryLabel={m.panel_retry()}
    >{#snippet children(Tasks)}<Tasks
        {session}
        {scope}
        page={session.trash ? null : capturable}
        compact
        query={props.state.query}
        composing={props.state.composing}
        oncomposed={() => props.edit({ composing: false })}
      />{/snippet}</LazyView
  >
</ToolFrame>

<style>
  /* One edge runs down the panel: the scope's title, the search glass, the
     composer's plus and every row's check start at the same inset. */
  :global(.shared-tool[data-tool="tasks"] .shared-tool-header) {
    gap: 4px;
    padding-inline: 18px 12px;
  }

  :global(.shared-tool[data-tool="tasks"] .shared-tool-controls) {
    padding: 0 8px 6px;
  }

  :global(.shared-tool[data-tool="tasks"] .shared-tool-search) {
    gap: 12px;
    height: 32px;
    box-sizing: border-box;
    padding: 0 10px 0 11px;
  }

  /* The tool's name and its scope are one control, as wide as its words: a
     title rather than a band across the header, with the actions at the end. */
  :global(.task-scope) {
    display: flex;
    align-items: center;
    gap: 4px;
    flex: none;
    max-width: 100%;
    min-width: 0;
    height: 26px;
    margin-inline: -6px auto;
    padding-inline: 6px;
    border: 0;
    border-radius: var(--radius-inset);
    background: transparent;
    color: var(--color-text);
    font: inherit;
    font-size: 14px;
    font-weight: 550;
    letter-spacing: -0.015em;
    text-align: start;
    cursor: default;
    outline: none;
    transition: background-color var(--motion-instant) var(--ease-smooth);
  }

  :global(.task-scope span) {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  :global(.task-scope:hover),
  :global(.task-scope[data-state="open"]) {
    background: var(--color-fill-hover);
  }

  :global(.task-scope:focus-visible),
  :global(.task-overflow:focus-visible) {
    outline: 2px solid var(--color-ring);
    outline-offset: -2px;
  }

  :global(.task-overflow) {
    display: grid;
    place-items: center;
    flex: none;
    width: 28px;
    height: 28px;
    border: 0;
    border-radius: var(--radius-row);
    background: transparent;
    color: var(--color-muted);
    cursor: default;
    outline: none;
    transition:
      background-color var(--motion-instant) var(--ease-smooth),
      color var(--motion-instant) var(--ease-smooth);
  }

  :global(.task-overflow:hover),
  :global(.task-overflow[data-state="open"]) {
    background: var(--color-fill-hover);
    color: var(--color-text);
  }
</style>
