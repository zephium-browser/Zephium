<script lang="ts">
  import { untrack } from "svelte";
  import {
    ArrowLeft01Icon,
    ArrowRight01Icon,
    Bookmark02Icon,
    Delete02Icon,
    Folder01Icon,
    Globe02Icon,
    PencilEdit02Icon,
  } from "@hugeicons/core-free-icons";
  import type { BookmarkView } from "$shared/ipc/bindings";
  import { commands } from "$shared/ipc/bindings";
  import { favicons } from "$domain/favicons";
  import { keymap } from "$domain/keymap";
  import { uiCommands } from "$domain/ui-commands";
  import * as notices from "$session/notice.svelte";
  import { IS_MAC } from "$shared/platform";
  import { acceleratorKeys } from "$shared/lib/accelerator";
  import { createPointerDrag } from "$shared/lib/pointer-drag.svelte";
  import Button from "$shared/ui/Button";
  import EmptyState from "$shared/ui/EmptyState";
  import FavIcon from "$shared/ui/FavIcon";
  import Icon from "$shared/ui/Icon";
  import IconButton from "$shared/ui/IconButton";
  import * as m from "$shared/i18n/messages";
  import { BookmarksSession } from "../lib/bookmarks-session.svelte";
  import * as compose from "../lib/compose.svelte";
  import * as reveal from "../lib/reveal.svelte";

  let { profile, query }: { profile: string; query: string } = $props();

  let session = $state.raw(untrack(() => new BookmarksSession(profile)));
  let renaming = $state<string | null>(null);
  let draft = $state("");
  /** A folder whose removal waits for a second press, because it takes its
   *  contents with it. */
  let confirming = $state<string | null>(null);
  let adding = $state<{
    address: string;
    name: string;
    pending: boolean;
    problem: "invalid" | "failed" | null;
  } | null>(null);
  /** The row a native menu was opened on; the menu is modal, so one at a time. */
  let menuTarget: BookmarkView | null = null;
  /** A link just deleted, kept long enough to put it back. */
  let deleted = $state<{
    item: { title: string; url: string; parent: string | null };
    index: number | null;
  } | null>(null);
  const UNDO_WINDOW = 6000;

  /** Where a carried bookmark would land: before or after a row of the
   *  folder in view, or inside a folder (a row or a crumb; null is the top). */
  type Landing = { kind: "before" | "after"; id: string } | { kind: "into"; folder: string | null };

  const drag = createPointerDrag<BookmarkView>({
    ondrop: (item, target, at) => {
      const landing = landingAt(item, target, at.y);
      if (!landing) return;
      if (landing.kind === "into") {
        void session.file(item.id, landing.folder);
        return;
      }
      const index = session.items.findIndex((candidate) => candidate.id === landing.id);
      if (index >= 0) void session.reorder(item.id, landing.kind === "after" ? index + 1 : index);
    },
  });

  function landingAt(item: BookmarkView, target: Element | null, y: number): Landing | null {
    const crumb = target?.closest<HTMLElement>("[data-bookmark-crumb]");
    if (crumb) return { kind: "into", folder: crumb.dataset.bookmarkCrumb || null };
    const row = target?.closest<HTMLElement>("[data-bookmark-id]");
    const id = row?.dataset.bookmarkId;
    if (!row || !id || id === item.id) return null;
    const rect = row.getBoundingClientRect();
    const share = (y - rect.top) / rect.height;
    if (row.dataset.folder !== undefined && share > 0.25 && share < 0.75)
      return { kind: "into", folder: id };
    return { kind: share < 0.5 ? "before" : "after", id };
  }

  let landing = $derived.by(() => {
    const item = drag.item;
    if (!item) return null;
    const { x, y } = drag.at;
    return landingAt(item, document.elementFromPoint(x, y), y);
  });

  function press(event: PointerEvent, item: BookmarkView) {
    // Search shows bookmarks from many folders, so there is no order to change.
    if (session.searching || renaming !== null) return;
    drag.begin(event, item);
  }

  function openAdd() {
    confirming = null;
    adding = { address: "", name: "", pending: false, problem: null };
  }

  function leaveAdd(event: KeyboardEvent) {
    if (event.key !== "Escape") return;
    event.stopPropagation();
    adding = null;
  }

  async function submitAdd() {
    const form = adding;
    if (!form || form.pending || !form.address.trim()) return;
    form.pending = true;
    form.problem = null;
    const outcome = await session.addLink(form.address.trim(), form.name.trim());
    form.pending = false;
    if (adding !== form) return;
    if (outcome === "added") adding = null;
    else form.problem = outcome;
  }

  // Only the owning profile decides which session this is; the first view is
  // a requested bookmark when there is one, otherwise the top level.
  $effect(() => {
    const owner = profile;
    return untrack(() => {
      const current = new BookmarksSession(owner);
      session = current;
      adding = null;
      const wanted = reveal.take();
      void (wanted ? current.reveal(wanted) : current.open(null));
      return () => current.stop();
    });
  });

  // Bookmark This Page while the panel is already open.
  $effect(() => {
    if (reveal.requested() === null) return;
    untrack(() => {
      const wanted = reveal.take();
      if (wanted) void session.reveal(wanted);
    });
  });

  // What the header's Add menu asked for.
  $effect(() => {
    if (compose.requested() === null) return;
    untrack(() => {
      const kind = compose.take();
      if (kind === "link") openAdd();
      else if (kind === "folder") void addFolder();
    });
  });

  // The frame owns the search field; mirror its value into the session.
  $effect(() => {
    const value = query;
    untrack(() => {
      if (value !== session.query) session.search(value);
    });
  });

  let addKeys = $derived.by(() => {
    const accelerator = keymap.all().find((entry) => entry.id === "bookmark.add")?.accelerator;
    if (!accelerator) return null;
    return acceleratorKeys(accelerator, IS_MAC).join(IS_MAC ? "" : "+");
  });

  function hostOf(url: string) {
    try {
      return new URL(url).host;
    } catch {
      return url;
    }
  }

  function activate(item: BookmarkView, event: MouseEvent | KeyboardEvent) {
    if (drag.absorbClick() || renaming === item.id) return;
    session.highlighted = null;
    if (item.url === null) {
      void session.open(item.id);
      return;
    }
    const newTab = event.metaKey || event.ctrlKey || ("button" in event && event.button === 1);
    void commands.browserOpenUrl(item.url, newTab);
  }

  function startRename(item: BookmarkView) {
    confirming = null;
    renaming = item.id;
    draft = item.title;
  }

  async function finishRename(item: BookmarkView, keep: boolean) {
    if (renaming !== item.id) return;
    renaming = null;
    const title = draft.trim();
    if (keep && title && title !== item.title) await session.rename(item.id, title);
  }

  /** A folder with contents asks first, in its row; a link goes at once and
   *  can be put back for a few seconds. */
  async function remove(item: BookmarkView) {
    if (item.url === null && item.children > 0) {
      confirming = item.id;
      return;
    }
    await erase(item);
  }

  async function erase(item: BookmarkView) {
    confirming = null;
    const at = session.items.findIndex((candidate) => candidate.id === item.id);
    const index = session.searching || at < 0 ? null : at;
    const outcome = await session.remove(item.id);
    if (outcome === false || item.url === null) return;
    deleted = { item: { title: item.title, url: item.url, parent: item.parent }, index };
  }

  async function undo() {
    const last = deleted;
    deleted = null;
    if (last) await session.restore(last.item, last.index);
  }

  $effect(() => {
    if (!deleted) return;
    const timer = setTimeout(() => (deleted = null), UNDO_WINDOW);
    return () => clearTimeout(timer);
  });

  function openMenu(event: MouseEvent, item: BookmarkView) {
    event.preventDefault();
    if (renaming !== null) return;
    menuTarget = item;
    void commands.bookmarkMenuPopup(event.clientX, event.clientY, item.url === null).catch(() => {
      menuTarget = null;
    });
  }

  function runMenu(action: string) {
    const item = menuTarget;
    menuTarget = null;
    if (!item) return;
    switch (action) {
      case "open":
        if (item.url === null) void session.open(item.id);
        else void commands.browserOpenUrl(item.url, false);
        break;
      case "openNewTab":
        if (item.url) void commands.browserOpenUrl(item.url, true);
        break;
      case "copyLink":
        if (item.url)
          void navigator.clipboard.writeText(item.url).then(
            () => notices.show(m.notice_link_copied()),
            () => {},
          );
        break;
      case "rename":
        startRename(item);
        break;
      case "remove":
      case "removeFolder":
        void remove(item);
        break;
    }
  }

  // Only commands that arrive while the panel is open are its own.
  let handledCommand = untrack(() => uiCommands.uiCommand().seq);
  $effect(() => {
    const command = uiCommands.uiCommand();
    if (command.seq === handledCommand) return;
    handledCommand = command.seq;
    if (command.id.startsWith("bookmark.menu."))
      untrack(() => runMenu(command.id.slice("bookmark.menu.".length)));
  });

  async function addFolder() {
    const id = await session.addFolder(m.bookmarks_new_folder_name());
    const created = session.items.find((item) => item.id === id);
    if (created) startRename(created);
  }

  const focusAndSelect = (node: HTMLInputElement) => {
    node.focus();
    node.select();
  };

  const scrollIntoView = (node: HTMLElement, lit: boolean) => {
    if (lit) node.scrollIntoView({ block: "nearest" });
    return {
      update(next: boolean) {
        if (next) node.scrollIntoView({ block: "nearest" });
      },
    };
  };
</script>

<div class="bookmarks">
  {#if !session.searching && session.folder !== null}
    <div class="place">
      <IconButton
        icon={ArrowLeft01Icon}
        label={m.bookmarks_back()}
        size={14}
        buttonSize={24}
        onclick={() => void session.open(session.path.at(-2)?.id ?? null)}
      />
      <nav class="crumbs" aria-label={m.bookmarks_location()}>
        <button
          type="button"
          class="crumb"
          data-bookmark-crumb=""
          data-landing={landing?.kind === "into" && landing.folder === null}
          onclick={() => void session.open(null)}>{m.tool_bookmarks()}</button
        >
        {#each session.path as crumb (crumb.id)}
          <span class="divider" aria-hidden="true">/</span>
          <button
            type="button"
            class="crumb"
            data-bookmark-crumb={crumb.id}
            data-landing={landing?.kind === "into" && landing.folder === crumb.id}
            aria-current={crumb.id === session.folder ? "location" : undefined}
            onclick={() => void session.open(crumb.id)}>{crumb.title}</button
          >
        {/each}
      </nav>
    </div>
  {/if}

  {#if adding && !session.searching}
    <form
      class="add"
      aria-label={m.bookmarks_add()}
      onsubmit={(event) => {
        event.preventDefault();
        void submitAdd();
      }}
    >
      <input
        class="field"
        aria-label={m.bookmarks_address()}
        placeholder={m.bookmarks_address()}
        maxlength={8192}
        spellcheck={false}
        autocapitalize="off"
        bind:value={adding.address}
        readonly={adding.pending}
        oninput={() => {
          if (adding) adding.problem = null;
        }}
        onkeydown={leaveAdd}
        use:focusAndSelect
      />
      <input
        class="field"
        aria-label={m.bookmarks_name_optional()}
        placeholder={m.bookmarks_name_optional()}
        maxlength={512}
        bind:value={adding.name}
        readonly={adding.pending}
        onkeydown={leaveAdd}
      />
      {#if adding.problem}<p class="add-problem" role="alert">
          {adding.problem === "invalid" ? m.bookmarks_add_invalid() : m.bookmarks_add_failed()}
        </p>{/if}
      <div class="add-actions">
        <Button size="compact" variant="ghost" onclick={() => (adding = null)}
          >{m.action_cancel()}</Button
        >
        <Button
          size="compact"
          variant="primary"
          type="submit"
          pending={adding.pending}
          disabled={!adding.address.trim()}>{m.action_add()}</Button
        >
      </div>
    </form>
  {/if}

  {#if session.error === "unavailable" || session.error === "capacity"}
    <EmptyState title={m.bookmarks_unavailable()} description={m.surface_retry()}>
      {#snippet icon()}<Icon icon={Bookmark02Icon} size={22} />{/snippet}
    </EmptyState>
  {:else if session.empty}
    <EmptyState
      title={session.searching
        ? m.bookmarks_no_matches()
        : session.folder === null
          ? m.bookmarks_empty()
          : m.bookmarks_folder_empty()}
      description={session.searching
        ? m.bookmarks_no_matches_help()
        : session.folder !== null
          ? m.bookmarks_folder_empty_help()
          : addKeys
            ? m.bookmarks_empty_help({ keys: addKeys })
            : m.bookmarks_empty_help_menu()}
    >
      {#snippet icon()}<Icon icon={Bookmark02Icon} size={22} />{/snippet}
    </EmptyState>
  {:else}
    {#if session.error === "full"}<p class="notice" role="status">{m.bookmarks_full()}</p>{/if}
    <ul class="list" aria-label={m.tool_bookmarks()}>
      {#each session.items as item (item.id)}
        {@const folder = item.url === null}
        {@const lit = session.highlighted === item.id}
        <li
          class="row"
          class:lit
          use:scrollIntoView={lit}
          data-bookmark-id={item.id}
          data-folder={folder ? "" : undefined}
          data-carried={drag.item?.id === item.id}
          data-landing={landing?.kind === "into"
            ? landing.folder === item.id
              ? "into"
              : undefined
            : landing?.id === item.id
              ? landing.kind
              : undefined}
        >
          {#if renaming === item.id}
            <span class="glyph"
              >{#if folder}<Icon icon={Folder01Icon} size={16} />{:else}<FavIcon
                  image={favicons.mark(item.icon, item.url)?.image ?? null}
                  tone={favicons.mark(item.icon, item.url)?.tone}
                  size={16}
                  lit
                  fallback={Globe02Icon}
                />{/if}</span
            >
            <input
              class="rename"
              aria-label={m.bookmarks_rename()}
              maxlength={512}
              bind:value={draft}
              use:focusAndSelect
              onkeydown={(event) => {
                if (event.key === "Enter") void finishRename(item, true);
                if (event.key === "Escape") {
                  event.stopPropagation();
                  void finishRename(item, false);
                }
              }}
              onblur={() => void finishRename(item, true)}
            />
          {:else}
            <button
              type="button"
              class="open"
              title={item.url ?? m.bookmarks_folder_count({ count: item.children })}
              onclick={(event) => activate(item, event)}
              onauxclick={(event) => {
                if (event.button === 1) activate(item, event);
              }}
              oncontextmenu={(event) => openMenu(event, item)}
              onpointerdown={(event) => press(event, item)}
              onpointermove={drag.move}
              onpointerup={drag.end}
              onpointercancel={drag.cancel}
            >
              <span class="glyph"
                >{#if folder}<Icon icon={Folder01Icon} size={16} />{:else}<FavIcon
                    image={favicons.mark(item.icon, item.url)?.image ?? null}
                    tone={favicons.mark(item.icon, item.url)?.tone}
                    size={16}
                    lit
                    fallback={Globe02Icon}
                  />{/if}</span
              >
              <span class="title">{item.title}</span>
              {#if folder}<span class="chevron" aria-hidden="true"
                  ><Icon icon={ArrowRight01Icon} size={13} /></span
                >{:else}<span class="detail">{hostOf(item.url ?? "")}</span>{/if}
            </button>
            {#if confirming === item.id}
              <!-- Escape only backs out of the question. -->
              <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
              <span
                class="confirm"
                role="group"
                aria-label={m.bookmarks_remove_folder_confirm({ count: item.children })}
                onkeydown={(event) => {
                  if (event.key !== "Escape") return;
                  event.stopPropagation();
                  confirming = null;
                }}
              >
                <Button size="compact" variant="ghost" onclick={() => (confirming = null)}
                  >{m.action_cancel()}</Button
                >
                <Button size="compact" variant="danger" onclick={() => void erase(item)}
                  >{item.children === 1
                    ? m.bookmarks_delete_items_one()
                    : m.bookmarks_delete_items({ count: item.children })}</Button
                >
              </span>
            {:else}
              <span class="actions">
                <IconButton
                  icon={PencilEdit02Icon}
                  label={m.bookmarks_rename()}
                  size={14}
                  buttonSize={22}
                  onclick={() => startRename(item)}
                />
                <IconButton
                  icon={Delete02Icon}
                  label={folder && item.children > 0
                    ? m.bookmarks_remove_folder_confirm({ count: item.children })
                    : m.bookmarks_remove()}
                  size={14}
                  buttonSize={22}
                  onclick={() => void remove(item)}
                />
              </span>
            {/if}
          {/if}
        </li>
      {/each}
    </ul>
  {/if}
</div>
{#if deleted}
  <div class="undo" role="status">
    <span>{m.bookmarks_deleted({ title: deleted.item.title || deleted.item.url })}</span>
    <Button size="compact" variant="ghost" onclick={() => void undo()}>{m.bookmarks_undo()}</Button>
  </div>
{/if}
{#if drag.item}
  {@const carried = drag.item}
  <div class="ghost" style:translate={`${drag.at.x + 12}px ${drag.at.y + 8}px`} aria-hidden="true">
    <span class="glyph"
      >{#if carried.url === null}<Icon icon={Folder01Icon} size={14} />{:else}<FavIcon
          image={favicons.mark(carried.icon, carried.url)?.image ?? null}
          tone={favicons.mark(carried.icon, carried.url)?.tone}
          size={14}
          lit
          fallback={Globe02Icon}
        />{/if}</span
    ><span class="ghost-title">{carried.title}</span>
  </div>
{/if}

<style>
  .bookmarks {
    display: flex;
    flex: 1;
    flex-direction: column;
    min-height: 0;
  }

  .place {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 0 12px 2px 10px;
  }

  .crumbs {
    display: flex;
    flex: 1;
    align-items: center;
    gap: 2px;
    min-width: 0;
    overflow: hidden;
  }

  .crumb {
    flex: 0 1 auto;
    min-width: 0;
    overflow: hidden;
    padding: 3px 4px;
    border: 0;
    border-radius: var(--radius-inset);
    background: none;
    color: var(--color-muted);
    font: inherit;
    font-size: var(--text-label);
    white-space: nowrap;
    text-overflow: ellipsis;
    cursor: default;
  }

  .crumb:hover {
    background: var(--color-fill-hover);
    color: var(--color-text);
  }

  .crumb[aria-current="location"] {
    color: var(--color-text);
    font-weight: 550;
  }

  .divider {
    flex: none;
    color: var(--color-faint);
    font-size: var(--text-label);
  }

  .notice {
    margin: 4px 18px 0;
    font-size: var(--text-label);
    color: var(--color-warning);
  }

  .list {
    flex: 1;
    min-height: 0;
    margin: 0;
    padding: 2px 8px 12px;
    overflow-y: auto;
    list-style: none;
  }

  .row {
    position: relative;
    display: flex;
    align-items: center;
    height: 34px;
    border-radius: var(--radius-row);
    transition: background-color var(--motion-instant) var(--ease-smooth);
  }

  .row:hover,
  .row:focus-within {
    background: var(--row-active);
  }

  /* The bookmark just added holds the plate until the reader moves on. */
  .row.lit {
    background: var(--row-active);
    box-shadow: var(--row-rim);
  }

  .open {
    display: grid;
    grid-template-columns: 16px minmax(0, 1fr) auto;
    flex: 1;
    align-items: center;
    gap: 10px;
    min-width: 0;
    height: 100%;
    padding: 0 10px;
    border: 0;
    background: none;
    color: inherit;
    font: inherit;
    text-align: start;
    cursor: default;
  }

  .glyph {
    display: grid;
    place-items: center;
    color: var(--color-muted);
  }

  .row > .glyph {
    flex: none;
    width: 16px;
    margin-inline: 10px;
  }

  .title {
    min-width: 0;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
    font-size: var(--text-label);
    color: var(--color-text);
  }

  .detail {
    max-width: 110px;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
    font-size: 11.5px;
    color: var(--color-faint);
  }

  /* Present but faded until the row is in hand, so revealing them never
     moves what the pointer is on and the keyboard can still reach them. */
  .actions {
    position: absolute;
    inset-inline-end: 6px;
    display: flex;
    gap: 2px;
    opacity: 0;
  }

  .row:hover .actions,
  .row:focus-within .actions {
    opacity: 1;
  }

  .chevron {
    display: grid;
    place-items: center;
    color: var(--color-faint);
  }

  .row:hover .detail,
  .row:focus-within .detail,
  .row:hover .chevron,
  .row:focus-within .chevron {
    visibility: hidden;
  }

  /* A folder with contents asks in its own row before it goes. */
  .confirm {
    display: flex;
    flex: none;
    align-items: center;
    gap: 4px;
    padding-inline-end: 4px;
  }

  .row:has(.confirm) :is(.detail, .chevron) {
    display: none;
  }

  .undo {
    display: flex;
    flex: none;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    margin: 0 8px 8px;
    padding: 4px 4px 4px 12px;
    border-radius: var(--radius-row);
    background: var(--row-active);
    box-shadow: var(--row-rim);
    color: var(--color-text);
    font-size: var(--text-label);
    animation: rise var(--motion-base) var(--ease-emphasized) both;
  }

  .undo span {
    min-width: 0;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }

  @keyframes rise {
    from {
      opacity: 0;
      translate: 0 6px;
    }
  }

  .add {
    display: grid;
    gap: 6px;
    margin: 4px 8px 2px;
    padding: 8px;
    border-radius: var(--radius-row);
    background: var(--row-active);
  }

  /* The kit's field: fill at rest, a ring only while the caret is in it. */
  .field,
  .rename {
    box-sizing: border-box;
    min-width: 0;
    padding: 0 var(--control-inset);
    border: 0;
    border-radius: var(--radius-field);
    background: var(--color-field);
    color: var(--color-text);
    font: inherit;
    font-size: var(--text-label);
    outline: none;
    user-select: text;
    transition:
      background-color var(--motion-instant) var(--ease-smooth),
      box-shadow var(--motion-instant) var(--ease-smooth);
  }

  .field {
    height: var(--field-height);
  }

  .field::placeholder {
    color: var(--color-faint);
  }

  .field:focus,
  .rename {
    background: var(--color-field-hover);
    box-shadow: var(--shadow-field-focus);
  }

  .field:hover:not(:focus) {
    background: var(--color-field-hover);
  }

  .add-problem {
    margin: 0;
    font-size: 11.5px;
    color: var(--color-danger);
  }

  .add-actions {
    display: flex;
    justify-content: flex-end;
    gap: 6px;
  }

  .row[data-carried="true"] {
    opacity: 0.45;
  }

  /* Where a carried bookmark lands: a line between rows, or the folder lit. */
  .row[data-landing="before"]::before,
  .row[data-landing="after"]::after {
    content: "";
    position: absolute;
    inset-inline: 8px;
    height: 2px;
    border-radius: 1px;
    background: var(--color-accent);
    pointer-events: none;
  }

  .row[data-landing="before"]::before {
    inset-block-start: -1px;
  }

  .row[data-landing="after"]::after {
    inset-block-end: -1px;
  }

  .row[data-landing="into"],
  .crumb[data-landing="true"] {
    background: var(--row-active);
    box-shadow: inset 0 0 0 1px var(--color-accent);
  }

  .ghost {
    position: fixed;
    inset-block-start: 0;
    inset-inline-start: 0;
    z-index: 60;
    display: flex;
    align-items: center;
    gap: 8px;
    max-width: 240px;
    padding: 6px 12px 6px 9px;
    border-radius: var(--radius-row);
    background: var(--color-raised);
    box-shadow: var(--shadow-float);
    color: var(--color-text);
    font-size: var(--text-label);
    pointer-events: none;
  }

  .ghost-title {
    min-width: 0;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }

  .rename {
    flex: 1;
    height: 26px;
    margin-inline-end: 8px;
  }

  @media (prefers-reduced-motion: reduce) {
    .row,
    .field,
    .rename {
      transition: none;
    }

    .undo {
      animation: none;
    }
  }

  @media (forced-colors: active) {
    .row.lit {
      outline: 1px solid Highlight;
    }
  }
</style>
