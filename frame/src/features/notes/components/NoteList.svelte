<script lang="ts">
  import { tick, untrack } from "svelte";
  import type { NoteSession, NoteSummary } from "$domain/notes";
  import Icon from "$shared/ui/Icon";
  import Menu, { type MenuEntry } from "$shared/ui/Menu";
  import { createVirtualWindow } from "$shared/lib/virtual-window.svelte";
  import { Delete02Icon, MoreHorizontalIcon, PinIcon } from "@hugeicons/core-free-icons";
  import Copy01Icon from "@hugeicons/core-free-icons/Copy01Icon";
  import DeletePutBackIcon from "@hugeicons/core-free-icons/DeletePutBackIcon";
  import FolderOpenIcon from "@hugeicons/core-free-icons/FolderOpenIcon";
  import PinOffIcon from "@hugeicons/core-free-icons/PinOffIcon";
  import * as m from "$shared/i18n/messages";
  import { glide, measure } from "../lib/glide";
  import { sections } from "../lib/sections";
  import { rowDate, sectionLabel, untilTomorrow } from "../lib/format";
  import { deleteShortcut, shortcutHint, revealLabel } from "../lib/platform";

  let {
    session,
    density,
    onopen,
    onedit,
  }: {
    session: NoteSession;
    density: "panel" | "page";
    /** A note was chosen. */
    onopen: (id: string) => void;
    /** Return was pressed on the chosen note: go to its text. */
    onedit?: () => void;
  } = $props();

  const ROW = { panel: 58, page: 62 };
  const HEADING = { panel: 32, page: 36 };

  // Rows show a time today, then a day: they change at midnight, and only then.
  let now = $state(Date.now());
  $effect(() => {
    const timer = setTimeout(() => (now = Date.now()), untilTomorrow(now) + 1000);
    return () => clearTimeout(timer);
  });

  let grouped = $derived(sections(session.items, now, session.query.trim() !== ""));
  type Line =
    | { kind: "heading"; id: string; label: string }
    | { kind: "note"; id: string; note: NoteSummary };
  let lines: Line[] = $derived(
    grouped.flatMap((section) => [
      ...(section.key.kind === "results" || session.trash
        ? []
        : [
            {
              kind: "heading" as const,
              id: `section:${section.id}`,
              label: sectionLabel(section.key, now),
            },
          ]),
      ...section.items.map((note) => ({ kind: "note" as const, id: note.id, note })),
    ]),
  );
  let notes = $derived(lines.filter((line) => line.kind === "note"));
  let selected = $derived(session.note?.id ?? null);

  const virtual = createVirtualWindow({
    heights: () => lines.map((line) => (line.kind === "heading" ? HEADING[density] : ROW[density])),
    threshold: 150,
  });

  let list = $state<HTMLDivElement>();
  let scroller = $state<HTMLDivElement>();
  // Rows glide to their new places when a note moves up its day; a long
  // list, drawn in windows, simply redraws.
  let shape = $derived(lines.map((line) => line.id).join("|"));
  let before: Map<string, number> | null = null;
  $effect.pre(() => {
    void shape;
    untrack(() => {
      before = virtual.active ? null : measure(list);
    });
  });
  $effect(() => {
    void shape;
    untrack(() => glide(list, before));
  });

  /** Words to mark in titles and previews while searching. */
  let terms = $derived(
    session.query
      .trim()
      .toLowerCase()
      .split(/\s+/u)
      .filter((term) => term.length > 0),
  );

  function pieces(text: string): { text: string; hit: boolean }[] {
    if (!terms.length) return [{ text, hit: false }];
    const lower = text.toLowerCase();
    const result: { text: string; hit: boolean }[] = [];
    let at = 0;
    while (at < text.length) {
      let next = -1;
      let length = 0;
      for (const term of terms) {
        const found = lower.indexOf(term, at);
        if (found !== -1 && (next === -1 || found < next)) {
          next = found;
          length = term.length;
        }
      }
      if (next === -1) break;
      if (next > at) result.push({ text: text.slice(at, next), hit: false });
      result.push({ text: text.slice(next, next + length), hit: true });
      at = next + length;
    }
    if (at < text.length) result.push({ text: text.slice(at), hit: false });
    return result;
  }

  function entries(note: NoteSummary): MenuEntry[] {
    if (note.trashed)
      return [
        { kind: "item", id: "restore", label: m.note_restore(), icon: DeletePutBackIcon },
        { kind: "separator" },
        {
          kind: "item",
          id: "destroy",
          label: m.note_delete_forever(),
          icon: Delete02Icon,
          danger: true,
        },
      ];
    return [
      {
        kind: "item",
        id: "pin",
        label: note.pinned ? m.note_unpin() : m.note_pin(),
        icon: note.pinned ? PinOffIcon : PinIcon,
      },
      { kind: "item", id: "copy", label: m.note_copy_markdown(), icon: Copy01Icon },
      { kind: "item", id: "reveal", label: revealLabel(), icon: FolderOpenIcon },
      { kind: "separator" },
      {
        kind: "item",
        id: "delete",
        label: m.note_delete(),
        icon: Delete02Icon,
        danger: true,
        hint: shortcutHint("CmdOrCtrl+Backspace"),
      },
    ];
  }

  async function act(note: NoteSummary, action: string) {
    if (action === "pin") await session.setPinned(note.id, !note.pinned);
    else if (action === "reveal") await session.reveal(note.id);
    else if (action === "copy") {
      const markdown = await session.markdownOf(note.id);
      if (markdown !== null) await navigator.clipboard.writeText(markdown);
    } else if (action === "delete") await remove(note.id);
    else if (action === "restore") await session.restore(note.id);
    else if (action === "destroy") await session.deleteForever(note.id);
  }

  /** Deletes a note and keeps the keyboard on the one that takes its place. */
  async function remove(id: string) {
    const index = notes.findIndex((line) => line.id === id);
    const neighbour = notes[index + 1] ?? notes[index - 1];
    if (!(await session.moveToTrash(id))) return;
    if (neighbour && neighbour.id !== id && selected === null) {
      onopen(neighbour.id);
      await focusRow(neighbour.id);
    }
  }

  async function focusRow(id: string) {
    const index = lines.findIndex((line) => line.id === id);
    if (index === -1) return;
    virtual.scrollToIndex(index);
    await tick();
    list?.querySelector<HTMLElement>(`[data-note-id="${id}"] .note-row-open`)?.focus();
  }

  function keydown(event: KeyboardEvent) {
    const current = notes.findIndex(
      (line) =>
        line.id ===
        (document.activeElement?.closest<HTMLElement>("[data-note-id]")?.dataset.noteId ??
          selected),
    );
    const go = (index: number) => {
      const target = notes[Math.max(0, Math.min(notes.length - 1, index))];
      if (!target) return;
      event.preventDefault();
      onopen(target.id);
      void focusRow(target.id);
    };
    if (event.key === "ArrowDown") go(current + 1);
    else if (event.key === "ArrowUp") go(current - 1);
    else if (event.key === "Home") go(0);
    else if (event.key === "End") go(notes.length - 1);
    else if (event.key === "Enter" && onedit && current !== -1) {
      event.preventDefault();
      onedit();
    } else if (deleteShortcut(event) && current !== -1) {
      event.preventDefault();
      const id = notes[current]!.id;
      if (session.trash) void session.deleteForever(id);
      else void remove(id);
    }
  }

  function scrolled() {
    if (!scroller || !session.next || session.loading) return;
    if (scroller.scrollTop + scroller.clientHeight > scroller.scrollHeight - 600)
      void session.reload(true);
  }
</script>

<div
  class="note-list-scroller"
  data-density={density}
  bind:this={scroller}
  use:virtual.attach
  onscroll={scrolled}
>
  <div
    class="note-list"
    role="listbox"
    tabindex="-1"
    aria-label={session.trash ? m.note_trash() : m.tool_notes()}
    bind:this={list}
    onkeydown={keydown}
    style:padding-block-start="{virtual.window.before}px"
    style:padding-block-end="{virtual.window.after}px"
  >
    {#each lines.slice(virtual.window.first, virtual.window.last) as line, offset (line.id)}
      <div
        class="note-line"
        data-glide={line.id}
        data-virtual-index={virtual.window.first + offset}
      >
        {#if line.kind === "heading"}
          <div class="note-section" role="presentation">{line.label}</div>
        {:else}
          {@const note = line.note}
          <div class="note-row" data-note-id={note.id} data-selected={selected === note.id}>
            <button
              type="button"
              class="note-row-open"
              role="option"
              aria-selected={selected === note.id}
              tabindex={selected === note.id || (selected === null && note.id === notes[0]?.id)
                ? 0
                : -1}
              onclick={() => onopen(note.id)}
              ondblclick={() => onedit?.()}
            >
              <span class="note-row-title"
                >{#if note.pinned && !grouped.some((section) => section.key.kind === "pinned")}<Icon
                    icon={PinIcon}
                    size={12}
                  />{/if}{#each pieces(note.title || m.note_untitled()) as piece, at (at)}{#if piece.hit}<mark
                      >{piece.text}</mark
                    >{:else}{piece.text}{/if}{/each}</span
              >
              <span class="note-row-meta"
                ><time datetime={new Date(Number(note.modified_at)).toISOString()}
                  >{rowDate(Number(note.modified_at), now)}</time
                ><span class="note-row-preview"
                  >{#each pieces(note.preview) as piece, at (at)}{#if piece.hit}<mark
                        >{piece.text}</mark
                      >{:else}{piece.text}{/if}{/each}</span
                ></span
              >
            </button>
            <Menu
              label={m.note_more()}
              entries={entries(note)}
              side="bottom"
              align="end"
              triggerClass="note-row-more"
              onselect={(action) => void act(note, action)}
            >
              {#snippet trigger()}<Icon icon={MoreHorizontalIcon} size={15} />{/snippet}
            </Menu>
          </div>
        {/if}
      </div>
    {/each}
  </div>
  {#if session.trash && session.items.length}<p class="note-trash-hint">
      {m.note_trash_hint()}
    </p>{/if}
</div>

<style>
  .note-list-scroller {
    contain: inline-size;
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    overscroll-behavior: contain;
    /* stylelint-disable-next-line property-no-vendor-prefix */
    -webkit-user-select: none;
    user-select: none;
  }

  .note-list {
    display: flex;
    flex-direction: column;
    padding-inline: 8px;
    outline: none;
  }

  .note-line {
    flex: none;
  }

  .note-section {
    display: flex;
    align-items: flex-end;
    flex: none;
    box-sizing: border-box;
    height: 32px;
    padding: 0 12px 6px;
    color: var(--color-faint);
    font-size: var(--text-label);
    font-weight: 600;
    letter-spacing: 0.01em;
  }

  [data-density="page"] .note-section {
    height: 36px;
    padding-block-end: 7px;
  }

  .note-row {
    position: relative;
    flex: none;
    box-sizing: border-box;
    height: 58px;
    padding-block: 1px;
  }

  [data-density="page"] .note-row {
    height: 62px;
  }

  .note-row-open {
    display: flex;
    flex-direction: column;
    justify-content: center;
    gap: 3px;
    width: 100%;
    height: 100%;
    min-width: 0;
    padding: 0 36px 0 12px;
    border: 0;
    border-radius: var(--radius-row);
    background: transparent;
    color: var(--color-text);
    font: inherit;
    text-align: start;
    cursor: default;
    outline: none;
    transition: background-color var(--motion-fast) var(--ease-out);
  }

  .note-row-open:focus-visible {
    outline: 2px solid var(--color-ring);
    outline-offset: -2px;
  }

  .note-row[data-selected="true"] .note-row-open {
    background: var(--row-active);
  }

  .note-row:not([data-selected="true"]):hover .note-row-open {
    background: var(--row-hover);
  }

  .note-row-title,
  .note-row-meta {
    display: block;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .note-row-title {
    display: flex;
    align-items: center;
    gap: 5px;
    font-size: 13.5px;
    font-weight: 600;
    line-height: 19px;
    letter-spacing: -0.008em;
  }

  .note-row-title :global(svg) {
    flex: none;
    color: var(--color-muted);
  }

  [data-density="page"] .note-row-title {
    font-size: 14px;
    line-height: 20px;
  }

  .note-row-meta {
    display: flex;
    gap: 8px;
    color: var(--color-muted);
    font-size: 12px;
    line-height: 16px;
  }

  .note-row-meta time {
    flex: none;
    color: var(--color-label-secondary);
    font-variant-numeric: tabular-nums;
  }

  .note-row-preview {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    color: var(--color-faint);
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  mark {
    border-radius: 3px;
    background: var(--color-accent-soft);
    color: inherit;
  }

  .note-row :global(.note-row-more) {
    position: absolute;
    top: 50%;
    inset-inline-end: 8px;
    display: grid;
    place-items: center;
    width: 26px;
    height: 26px;
    border: 0;
    border-radius: var(--radius-inset);
    background: transparent;
    color: var(--color-muted);
    opacity: 0;
    translate: 0 -50%;
    cursor: default;
    outline: none;
    transition:
      opacity var(--motion-fast) var(--ease-out),
      background-color var(--motion-instant) var(--ease-smooth);
  }

  .note-row:hover :global(.note-row-more),
  .note-row :global(.note-row-more[data-state="open"]),
  .note-row :global(.note-row-more:focus-visible) {
    opacity: 1;
  }

  .note-row :global(.note-row-more:hover),
  .note-row :global(.note-row-more[data-state="open"]) {
    background: var(--color-fill-hover);
    color: var(--color-text);
  }

  .note-row :global(.note-row-more:focus-visible) {
    outline: 2px solid var(--color-ring);
    outline-offset: -2px;
  }

  .note-trash-hint {
    margin: 12px 20px 20px;
    color: var(--color-faint);
    font-size: var(--text-label);
    line-height: 16px;
    text-align: center;
  }
</style>
