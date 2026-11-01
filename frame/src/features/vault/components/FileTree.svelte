<!--
  The vault as an Obsidian-style folder tree: folders expand in place, notes
  show by title, other files carry a type badge. Fully keyboard driven (arrows,
  Enter, F2, Delete) and drag-and-drop to move.
-->
<script lang="ts">
  import { tick } from "svelte";
  import type { VaultEntry, VaultSession } from "$domain/vault";
  import * as m from "$shared/i18n/messages";
  import { badgeFor, displayName } from "../lib/kinds";
  import { renamedWithExtension, visibleRows, type Row } from "../lib/tree";

  let {
    session,
    expanded,
    selected = $bindable(),
    renaming = $bindable(),
    active,
    onopen,
    oncontext,
    onrename,
    onmove,
    ontrash,
    ontoggle,
  }: {
    session: VaultSession;
    expanded: ReadonlySet<string>;
    selected: string | null;
    renaming: string | null;
    /** The file in front of the focused pane, highlighted like Obsidian does. */
    active: string | null;
    onopen: (entry: VaultEntry, beside: boolean) => void;
    oncontext: (entry: VaultEntry | null, x: number, y: number) => void;
    onrename: (entry: VaultEntry, name: string) => void;
    onmove: (path: string, dest: string) => void;
    ontrash: (entry: VaultEntry) => void;
    ontoggle: (path: string, open?: boolean) => void;
  } = $props();

  let rows = $derived(visibleRows(session.listings, expanded));
  let tree = $state<HTMLDivElement | null>(null);
  let dropTarget = $state<string | null>(null);
  let draft = $state("");

  $effect(() => {
    const path = renaming;
    if (path === null) return;
    const row = rows.find((candidate) => candidate.entry.path === path);
    if (!row) return;
    draft = row.entry.kind === "file" ? displayName(row.entry.name) : row.entry.name;
    void tick().then(() => {
      const input = tree?.querySelector<HTMLInputElement>("input.rename");
      input?.focus();
      input?.select();
    });
  });

  function select(path: string) {
    selected = path;
    void tick().then(() =>
      tree
        ?.querySelector<HTMLElement>(`[data-path="${CSS.escape(path)}"]`)
        ?.scrollIntoView({ block: "nearest" }),
    );
  }

  function commitRename(row: Row) {
    const name = draft.trim();
    renaming = null;
    tree?.focus();
    if (!name) return;
    const finalName =
      row.entry.kind === "file" && row.entry.ext === "md"
        ? `${name.replace(/\.md$/iu, "")}.md`
        : row.entry.kind === "file"
          ? renamedWithExtension(row.entry.name, name)
          : name;
    if (finalName !== row.entry.name) onrename(row.entry, finalName);
  }

  function keydown(event: KeyboardEvent) {
    if (renaming !== null) return;
    const index = rows.findIndex((row) => row.entry.path === selected);
    const row = rows[index];
    const move = (to: number) => {
      const target = rows[Math.max(0, Math.min(rows.length - 1, to))];
      if (target) select(target.entry.path);
    };
    switch (event.key) {
      case "ArrowDown":
        move(index + 1);
        break;
      case "ArrowUp":
        move(index < 0 ? 0 : index - 1);
        break;
      case "Home":
        move(0);
        break;
      case "End":
        move(rows.length - 1);
        break;
      case "ArrowRight":
        if (row?.entry.kind === "folder") {
          if (!row.expanded) ontoggle(row.entry.path, true);
          else move(index + 1);
        }
        break;
      case "ArrowLeft":
        if (row?.entry.kind === "folder" && row.expanded) ontoggle(row.entry.path, false);
        else if (row) {
          const parent = row.entry.path.slice(0, row.entry.path.lastIndexOf("/"));
          if (parent) select(parent);
        }
        break;
      case "Enter":
        if (!row) return;
        if (row.entry.kind === "folder" && !event.ctrlKey) ontoggle(row.entry.path);
        else onopen(row.entry, event.ctrlKey || event.metaKey);
        break;
      case " ":
        if (row?.entry.kind === "folder") ontoggle(row.entry.path);
        else if (row) onopen(row.entry, false);
        break;
      case "F2":
        if (row) renaming = row.entry.path;
        break;
      case "Delete":
        if (row) ontrash(row.entry);
        break;
      case "ContextMenu": {
        if (!row) return;
        const box = tree
          ?.querySelector<HTMLElement>(`[data-path="${CSS.escape(row.entry.path)}"]`)
          ?.getBoundingClientRect();
        oncontext(row.entry, box ? box.left + 24 : 24, box ? box.bottom : 24);
        break;
      }
      default:
        return;
    }
    event.preventDefault();
    event.stopPropagation();
  }

  function dragStart(event: DragEvent, entry: VaultEntry) {
    event.dataTransfer?.setData("application/x-zephium-vault", entry.path);
    event.dataTransfer?.setData("text/plain", entry.path);
    if (event.dataTransfer) event.dataTransfer.effectAllowed = "move";
  }

  function dropFolder(row: Row | null): string {
    if (!row) return "";
    if (row.entry.kind === "folder") return row.entry.path;
    const at = row.entry.path.lastIndexOf("/");
    return at < 0 ? "" : row.entry.path.slice(0, at);
  }

  function dragOver(event: DragEvent, row: Row | null) {
    if (!event.dataTransfer?.types.includes("application/x-zephium-vault")) return;
    event.preventDefault();
    event.stopPropagation();
    event.dataTransfer.dropEffect = "move";
    dropTarget = dropFolder(row);
  }

  function drop(event: DragEvent, row: Row | null) {
    const path = event.dataTransfer?.getData("application/x-zephium-vault");
    const dest = dropFolder(row);
    dropTarget = null;
    if (!path) return;
    event.preventDefault();
    event.stopPropagation();
    if (dest === path || dest.startsWith(`${path}/`)) return;
    onmove(path, dest);
  }
</script>

<div
  bind:this={tree}
  class="tree"
  role="tree"
  tabindex="0"
  aria-label={m.vault_files()}
  onkeydown={keydown}
  onfocus={() => {
    if (selected === null && rows[0]) selected = rows[0].entry.path;
  }}
  ondragover={(event) => dragOver(event, null)}
  ondragleave={() => (dropTarget = null)}
  ondrop={(event) => drop(event, null)}
  oncontextmenu={(event) => {
    event.preventDefault();
    oncontext(null, event.clientX, event.clientY);
  }}
  class:drop-root={dropTarget === ""}
>
  {#each rows as row (row.entry.path)}
    {@const entry = row.entry}
    {@const folder = entry.kind === "folder"}
    <div
      class="row"
      role="treeitem"
      tabindex="-1"
      aria-selected={selected === entry.path}
      aria-expanded={folder ? row.expanded : undefined}
      aria-level={row.depth + 1}
      data-path={entry.path}
      class:selected={selected === entry.path}
      class:active={active === entry.path}
      class:drop={folder && dropTarget === entry.path}
      style:--depth={row.depth}
      draggable={renaming !== entry.path}
      ondragstart={(event) => dragStart(event, entry)}
      ondragover={(event) => dragOver(event, row)}
      ondrop={(event) => drop(event, row)}
      onclick={(event) => {
        select(entry.path);
        tree?.focus();
        if (folder) ontoggle(entry.path);
        else onopen(entry, event.ctrlKey || event.metaKey);
      }}
      oncontextmenu={(event) => {
        event.preventDefault();
        event.stopPropagation();
        select(entry.path);
        oncontext(entry, event.clientX, event.clientY);
      }}
      onkeydown={() => {}}
    >
      {#each { length: row.depth } as _, guide (guide)}<span class="guide"></span>{/each}
      <span class="chevron" class:open={row.expanded} aria-hidden="true">
        {#if folder}<svg viewBox="0 0 16 16"><path d="M6 4l4 4-4 4" /></svg>{/if}
      </span>
      {#if renaming === entry.path}
        <input
          class="rename"
          bind:value={draft}
          aria-label={m.vault_rename()}
          onclick={(event) => event.stopPropagation()}
          onkeydown={(event) => {
            event.stopPropagation();
            if (event.key === "Enter") commitRename(row);
            if (event.key === "Escape") {
              renaming = null;
              tree?.focus();
            }
          }}
          onblur={() => {
            if (renaming === entry.path) commitRename(row);
          }}
        />
      {:else}
        <span class="name">{folder ? entry.name : displayName(entry.name)}</span>
        {#if !folder && badgeFor(entry.ext)}<span class="badge">{badgeFor(entry.ext)}</span>{/if}
      {/if}
    </div>
  {/each}
  {#if session.listings.get("")?.length === 0}
    <p class="empty">{m.vault_empty_folder()}</p>
  {/if}
</div>

<style>
  .tree {
    flex: 1;
    min-block-size: 0;
    overflow: auto;
    padding: 4px 6px 24px;
    outline: none;
    font-size: 13px;
  }

  .tree.drop-root {
    background: var(--color-fill);
  }

  .row {
    display: flex;
    align-items: center;
    gap: 2px;
    block-size: 26px;
    padding-inline-end: 8px;
    border-radius: var(--radius-row);
    color: var(--color-label-secondary);
    cursor: default;
    user-select: none;
    white-space: nowrap;
  }

  .row:hover {
    background: var(--row-hover);
    color: var(--color-text);
  }

  .row.active {
    background: var(--color-fill-active);
    color: var(--color-text);
  }

  .tree:focus-visible .row.selected,
  .tree:focus .row.selected {
    box-shadow: inset 0 0 0 1px var(--color-border-strong);
  }

  .row.drop {
    background: var(--color-fill-active);
    color: var(--color-text);
  }

  .guide {
    flex: none;
    align-self: stretch;
    inline-size: 14px;
    margin-inline-start: 4px;
    border-inline-start: 1px solid var(--color-border);
  }

  .chevron {
    display: grid;
    flex: none;
    place-items: center;
    inline-size: 16px;
    block-size: 16px;
    color: var(--color-faint);
    transition: transform var(--motion-fast) var(--ease-out);
  }

  .chevron.open {
    transform: rotate(90deg);
  }

  .chevron svg {
    inline-size: 12px;
    block-size: 12px;
    fill: none;
    stroke: currentcolor;
    stroke-width: 1.6;
    stroke-linecap: round;
    stroke-linejoin: round;
  }

  .name {
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .badge {
    flex: none;
    margin-inline-start: 6px;
    padding: 0 4px;
    border-radius: 4px;
    background: var(--color-fill);
    color: var(--color-muted);
    font-size: 9px;
    font-weight: 600;
    letter-spacing: 0.04em;
    line-height: 15px;
  }

  .rename {
    flex: 1;
    min-inline-size: 0;
    block-size: 22px;
    padding-inline: 4px;
    border: 1px solid var(--color-field-ring);
    border-radius: 4px;
    background: var(--color-field);
    color: var(--color-text);
    font: inherit;
  }

  .empty {
    padding: 12px;
    color: var(--color-muted);
  }

  @media (prefers-reduced-motion: reduce) {
    .chevron {
      transition: none;
    }
  }
</style>
