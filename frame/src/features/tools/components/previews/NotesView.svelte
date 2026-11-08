<script lang="ts">
  import { untrack } from "svelte";
  import type { ToolHostProps } from "$session/tool-drafts.svelte";
  import { noteSession } from "$domain/notes";
  import { surface as browser } from "$domain/surface";
  import { tabs } from "$domain/tabs";
  import { IS_MAC, IS_WINDOWS } from "$shared/platform";
  import LazyView from "$shared/ui/LazyView";
  import Icon from "$shared/ui/Icon";
  import Menu, { type MenuEntry } from "$shared/ui/Menu";
  import {
    Delete02Icon,
    Globe02Icon,
    MoreHorizontalIcon,
    PinIcon,
  } from "@hugeicons/core-free-icons";
  import ArrowLeft01Icon from "@hugeicons/core-free-icons/ArrowLeft01Icon";
  import Copy01Icon from "@hugeicons/core-free-icons/Copy01Icon";
  import DeletePutBackIcon from "@hugeicons/core-free-icons/DeletePutBackIcon";
  import FolderOpenIcon from "@hugeicons/core-free-icons/FolderOpenIcon";
  import PinOffIcon from "@hugeicons/core-free-icons/PinOffIcon";
  import * as m from "$shared/i18n/messages";
  import ToolFrame from "../ToolFrame.svelte";

  let props: ToolHostProps = $props();

  // Reached dynamically so the feature's entry stays in the browser chunk that
  // already loads it, rather than becoming a startup request of its own.
  const loadNotes = () => import("$features/notes").then((notes) => notes.loadNotes());

  // The host owns the session so the frame's header, search and body all read
  // the one it draws.
  let session = $state.raw(untrack(() => noteSession(props.profile, "sidebar")));
  $effect(() => {
    const owner = props.profile;
    return untrack(() => {
      const current = noteSession(owner, "sidebar");
      session = current;
      if (!current) return;
      if (props.state.query) current.search(props.state.query);
      void current.start();
      return () => current.stop();
    });
  });

  $effect(() => {
    const value = props.state.query;
    untrack(() => session?.search(value));
  });

  let note = $derived(session?.note ?? null);
  const reveal = IS_MAC
    ? m.note_reveal()
    : IS_WINDOWS
      ? m.note_reveal_explorer()
      : m.note_reveal_folder_generic();

  // A note can start from the page being read: its title, and a link back.
  let page = $derived(tabs.activeTab());
  let capturable = $derived(page?.url && /^https?:\/\//iu.test(page.url) ? page : null);

  let listEntries: MenuEntry[] = $derived([
    ...(capturable && !session?.trash
      ? [{ kind: "item" as const, id: "capture", label: m.note_from_page(), icon: Globe02Icon }]
      : []),
    {
      kind: "item",
      id: "trash",
      label: session?.trash ? m.note_all() : m.note_trash(),
      icon: session?.trash ? ArrowLeft01Icon : Delete02Icon,
    },
    { kind: "item", id: "folder", label: m.note_reveal_folder(), icon: FolderOpenIcon },
  ]);

  let noteEntries: MenuEntry[] = $derived(
    note?.trashed
      ? [{ kind: "item", id: "restore", label: m.note_restore(), icon: DeletePutBackIcon }]
      : [
          { kind: "item", id: "copy", label: m.note_copy_markdown(), icon: Copy01Icon },
          ...(note?.id
            ? [
                { kind: "item" as const, id: "reveal", label: reveal, icon: FolderOpenIcon },
                { kind: "separator" as const },
                {
                  kind: "item" as const,
                  id: "delete",
                  label: m.note_delete(),
                  icon: Delete02Icon,
                  danger: true,
                },
              ]
            : []),
        ],
  );

  async function expand() {
    const current = session;
    if (!current || !(await current.flush())) return;
    const id = current.note?.id ?? null;
    const destination = noteSession(props.profile, "page");
    if (id) void destination?.requestOpen(id);
    void browser.open("notes");
    // The page shows the same notes; the panel does not stay open behind it.
    props.onclose();
  }

  async function act(action: string) {
    const current = session;
    if (!current) return;
    const id = current.note?.id ?? null;
    if (action === "capture" && capturable) {
      const title = capturable.title?.trim() || capturable.url!;
      // Page titles are text, not Markdown.
      const escaped = title.replace(/[\\`*_[\]<>~]/gu, (c) => `\\${c}`);
      await current.create(`# ${escaped}\n\n[${escaped}](<${capturable.url}>)\n\n`);
    } else if (action === "trash") await current.showTrash(!current.trash);
    else if (action === "folder") await current.reveal(null);
    else if (action === "copy") await navigator.clipboard.writeText(current.markdown);
    else if (action === "reveal" && id) await current.reveal(id);
    else if (action === "delete" && id) await current.moveToTrash(id);
    else if (action === "restore" && id) await current.restore(id);
  }
</script>

{#if session}
  <ToolFrame
    {...props}
    caption={false}
    scrolls={false}
    onexpand={() => void expand()}
    searchLabel={m.note_search()}
    searchOpen={!note}
    searchFocus={false}
    onsearchdismiss={() => props.edit({ query: "" })}
  >
    {#snippet heading()}
      {#if note || session?.trash}
        <button
          type="button"
          class="notes-back"
          onclick={() => void (note ? session?.close() : session?.showTrash(false))}
          ><Icon icon={ArrowLeft01Icon} size={15} /><span
            >{note && session?.trash ? m.note_trash() : note ? m.note_back() : m.note_trash()}</span
          ></button
        >
      {:else}
        <h2 class="notes-title">{m.tool_notes()}</h2>
      {/if}
    {/snippet}
    {#snippet actions()}
      {#if note?.id && !note.trashed}
        <button
          type="button"
          class="notes-action"
          aria-label={note.summary?.pinned ? m.note_unpin() : m.note_pin()}
          title={note.summary?.pinned ? m.note_unpin() : m.note_pin()}
          aria-pressed={note.summary?.pinned ?? false}
          onclick={() => note.id && void session?.setPinned(note.id, !note.summary?.pinned)}
          ><Icon icon={note.summary?.pinned ? PinOffIcon : PinIcon} size={15} /></button
        >
      {/if}
      <Menu
        label={note ? m.note_more() : m.note_list_more()}
        entries={note ? noteEntries : listEntries}
        side="bottom"
        align="end"
        triggerClass="notes-action"
        onselect={(action) => void act(action)}
      >
        {#snippet trigger()}<Icon icon={MoreHorizontalIcon} size={16} />{/snippet}
      </Menu>
    {/snippet}
    <LazyView
      loader={loadNotes}
      loadingLabel={m.panel_loading()}
      failureLabel={m.panel_load_failed()}
      retryLabel={m.panel_retry()}
      >{#snippet children(Notes)}<Notes session={session!} />{/snippet}</LazyView
    >
  </ToolFrame>
{:else}<p role="alert">{m.note_unavailable()}</p>{/if}

<style>
  :global(.shared-tool[data-tool="notes"] .shared-tool-header) {
    gap: 4px;
    padding-inline: 18px 12px;
  }

  :global(.shared-tool[data-tool="notes"] .shared-tool-controls) {
    padding: 0 8px 6px;
  }

  :global(.shared-tool[data-tool="notes"] .shared-tool-search) {
    gap: 12px;
    height: 32px;
    box-sizing: border-box;
    padding: 0 10px 0 11px;
  }

  .notes-title {
    flex: 1;
    min-width: 0;
    margin: 0;
    color: var(--color-text);
    font-size: 14px;
    font-weight: 550;
    letter-spacing: -0.015em;
  }

  /* Back is where the title was, so the header keeps one shape. */
  .notes-back {
    display: flex;
    align-items: center;
    gap: 4px;
    min-width: 0;
    height: 26px;
    margin-inline: -8px auto;
    padding-inline: 4px 8px;
    border: 0;
    border-radius: var(--radius-inset);
    background: transparent;
    color: var(--color-text);
    font: inherit;
    font-size: 14px;
    font-weight: 550;
    letter-spacing: -0.015em;
    cursor: default;
    outline: none;
    transition: background-color var(--motion-instant) var(--ease-smooth);
  }

  .notes-back span {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .notes-back :global(svg) {
    flex: none;
    color: var(--color-muted);
  }

  .notes-back:hover {
    background: var(--color-fill-hover);
  }

  .notes-back:focus-visible,
  :global(.notes-action:focus-visible) {
    outline: 2px solid var(--color-ring);
    outline-offset: -2px;
  }

  :global(.notes-action) {
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

  :global(.notes-action:hover),
  :global(.notes-action[data-state="open"]),
  :global(.notes-action[aria-pressed="true"]) {
    background: var(--color-fill-hover);
    color: var(--color-text);
  }
</style>
