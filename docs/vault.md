# Vault (draft)

Zephium's Vault mode is an Obsidian-style workspace over a folder you own, such
as your OneDrive coursework folder. Design: `docs/superpowers/specs/2026-11-01-vault-workspace-design.md`.

## Open it

Switch the sidebar's mode control to **Vault**, or press **Ctrl+Shift+X**. The
first time, choose your vault folder (for example your existing Obsidian vault
in OneDrive). Zephium never moves anything out of it; `.obsidian` and `.trash`
are hidden and left alone, so Obsidian keeps working on the same folder.

## What is in the draft

| Area | What works |
|---|---|
| File tree | Folders first, natural order, type badges, drag to move, right-click menu, new note/folder, rename, delete to the vault `.trash` with Undo, live refresh when OneDrive or Explorer changes files |
| Panes | One, two side by side, two stacked or four panes; each has its own tabs; named layouts you can save and restore |
| Markdown | Every `.md` file in the vault is a note in Zephium's notes editor (autosave, conflict handling, `[[links]]`, full-text search in the launcher) |
| PDF | PDF.js viewer: page jump, zoom, fit width, find, **highlight / draw / text annotations saved back into the PDF**, "Add to note" quotes the selection into `<pdf name> notes.md` |
| Word / Excel | Read-only preview of `.docx`, `.xlsx`, `.xlsm`, `.csv`, with **Edit in Word/Excel** (macros only run in desktop Excel) |
| Other files | Images and text inline; anything else opens in its default app |
| Downloads | **Downloads** shelf under the tree lists recent files in your Downloads folder; click one and type a folder name to file it. Or set Settings → Downloads → Choose folder to `<vault>/Inbox` |
| Right panel | Outline (Markdown headings, PDF bookmarks), backlinks, calendar that opens `Journal/YYYY-MM-DD.md` |

## Keys

| Key | Action |
|---|---|
| Ctrl+Shift+X | Open the Vault |
| Ctrl+O | Quick open any file or folder (Ctrl+Enter opens beside) |
| Ctrl+M | Move the selected file to a folder ("File to…") |
| Ctrl+N / Ctrl+Shift+N | New note / new folder in the selected folder |
| F2 | Rename (tree selection, or the file in front) |
| Delete | Move to vault trash (Undo in the message) |
| Enter / Ctrl+Enter | Open / open beside |
| Ctrl+\ | Open the current file beside |
| Ctrl+PageDown / PageUp | Next / previous tab in the pane |
| Ctrl+W | Close the tab |
| F6 | Move between the tree and the panes |
| Ctrl+Shift+L | Layouts |
| Alt | Show key tips; press a letter to use a control |
| In a PDF: Ctrl+S, Ctrl+F, Ctrl+wheel | Save annotations, find, zoom |

## Not done yet

- Vault files in the global launcher (Ctrl+Shift+Space) and full-text search inside PDFs.
- A file docked beside a **website** (needs a sidebar file pane; web pages are native views that the Vault page's panes cannot hold).
- Linking a course folder to a website space, web clipper.
- Layouts and open tabs are kept in the chrome's local storage, not yet in the Rust profile store.
- `frame/src/shared/ipc/bindings.ts` vault entries were written by hand to match specta's output because the desktop crate does not build on Linux; CI's "Bindings drift" job (macOS) regenerates and checks them.

## Build it on Windows

Prerequisites are in `CONTRIBUTING.md` (Rust from `rust-toolchain.toml`, Node 24, pnpm via Corepack,
Microsoft C++ Build Tools, WebView2). Then, in this branch:

```
pnpm install --frozen-lockfile
pnpm dev
```
