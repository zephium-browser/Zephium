<!--
  A Markdown file is a note: the notes editor opens it by identity, with its
  own autosave and conflict handling. Until notes has indexed it (just after
  choosing a vault), it shows as text.
-->
<script lang="ts">
  import type { VaultSession } from "$domain/vault";
  import { loadNoteHost } from "$features/notes";
  import * as m from "$shared/i18n/messages";
  import { markdownOutline, type OutlineItem } from "../../lib/outline";

  let {
    session,
    path,
    host,
    onoutline,
  }: {
    session: VaultSession;
    path: string;
    host: string;
    onoutline: (items: OutlineItem[], go: (target: unknown) => void) => void;
  } = $props();

  let element = $state<HTMLDivElement | null>(null);
  let id = $state<string | null | undefined>(undefined);

  $effect(() => {
    const target = path;
    void session.revision;
    void session.noteId(target).then((found) => {
      if (target === path) id = found;
    });
    void session.readText(target).then((result) => {
      if (result.ok && target === path) onoutline(markdownOutline(result.value.text), go);
    });
  });

  /** Scrolls to the n-th heading the outline listed. */
  function go(target: unknown) {
    if (typeof target !== "number" || !element) return;
    const headings = [...element.querySelectorAll("h1, h2, h3, h4, h5, h6")];
    const items = headings.length;
    if (items === 0) return;
    void session.readText(path).then((result) => {
      if (!result.ok) return;
      const index = markdownOutline(result.value.text).findIndex((item) => item.target === target);
      headings[Math.min(index, items - 1)]?.scrollIntoView({ block: "start", behavior: "smooth" });
    });
  }
</script>

<div class="markdown" bind:this={element}>
  {#if id === undefined}
    <p>{m.surface_loading()}</p>
  {:else if id === null}
    <p role="status">{m.vault_note_indexing()}</p>
  {:else}
    {#await loadNoteHost() then { default: NoteHost }}
      <NoteHost profile={session.profile} {host} {id} />
    {/await}
  {/if}
</div>

<style>
  .markdown {
    block-size: 100%;
    overflow: hidden;
  }

  p {
    padding: 24px;
    color: var(--color-muted);
  }
</style>
