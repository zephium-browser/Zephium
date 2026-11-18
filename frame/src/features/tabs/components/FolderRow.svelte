<script lang="ts">
  import * as m from "$shared/i18n/messages";
  import { Folder01Icon, FolderOpenIcon, VolumeHighIcon } from "@hugeicons/core-free-icons";
  import Icon from "$shared/ui/Icon";
  import FavIcon from "$shared/ui/FavIcon";
  import { favicons } from "$domain/favicons";
  import { tabFallbackIcon } from "../lib/tab-icon";
  import type { SidebarFolderEntry, FolderSummary } from "../lib/sidebar-model";
  import { folderHue, spineLayers } from "../lib/folder-look";
  import type { FolderColor } from "$shared/ipc/bindings";

  let {
    entry,
    summary,
    expanded,
    spines,
    editing = false,
    class: className = "",
    ontoggle,
    onrename,
    onrenamed,
    onmenu,
    onPointerDown,
    onPointerMove,
    onPointerUp,
    onPointerCancel,
  }: {
    entry: SidebarFolderEntry;
    summary: FolderSummary;
    /** Shown open: unfolded, or folded around the tab in front. */
    expanded: boolean;
    spines?: (FolderColor | null)[];
    editing?: boolean;
    class?: string;
    ontoggle: () => void;
    onrename: () => void;
    /** The typed name, or null when renaming was abandoned. */
    onrenamed: (name: string | null) => void;
    /** Its menu, at a point or, from the keyboard, at the row. */
    onmenu: (x: number | null, y: number | null) => void;
    onPointerDown: (event: PointerEvent) => void;
    onPointerMove: (event: PointerEvent) => void;
    onPointerUp: (event: PointerEvent) => void;
    onPointerCancel: (event: PointerEvent) => void;
  } = $props();

  const name = $derived(entry.node.kind.name);
  const hue = $derived(folderHue(entry.node.kind.color));
  const indent = $derived(8 + Math.min(entry.depth, 8) * 13);
  const label = $derived(
    (summary.count === 1
      ? (expanded ? m.folder_label_one : m.folder_label_folded_one)({ name })
      : (expanded ? m.folder_label : m.folder_label_folded)({ name, count: summary.count })) +
      (summary.audible ? `, ${m.folder_playing()}` : ""),
  );

  let row = $state<HTMLLIElement>();
  let input = $state<HTMLInputElement>();
  let done = false;

  $effect(() => {
    if (!editing || !input) return;
    done = false;
    input.value = name;
    input.focus();
    input.select();
  });

  function finish(commit: boolean) {
    if (done) return;
    done = true;
    onrenamed(commit ? (input?.value ?? null) : null);
    row?.querySelector<HTMLButtonElement>(".folder-row")?.focus();
  }

  // The rows are siblings drawn at depths, so the tree is walked by depth.
  function rowButton(element: Element | null | undefined) {
    return element?.querySelector<HTMLElement>("button") ?? null;
  }

  function step(forward: boolean) {
    let next = forward ? row?.nextElementSibling : row?.previousElementSibling;
    while (next && (next as HTMLElement).getClientRects().length === 0)
      next = forward ? next.nextElementSibling : next.previousElementSibling;
    rowButton(next)?.focus();
  }

  function parentRow() {
    let before = row?.previousElementSibling;
    while (before) {
      const element = before as HTMLElement;
      if (element.dataset.folder !== undefined && Number(element.dataset.depth) < entry.depth)
        return element;
      before = before.previousElementSibling;
    }
    return null;
  }

  function keydown(event: KeyboardEvent) {
    if (event.isComposing) return;
    const rtl = getComputedStyle(event.currentTarget as Element).direction === "rtl";
    const inward = rtl ? "ArrowLeft" : "ArrowRight";
    const outward = rtl ? "ArrowRight" : "ArrowLeft";
    if (event.key === inward) {
      event.preventDefault();
      if (!expanded) ontoggle();
      else step(true);
    } else if (event.key === outward) {
      event.preventDefault();
      if (expanded) ontoggle();
      else rowButton(parentRow())?.focus();
    } else if (event.key === "F2") {
      event.preventDefault();
      onrename();
    } else if (event.key === "ContextMenu" || (event.shiftKey && event.key === "F10")) {
      event.preventDefault();
      const rect = (event.currentTarget as HTMLElement).getBoundingClientRect();
      onmenu(rect.left + indent, rect.bottom);
    }
  }
</script>

<li
  bind:this={row}
  class={["folder", className]}
  data-motion-key={entry.key}
  data-node-id={entry.node.id}
  data-depth={entry.depth}
  data-folder
  data-hue={entry.node.kind.color ?? undefined}
  data-expanded={expanded}
  style:--folder-hue={hue}
  data-spined={spines?.length ? "" : undefined}
  style:--spines={spines?.length ? spineLayers(spines) : undefined}
>
  {#if editing}
    <div class="folder-row" style:padding-inline-start={`${indent}px`}>
      <span class="folder-glyph"><Icon icon={FolderOpenIcon} size={16} /></span>
      <input
        bind:this={input}
        class="folder-name-input"
        aria-label={m.folder_name()}
        maxlength={128}
        spellcheck="false"
        onkeydown={(event) => {
          if (event.isComposing) return;
          if (event.key === "Enter") {
            event.preventDefault();
            finish(true);
          } else if (event.key === "Escape") {
            event.preventDefault();
            event.stopPropagation();
            finish(false);
          }
        }}
        onblur={() => finish(true)}
      />
    </div>
  {:else}
    <button
      type="button"
      class="folder-row"
      style:padding-inline-start={`${indent}px`}
      aria-expanded={expanded}
      aria-label={label}
      onclick={ontoggle}
      ondblclick={(event) => {
        event.preventDefault();
        onrename();
      }}
      oncontextmenu={(event) => {
        event.preventDefault();
        onmenu(event.clientX, event.clientY);
      }}
      onkeydown={keydown}
      onpointerdown={onPointerDown}
      onpointermove={onPointerMove}
      onpointerup={onPointerUp}
      onpointercancel={onPointerCancel}
    >
      <span class="folder-glyph" aria-hidden="true"
        ><Icon icon={expanded ? FolderOpenIcon : Folder01Icon} size={16} /></span
      >
      <span class="folder-name">{name}</span>
      {#if !expanded && summary.count > 0}
        <span class="folder-summary" aria-hidden="true">
          {#if summary.audible}<span class="folder-sound"
              ><Icon icon={VolumeHighIcon} size={12} /></span
            >{/if}
          <span class="folder-faces">
            {#each summary.faces as tab (tab.id)}
              <span class="folder-face" data-asleep={tab.asleep || undefined}
                ><FavIcon
                  image={favicons.image(tab.icon)}
                  tone={favicons.tone(tab.icon)}
                  loading={tab.loading}
                  size={14}
                  fallback={tabFallbackIcon(tab.content)}
                /></span
              >
            {/each}
          </span>
          {#if summary.count > summary.faces.length}<span class="folder-count"
              >{summary.faces.length > 0
                ? `+${summary.count - summary.faces.length}`
                : summary.count}</span
            >{/if}
        </span>
      {/if}
    </button>
  {/if}
</li>

<style>
  .folder {
    position: relative;

    --folder-ink: var(--folder-hue, var(--color-label-secondary));
  }

  .folder-row {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    height: var(--row-sidebar);
    padding-inline-end: 8px;
    border: 0;
    border-radius: var(--radius-row);
    background: transparent;
    color: var(--color-label-secondary);
    font: inherit;
    font-size: var(--sidebar-row-text);
    font-weight: var(--sidebar-row-weight);
    letter-spacing: -0.005em;
    text-align: start;
    cursor: default;
    transition:
      background-color var(--motion-fast) var(--ease-out),
      box-shadow var(--motion-fast) var(--ease-out),
      color var(--motion-fast) var(--ease-out);
  }

  button.folder-row:hover {
    background: var(--row-hover);
    color: var(--color-text);
  }

  .folder-row:focus-visible {
    outline-offset: -2px;
  }

  .folder-glyph {
    display: flex;
    flex: none;
    color: var(--folder-ink);
    transition: color var(--motion-fast) var(--ease-out);
  }

  .folder:not([data-hue]) .folder-glyph {
    color: var(--color-muted);
  }

  .folder-name {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    white-space: nowrap;
    mask-image: var(--mask-fade-end);
  }

  .folder-name:dir(rtl) {
    mask-image: var(--mask-fade-end-rtl);
  }

  .folder-name-input {
    flex: 1;
    min-width: 0;
    height: 24px;
    margin-inline-start: -4px;
    padding-inline: 4px;
    border: 0;
    border-radius: var(--radius-inset);
    background: var(--color-field, var(--row-active));
    box-shadow: inset 0 0 0 1px var(--color-accent);
    color: var(--color-text);
    font: inherit;
    outline: none;
  }

  /* Folded, a folder says what it holds: the first few sites, live, and how
     many more. */
  .folder-summary {
    display: flex;
    flex: none;
    align-items: center;
    gap: 6px;
    color: var(--color-faint);
    font-size: 11px;
    font-variant-numeric: tabular-nums;
  }

  .folder-faces {
    display: flex;
  }

  .folder-face {
    display: flex;
    padding: 1px;
    border-radius: 5px;
    background: var(--color-sidebar, var(--color-surface));
  }

  .folder-face + .folder-face {
    margin-inline-start: -5px;
  }

  .folder-face[data-asleep] {
    opacity: 0.6;
  }

  .folder-sound {
    display: flex;
    color: var(--color-label-secondary);
  }

  /* A drag over the folder it would go into. */
  .folder:global([data-drop-into]) > .folder-row {
    background: color-mix(in oklab, var(--folder-hue, var(--color-accent)) 18%, transparent);
    box-shadow: inset 0 0 0 1px
      color-mix(in oklab, var(--folder-hue, var(--color-accent)) 55%, transparent);
    color: var(--color-text);
  }

  @media (forced-colors: active) {
    .folder-name {
      mask-image: none;
    }

    .folder:global([data-drop-into]) > .folder-row {
      outline: 2px solid Highlight;
      outline-offset: -2px;
    }
  }
</style>
