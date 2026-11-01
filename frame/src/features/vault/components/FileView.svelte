<!--
  One open file, shown by the viewer for its type. Each viewer is its own lazy
  chunk, so PDF.js and the Office readers load only when such a file opens.
-->
<script lang="ts">
  import type { VaultSession } from "$domain/vault";
  import * as m from "$shared/i18n/messages";
  import { extOf, viewerFor } from "../lib/kinds";
  import type { OutlineItem } from "../lib/outline";

  let {
    session,
    path,
    host,
    onoutline,
    ontonote,
  }: {
    session: VaultSession;
    path: string;
    /** Distinguishes panes that show notes side by side. */
    host: string;
    onoutline: (items: OutlineItem[], go: (target: unknown) => void) => void;
    /** Appends quoted text to the note beside a PDF. */
    ontonote: (source: string, text: string, page: number) => void;
  } = $props();

  let kind = $derived(viewerFor(extOf(path)));
  const loaders = {
    markdown: () => import("./viewers/MarkdownViewer.svelte"),
    pdf: () => import("./viewers/PdfViewer.svelte"),
    docx: () => import("./viewers/DocxViewer.svelte"),
    sheet: () => import("./viewers/SheetViewer.svelte"),
    image: () => import("./viewers/ImageViewer.svelte"),
    text: () => import("./viewers/TextViewer.svelte"),
    other: () => import("./viewers/OtherViewer.svelte"),
  } as const;
</script>

{#key path}
  {#await loaders[extOf(path) === "csv" ? "sheet" : kind]()}
    <p class="status">{m.surface_loading()}</p>
  {:then { default: Viewer }}
    <Viewer {session} {path} {host} {onoutline} {ontonote} />
  {:catch}
    <p class="status" role="alert">{m.surface_render_failed()}</p>
  {/await}
{/key}

<style>
  .status {
    padding: 24px;
    color: var(--color-muted);
  }
</style>
