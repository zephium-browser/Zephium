<!--
  A PDF in the vault, read and annotated in place with PDF.js: highlight,
  draw and type on pages, then the annotations are written back into the PDF
  (on Ctrl+S, and on their own a moment after you stop). Selected text can be
  sent to the note beside the PDF.
-->
<script lang="ts">
  import { onDestroy, onMount, untrack } from "svelte";
  import { toBase64, type VaultSession } from "$domain/vault";
  import * as m from "$shared/i18n/messages";
  import { loadPdf } from "../../lib/pdf";
  import type { OutlineItem } from "../../lib/outline";

  let {
    session,
    path,
    onoutline,
    ontonote,
  }: {
    session: VaultSession;
    path: string;
    host: string;
    onoutline: (items: OutlineItem[], go: (target: unknown) => void) => void;
    ontonote: (source: string, text: string, page: number) => void;
  } = $props();

  type Tool = "select" | "highlight" | "draw" | "text";
  const COLORS = ["#FFF176", "#A5D6A7", "#90CAF9", "#F48FB1", "#FFAB91"];

  let container = $state<HTMLDivElement | null>(null);
  let status = $state<"loading" | "ready" | "failed" | "too_large">("loading");
  let page = $state(1);
  let pages = $state(0);
  let scale = $state(1);
  let tool = $state<Tool>("select");
  let color = $state<string>(COLORS[0] ?? "#FFF176");
  let dirty = $state(false);
  let saving = $state<"idle" | "saving" | "saved" | "conflict" | "failed">("idle");
  let selection = $state("");
  let finding = $state(false);
  let query = $state("");
  let findInput = $state<HTMLInputElement | null>(null);

  type Loaded = Awaited<ReturnType<typeof loadPdf>>;
  let lib: Loaded | null = null;
  let viewer: InstanceType<Loaded["web"]["PDFViewer"]> | null = null;
  let links: InstanceType<Loaded["web"]["PDFLinkService"]> | null = null;
  let bus: InstanceType<Loaded["web"]["EventBus"]> | null = null;
  let documentProxy: Awaited<ReturnType<Loaded["pdfjs"]["getDocument"]>["promise"]> | null = null;
  let modified = "";
  let autosave: ReturnType<typeof setTimeout> | null = null;
  let alive = true;

  onMount(() => {
    void open();
    document.addEventListener("selectionchange", selectionChanged);
  });

  onDestroy(() => {
    alive = false;
    document.removeEventListener("selectionchange", selectionChanged);
    if (autosave) clearTimeout(autosave);
    if (dirty) void save();
    else void documentProxy?.destroy();
    viewer?.cleanup();
  });

  async function open() {
    try {
      const stat = await session.call({ kind: "stat", path });
      if (stat.kind !== "entry") throw new Error("missing");
      modified = stat.entry.modified;
      if (Number(stat.entry.size) > 200 * 1024 * 1024) {
        status = "too_large";
        return;
      }
      const [loaded, response] = await Promise.all([loadPdf(), fetch(session.url(path))]);
      if (!response.ok) throw new Error(String(response.status));
      const data = new Uint8Array(await response.arrayBuffer());
      if (!alive || !container) return;
      lib = loaded;
      const { pdfjs, web } = loaded;
      bus = new web.EventBus();
      links = new web.PDFLinkService({ eventBus: bus });
      const findController = new web.PDFFindController({ eventBus: bus, linkService: links });
      viewer = new web.PDFViewer({
        container,
        eventBus: bus,
        linkService: links,
        findController,
        annotationEditorMode: pdfjs.AnnotationEditorType.NONE,
        annotationEditorHighlightColors: COLORS.map((value, index) => `c${index}=${value}`).join(
          ",",
        ),
        removePageBorders: false,
      });
      links.setViewer(viewer);
      bus.on("pagechanging", ({ pageNumber }: { pageNumber: number }) => (page = pageNumber));
      bus.on("scalechanging", ({ scale: next }: { scale: number }) => (scale = next));
      bus.on("pagesinit", () => {
        if (viewer) viewer.currentScaleValue = "page-width";
      });
      // Script in PDFs never runs: no eval, no XFA forms, no scripting.
      const task = pdfjs.getDocument({
        data,
        isEvalSupported: false,
        enableXfa: false,
        useSystemFonts: true,
      } as Parameters<typeof pdfjs.getDocument>[0]);
      documentProxy = await task.promise;
      if (!alive) {
        void documentProxy.destroy();
        return;
      }
      (documentProxy.annotationStorage as { onSetModified: (() => void) | null }).onSetModified =
        () => {
          dirty = true;
          saving = "idle";
          if (autosave) clearTimeout(autosave);
          autosave = setTimeout(() => void save(), 2500);
        };
      viewer.setDocument(documentProxy);
      links.setDocument(documentProxy);
      pages = documentProxy.numPages;
      status = "ready";
      void reportOutline();
    } catch {
      if (alive) status = "failed";
    }
  }

  async function reportOutline() {
    const outline = await documentProxy?.getOutline().catch(() => null);
    const items: OutlineItem[] = [];
    const walk = (nodes: { title: string; dest: unknown; items: unknown[] }[], level: number) => {
      for (const node of nodes) {
        items.push({ title: node.title, level, target: node.dest });
        if (level < 4) walk(node.items as typeof nodes, level + 1);
      }
    };
    if (outline) walk(outline as never, 1);
    onoutline(items, goTo);
  }

  /** Where the right panel's outline sends a click. */
  function goTo(target: unknown) {
    if (typeof target === "number") {
      if (viewer) viewer.currentPageNumber = target;
    } else if (target) void links?.goToDestination(target as string);
  }

  async function save() {
    if (!documentProxy || saving === "saving") return;
    if (autosave) clearTimeout(autosave);
    saving = "saving";
    try {
      const bytes = await documentProxy.saveDocument();
      const response = await session.call({
        kind: "write",
        path,
        base64: toBase64(bytes),
        expected_modified: modified,
      });
      if (response.kind === "entry") {
        modified = response.entry.modified;
        dirty = false;
        saving = "saved";
      } else if (response.kind === "error" && response.error === "changed") saving = "conflict";
      else saving = "failed";
    } catch {
      saving = "failed";
    }
  }

  function setTool(next: Tool) {
    if (!viewer || !lib) return;
    tool = next;
    const types = lib.pdfjs.AnnotationEditorType;
    const mode =
      next === "highlight"
        ? types.HIGHLIGHT
        : next === "draw"
          ? types.INK
          : next === "text"
            ? types.FREETEXT
            : types.NONE;
    viewer.annotationEditorMode = { mode };
    applyColor(color);
  }

  function applyColor(value: string) {
    color = value;
    if (!bus || !lib) return;
    const params = lib.pdfjs.AnnotationEditorParamsType;
    const type =
      tool === "highlight"
        ? params.HIGHLIGHT_COLOR
        : tool === "draw"
          ? params.INK_COLOR
          : tool === "text"
            ? params.FREETEXT_COLOR
            : null;
    if (type !== null)
      bus.dispatch("switchannotationeditorparams", { source: untrack(() => viewer), type, value });
  }

  function zoom(step: number) {
    if (!viewer) return;
    viewer.currentScale = Math.min(
      5,
      Math.max(0.25, Math.round((viewer.currentScale + step) * 100) / 100),
    );
  }

  function find(again = false) {
    bus?.dispatch("find", {
      source: viewer,
      type: again ? "again" : "",
      query,
      caseSensitive: false,
      entireWord: false,
      highlightAll: true,
      findPrevious: false,
      matchDiacritics: false,
    });
  }

  function selectionChanged() {
    const current = document.getSelection();
    if (!current || current.isCollapsed || !container?.contains(current.anchorNode)) {
      selection = "";
      return;
    }
    selection = current.toString().trim().slice(0, 4000);
  }

  function keydown(event: KeyboardEvent) {
    const mod = event.ctrlKey || event.metaKey;
    if (mod && event.key.toLowerCase() === "s") {
      event.preventDefault();
      event.stopPropagation();
      void save();
    } else if (mod && event.key.toLowerCase() === "f") {
      event.preventDefault();
      event.stopPropagation();
      finding = true;
      queueMicrotask(() => findInput?.focus());
    } else if (mod && (event.key === "=" || event.key === "+")) {
      event.preventDefault();
      zoom(0.1);
    } else if (mod && event.key === "-") {
      event.preventDefault();
      zoom(-0.1);
    }
  }

  function wheel(event: WheelEvent) {
    if (!event.ctrlKey) return;
    event.preventDefault();
    zoom(event.deltaY < 0 ? 0.1 : -0.1);
  }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="pdf" onkeydown={keydown}>
  <div class="toolbar" role="toolbar" aria-label={m.vault_pdf_tools()}>
    <div class="group">
      <input
        class="page"
        type="number"
        min="1"
        max={pages}
        value={page}
        aria-label={m.vault_page()}
        onchange={(event) => {
          const next = Number(event.currentTarget.value);
          if (viewer && next >= 1 && next <= pages) viewer.currentPageNumber = next;
        }}
      />
      <span class="of">{m.vault_of_pages({ count: pages })}</span>
    </div>
    <div class="group">
      <button
        type="button"
        aria-label={m.vault_zoom_out()}
        data-keytip="-"
        onclick={() => zoom(-0.1)}>−</button
      >
      <span class="zoom">{Math.round(scale * 100)}%</span>
      <button type="button" aria-label={m.vault_zoom_in()} data-keytip="+" onclick={() => zoom(0.1)}
        >+</button
      >
      <button
        type="button"
        data-keytip="W"
        onclick={() => {
          if (viewer) viewer.currentScaleValue = "page-width";
        }}>{m.vault_fit_width()}</button
      >
    </div>
    <div class="group tools" role="radiogroup" aria-label={m.vault_annotate()}>
      {#each [["select", m.vault_tool_select(), "S"], ["highlight", m.vault_tool_highlight(), "H"], ["draw", m.vault_tool_draw(), "D"], ["text", m.vault_tool_text(), "T"]] as const as [id, label, tip] (id)}
        <button
          type="button"
          role="radio"
          aria-checked={tool === id}
          class:on={tool === id}
          data-keytip={tip}
          onclick={() => setTool(id)}>{label}</button
        >
      {/each}
      {#if tool !== "select"}
        {#each COLORS as swatch (swatch)}
          <button
            type="button"
            class="swatch"
            class:on={color === swatch}
            style:background={swatch}
            aria-label={swatch}
            onclick={() => applyColor(swatch)}
          ></button>
        {/each}
      {/if}
    </div>
    <div class="group end">
      {#if selection}
        <button
          type="button"
          data-keytip="N"
          onclick={() => {
            ontonote(path, selection, page);
            document.getSelection()?.removeAllRanges();
          }}>{m.vault_to_note()}</button
        >
      {/if}
      <button type="button" data-keytip="F" onclick={() => (finding = !finding)}
        >{m.vault_find()}</button
      >
      <button
        type="button"
        class:primary={dirty}
        disabled={!dirty && saving !== "conflict"}
        data-keytip="V"
        onclick={() => void save()}
      >
        {saving === "saving"
          ? m.vault_saving()
          : saving === "conflict"
            ? m.vault_save_conflict()
            : saving === "failed"
              ? m.vault_save_failed()
              : dirty
                ? m.vault_save()
                : m.vault_saved()}
      </button>
    </div>
  </div>
  {#if finding}
    <div class="find">
      <input
        bind:this={findInput}
        bind:value={query}
        placeholder={m.vault_find_in_pdf()}
        aria-label={m.vault_find_in_pdf()}
        oninput={() => find()}
        onkeydown={(event) => {
          if (event.key === "Enter") find(true);
          if (event.key === "Escape") {
            finding = false;
            query = "";
            find();
          }
          event.stopPropagation();
        }}
      />
    </div>
  {/if}
  <div class="body">
    <div class="stage" bind:this={container} onwheel={wheel} tabindex="-1">
      <div class="pdfViewer"></div>
    </div>
  </div>
  {#if status === "loading"}
    <p class="status">{m.surface_loading()}</p>
  {:else if status === "failed"}
    <p class="status" role="alert">{m.vault_open_failed()}</p>
  {:else if status === "too_large"}
    <p class="status" role="alert">{m.vault_too_large()}</p>
  {/if}
</div>

<style>
  .pdf {
    position: relative;
    display: flex;
    flex-direction: column;
    block-size: 100%;
    min-block-size: 0;
  }

  .toolbar {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px 12px;
    padding: 6px 10px;
    border-block-end: 1px solid var(--color-border);
    font-size: 12px;
  }

  .group {
    display: flex;
    align-items: center;
    gap: 4px;
  }

  .end {
    margin-inline-start: auto;
  }

  button {
    block-size: 26px;
    padding-inline: 8px;
    border: 0;
    border-radius: var(--radius-control-compact);
    background: transparent;
    color: var(--color-label-secondary);
    font: inherit;
  }

  button:disabled {
    color: var(--color-faint);
  }

  button:hover:not(:disabled) {
    background: var(--color-fill-hover);
    color: var(--color-text);
  }

  button.on,
  button.primary {
    background: var(--color-fill-active);
    color: var(--color-text);
  }

  .swatch {
    inline-size: 18px;
    block-size: 18px;
    padding: 0;
    border-radius: 50%;
    box-shadow: inset 0 0 0 1px var(--color-border-strong);
  }

  .swatch.on {
    outline: 2px solid var(--color-ring);
    outline-offset: 1px;
  }

  .page {
    inline-size: 48px;
    block-size: 24px;
    border: 1px solid var(--color-border);
    border-radius: var(--radius-control-compact);
    background: var(--color-field);
    color: var(--color-text);
    font: inherit;
    text-align: center;
  }

  .of,
  .zoom {
    color: var(--color-muted);
  }

  .zoom {
    min-inline-size: 40px;
    text-align: center;
  }

  .find {
    padding: 6px 10px;
    border-block-end: 1px solid var(--color-border);
  }

  .find input {
    inline-size: min(320px, 100%);
    block-size: 26px;
    padding-inline: 8px;
    border: 1px solid var(--color-border);
    border-radius: var(--radius-control-compact);
    background: var(--color-field);
    color: var(--color-text);
    font: inherit;
  }

  .stage {
    position: absolute;
    inset: 0;
    overflow: auto;
    outline: none;
    background: var(--color-canvas);
  }

  .body {
    position: relative;
    flex: 1;
    min-block-size: 0;
  }

  .stage :global(.page) {
    margin: 12px auto;
    border: 0;
    box-shadow: var(--shadow-float);
  }

  .status {
    position: absolute;
    inset-block-start: 64px;
    inset-inline: 0;
    color: var(--color-muted);
    text-align: center;
  }
</style>
