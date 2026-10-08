<script lang="ts">
  import type { NoteSession } from "$domain/notes";
  import { commands } from "$shared/ipc/bindings";
  import LazyView from "$shared/ui/LazyView";
  import Icon from "$shared/ui/Icon";
  import { Alert02Icon, Note01Icon } from "@hugeicons/core-free-icons";
  import * as m from "$shared/i18n/messages";
  import { revealLabel } from "../lib/platform";

  let {
    session,
    density,
    autofocus = false,
  }: { session: NoteSession; density: "panel" | "page"; autofocus?: boolean } = $props();

  const loadEditor = () => import("./editor/NoteEditor.svelte");

  let note = $derived(session.note);
  let blocked = $derived(session.openError === "too_large" || session.openError === "unavailable");

  function openLink(href: string) {
    if (/^(?:https?|mailto):/iu.test(href)) void commands.browserOpenUrl(href, true);
  }

  async function openNote(target: string) {
    const found = (await session.resolveTargets([target]))[target];
    if (found) await session.open(found.id);
    else await session.create(`# ${target}\n\n`);
  }
</script>

{#if note}
  <div class="note-view" data-density={density}>
    {#if session.saveState === "conflict" && session.conflict}
      <div class="note-banner" role="alert">
        <Icon icon={Alert02Icon} size={16} />
        <div class="banner-text">
          <strong>{m.note_conflict()}</strong>
          <span>{m.note_conflict_body()}</span>
        </div>
        <div class="banner-actions">
          <button type="button" class="banner-button" onclick={() => void session.resolve("theirs")}
            >{m.note_use_theirs()}</button
          >
          <button
            type="button"
            class="banner-button primary"
            onclick={() => void session.resolve("mine")}>{m.note_keep_mine()}</button
          >
        </div>
      </div>
    {:else if note.trashed}
      <div class="note-banner quiet">
        <div class="banner-text"><strong>{m.note_trashed()}</strong></div>
        <div class="banner-actions">
          <button
            type="button"
            class="banner-button primary"
            onclick={() => note.id && void session.restore(note.id)}>{m.note_restore()}</button
          >
        </div>
      </div>
    {:else if session.saveState === "failed"}
      <div class="note-banner" role="alert">
        <Icon icon={Alert02Icon} size={16} />
        <div class="banner-text">
          <strong
            >{session.saveError === "too_large" ? m.note_too_large() : m.note_read_only()}</strong
          >
        </div>
      </div>
    {:else if !note.editable && !blocked && session.openError === null}
      <div class="note-banner quiet">
        <div class="banner-text">
          <strong>{m.note_read_only()}</strong>
          <span>{m.note_read_only_body()}</span>
        </div>
      </div>
    {/if}

    {#if blocked}
      <div class="note-blocked">
        <span class="blocked-icon"><Icon icon={Note01Icon} size={24} /></span>
        <h3>{session.openError === "too_large" ? m.note_too_large() : m.note_open_failed()}</h3>
        <p>
          {session.openError === "too_large" ? m.note_too_large_body() : m.note_unavailable_body()}
        </p>
        {#if note.id && session.openError === "too_large"}<button
            type="button"
            class="banner-button"
            onclick={() => void session.reveal(note.id)}>{revealLabel()}</button
          >{:else if note.id}<button
            type="button"
            class="banner-button"
            onclick={() => {
              const id = note.id!;
              void session.close().then(() => session.open(id));
            }}>{m.note_try_again()}</button
          >{/if}
      </div>
    {:else}
      <LazyView
        loader={loadEditor}
        loadingLabel=""
        failureLabel={m.surface_render_failed()}
        retryLabel={m.surface_retry()}
        >{#snippet children(NoteEditor)}{#key note.version}<NoteEditor
              source={note.source}
              editable={note.editable && session.saveState !== "conflict"}
              {density}
              autofocus={autofocus && note.id === null}
              linksRevision={session.linksRevision}
              onchange={(read) => session.edit(read)}
              onleave={() => void session.settle()}
              onopenlink={openLink}
              onopennote={(target) => void openNote(target)}
              resolve={(targets) => session.resolveTargets(targets)}
              find={(query) => session.find(query)}
            />{/key}{/snippet}</LazyView
      >
      {#if session.backlinks.length}
        <section class="note-backlinks" aria-label={m.note_linked_from()}>
          <h4>{m.note_linked_from()}</h4>
          <ul>
            {#each session.backlinks as link (link.id)}
              <li>
                <button type="button" onclick={() => void session.open(link.id)}>
                  <Icon icon={Note01Icon} size={14} />
                  <span class="backlink-title">{link.title}</span>
                  {#if link.preview}<span class="backlink-preview">{link.preview}</span>{/if}
                </button>
              </li>
            {/each}
          </ul>
        </section>
      {/if}
    {/if}
  </div>
{/if}

<style>
  .note-view {
    display: flex;
    flex: 1 0 auto;
    flex-direction: column;
  }

  .note-banner {
    display: flex;
    align-items: flex-start;
    gap: 10px;
    margin-block-end: 18px;
    padding: 12px 12px 12px 14px;
    border-radius: var(--radius-control);
    background: var(--color-fill);
    color: var(--color-text);
    animation: banner-in var(--motion-base) var(--ease-out);
  }

  .note-banner > :global(svg) {
    flex: none;
    margin-block-start: 1px;
    color: var(--color-warning);
  }

  .note-banner.quiet {
    align-items: center;
    background: var(--color-fill);
  }

  @keyframes banner-in {
    from {
      opacity: 0;
      translate: 0 -4px;
    }
  }

  .banner-text {
    display: flex;
    flex: 1;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
    font-size: var(--text-body);
    line-height: 18px;
  }

  .banner-text strong {
    font-weight: 600;
  }

  .banner-text span {
    color: var(--color-label-secondary);
  }

  .banner-actions {
    display: flex;
    flex-wrap: wrap;
    justify-content: flex-end;
    gap: 6px;
  }

  [data-density="panel"] .note-banner {
    flex-wrap: wrap;
  }

  [data-density="panel"] .banner-actions {
    width: 100%;
  }

  .banner-button {
    height: 28px;
    padding-inline: 12px;
    border: 0;
    border-radius: var(--radius-inset);
    background: var(--color-control);
    color: var(--color-text);
    font: inherit;
    font-size: var(--text-body);
    font-weight: 500;
    white-space: nowrap;
    cursor: default;
    transition: background-color var(--motion-instant) var(--ease-smooth);
  }

  .banner-button:hover {
    background: var(--color-control-hover);
  }

  .banner-button.primary {
    background: var(--color-accent);
    color: var(--color-on-accent);
  }

  .banner-button.primary:hover {
    background: var(--color-accent-hover);
  }

  .banner-button:focus-visible {
    outline: 2px solid var(--color-ring);
    outline-offset: 2px;
  }

  .note-blocked {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 6px;
    margin: auto;
    padding: 48px 24px;
    text-align: center;
  }

  .blocked-icon {
    display: grid;
    place-items: center;
    width: 48px;
    height: 48px;
    margin-block-end: 6px;
    border-radius: var(--radius-control-large);
    background: var(--color-fill);
    color: var(--color-muted);
  }

  .note-blocked h3 {
    margin: 0;
    font-size: 15px;
    font-weight: 600;
  }

  .note-blocked p {
    max-width: 320px;
    text-wrap: balance;
    margin: 0 0 10px;
    color: var(--color-muted);
    font-size: var(--text-body);
    line-height: 19px;
  }

  .note-backlinks {
    margin-block: 8px 24px;
    padding-block-start: 16px;
    border-block-start: 1px solid var(--color-border);
  }

  .note-backlinks h4 {
    margin: 0 0 6px;
    color: var(--color-faint);
    font-size: var(--text-label);
    font-weight: 550;
  }

  .note-backlinks ul {
    display: grid;
    gap: 2px;
    margin: 0 -8px;
    padding: 0;
    list-style: none;
  }

  .note-backlinks button {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    min-width: 0;
    height: 32px;
    padding-inline: 8px;
    border: 0;
    border-radius: var(--radius-inset);
    background: transparent;
    color: var(--color-muted);
    font: inherit;
    font-size: var(--text-body);
    text-align: start;
    cursor: default;
  }

  .note-backlinks button:hover {
    background: var(--row-hover);
  }

  .backlink-title {
    flex: none;
    max-width: 60%;
    overflow: hidden;
    color: var(--color-text);
    font-weight: 500;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .backlink-preview {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    color: var(--color-faint);
    text-overflow: ellipsis;
    white-space: nowrap;
  }
</style>
