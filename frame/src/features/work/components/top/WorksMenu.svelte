<script lang="ts">
  import { isComposing } from "$shared/lib/keyboard";
  import { Popover } from "bits-ui";
  import { tick } from "svelte";
  import type { WorkEnvironmentSummary } from "$shared/ipc/bindings";
  import type { WorkEnvironmentSession } from "$domain/work-environment";
  import Icon from "$shared/ui/Icon";
  import SearchField from "$shared/ui/SearchField";
  import {
    Add01Icon,
    ArrowDown01Icon,
    ArrowRight01Icon,
    MoreHorizontalIcon,
    Tick02Icon,
  } from "../../lib/icons";
  import WorkRowMenu from "./WorkRowMenu.svelte";
  import { defaultTitle, listedName, workedAt } from "../../lib/work-name";
  import { ulidTime } from "../../lib/ulid-time";
  import * as m from "$shared/i18n/messages";
  import "$shared/ui/Menu/popover.css";

  let {
    session,
    untitled,
    live = false,
  }: {
    session: WorkEnvironmentSession;
    /** The name a work carries until its first request names it. */
    untitled: string;
    /** The open work's agent is working. */
    live?: boolean;
  } = $props();

  let open = $state(false);
  let query = $state("");
  let field = $state<HTMLInputElement>();
  let list = $state<HTMLElement>();
  let showArchived = $state(false);
  let renaming = $state<string | null>(null);
  let draft = $state("");

  const works = $derived(session.works);
  const current = $derived(session.snapshot?.id ?? session.selected ?? null);
  const title = $derived(session.snapshot?.title ?? "");
  const busy = $derived(!!session.pending || session.loading);
  const needle = $derived(query.trim().toLocaleLowerCase());
  const matches = (work: WorkEnvironmentSummary) =>
    !needle ||
    [shown(work), ...work.requests].some((text) => text.toLocaleLowerCase().includes(needle));
  /** Most recently worked in first; an untouched draft is not a work to go back to. */
  const listed = $derived(
    works
      .filter((work) => !(work.empty && defaultTitle(work.title, untitled)) && matches(work))
      .sort((a, b) => touched(b) - touched(a)),
  );
  const active = $derived(listed.filter((work) => work.lifecycle === "active"));
  const archived = $derived(listed.filter((work) => work.lifecycle === "archived"));
  const now = $derived(open ? Date.now() : 0);

  function touched(work: WorkEnvironmentSummary) {
    return Number(work.touched_ms) || ulidTime(work.id)?.getTime() || 0;
  }
  function shown(work: WorkEnvironmentSummary) {
    return listedName(work, untitled) || m.work_untitled();
  }
  /** The person's first words under the name, unless the name is already made of them. */
  function asked(work: WorkEnvironmentSummary) {
    const first = work.requests[0] ?? "";
    return first.toLocaleLowerCase().startsWith(shown(work).toLocaleLowerCase()) ? "" : first;
  }
  const openWork = $derived(works.find((work) => work.id === current));
  const heading = $derived(
    openWork && !openWork.empty
      ? shown(openWork)
      : title && !defaultTitle(title, untitled)
        ? title
        : "",
  );

  async function opened(next: boolean) {
    open = next;
    if (!next) {
      renaming = null;
      return;
    }
    query = "";
    showArchived = false;
    await tick();
    field?.focus();
  }

  function choose(work: WorkEnvironmentSummary) {
    if (renaming === work.id) return;
    open = false;
    if (work.id !== current) void session.open(work.id);
  }

  async function create() {
    if (busy) return;
    open = false;
    await session.create(untitled);
  }

  async function beginRename(work: WorkEnvironmentSummary) {
    renaming = work.id;
    draft = defaultTitle(work.title, untitled) ? listedName(work, untitled) : work.title;
    await tick();
    const input = list?.querySelector<HTMLInputElement>("[data-work-rename]");
    input?.focus();
    input?.select();
  }

  async function commitRename(work: WorkEnvironmentSummary) {
    if (renaming !== work.id) return;
    renaming = null;
    const next = draft.trim();
    if (!next || next === work.title) return;
    const shownWork = current;
    // An edit applies to the open work, so another one is opened for it and the canvas returns.
    if (
      (await session.editWork(work.id, { kind: "rename", title: next })) &&
      shownWork &&
      shownWork !== work.id
    )
      await session.open(shownWork);
  }

  async function archive(work: WorkEnvironmentSummary) {
    const shownWork = current;
    const next =
      shownWork && shownWork !== work.id
        ? shownWork
        : works.find((candidate) => candidate.lifecycle === "active" && candidate.id !== work.id)
            ?.id;
    if (!(await session.editWork(work.id, { kind: "set_lifecycle", lifecycle: "archived" })))
      return;
    if (next) await session.open(next);
    else await session.create(untitled);
  }

  function restore(work: WorkEnvironmentSummary) {
    void session.editWork(work.id, { kind: "set_lifecycle", lifecycle: "active" });
  }

  function rows() {
    return [...(list?.querySelectorAll<HTMLElement>("[data-work-row]") ?? [])];
  }

  function move(event: KeyboardEvent) {
    if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
    const all = rows();
    if (!all.length) return;
    event.preventDefault();
    const index = all.indexOf(document.activeElement as HTMLElement);
    if (index === -1) {
      all[event.key === "ArrowDown" ? 0 : all.length - 1]?.focus();
      return;
    }
    const next = index + (event.key === "ArrowDown" ? 1 : -1);
    if (next < 0) field?.focus();
    else all[Math.min(next, all.length - 1)]?.focus();
  }

  function renameKey(event: KeyboardEvent, work: WorkEnvironmentSummary) {
    if (isComposing(event)) return;
    event.stopPropagation();
    if (event.key === "Enter") {
      event.preventDefault();
      void commitRename(work);
    } else if (event.key === "Escape") {
      event.preventDefault();
      renaming = null;
    }
  }
</script>

{#snippet row(work: WorkEnvironmentSummary)}
  {@const selected = work.id === current}
  <li>
    {#if renaming === work.id}
      <div class="ui-menu-item row editing">
        <input
          class="rename"
          data-work-rename
          aria-label={m.work_title_field()}
          placeholder={m.work_untitled()}
          maxlength="128"
          bind:value={draft}
          onkeydown={(event) => renameKey(event, work)}
          onblur={() => void commitRename(work)}
        />
      </div>
    {:else}
      <WorkRowMenu
        archived={work.lifecycle === "archived"}
        disabled={busy}
        onrename={() => void beginRename(work)}
        onarchive={() => void archive(work)}
        onrestore={() => restore(work)}
        >{#snippet children(openAt)}
          <div class="row-shell">
            <button
              type="button"
              class="ui-menu-item row"
              data-work-row
              aria-current={selected ? "page" : undefined}
              disabled={busy && !selected}
              onclick={() => (work.lifecycle === "archived" ? restore(work) : choose(work))}
              ondblclick={() => void beginRename(work)}
            >
              <span class="words">
                <span class="name">{shown(work)}</span>
                {#if asked(work)}<span class="asked">{asked(work)}</span>{/if}
              </span>
              {#if selected}<span class="mark" aria-hidden="true"
                  ><Icon icon={Tick02Icon} size={14} strokeWidth={2} /></span
                >{:else if touched(work)}<span class="when">{workedAt(touched(work), now)}</span
                >{/if}
            </button>
            <button
              type="button"
              class="options"
              aria-label={m.work_row_options({ title: shown(work) })}
              tabindex="-1"
              onclick={(event) => {
                const rect = event.currentTarget.getBoundingClientRect();
                openAt(rect.left, rect.bottom);
              }}
            >
              <Icon icon={MoreHorizontalIcon} size={15} />
            </button>
          </div>
        {/snippet}</WorkRowMenu
      >
    {/if}
  </li>
{/snippet}

<Popover.Root {open} onOpenChange={(next) => void opened(next)}>
  <Popover.Trigger class="works-trigger" aria-label={m.work_menu_label()}>
    {#if live}<span class="live" aria-hidden="true"></span>{/if}
    <span class="title" class:untitled={!heading}>{heading || m.work_untitled()}</span>
    <Icon icon={ArrowDown01Icon} size={13} strokeWidth={2} />
  </Popover.Trigger>
  <Popover.Portal>
    <Popover.Content
      class="ui-menu works-menu"
      side="bottom"
      align="start"
      sideOffset={6}
      collisionPadding={12}
      aria-label={m.work_menu_label()}
      onkeydown={move}
    >
      <div class="search">
        <SearchField
          label={m.work_menu_search()}
          placeholder={m.work_menu_search()}
          bind:value={query}
          bind:ref={field}
          onsubmit={() => {
            const first = active[0];
            if (first) choose(first);
          }}
        />
      </div>
      <button
        type="button"
        class="ui-menu-item create"
        disabled={busy}
        onclick={() => void create()}
      >
        <span class="ui-menu-icon" aria-hidden="true"><Icon icon={Add01Icon} size={15} /></span>
        {m.work_new()}
      </button>
      <div class="ui-menu-separator" role="presentation"></div>
      <div class="scroller" bind:this={list}>
        {#if active.length}<ul aria-label={m.work_menu_label()}>
            {#each active as work (work.id)}{@render row(work)}{/each}
          </ul>{:else if needle}<p class="empty">{m.work_menu_empty()}</p>{/if}
        {#if archived.length}
          <button
            type="button"
            class="ui-menu-item folder"
            aria-expanded={showArchived || !!needle}
            onclick={() => (showArchived = !showArchived)}
          >
            <span class="chevron" class:open={showArchived || !!needle} aria-hidden="true"
              ><Icon icon={ArrowRight01Icon} size={12} strokeWidth={2} /></span
            >
            {m.work_archived()}
            <span class="count">{archived.length}</span>
          </button>
          {#if showArchived || needle}<ul class="archived" aria-label={m.work_archived()}>
              {#each archived as work (work.id)}{@render row(work)}{/each}
            </ul>{/if}
        {/if}
      </div>
    </Popover.Content>
  </Popover.Portal>
</Popover.Root>

<style>
  :global(.works-trigger) {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    max-inline-size: min(420px, 42vw);
    block-size: 30px;
    padding-inline: 10px 8px;
    border: 0;
    border-radius: var(--radius-control);
    background: transparent;
    color: var(--color-muted);
    font: inherit;
    cursor: default;
    pointer-events: auto;
    transition: background-color var(--motion-fast) var(--ease-out);
  }

  :global(.works-trigger:hover),
  :global(.works-trigger[data-state="open"]) {
    background: var(--color-fill-hover);
  }

  :global(.works-trigger:focus-visible) {
    outline: 2px solid var(--color-ring);
    outline-offset: 1px;
  }

  .title {
    overflow: hidden;
    color: var(--color-muted);
    transition: color var(--motion-fast) var(--ease-out);
    font-size: 14px;
    font-weight: 600;
    letter-spacing: -0.006em;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  :global(.works-trigger:hover) .title,
  :global(.works-trigger[data-state="open"]) .title {
    color: var(--color-text);
  }

  .title.untitled {
    color: var(--color-muted);
  }

  .live {
    flex: none;
    inline-size: 6px;
    block-size: 6px;
    border-radius: var(--radius-capsule);
    background: var(--color-accent);
  }

  /* Over the canvas the menu's glass shows the cards through it: Work's menus
     stand on the solid floating material. */
  :global(.works-menu) {
    background: var(--color-float);
    backdrop-filter: none;
    display: flex;
    flex-direction: column;
    inline-size: 340px;
    max-block-size: min(520px, var(--bits-floating-available-height, 480px));
    transform-origin: var(--bits-floating-transform-origin, top left);
    animation:
      ui-menu-fade var(--motion-fast) var(--ease-out),
      ui-menu-in var(--motion-base) var(--ease-out);
  }

  .search {
    padding: 2px 2px 6px;
  }

  .scroller {
    flex: 1;
    min-block-size: 0;
    overflow-y: auto;
    overscroll-behavior: contain;
  }

  ul {
    margin: 0;
    padding: 0;
    list-style: none;
  }

  button.ui-menu-item {
    inline-size: 100%;
    border: 0;
    background: transparent;
    font: inherit;
    text-align: start;
  }

  button.ui-menu-item:disabled {
    opacity: 0.45;
  }

  button.ui-menu-item:focus-visible,
  button.ui-menu-item:hover:not(:disabled) {
    background: var(--row-active);
  }

  .row-shell {
    position: relative;
  }

  .row {
    padding-inline-end: 34px;
  }

  .row:has(.asked) {
    align-items: flex-start;
    padding-block: 7px;
  }

  .words {
    display: flex;
    flex: 1;
    flex-direction: column;
    gap: 1px;
    min-inline-size: 0;
  }

  .name,
  .asked {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .name {
    font-weight: 500;
  }

  .asked {
    color: var(--color-muted);
    font-size: var(--text-label);
    line-height: 16px;
  }

  .when {
    flex: none;
    color: var(--color-faint);
    font-size: var(--text-caption);
    font-variant-numeric: tabular-nums;
    line-height: 18px;
  }

  .row-shell:hover .when,
  .row-shell:focus-within .when {
    visibility: hidden;
  }

  .mark {
    display: grid;
    flex: none;
    color: var(--color-text);
    place-items: center;
  }

  .options {
    position: absolute;
    inset-block: 0;
    inset-inline-end: 4px;
    display: grid;
    inline-size: 26px;
    margin-block: auto;
    block-size: 22px;
    padding: 0;
    border: 0;
    border-radius: var(--radius-inset);
    background: transparent;
    color: var(--color-muted);
    opacity: 0;
    place-items: center;
    transition: opacity var(--motion-fast) var(--ease-out);
  }

  .row-shell:hover .options,
  .row-shell:focus-within .options {
    opacity: 1;
  }

  .row-shell .options:hover {
    background: var(--color-fill-hover);
    color: var(--color-text);
  }

  .row-shell:hover .mark,
  .row-shell:focus-within .mark {
    visibility: hidden;
  }

  .editing {
    padding-block: 4px;
  }

  .rename {
    flex: 1;
    min-inline-size: 0;
    block-size: 24px;
    padding-inline: 6px;
    border: 0;
    border-radius: var(--radius-inset);
    background: var(--color-fill);
    color: var(--color-text);
    font: inherit;
    outline: none;
    box-shadow: 0 0 0 2px var(--color-ring);
  }

  /* Its count ends where the rows' dates and the check do, not at the menu's edge. */
  .folder {
    margin-block-start: 4px;
    padding-inline-end: 34px;
    color: var(--color-muted);
  }

  .chevron {
    display: grid;
    flex: none;
    inline-size: 16px;
    place-items: center;
    transition: rotate var(--motion-base) var(--ease-out);
  }

  .chevron.open {
    rotate: 90deg;
  }

  .count {
    margin-inline-start: auto;
    color: var(--color-faint);
    font-variant-numeric: tabular-nums;
  }

  .archived .name {
    color: var(--color-muted);
  }

  .row-shell:has(.asked) .options {
    inset-block: 5px auto;
    margin-block: 0;
  }

  .empty {
    margin: 0;
    padding: 10px var(--menu-item-inset);
    color: var(--color-faint);
    font-size: var(--text-body);
  }
</style>
