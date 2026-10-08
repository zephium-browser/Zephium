<script lang="ts">
  import { untrack } from "svelte";
  import { MediaQuery } from "svelte/reactivity";
  import { noteSession } from "$domain/notes";
  import Icon from "$shared/ui/Icon";
  import Menu, { type MenuEntry } from "$shared/ui/Menu";
  import SearchField from "$shared/ui/SearchField";
  import {
    Cancel01Icon,
    Delete02Icon,
    MoreHorizontalIcon,
    PinIcon,
  } from "@hugeicons/core-free-icons";
  import ArrowLeft01Icon from "@hugeicons/core-free-icons/ArrowLeft01Icon";
  import ArrowLeft02Icon from "@hugeicons/core-free-icons/ArrowLeft02Icon";
  import PencilEdit02Icon from "@hugeicons/core-free-icons/PencilEdit02Icon";
  import Copy01Icon from "@hugeicons/core-free-icons/Copy01Icon";
  import DeletePutBackIcon from "@hugeicons/core-free-icons/DeletePutBackIcon";
  import FolderOpenIcon from "@hugeicons/core-free-icons/FolderOpenIcon";
  import PinOffIcon from "@hugeicons/core-free-icons/PinOffIcon";
  import * as m from "$shared/i18n/messages";
  import { lagging } from "$shared/lib/lag.svelte";
  import NoteList from "./NoteList.svelte";
  import NoteView from "./NoteView.svelte";
  import NoteNotice from "./NoteNotice.svelte";
  import { edited, noteCount, untilEditedChanges } from "../lib/format";
  import { enter } from "../lib/enter";
  import { revealLabel } from "../lib/platform";

  let { profile, onclose }: { profile: string; onclose: () => void } = $props();
  let session = $derived(profile ? noteSession(profile, "page") : null);
  $effect(() => {
    const current = session;
    if (!current) return;
    return untrack(() => {
      void current.start();
      return () => current.stop();
    });
  });

  const wide = new MediaQuery("(min-width: 900px)");
  let note = $derived(session?.note ?? null);
  let query = $state(untrack(() => session?.query ?? ""));
  let stage = $state<HTMLDivElement>();
  /** Set once the note's own title has scrolled out from under the bar. */
  let scrolled = $state(false);
  $effect.pre(() => {
    void note?.version;
    scrolled = false;
  });
  let now = $state(Date.now());
  let modified = $derived(Number(note?.summary?.modified_at) || 0);
  $effect.pre(() => {
    void modified;
    now = Date.now();
  });
  // Wakes once for each time "Edited …" reads differently, and not at all
  // without a note open.
  $effect(() => {
    void now;
    if (!modified) return;
    const timer = setTimeout(
      () => (now = Date.now()),
      untilEditedChanges(modified, Date.now()) + 250,
    );
    return () => clearTimeout(timer);
  });

  let heading = $derived(session?.trash ? m.note_trash() : m.tool_notes());
  let subtitle = $derived.by(() => {
    if (!session?.loaded) return "";
    const count = session.items.length;
    const more = session.next ? "+" : "";
    return `${noteCount(count)}${more}`;
  });

  // Typing saves on every pause; only a save that is taking its time, or one
  // being retried, is worth a word.
  const writing = lagging(() => session?.saveState === "saving");
  let retrying = $derived(session?.saveState === "retrying");
  let status = $derived.by(() => {
    if (!session || !note?.summary) return "";
    if (retrying) return m.note_retrying();
    if (writing.current) return m.note_saving();
    return edited(modified, now);
  });

  let noteEntries: MenuEntry[] = $derived(
    note?.trashed
      ? [
          { kind: "item", id: "restore", label: m.note_restore(), icon: DeletePutBackIcon },
          { kind: "separator" },
          {
            kind: "item",
            id: "destroy",
            label: m.note_delete_forever(),
            icon: Delete02Icon,
            danger: true,
          },
        ]
      : [
          { kind: "item", id: "copy", label: m.note_copy_markdown(), icon: Copy01Icon },
          ...(note?.id
            ? [
                { kind: "item" as const, id: "reveal", label: revealLabel(), icon: FolderOpenIcon },
                { kind: "separator" as const },
                {
                  kind: "item" as const,
                  id: "delete",
                  label: m.note_delete(),
                  icon: Delete02Icon,
                  danger: true,
                  hint: "⌘⌫",
                },
              ]
            : []),
        ],
  );

  async function act(action: string) {
    const current = session;
    const id = current?.note?.id;
    if (!current) return;
    if (action === "copy") await navigator.clipboard.writeText(current.markdown);
    else if (action === "reveal" && id) await current.reveal(id);
    else if (action === "delete" && id) await current.moveToTrash(id);
    else if (action === "restore" && id) await current.restore(id);
    else if (action === "destroy" && id) await current.deleteForever(id);
  }

  function focusEditor() {
    stage?.querySelector<HTMLElement>(".note-document")?.focus();
  }

  async function leave() {
    if (session && !(await session.close())) return;
    onclose();
  }

  function keydown(event: KeyboardEvent) {
    if (event.defaultPrevented) return;
    const target = event.target as HTMLElement;
    const typing = target.closest("input, textarea, [contenteditable='true']");
    if (event.key === "Escape" && typing?.closest(".note-document")) {
      // Out of the text, back to the note's row, as Tab-less as a list allows.
      event.preventDefault();
      (
        document.querySelector(`[data-note-id="${note?.id}"] .note-row-open`) as HTMLElement | null
      )?.focus();
      return;
    }
    if (typing) return;
    if (event.key === "/" && !event.metaKey && !event.ctrlKey) {
      event.preventDefault();
      document.querySelector<HTMLInputElement>(".notes-page .ui-search input")?.focus();
    } else if (event.key === "n" && !event.metaKey && !event.ctrlKey && !event.altKey) {
      event.preventDefault();
      void session?.create();
    }
  }
</script>

{#if session}
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
  <section
    class="notes-page"
    class:narrow={!wide.current}
    class:reading={note !== null}
    aria-label={m.tool_notes()}
    onkeydown={keydown}
  >
    <div class="notes-column">
      <header class="column-head">
        <div class="column-title">
          {#if session.trash}<button
              type="button"
              class="head-button"
              aria-label={m.note_all()}
              title={m.note_all()}
              onclick={() => void session?.showTrash(false)}
              ><Icon icon={ArrowLeft02Icon} size={17} /></button
            >{/if}
          <div class="title-text">
            <h1>{heading}</h1>
            <p>{subtitle}</p>
          </div>
          {#if !session.trash}<button
              type="button"
              class="head-button"
              aria-label={m.note_new()}
              title="{m.note_new()}  ⌥⌘N"
              onclick={() => void session?.create()}
              ><Icon icon={PencilEdit02Icon} size={17} /></button
            >{/if}
          <button
            type="button"
            class="head-button"
            aria-label={m.note_close()}
            title={m.note_close()}
            onclick={() => void leave()}><Icon icon={Cancel01Icon} size={17} /></button
          >
        </div>
        <SearchField
          label={m.note_search()}
          placeholder={m.note_search()}
          size="chrome"
          value={query}
          oninput={(value) => {
            query = value;
            session?.search(value);
          }}
        />
      </header>
      {#if session.loaded && !session.items.length}
        <div class="column-empty">
          {#if query.trim()}<strong>{m.note_empty_search()}</strong>
            <span>{m.note_empty_search_body()}</span>
          {:else if session.trash}<strong>{m.note_trash_empty()}</strong>
            <span>{m.note_trash_hint()}</span>
          {:else}<strong>{m.note_empty_title()}</strong>
            <span>{m.note_empty_body()}</span>{/if}
        </div>
      {:else}
        <NoteList
          {session}
          density="page"
          onopen={(id) => void session?.open(id)}
          onedit={focusEditor}
        />
      {/if}
      {#if !session.trash}
        <footer class="column-foot">
          <button type="button" class="foot-row" onclick={() => void session?.showTrash(true)}
            ><Icon icon={Delete02Icon} size={15} /><span>{m.note_trash()}</span></button
          >
        </footer>
      {/if}
    </div>

    <div class="notes-stage" bind:this={stage} data-note-bounds>
      {#if note}
        <header class="stage-bar" class:scrolled>
          <div class="stage-side">
            {#if !wide.current}<button
                type="button"
                class="head-button"
                aria-label={m.note_back()}
                onclick={() => void session?.close()}
                ><Icon icon={ArrowLeft01Icon} size={16} /></button
              >{/if}
          </div>
          <div class="stage-heading">
            <span class="stage-status" class:away={scrolled}>{status}</span>
            <span class="sr-only" aria-live="polite">{retrying ? m.note_retrying() : ""}</span>
            <span class="stage-title" class:away={!scrolled} aria-hidden={!scrolled}
              >{note.summary?.title || m.note_untitled()}</span
            >
          </div>
          <div class="stage-side end">
            {#if note.id && !note.trashed}<button
                type="button"
                class="head-button"
                aria-label={note.summary?.pinned ? m.note_unpin() : m.note_pin()}
                title={note.summary?.pinned ? m.note_unpin() : m.note_pin()}
                aria-pressed={note.summary?.pinned ?? false}
                onclick={() => note.id && void session?.setPinned(note.id, !note.summary?.pinned)}
                ><Icon icon={note.summary?.pinned ? PinOffIcon : PinIcon} size={16} /></button
              >{/if}
            <Menu
              label={m.note_more()}
              entries={noteEntries}
              side="bottom"
              align="end"
              triggerClass="head-button"
              onselect={(action) => void act(action)}
            >
              {#snippet trigger()}<Icon icon={MoreHorizontalIcon} size={16} />{/snippet}
            </Menu>
          </div>
        </header>
        {#key note.version}
          <div
            class="stage-scroll"
            use:enter={"forward"}
            onscroll={(event) => (scrolled = event.currentTarget.scrollTop > 56)}
          >
            <div class="stage-column">
              <NoteView {session} density="page" autofocus />
            </div>
          </div>
        {/key}
      {:else}
        <div class="stage-empty">
          <span class="stage-empty-icon"><Icon icon={PencilEdit02Icon} size={26} /></span>
          <div class="stage-empty-text">
            <h2>{m.note_stage_title()}</h2>
            <p>{m.note_stage_body()}</p>
          </div>
          <div class="stage-empty-actions">
            <button type="button" class="stage-new" onclick={() => void session?.create()}
              ><Icon icon={PencilEdit02Icon} size={15} /><span>{m.note_new()}</span><span
                class="stage-hint">⌥⌘N</span
              ></button
            >
            <button type="button" class="stage-link" onclick={() => void session?.reveal(null)}
              >{m.note_reveal_folder()}</button
            >
          </div>
        </div>
      {/if}
      <NoteNotice {session} />
    </div>
  </section>
{/if}

<style>
  .notes-page {
    display: grid;
    grid-template-columns: minmax(260px, 320px) minmax(0, 1fr);
    gap: 8px;
    min-width: 0;
    min-height: 0;
    height: 100%;
  }

  .notes-column,
  .notes-stage {
    position: relative;
    display: flex;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
    overflow: hidden;
    border-radius: var(--content-radius);
    background: var(--color-page);
  }

  .narrow {
    grid-template-columns: minmax(0, 1fr);
  }

  .narrow.reading .notes-column,
  .narrow:not(.reading) .notes-stage {
    display: none;
  }

  .column-head {
    display: flex;
    flex-direction: column;
    gap: 12px;
    flex: none;
    padding: 20px 14px 10px 18px;
  }

  .column-title {
    display: flex;
    align-items: center;
    gap: 4px;
  }

  .title-text {
    flex: 1;
    min-width: 0;
  }

  .title-text h1 {
    margin: 0;
    overflow: hidden;
    color: var(--color-text);
    font-size: 22px;
    font-weight: 650;
    line-height: 28px;
    letter-spacing: -0.028em;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .title-text p {
    min-height: 16px;
    margin: 1px 0 0;
    color: var(--color-faint);
    font-size: var(--text-label);
    font-variant-numeric: tabular-nums;
  }

  .column-head :global(.ui-search) {
    width: auto;
  }

  .column-empty {
    text-wrap: pretty;
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding: 28px 22px;
    color: var(--color-muted);
    font-size: var(--text-body);
    line-height: 19px;
  }

  .column-empty strong {
    color: var(--color-text);
    font-weight: 600;
  }

  .column-foot {
    flex: none;
    padding: 6px 8px 8px;
    border-block-start: 1px solid var(--color-border);
  }

  .foot-row {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    height: 34px;
    padding-inline: 12px;
    border: 0;
    border-radius: var(--radius-row);
    background: transparent;
    color: var(--color-label-secondary);
    font: inherit;
    font-size: 13px;
    text-align: start;
    cursor: default;
    outline: none;
  }

  .foot-row:hover {
    background: var(--row-hover);
    color: var(--color-text);
  }

  .foot-row:focus-visible {
    outline: 2px solid var(--color-ring);
    outline-offset: -2px;
  }

  /* Three columns so the middle is centred on the note, whatever sits at
     either side of it. */
  .stage-bar {
    position: relative;
    z-index: 1;
    display: grid;
    grid-template-columns: minmax(0, 1fr) minmax(0, auto) minmax(0, 1fr);
    align-items: center;
    gap: 8px;
    flex: none;
    height: 48px;
    padding-inline: 12px;
    box-shadow: 0 1px 0 transparent;
    transition: box-shadow var(--motion-base) var(--ease-out);
  }

  /* The bar earns an edge only once text runs under it. */
  .stage-bar.scrolled {
    box-shadow: 0 1px 0 var(--color-border);
  }

  .stage-side {
    display: flex;
    align-items: center;
    gap: 2px;
    min-width: 0;
  }

  .stage-side.end {
    justify-content: flex-end;
  }

  .stage-heading {
    display: grid;
    min-width: 0;
    max-width: 420px;
    text-align: center;
  }

  .stage-status,
  .stage-title {
    grid-area: 1 / 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    transition:
      opacity var(--motion-base) var(--ease-out),
      translate var(--motion-base) var(--ease-out);
  }

  .stage-status {
    color: var(--color-faint);
    font-size: var(--text-label);
    font-variant-numeric: tabular-nums;
  }

  .stage-title {
    color: var(--color-text);
    font-size: 13px;
    font-weight: 600;
    letter-spacing: -0.01em;
  }

  .stage-status.away {
    opacity: 0;
    translate: 0 -6px;
  }

  .stage-title.away {
    opacity: 0;
    translate: 0 6px;
  }

  @media (prefers-reduced-motion: reduce) {
    .stage-status,
    .stage-title {
      translate: none !important;
    }
  }

  .stage-scroll {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    overscroll-behavior: contain;
    scrollbar-gutter: stable;
  }

  /* A reading measure: about seventy characters, however wide the window. */
  .stage-column {
    display: flex;
    flex-direction: column;
    box-sizing: border-box;
    max-width: 760px;
    min-height: 100%;
    margin-inline: auto;
    padding: 20px 48px 0;
  }

  .narrow .stage-column {
    padding-inline: 24px;
  }

  .head-button,
  :global(.notes-page .head-button) {
    display: grid;
    place-items: center;
    flex: none;
    width: 30px;
    height: 30px;
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

  :global(.notes-page .head-button:hover),
  :global(.notes-page .head-button[data-state="open"]),
  :global(.notes-page .head-button[aria-pressed="true"]) {
    background: var(--color-fill-hover);
    color: var(--color-text);
  }

  :global(.notes-page .head-button:focus-visible) {
    outline: 2px solid var(--color-ring);
    outline-offset: -2px;
  }

  .stage-empty {
    display: flex;
    flex: 1;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 18px;
    padding: 32px;
    padding-block-end: 12vh;
    text-align: center;
    animation: stage-in var(--motion-slow) var(--ease-out);
  }

  @keyframes stage-in {
    from {
      opacity: 0;
      translate: 0 6px;
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .stage-empty {
      animation-name: stage-fade;
    }

    @keyframes stage-fade {
      from {
        opacity: 0;
      }
    }
  }

  .stage-empty-icon {
    display: grid;
    place-items: center;
    width: 60px;
    height: 60px;
    border-radius: var(--radius-control-large);
    background: var(--color-fill);
    box-shadow: inset 0 0 0 1px var(--color-border);
    color: var(--color-muted);
  }

  .stage-empty-text {
    display: grid;
    gap: 6px;
    max-width: 340px;
  }

  .stage-empty-text h2 {
    margin: 0;
    color: var(--color-text);
    font-size: 17px;
    font-weight: 650;
    letter-spacing: -0.018em;
  }

  .stage-empty-text p {
    margin: 0;
    color: var(--color-muted);
    font-size: var(--text-body);
    line-height: 19px;
    text-wrap: balance;
  }

  .stage-empty-actions {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 10px;
  }

  .stage-new {
    display: flex;
    align-items: center;
    gap: 8px;
    height: 34px;
    padding-inline: 13px 11px;
    border: 0;
    border-radius: var(--radius-control);
    background: var(--color-accent);
    color: var(--color-on-accent);
    font: inherit;
    font-size: var(--text-body);
    font-weight: 550;
    cursor: default;
    outline: none;
    transition:
      background-color var(--motion-instant) var(--ease-smooth),
      scale var(--motion-fast) var(--ease-out);
  }

  .stage-new:hover {
    background: var(--color-accent-hover);
  }

  .stage-new:active {
    scale: 0.97;
  }

  .stage-new:focus-visible {
    outline: 2px solid var(--color-ring);
    outline-offset: 2px;
  }

  .stage-hint {
    margin-inline-start: 4px;
    color: color-mix(in srgb, var(--color-on-accent) 52%, transparent);
    font: inherit;
    font-size: var(--text-label);
    font-weight: 500;
  }

  .stage-link {
    height: 26px;
    padding-inline: 8px;
    border: 0;
    border-radius: var(--radius-inset);
    background: transparent;
    color: var(--color-faint);
    font: inherit;
    font-size: var(--text-label);
    cursor: default;
    outline: none;
    transition: color var(--motion-instant) var(--ease-smooth);
  }

  .stage-link:hover {
    color: var(--color-text);
  }

  .stage-link:focus-visible {
    outline: 2px solid var(--color-ring);
    outline-offset: 2px;
  }
</style>
