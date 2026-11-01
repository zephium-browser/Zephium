<!--
  The vault workspace: an Obsidian-style file explorer on the left, files open
  in up to four panes in the middle, and the outline, backlinks and journal on
  the right. Everything has a key: see the hint row and tap Alt for key tips.
-->
<script lang="ts">
  import { untrack } from "svelte";
  import { SvelteSet } from "svelte/reactivity";
  import { nameOf, parentOf, vaultSession, type VaultEntry } from "$domain/vault";
  import * as m from "$shared/i18n/messages";
  import { displayName, extOf } from "../lib/kinds";
  import { journalName, journalPath, journalTemplate, JOURNAL } from "../lib/journal";
  import { KeyTips } from "../lib/keytips.svelte";
  import type { OutlineItem } from "../lib/outline";
  import { ancestors } from "../lib/tree";
  import { Workspace, type Layout } from "../lib/workspace.svelte";
  import ContextMenu, { type ContextItem } from "./ContextMenu.svelte";
  import FileTree from "./FileTree.svelte";
  import Palette, { type PaletteItem, type PaletteMode } from "./Palette.svelte";
  import PaneGrid from "./PaneGrid.svelte";
  import RightPanel from "./RightPanel.svelte";

  let { profile, onclose }: { profile: string; onclose: () => void } = $props();

  const storage = typeof localStorage === "undefined" ? null : localStorage;
  const session = untrack(() => vaultSession(profile));
  const workspace = untrack(() => new Workspace(profile, storage));
  const tips = new KeyTips();
  const EXPANDED = `zephium.vault.expanded.${untrack(() => profile)}`;

  let root = $state<HTMLDivElement | null>(null);
  let expanded = new SvelteSet<string>(readExpanded());
  let selected = $state<string | null>(null);
  let renaming = $state<string | null>(null);
  let menu = $state<{ x: number; y: number; entry: VaultEntry | null } | null>(null);
  let palette = $state<{ mode: PaletteMode; target: string | null } | null>(null);
  let outline = $state.raw<OutlineItem[]>([]);
  let outlineGo: (target: unknown) => void = () => {};
  let leftOpen = $state(true);
  let rightOpen = $state(true);
  let leftWidth = $state(280);
  let toast = $state<{ text: string; action?: { label: string; run: () => void } } | null>(null);
  let toastTimer: ReturnType<typeof setTimeout> | null = null;

  let current = $derived(workspace.current);
  let crumbs = $derived(current ? current.split("/") : []);

  function readExpanded(): string[] {
    try {
      const value = JSON.parse(storage?.getItem(EXPANDED) ?? "[]");
      return Array.isArray(value) ? value.filter((item) => typeof item === "string") : [];
    } catch {
      return [];
    }
  }

  function saveExpanded() {
    try {
      storage?.setItem(EXPANDED, JSON.stringify([...expanded]));
    } catch {
      // The tree only forgets which folders were open.
    }
  }

  // Load every remembered open folder once the vault is known.
  $effect(() => {
    if (!session.root) return;
    untrack(() => {
      for (const dir of expanded) if (!session.listings.has(dir)) void session.list(dir);
    });
  });

  function say(text: string, action?: { label: string; run: () => void }) {
    toast = { text, action };
    if (toastTimer) clearTimeout(toastTimer);
    toastTimer = setTimeout(() => (toast = null), action ? 8000 : 4000);
  }

  function errorText(error: string): string {
    if (error === "exists") return m.vault_error_exists();
    if (error === "not_found") return m.vault_error_not_found();
    if (error === "invalid") return m.vault_error_invalid();
    if (error === "changed") return m.vault_save_conflict();
    return m.vault_error_generic();
  }

  function toggle(path: string, open?: boolean) {
    const next = open ?? !expanded.has(path);
    if (next) {
      expanded.add(path);
      if (!session.listings.has(path)) void session.list(path);
    } else expanded.delete(path);
    saveExpanded();
  }

  /** Opens the folders above a path so the tree shows it. */
  function reveal(path: string) {
    for (const dir of ancestors(path)) {
      if (!expanded.has(dir)) toggle(dir, true);
    }
    selected = path;
  }

  function openPath(path: string, beside = false) {
    if (beside) workspace.openBeside(path);
    else workspace.open(path);
    reveal(path);
  }

  function openEntry(entry: VaultEntry, beside: boolean) {
    if (entry.kind === "folder") toggle(entry.path);
    else openPath(entry.path, beside);
  }

  /** The folder new things go in: the selected folder, or the selected/current file's folder. */
  function targetFolder(): string {
    const pick = selected ?? current;
    if (!pick) return "";
    const listed = session.listings.get(parentOf(pick))?.find((entry) => entry.path === pick);
    return listed?.kind === "folder" ? pick : parentOf(pick);
  }

  async function newNote(dir = targetFolder()) {
    const result = await session.change(
      { kind: "create_file", dir, name: `${m.vault_untitled()}.md`, text: "" },
      [dir],
    );
    if (!result.ok || !result.value) return say(errorText(result.ok ? "io" : result.error));
    if (dir) toggle(dir, true);
    openPath(result.value.path);
    renaming = result.value.path;
  }

  async function newFolder(dir = targetFolder()) {
    const result = await session.change(
      { kind: "create_folder", dir, name: m.vault_new_folder_name() },
      [dir],
    );
    if (!result.ok || !result.value) return say(errorText(result.ok ? "io" : result.error));
    if (dir) toggle(dir, true);
    selected = result.value.path;
    renaming = result.value.path;
  }

  function follow(from: string, to: string) {
    workspace.renamed(from, to);
    for (const dir of [...expanded]) {
      if (dir === from || dir.startsWith(`${from}/`)) {
        expanded.delete(dir);
        expanded.add(to + dir.slice(from.length));
      }
    }
    saveExpanded();
    if (selected === from) selected = to;
  }

  async function rename(entry: VaultEntry, name: string) {
    const result = await session.change({ kind: "rename", path: entry.path, name }, [
      parentOf(entry.path),
    ]);
    if (!result.ok || !result.value) return say(errorText(result.ok ? "io" : result.error));
    follow(entry.path, result.value.path);
  }

  async function move(path: string, dest: string) {
    if (parentOf(path) === dest) return;
    const result = await session.change({ kind: "move", path, dest }, [parentOf(path), dest]);
    if (!result.ok || !result.value) return say(errorText(result.ok ? "io" : result.error));
    const moved = result.value.path;
    follow(path, moved);
    if (dest) toggle(dest, true);
    selected = moved;
    say(m.vault_moved({ name: displayName(nameOf(moved)), folder: dest || m.vault_root_name() }), {
      label: m.vault_undo(),
      run: () => void move(moved, parentOf(path)),
    });
  }

  async function trash(entry: VaultEntry) {
    const result = await session.change({ kind: "trash", path: entry.path }, [
      parentOf(entry.path),
    ]);
    if (!result.ok) return say(errorText(result.error));
    workspace.removed(entry.path);
    const trashed = result.value;
    say(
      m.vault_trashed({ name: displayName(entry.name) }),
      trashed
        ? {
            label: m.vault_undo(),
            run: () =>
              void session
                .change({ kind: "move", path: trashed.path, dest: parentOf(entry.path) }, [
                  parentOf(entry.path),
                ])
                .then((restored) => {
                  if (restored.ok && restored.value && restored.value.path !== entry.path) {
                    void rename(restored.value, entry.name);
                  }
                }),
          }
        : undefined,
    );
  }

  function startRename(path: string | null) {
    if (!path) return;
    reveal(path);
    renaming = path;
  }

  /** Appends a quote from a PDF to "<pdf name> notes.md" beside it. */
  async function toNote(source: string, text: string, page: number) {
    const dir = parentOf(source);
    const stem = nameOf(source).replace(/\.[^.]+$/u, "");
    const name = `${stem} notes.md`;
    const path = dir ? `${dir}/${name}` : name;
    const quote = `${text
      .split(/\r?\n/u)
      .map((line) => `> ${line}`)
      .join("\n")}\n> — [[${nameOf(source)}]], p. ${page}\n\n`;
    const existing = await session.readText(path);
    let response;
    if (existing.ok) {
      const bytes = new TextEncoder().encode(
        `${existing.value.text.replace(/\s*$/u, "\n\n")}${quote}`,
      );
      const { toBase64 } = await import("$domain/vault");
      response = await session.change(
        {
          kind: "write",
          path,
          base64: toBase64(bytes),
          expected_modified: existing.value.modified,
        },
        [dir],
      );
    } else {
      response = await session.change(
        { kind: "create_file", dir, name, text: `# ${stem} notes\n\n${quote}` },
        [dir],
      );
    }
    if (!response.ok) return say(errorText(response.error));
    say(m.vault_added_to_note({ name: displayName(name) }), {
      label: m.vault_open(),
      run: () => openPath(path, true),
    });
  }

  async function journal(date: Date) {
    const path = journalPath(date);
    const exists = await session.call({ kind: "stat", path });
    if (exists.kind !== "entry") {
      const folder = await session.call({ kind: "stat", path: JOURNAL });
      if (folder.kind !== "entry") {
        const made = await session.change({ kind: "create_folder", dir: "", name: JOURNAL }, [""]);
        if (!made.ok) return say(errorText(made.error));
      }
      const created = await session.change(
        { kind: "create_file", dir: JOURNAL, name: journalName(date), text: journalTemplate(date) },
        [JOURNAL],
      );
      if (!created.ok) return say(errorText(created.error));
    }
    openPath(path);
  }

  async function chooseVault() {
    const result = await session.chooseRoot();
    if (!result.ok) say(errorText(result.error));
    else if (result.value) {
      expanded.clear();
      saveExpanded();
    }
  }

  function contextItems(entry: VaultEntry | null): ContextItem[] {
    if (!entry || entry.kind === "folder") {
      return [
        { id: "new-note", label: m.vault_new_note(), hint: "Ctrl+N" },
        { id: "new-folder", label: m.vault_new_folder(), hint: "Ctrl+Shift+N" },
        ...(entry
          ? ([
              { separator: true },
              { id: "rename", label: m.vault_rename(), hint: "F2" },
              { id: "move", label: m.vault_move_to(), hint: "Ctrl+M" },
              { id: "reveal", label: m.vault_show_in_folder() },
              { separator: true },
              { id: "trash", label: m.vault_delete(), hint: "Del", danger: true },
            ] satisfies ContextItem[])
          : [{ id: "reveal", label: m.vault_show_in_folder() }]),
      ];
    }
    return [
      { id: "open", label: m.vault_open(), hint: "Enter" },
      { id: "beside", label: m.vault_open_beside(), hint: "Ctrl+Enter" },
      { id: "native", label: m.vault_open_default() },
      { separator: true },
      { id: "rename", label: m.vault_rename(), hint: "F2" },
      { id: "move", label: m.vault_move_to(), hint: "Ctrl+M" },
      { id: "reveal", label: m.vault_show_in_folder() },
      { separator: true },
      { id: "trash", label: m.vault_delete(), hint: "Del", danger: true },
    ];
  }

  function runContext(id: string, entry: VaultEntry | null) {
    const dir = entry ? (entry.kind === "folder" ? entry.path : parentOf(entry.path)) : "";
    if (id === "new-note") void newNote(dir);
    else if (id === "new-folder") void newFolder(dir);
    else if (!entry) {
      if (id === "reveal") void session.call({ kind: "reveal", path: "" });
    } else if (id === "open") openEntry(entry, false);
    else if (id === "beside") openEntry(entry, true);
    else if (id === "native") void session.call({ kind: "open_native", path: entry.path });
    else if (id === "rename") renaming = entry.path;
    else if (id === "move") palette = { mode: "move", target: entry.path };
    else if (id === "reveal") void session.call({ kind: "reveal", path: entry.path });
    else if (id === "trash") void trash(entry);
  }

  async function paletteSearch(query: string): Promise<PaletteItem[]> {
    const mode = palette?.mode;
    if (mode === "layout") {
      const names = Object.keys(workspace.layouts());
      return names
        .filter((name) => name.toLowerCase().includes(query.trim().toLowerCase()))
        .map((name) => ({ id: name, title: name, detail: m.vault_layout_saved() }));
    }
    const entries = await session.search(query, 50, mode === "move");
    const items = entries.map((entry) => ({
      id: entry.path,
      title: entry.kind === "folder" ? entry.name : displayName(entry.name),
      detail: parentOf(entry.path) || m.vault_root_name(),
    }));
    if (mode === "move" && !query.trim())
      items.unshift({ id: "", title: m.vault_root_name(), detail: "" });
    return items;
  }

  function palettePick(item: PaletteItem, query: string, alternate: boolean) {
    const state = palette;
    palette = null;
    if (!state) return;
    if (state.mode === "open") {
      const listed = session.listings
        .get(parentOf(item.id))
        ?.find((entry) => entry.path === item.id);
      if (listed?.kind === "folder" || !extOf(item.id)) {
        reveal(item.id);
        toggle(item.id, true);
      } else openPath(item.id, alternate);
    } else if (state.mode === "move" && state.target !== null) void move(state.target, item.id);
    else if (state.mode === "layout") {
      if (alternate || !item.id) {
        workspace.saveLayout(item.title || query.trim());
        say(m.vault_layout_saved_named({ name: item.title || query.trim() }));
      } else if (workspace.loadLayout(item.id)) say(m.vault_layout_restored({ name: item.id }));
    }
  }

  function setLayout(layout: Layout) {
    workspace.setLayout(layout);
  }

  function keydown(event: KeyboardEvent) {
    if (tips.keydown(event, root)) {
      event.preventDefault();
      event.stopPropagation();
      return;
    }
    if (event.defaultPrevented || palette || menu) return;
    const mod = event.ctrlKey || event.metaKey;
    const key = event.key.toLowerCase();
    const editing =
      event.target instanceof HTMLElement &&
      (event.target.isContentEditable || ["INPUT", "TEXTAREA"].includes(event.target.tagName));
    let handled = true;
    if (mod && !event.shiftKey && key === "o") palette = { mode: "open", target: null };
    else if (mod && !event.shiftKey && key === "m")
      palette = { mode: "move", target: selected ?? current };
    else if (mod && event.shiftKey && key === "n") void newFolder();
    else if (mod && !event.shiftKey && key === "n") void newNote();
    else if (mod && key === "w") workspace.closeCurrent();
    else if (mod && event.key === "PageDown") workspace.cycle(1);
    else if (mod && event.key === "PageUp") workspace.cycle(-1);
    else if (mod && event.key === "\\") {
      if (current) workspace.openBeside(current);
      else setLayout("columns");
    } else if (mod && event.shiftKey && key === "l") palette = { mode: "layout", target: null };
    else if (event.key === "F6") {
      const panes = root?.querySelectorAll<HTMLElement>(".pane");
      if (document.activeElement?.closest(".tree") && panes?.length) {
        panes[workspace.focused]
          ?.querySelector<HTMLElement>("[role=tab][tabindex='0'], .stage")
          ?.focus();
      } else if (
        event.shiftKey ||
        workspace.panes.length === 1 ||
        workspace.focused === workspace.panes.length - 1
      ) {
        root?.querySelector<HTMLElement>(".tree")?.focus();
      } else workspace.focusPane(1);
    } else if (event.key === "F2" && !editing && !document.activeElement?.closest(".tree"))
      startRename(current);
    else handled = false;
    if (handled) {
      event.preventDefault();
      event.stopPropagation();
    }
  }

  function keyup(event: KeyboardEvent) {
    if (tips.keyup(event, root)) {
      event.preventDefault();
      event.stopPropagation();
    }
  }

  function resizeStart(event: PointerEvent) {
    const start = event.clientX;
    const width = leftWidth;
    const target = event.currentTarget as HTMLElement;
    target.setPointerCapture(event.pointerId);
    const moveHandler = (next: PointerEvent) => {
      leftWidth = Math.min(520, Math.max(180, width + next.clientX - start));
    };
    const up = () => {
      target.removeEventListener("pointermove", moveHandler);
      target.removeEventListener("pointerup", up);
    };
    target.addEventListener("pointermove", moveHandler);
    target.addEventListener("pointerup", up);
  }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="vault" bind:this={root} onkeydown={keydown} onkeyup={keyup}>
  {#if session.root === undefined}
    <p class="loading">{m.surface_loading()}</p>
  {:else if session.root === null}
    <div class="welcome">
      <h1>{m.vault_welcome_title()}</h1>
      <p>{m.vault_welcome_body()}</p>
      <button type="button" class="primary" data-keytip="V" onclick={() => void chooseVault()}
        >{m.vault_choose()}</button
      >
      <button type="button" onclick={onclose}>{m.vault_back_to_browsing()}</button>
    </div>
  {:else}
    <header class="bar">
      <button
        type="button"
        class="icon"
        aria-label={m.vault_toggle_files()}
        aria-pressed={leftOpen}
        data-keytip="1"
        onclick={() => (leftOpen = !leftOpen)}>☰</button
      >
      <nav class="crumbs" aria-label={m.vault_location()}>
        {#each crumbs as crumb, index (index)}
          {#if index > 0}<span class="sep">/</span>{/if}
          <span class:last={index === crumbs.length - 1}
            >{index === crumbs.length - 1 ? displayName(crumb) : crumb}</span
          >
        {:else}
          <span>{m.vault_title()}</span>
        {/each}
      </nav>
      <div class="layouts" role="radiogroup" aria-label={m.vault_layout()}>
        {#each [["single", m.vault_layout_single(), "Q"], ["columns", m.vault_layout_columns(), "W"], ["rows", m.vault_layout_rows(), "E"], ["grid", m.vault_layout_grid(), "R"]] as const as [id, label, tip] (id)}
          <button
            type="button"
            role="radio"
            class="layout"
            data-layout={id}
            aria-label={label}
            aria-checked={workspace.layout === id}
            class:on={workspace.layout === id}
            data-keytip={tip}
            onclick={() => setLayout(id)}
            ><span></span><span></span><span></span><span></span></button
          >
        {/each}
      </div>
      <button
        type="button"
        data-keytip="L"
        onclick={() => (palette = { mode: "layout", target: null })}>{m.vault_layouts()}</button
      >
      <button
        type="button"
        data-keytip="P"
        onclick={() => (palette = { mode: "open", target: null })}>{m.vault_quick_open()}</button
      >
      <button
        type="button"
        class="icon"
        aria-label={m.vault_toggle_details()}
        aria-pressed={rightOpen}
        data-keytip="2"
        onclick={() => (rightOpen = !rightOpen)}>☷</button
      >
      <button
        type="button"
        class="icon"
        aria-label={m.vault_back_to_browsing()}
        data-keytip="X"
        onclick={onclose}>×</button
      >
    </header>
    <div class="body">
      {#if leftOpen}
        <aside class="files" style:inline-size="{leftWidth}px" aria-label={m.vault_files()}>
          <div class="files-bar">
            <button
              type="button"
              aria-label={m.vault_new_note()}
              title="{m.vault_new_note()} (Ctrl+N)"
              data-keytip="N"
              onclick={() => void newNote()}>✎</button
            >
            <button
              type="button"
              aria-label={m.vault_new_folder()}
              title="{m.vault_new_folder()} (Ctrl+Shift+N)"
              data-keytip="F"
              onclick={() => void newFolder()}>▣</button
            >
            <button
              type="button"
              aria-label={m.vault_collapse_all()}
              title={m.vault_collapse_all()}
              data-keytip="C"
              onclick={() => {
                expanded.clear();
                saveExpanded();
              }}>⇤</button
            >
            <button
              type="button"
              aria-label={m.vault_refresh()}
              title={m.vault_refresh()}
              data-keytip="U"
              onclick={() => void session.refresh(null)}>↻</button
            >
            <span class="vault-name" title={session.root}
              >{nameOf(session.root.replaceAll("\\", "/"))}</span
            >
            <button
              type="button"
              aria-label={m.vault_choose()}
              title={m.vault_choose()}
              data-keytip="V"
              onclick={() => void chooseVault()}>⋯</button
            >
          </div>
          <FileTree
            {session}
            {expanded}
            bind:selected
            bind:renaming
            active={current}
            onopen={openEntry}
            oncontext={(entry, x, y) => (menu = { x, y, entry })}
            onrename={(entry, name) => void rename(entry, name)}
            onmove={(path, dest) => void move(path, dest)}
            ontrash={(entry) => void trash(entry)}
            ontoggle={toggle}
          />
        </aside>
        <!-- svelte-ignore a11y_no_static_element_interactions -->
        <div class="resize" onpointerdown={resizeStart}></div>
      {/if}
      <PaneGrid
        {session}
        {workspace}
        onoutline={(pane, items, go) => {
          if (pane === workspace.focused) {
            outline = items;
            outlineGo = go;
          }
        }}
        ontonote={(source, text, page) => void toNote(source, text, page)}
        onrename={(path) => startRename(path)}
      />
      {#if rightOpen}
        <RightPanel
          {session}
          {current}
          {outline}
          ongo={(target) => outlineGo(target)}
          onopen={(path) => openPath(path)}
          onjournal={(date) => void journal(date)}
        />
      {/if}
    </div>
    <footer class="hints">
      <span><kbd>Ctrl+O</kbd> {m.vault_quick_open()}</span>
      <span><kbd>Ctrl+M</kbd> {m.vault_move_to()}</span>
      <span><kbd>F2</kbd> {m.vault_rename()}</span>
      <span><kbd>Ctrl+\</kbd> {m.vault_open_beside()}</span>
      <span><kbd>Ctrl+PgDn</kbd> {m.vault_next_tab()}</span>
      <span><kbd>F6</kbd> {m.vault_next_area()}</span>
      <span><kbd>Alt</kbd> {m.vault_key_tips()}</span>
    </footer>
  {/if}

  {#each tips.tips as tip (tip.key)}
    <span class="tip" style:left="{tip.left}px" style:top="{tip.top}px">{tip.key}</span>
  {/each}

  {#if menu}
    {@const entry = menu.entry}
    <ContextMenu
      x={menu.x}
      y={menu.y}
      items={contextItems(entry)}
      onselect={(id) => runContext(id, entry)}
      onclose={() => (menu = null)}
    />
  {/if}

  {#if palette}
    <Palette
      mode={palette.mode}
      placeholder={palette.mode === "open"
        ? m.vault_palette_open()
        : palette.mode === "move"
          ? m.vault_palette_move({ name: displayName(nameOf(palette.target ?? "")) })
          : m.vault_palette_layout()}
      search={paletteSearch}
      onpick={palettePick}
      onclose={() => (palette = null)}
    />
  {/if}

  {#if toast}
    <div class="toast" role="status">
      <span>{toast.text}</span>
      {#if toast.action}
        {@const action = toast.action}
        <button
          type="button"
          onclick={() => {
            toast = null;
            action.run();
          }}>{action.label}</button
        >
      {/if}
    </div>
  {/if}
</div>

<style>
  .vault {
    position: relative;
    display: flex;
    flex-direction: column;
    block-size: 100%;
    min-block-size: 0;
    overflow: hidden;
    border-radius: var(--content-radius);
    background: var(--color-page);
    color: var(--color-text);
  }

  .loading {
    padding: 32px;
    color: var(--color-muted);
  }

  .welcome {
    display: grid;
    place-content: center;
    justify-items: center;
    gap: 12px;
    block-size: 100%;
    padding: 32px;
    text-align: center;
  }

  .welcome h1 {
    margin: 0;
    font-size: 22px;
    font-weight: 600;
  }

  .welcome p {
    max-inline-size: 460px;
    margin: 0 0 8px;
    color: var(--color-muted);
  }

  button {
    block-size: 28px;
    padding-inline: 10px;
    border: 0;
    border-radius: var(--radius-control-compact);
    background: transparent;
    color: var(--color-label-secondary);
    font: inherit;
    font-size: 12.5px;
  }

  button:hover {
    background: var(--color-fill-hover);
    color: var(--color-text);
  }

  button.primary {
    padding-inline: 16px;
    background: var(--color-fill-active);
    color: var(--color-text);
  }

  .bar {
    display: flex;
    flex: none;
    align-items: center;
    gap: 6px;
    block-size: 40px;
    padding-inline: 8px;
    border-block-end: 1px solid var(--color-border);
    background: var(--color-surface);
  }

  .icon {
    inline-size: 28px;
    padding: 0;
    font-size: 15px;
  }

  .icon[aria-pressed="true"] {
    color: var(--color-text);
  }

  .crumbs {
    display: flex;
    flex: 1;
    gap: 6px;
    min-inline-size: 0;
    overflow: hidden;
    color: var(--color-muted);
    font-size: 12.5px;
    white-space: nowrap;
  }

  .crumbs .last {
    color: var(--color-text);
  }

  .sep {
    color: var(--color-faint);
  }

  .layouts {
    display: flex;
    gap: 2px;
  }

  .layout {
    display: grid;
    gap: 2px;
    inline-size: 28px;
    padding: 7px;
  }

  .layout span {
    border-radius: 1.5px;
    background: currentcolor;
    opacity: 0.55;
  }

  .layout[data-layout="single"] {
    grid-template: 1fr / 1fr;
  }

  .layout[data-layout="single"] span:not(:first-child),
  .layout[data-layout="columns"] span:nth-child(n + 3),
  .layout[data-layout="rows"] span:nth-child(n + 3) {
    display: none;
  }

  .layout[data-layout="columns"] {
    grid-template: 1fr / 1fr 1fr;
  }

  .layout[data-layout="rows"] {
    grid-template: 1fr 1fr / 1fr;
  }

  .layout[data-layout="grid"] {
    grid-template: 1fr 1fr / 1fr 1fr;
  }

  .layout.on {
    background: var(--color-fill-active);
    color: var(--color-text);
  }

  .body {
    display: flex;
    flex: 1;
    min-block-size: 0;
  }

  .files {
    display: flex;
    flex: none;
    flex-direction: column;
    min-block-size: 0;
    background: var(--color-surface);
  }

  .files-bar {
    display: flex;
    align-items: center;
    gap: 2px;
    padding: 6px;
  }

  .files-bar button {
    inline-size: 28px;
    padding: 0;
    font-size: 14px;
  }

  .vault-name {
    flex: 1;
    overflow: hidden;
    padding-inline: 6px;
    color: var(--color-muted);
    font-size: 11.5px;
    font-weight: 600;
    letter-spacing: 0.03em;
    text-align: end;
    text-overflow: ellipsis;
    text-transform: uppercase;
    white-space: nowrap;
  }

  .resize {
    flex: none;
    inline-size: 4px;
    margin-inline: -2px;
    z-index: 2;
    cursor: col-resize;
  }

  .resize:hover {
    background: var(--color-border-strong);
  }

  .body > :global(.panel) {
    flex: none;
    inline-size: 260px;
  }

  .hints {
    display: flex;
    flex: none;
    gap: 16px;
    padding: 4px 12px;
    overflow: hidden;
    border-block-start: 1px solid var(--color-border);
    background: var(--color-surface);
    color: var(--color-faint);
    font-size: 11px;
    white-space: nowrap;
  }

  kbd {
    color: var(--color-muted);
    font: inherit;
    font-weight: 600;
  }

  .tip {
    position: absolute;
    z-index: 60;
    min-inline-size: 16px;
    padding: 1px 4px;
    border-radius: 4px;
    background: var(--color-text);
    color: var(--color-page);
    font-size: 10.5px;
    font-weight: 700;
    line-height: 14px;
    text-align: center;
    pointer-events: none;
  }

  .toast {
    position: absolute;
    inset-block-end: 40px;
    inset-inline-start: 50%;
    z-index: 55;
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 8px 8px 8px 14px;
    border: 1px solid var(--color-border);
    border-radius: var(--radius-panel);
    background: var(--color-menu);
    box-shadow: var(--shadow-float);
    font-size: 12.5px;
    transform: translateX(-50%);
  }

  .toast button {
    background: var(--color-fill-active);
    color: var(--color-text);
  }
</style>
