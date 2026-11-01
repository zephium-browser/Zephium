# Vault workspace: Zephium as an Obsidian replacement

Status: approved direction (owner delegated detailed decisions), 2026-11-01.

## Intent

The owner keeps coursework in an Obsidian vault on OneDrive (`DBKF/Year 2/Sem 2/ITF/CA1/...`)
full of PDFs, Office files and Markdown notes. Obsidian is used as a file viewer and
note-taker but cannot manage files, is slow as a browser and has no web workspaces.

Success: the owner never opens Windows File Explorer or Obsidian for this workflow. A
downloaded PDF lands in the vault, gets filed with the keyboard, opens as a tab beside
other files and websites, and can be annotated. Office files are viewed in-app and
edited in desktop Office. Windows is the primary target; macOS must keep compiling.

Out of scope: in-app Office editing, macro execution, deadlines parsed from file
names, graph view, canvases, bases, Claude subscription sign-in.

## Shape

```
+-----------+-------------------------------------------+-------------+
| file tree |  tabs: [Brief.pdf] [Notes.md] [Marks.xlsx] |  outline /  |
| (vault)   |  +-------------------+------------------+  |  backlinks  |
|           |  |  pane 1           |  pane 2          |  |             |
|           |  +-------------------+------------------+  |             |
+-----------+-------------------------------------------+-------------+
```

* **Vault page**: a new browser page (`BrowserPage::Vault`, command `browser.vault`),
  rendered in the chrome WebView like Notes/Work. Obsidian-style three columns: tree,
  file panes, right info panel. Opened from the sidebar and `Ctrl+Shift+E`.
* **Panes**: an in-page grid of 1-4 panes, each with its own tab strip of open files.
  Web pages are native content WebViews and cannot render inside this DOM grid;
  "file next to a website" is delivered later by docking a single file viewer in a
  widened sidebar while browsing (phase 6), never by weakening the engine's
  `file://` and custom-scheme refusal for content tabs.

## Units

### `crates/zephium-vault` (new, pure Rust, `#![forbid(unsafe_code)]`)
Owns every operation on the vault folder. Paths across the boundary are
vault-relative with `/` separators. Each component is validated: no empty, `.`,
`..`, separators, NUL or Windows-reserved names; no hidden (dot) components except
read-only listing is suppressed for them; nothing follows a symlink/junction out of
the root (directories are checked with `symlink_metadata` component by component).

* `Vault::open(root)`; `list(dir) -> Vec<Entry{name,path,kind,size,modified,ext}>`
  (folders first, natural sort); `walk(limit)` for search indexing.
* `create_folder`, `create_file(dir, name, initial_bytes)`, `rename(path, new_name)`,
  `move_to(path, dest_dir)`, `trash(path)` (moves into `<root>/.trash/`, the folder
  Obsidian and Zephium notes already use, never permanent delete), `import(external,
  dest_dir)` (copy a file in, used for downloads and drag-in), `read(path, max)`,
  `write(path, bytes, expected_modified)` (atomic temp+rename, refuses if the file
  changed since read; used by PDF annotation save).
* `search_names(query, limit)` fuzzy subsequence match over a cached walk.
* Name collisions get ` 1`, ` 2` suffixes; Windows-invalid characters are refused.

### Desktop shell (`desktop/src/vault.rs`)
* Root per profile stored in `<data>/vault/<profile>.root` (UTF-8 path). Deleting a
  profile removes this pointer only, never the vault.
* Commands (specta, `authorize(.., CallerPolicy::Main)`): `vault_root`,
  `vault_choose_root` (folder dialog), `vault_call(expected_profile, VaultCall) ->
  VaultReply`, `vault_open_native(path)` (default app via `opener`), `vault_reveal`.
* Scheme `zephium-vault://<profile>/<path>` serves file bytes to the main chrome
  WebView only (same guard as `zephium-media`); CSP gains it in `connect-src` and
  `img-src`. Writes go through `vault_call` with base64 bytes.
* A `notify` watcher emits `vault-changed {profile, dirs}` so OneDrive and Explorer
  changes appear.
* Notes: when a vault root is set, the notes library opens the vault root instead of
  `notes/<profile>/Notes`; its index stays in the data directory. Every `.md` in the
  vault is then a note with the existing editor, autosave, conflict handling and
  full-text index.

### Frontend `frame/src/features/vault` (lazy page)
* `VaultPage` (three columns, resizable), `FileTree` (lazy folders, keyboard:
  arrows/Enter/F2/Delete/Ctrl+N/Ctrl+Shift+N, drag-drop move, native context menu),
  `PaneGrid` + `PaneTabs`, `FileToPicker` (fuzzy folder picker, `Ctrl+M`),
  `QuickSwitcher` (`Ctrl+O`), viewers loaded per type:
  * `.md` -> existing notes editor (resolved by path to note id).
  * `.pdf` -> `pdfjs-dist` viewer with annotation editor (highlight, ink, free text),
    saved back into the PDF; "copy highlight to note".
  * `.docx` -> `docx-preview` read-only; `.xlsx/.xlsm/.csv` -> SheetJS read-only grid;
    both with "Open in Word/Excel".
  * images, plain text -> inline; everything else -> "Open with default app".
* Right panel: outline (Markdown headings / PDF outline), backlinks (`[[name]]`
  references found through the notes index).
* Journal: `Ctrl+Shift+J` opens/creates `Journal/YYYY-MM-DD.md`; mini calendar in the
  right panel.

### Launcher
Adds vault file-name results (`SearchAction::OpenVaultFile`) beside notes, tabs and
history. PDF text search: extracted text is cached in the vault index (phase 4).

### Keyboard (Excel-inspired)
`F2` rename (tree, tab title), `Ctrl+PgUp/PgDn` cycle tabs in a pane (already global
for browser tabs), `Alt` shows key-tip badges over the vault page's controls; letters
then activate them.

### Downloads
Setting "Save downloads into vault inbox" (`<vault>/Inbox`). Inbox items show a "File
to..." action.

### Folder <-> space link, layouts, clipper
* A folder can be linked to a space: opening one offers the other.
* Saved layouts: pane grid + open files persisted per name in frontend preference
  storage via a vault-root `.zephium/layouts.json` (hidden, ignored by Obsidian).
* Web clipper: command "Clip page to vault" saves title, URL and selection as Markdown
  into the current vault folder.

## Errors and safety
All vault failures return typed `VaultError {Invalid, NotFound, Exists, Changed,
TooLarge, NoRoot, Io}` shown inline with retry. The frame never claims success before
the reply. Trash, not delete. No path outside the root is ever read, written or
served; tests cover traversal, symlink escape, reserved names and collisions.

## Testing
Unit tests in `zephium-vault` (temp dirs), shell command authorization tests, Vitest
for tree/pane/picker models. Native visual QA happens on the owner's Windows PC.

## Phases
1. Vault crate, commands, scheme, notes-on-vault, Vault page with tree and file ops,
   File-to, open native.
2. Pane grid + viewers (md, pdf, docx, xlsx, image, text) + saved layouts.
3. PDF annotation save + highlight to note.
4. Launcher file names + PDF text + quick switcher.
5. Outline, backlinks, wikilinks, journal/calendar.
6. Downloads to inbox, folder<->space link, clipper, sidebar file dock, F2/Alt key tips.
