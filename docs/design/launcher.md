# Launcher

The existing `panel` window is the single floating host, and it hosts search
only. It adds no window or WebView. Search is backed by the browser actor;
Notes, Tasks, History and Downloads are destinations the launcher hands to the
browser, which already has their views loaded.

## Why the launcher hosts no tools

The panel is its own WebContent process on macOS and its own WebView2 tree on
Windows. Nothing parsed or allocated in one is shared with the other, so a tool
opened in both is paid twice for as long as both stay alive — for Notes that is
the ~430 KB Tiptap editor and ProseMirror state, for Tasks a second copy of the
task lists and their subscriptions. Earlier revisions rendered full tool views
in the panel (`tools.presentation = floating`); that route, its preference and
the floating tool host are removed.

The launcher offers the same things without the cost: notes are search results
with the line that matched, anything typed can be captured as a task, and the
destinations open the browser's own page for them. The build enforces it:
`bootstrap-report.ts` fails if Tiptap, ProseMirror, or any notes, tools, work,
history or downloads module is reachable from the panel entry, statically or
through a dynamic import. Native no longer sends task or note change events to
the panel.

## Ownership and routing

`desktop/src/overlay.rs` owns presentation, focus, placement and operation
settlement; `overlay/model.rs` separates publication revision from search
session identity. Rust publishes `PanelState`; the frontend sends `PanelIntent`
(`search`, `dismiss`, `open { tool }`) and reports what it shows, as a height and
the rectangles of its field and sheet, through `panel_layout`. It never positions
the window itself.

The shortcut toggles the launcher. It goes away when focus moves to the browser
or another application; an owned native surface such as a menu keeps Zephium
active and does not dismiss it. On macOS the launcher is made key and main
before activating, so activation brings the launcher forward rather than a
browser window behind the user's current app, and an explicit dismissal hands
activation back to that app.

Every accepted open hides the launcher and raises the browser, un-minimizing it
first (Tao's `set_focus` ignores a miniaturized window). `launcher_run` takes a
`background` flag, honoured only for an address: the actor inserts a tab behind
the current one, keeps the search alive so further rows still admit, and the
launcher stays open. The caller policy, operation ledger and scoped admission
are unchanged.

## Opening it

`desktop/src/launcher_trigger.rs` owns what opens the launcher. The default
is ⌘⇧Space (Ctrl+Shift+Space elsewhere). Every quick combination is claimed
by something — ⌘Space by Spotlight, ⌥Space by Raycast, ChatGPT and Claude,
the other Space chords by input sources, emoji and Finder — so the shortcut
is recorded in Settings → Keyboard rather than guessed. A recorded shortcut is
refused before the working one is touched when it needs a real modifier, is
the system's own, or is an app command such as ⌘⇧Z (Redo): a global shortcut
is taken before any app sees it, so that would break Redo everywhere. If
registration fails, the previous shortcut is put back and the page says the
combination is taken; a shortcut that is not active is shown as such instead
of only being logged. The current shortcut is silenced while recording, so it
can be recorded again.

On macOS, double-tapping ⌘ or ⌥ can open it too. It collides with no
combination but needs Accessibility permission to see another app's modifier
changes, so it is opt-in: enabling it shows the system prompt, and the
observers are installed only once permission is granted, which the settings
page notices by asking again while it waits. A tap is a lone press released
within 250 ms and the second must follow within 300 ms; any key or other
modifier in between starts over, so ⌘C then ⌘V never counts.

## Loading and presentation

The panel is created on first use on Windows and macOS. It loads the launcher
while hidden, before `panel_ready`, and keeps it mounted for fast reopening.
After hiding and draining pending captures, Windows asks WebView2 to suspend;
macOS detaches the hidden WKWebView from its parent and allows background
suspension. Reopening resumes the renderer before publishing the new session. Each presentation binds a new search session; putting it away disposes
the controller and native work. What was typed is kept for 30 seconds so a
launcher reopened a moment later resumes, and the reset to the home list happens
while hidden, so the window is never resized on the frame it appears. The new
session's controller starts from the rows already on screen, so the list is
never emptied while its first answer is on the way. A fulfilled action or
capture resets at once.

The page keeps the icon rasters it was sent between presentations; native
sends them again only after the page has loaded afresh, which the page reports
when it is ready.

## Geometry and materials

The launcher is two objects: a 60-point field capsule and a result sheet 10
points below it, both 680 points wide. Where the platform has a material,
`desktop/src/panel/shapes.rs` draws them natively behind the WebView — Liquid
Glass shapes inside an `NSGlassEffectContainerView`, or two vibrancy views on
older macOS — at the rectangles the content reports through `panel_layout`.
The container merges shapes closer than 8 points, so a sheet appearing grows
out of the capsule and separates from it. Shape changes animate on the
compositor over 260 ms, and the page moves its sheet over the same interval
and curve. The window is transparent around the shapes, 40 points on each
side — the reach of the glass's own shadow, which a narrower margin clipped
into a visible rectangle — and has no shadow of its own. A click in that
margin, or between the two shapes, dismisses the launcher as a click outside
it would.

Everywhere else — Reduce Transparency, Windows, Linux — the page draws one
card with the same contents and the window supplies the shadow. On Windows
the card sits on Acrylic, and DWM draws its corners and rim at the system's
8-point radius, which the card adopts; rows and their highlight take the
system's 4-point list radius inside it. Two separate glass objects would need
two windows, and so two WebView2 instances, for a look; they are not worth
that memory.

On Windows, the WebView retains a bounded maximum-height viewport while the
native window clips to the measured content height. A single CSS wash covers
that viewport; DWM owns the visible rounded edge. Reports coalesce until the
content has painted, then commit one native size change. There is no independent
native height animation or resize timer to drift behind the content. Hidden
layout reports do not depend on animation frames, which suspension may pause.
This avoids repeated WebView surface resizing while keeping short lists compact.
Long lists scroll within the maximum height without visible scrollbars.

The window keeps a fixed width and a height that follows the content, between
the field alone and 600 points, bounded by the display: the work area less 24
points above and below, which the page reads from the display's usable height
so its footer is never cut off on a short screen. With macOS shapes it grows at
once and shrinks only after the shapes have settled, so nothing is cut off
mid-motion; the shapes sit on a canvas pinned to the window's top edge, so a
resize never moves them. The window is made its final width, shapes included,
before the page first measures it, so the glass does not glide in from the
corner on the first arrival. It opens on the display under the pointer with its
top edge at a fifth of the work area. On macOS the pointer, the displays and
the window's frame are all read and set in AppKit points, so displays of
different scale are never converted between; on Windows the window moves to
the display first and is then sized in that display's own units. It is not
resizable or draggable, so no geometry is persisted. AppKit's utility-window animation fades it in and out, and the
shapes are reinstalled when the appearance changes.

## Verification record — 2026-09-11

- Frame typecheck, lint, formatting, 97 tests, and production build pass.
- App tests: 325 pass, including queued search-action admission.
- Desktop suite: 101 pass and 2 ignored before two additional owner/focus tests;
  the additional overlay tests are checked separately.
- Native Clippy across desktop targets passes with warnings denied.
- `frame/reports/bootstrap-report.json` traverses emitted static imports, including
  shared chunks. The build rejects eager browser domains, Settings, sidebar,
  or tool-view code in the panel graph. Current panel graph is approximately
  127 KB JS and 17 KB CSS (uncompressed); the main graph is approximately
  283 KB JS and 84 KB CSS. These are emitted-asset counts, not runtime memory.
- The original pre-split asset snapshot counted only its entry JS and is not
  comparable to this full static-graph count; no JS reduction percentage is
  claimed.

### Native acceptance still required

Computer Use could not inspect Zephium: macOS ScreenCaptureKit failed to start
capture (error -3811). Therefore the following are **not verified** by this
implementation pass:

- Actual keyboard/pointer and IME operation, focus restoration, import-failure
  recovery, light/dark, reduced transparency/motion, and accessible focus.
- Native radius/clipping, drag/resize, owned menu/dialog transitions, app
  activation, monitor changes, and geometry restoration after a restart.
- Windows and Linux behavior, including Wayland compositor and portal dialogs.
- Runtime WebView count, idle CPU, RSS, and warm-cache open/close-cycle trends.

For resource qualification, record the process tree, WebView count, CPU and RSS
at hidden cold start, visible search, each tool's first open, then hidden after
at least 20 repeated cycles with caches warm. Verify no continuing feature
activity while hidden; compare stable settled measurements rather than assuming
cached code will be unloaded. The structural single-window implementation and
passing source tests do not replace that live evidence. This is a review build,
not a declaration of production readiness.

### Stylesheet-loading correction

A subsequent native screenshot exposed unstyled launcher content despite the
static module-graph check passing. Vite combined a conditional dynamic import
into one preload wrapper containing only the main surface's CSS dependencies.
The router now has separate awaited imports and mount branches. The build guard
also checks final stylesheet preload references in `writeBundle`, after Vite
resolves them. Rebuilding with the original conditional import was verified to
fail this new guard; the corrected import branches pass. Native appearance
following this fix still requires review of the restarted build.

## Search presentation refinement — 2026-09-16

New Tab keeps the original `SearchField` page capsule. Its shared search view
opens an anchored dropdown only while an entered query is active; it has no
launcher drag header, home tool grid, or footer. Escape dismisses the dropdown
without discarding the query. Focus leaving the control cancels the scoped native
request and pauses its UI deadline. Dropdown height is bounded by the available
viewport space and does not move the New Tab layout.

The launcher and New Tab share compact 34 px result rows, grouped by source.
Searches use a magnifier, history a clock, and notes a note icon; source names
appear once as group headings. Keyboard selection follows action identity through
incremental updates. Retained rows cannot execute while a replacement query is
pending. Empty intermediate responses retain the previous visual results; an empty
state is admitted only after native history and supplementary providers finish.
The native `SearchResults.pending` field carries this state; it is not guessed
from an arbitrary UI delay. Obsolete provider completion cannot settle a newer
query. The address bar remains a plain URL/search submission field.

Browser component tests now run the same Tailwind compiler as the production
build, so semantic token definitions and utility classes are present during visual
checks. Light/dark New Tab screenshots are emitted beneath `target/`. These tests
verify WebKit component behavior, not native window focus/material or cross-platform
qualification. Native app access was not granted by Computer Use during this work.

Search and New Tab remain lazy. The search lazy graph includes its shared parent
chunks; its budget measures that complete graph rather than an incremental download.
Existing browser/panel startup limits remain unchanged. No CSS blur, result-list
geometry animation, or new polling has been introduced.

## Search production pass — 2026-09-16

The shared view moved to `frame/src/features/search/`: `SearchSurface` behaviour
lives in `lib/search-surface.svelte.ts`, presentation in `ResultList` and
`ResultRow`, and `LauncherPanel` and `NewTabSearch` are thin hosts that own only
their own chrome. `features/launcher/` and its global `.panel-*` result
stylesheet are gone; result styling is component-owned and token-based.

### Ordering

`zephium_core::search::ResultSection` gives every result a fixed presentation
order (Search, Tabs, History, Notes, Commands) with a per-section capacity.
`publish_search` stable-sorts by section only, because each result is already
pushed in its section's own preference order. A late note or online suggestion
therefore extends its own run instead of re-ranking rows the user is reading.
The earlier flat score re-sorted and re-truncated the whole merged list on every
provider arrival, and truncated again before the merge, so the settled order
depended on provider timing.

What the user typed is always the first row and is the action Enter runs. A
prefix-matching tab title previously scored above the search row, so typing two
characters could switch tabs when a search was intended.

### Provider lifecycle

`desktop/src/search_providers.rs` keys in-flight work by search session, so a
New Tab field, the floating launcher and a second window no longer abort each
other. A `Completion` guard dispatches `SearchSupplementaryFinished` on every
exit path including `abort()`. Previously an early return left
`supplementary_pending` set permanently, and because the New Tab list only opens
once results exist or pending clears, a query with no local match never opened
its list at all. Input that cannot produce supplementary results settles without
spawning a task.

### Storage

`search_history` bounds its scan by recency and then groups, instead of capping
FTS candidates before the group-by. `history` holds one row per visit, so the
previous 256-row candidate cap was consumed by whichever address was visited
most, hiding every other match. Ranking is now frecency (visit count weighted by
recency bucket) rather than `last DESC`. No `history_fts` prefix-index migration
was added: rebuilding that index on an existing profile is a blocking startup
cost that the bounded scan makes unnecessary.

### Measured

Native search requests for typing "rust language" (13 characters), driving the
real controller under fake timers:

| Keystroke interval | Before | After |
| ------------------ | ------ | ----- |
| 30–50 ms           | 13     | 2     |
| 60 ms and slower   | 13     | 13    |

The previous scheduling was `setTimeout(…, 0)`, which is one request per
character at any speed. The 60 ms coalescing window therefore bounds the request
_rate_ at roughly 16/s rather than removing per-keystroke requests for ordinary
typing; deliberate typing still queries per keystroke, as shipping browsers do.
Expensive supplementary work is bounded separately: the note index and the
network are reached only after a 140 ms quiet window, so a burst aborts the task
before it queries either.

Emitted graphs: `LauncherPanel` 118,213 B JS / 5,848 B CSS, `NewTabSearch`
117,424 B JS / 5,727 B CSS. No existing budget was raised. The New Tab clear
control is placed by its own host rather than threaded through the shared
`SearchField`, because widening that primitive pushed `PrivacyPage` past its
budget for one caller's benefit.

### Field behaviour pass

New Tab offers places to go, not actions: browser commands are excluded when the
bound session is a New Tab one and stay in the launcher, which is the surface
built for them. Notes remain, as a destination rather than a command.

Enter follows what the field is showing. Once an inline completion has been
applied the field reads `notion.so`, so that is the row selected and the row
Enter opens; searching the fragment that was typed is the wrong answer.
`completionTarget` resolves the offered host back to the row that supplied it,
accepting either spelling native uses for a host — a tab carries its bare host,
a visit its full address.

Emphasis is measured against `SearchSnapshot.answered`, the query the visible
rows actually answer, rather than the live text. Measuring against the live text
made a row's bold match drop out for the moment between a keystroke and its
answer, which read as the list lagging behind the typing.

The provider windows were split. One shared window meant every suggestion also
waited out the note-index window, so the fixed delay before leaving the machine
was 180 ms; it is now 60 ms, while the note index keeps a 130 ms window and the
resource-bounding property that comes with it. The remaining wait is the round
trip itself.

The list animates its own height to its content, measured on the border box so
the padding is included — a content-box measurement leaves a permanent
scrollbar on a list that fits. There is deliberately no per-row entry animation:
the typed row's action carries the query, so its identity changes on every
keystroke and a fade would re-run as fast as the user types, reading as exactly
the lag it was meant to soften.

### History crowding

Recorded searches and visited pages both live in the History section. Ranked
together under one shared cap of three, a browser that has been used for a
while filled every slot with its own past queries and stopped offering pages
at all — which reads as history search being broken, on both surfaces, getting
worse the more the browser is used.

Capacity is now counted per kind (`kind_capacity`) rather than per section: a
section can mix sources worth very different amounts, and the cheaper one must
not be able to spend the whole budget. A past query is not a destination and
retyping it is cheap, so it gets one slot; visited pages get four. The reader
also asks for ten candidates rather than six, so dedup against already-open
tabs cannot leave the section short.

`history_reaches_the_field_through_the_real_store_and_read_queue` covers the
seam this hid behind: a real profile database, the real read queue and the real
worker loop, driven by the scoped command the shipping surfaces send. Each piece
had coverage; the path between them had none.

### Bookmarks

Saved pages are offered in the History section, marked with a star. The history
read also asks the store for bookmarks whose title or address contains every
word typed (`BookmarkRequest::Search`), so the launcher keeps no second copy of
the tree to go stale; it runs on the same read worker, in the same request.
Each bookmark and each visit is scored with `match_quality`, and
`interleave_bookmarks` puts a bookmark ahead of the first visit it matches at
least as well. Visits keep their frecency order. Bookmarks draw on the visited
pages' allowance of four (`capacity_group`), so the list never gets longer, and
an address that is both saved and visited shows once, as the bookmark. They are
offered with visited pages turned off, and not in an `@history` search or a
private profile.

### Completion and deletion

Deletion is judged against what the field is showing, not against the typed
text. With a completion applied the field reads `youtube.com` while the typed
text is `you`; deleting the completion leaves the typed text the same length,
so a length comparison against it saw no change, allowed the completion to be
re-applied, and backspace did nothing however many times it was pressed.

The field stays bound to the typed text. Binding it to the displayed text was
tried and reverted: Svelte writes `input.value` whenever its tracked value
differs from the prop, and that write collapses the selection that makes the
next keystroke replace the offer.

### Not verified

- No native app run. Light and dark checks are WebKit component screenshots
  under `target/search-newtab-*.png` and `target/search-launcher-*.png`, not
  native window, material, focus or IME behaviour.
- No cross-platform qualification, process-memory benchmark, or end-to-end
  latency measurement. The table above counts requests, not latency.
- Clippy still reports the pre-existing `ResourceCall` `large_enum_variant` and
  `api.rs` `type_complexity` warnings. The repository release gate is not clean.

## Calculator — 2026-09-28

Arithmetic typed into the field is answered on the page as it is typed, in a
result card above the search row, and Enter copies the answer. It is a small
recursive-descent parser (`features/search/lib/calculator.ts`): no `eval`,
which the CSP forbids, no dependency and no native round trip. It reads
precedence, grouping, powers (tighter than a leading sign), implicit
multiplication ("2pi", "3(4+5)"), decimal commas, common functions and
constants, and percentages as people mean them: "80 + 15%" is 92. A bare
number, a word or anything that does not compute is left to search, and New
Tab never asks.

It adds about 4 KB to the launcher graph, whose JS limit went from 170 KB to
175 KB for it. Its glyph is defined in the search feature rather than imported:
every icon taken from the icon package lands in one shared chunk, and the note
editor's and Notes view's graphs, each within a kilobyte of their limits,
count that chunk.

## Arrival — 2026-09-28

The window fades in over 180 ms, so the field takes typing on its first frame,
while the glass carries the motion: the capsule grows in from 96% and the
sheet flows out of it over 420 ms on a curve that overshoots by a hair. The
page reveals the list from the top at the same pace and lets its rows drift
8 points into place 60 ms behind. Leaving is a 120 ms fade. AppKit's own
window animation is off so the two never stack. Reduce Motion — the system's
or Zephium's — leaves only a short fade.

## Measured — 2026-09-28

Development build (Vite dev server, unminified modules, so both figures are
above production), launcher hidden and never opened, measured with
`footprint` and `top` against the content processes the debug build logs by
label:

| Process                | Footprint | CPU      | Idle wake-ups over 10 s |
| ---------------------- | --------- | -------- | ----------------------- |
| Launcher WebView       | 47 MB     | 0.0%     | 0                       |
| Browser chrome WebView | 92 MB     | 0.0–1.2% | 4                       |

The launcher's material costs nothing while hidden: the window is ordered
out, and the glass is composited by the window server only while visible.
The hidden WebView receives no task, note, resource or download events, and
only the UI commands it acts on — appearance, language and Reduce Motion. Media
and download changes were still sent to it when this was measured; they now go
to the browser alone. Open and repeated open/close cycles, and a release build,
remain to be measured.

## Glass redesign — 2026-09-28

Rows are 44 points, carry a bare glyph and name their kind at the far edge
("Tab", "History", "Note"), so the list has no section headings. One
highlight is drawn by the list and moves between rows with a transform. At
rest the sheet shows up to three recent tabs and the destinations as chips;
`⌘1`–`⌘4` open them from anywhere, and holding `⌘` shows the numbers. The
action capsule floats at the sheet's bottom edge. Every modifier in a command
accelerator is translated, so `Ctrl+Shift+Tab` no longer renders
half-converted on a Mac.

Some queries, such as "h", took the whole launcher down to its render-error
state. Groups of adjacent rows were keyed by section, and a section can recur:
typed commands, then a destination, then the capture row, which is a command
again. The duplicate key threw inside Svelte. Groups are now keyed by position,
with a regression test for that exact sequence.

A refused or unanswered search is retried once after 150 ms before the
launcher says anything, and then it says the search is not responding rather
than that an action failed. `launcher_search` records why it refused a
request. A new presentation clears any action still awaiting settlement: a
disposition that never reached the launcher previously refused every later
action.

The launcher graph is 165 KB JS and 12.2 KB CSS; its CSS limit was raised from
12 KB to 14 KB for the capsule, the destination chips and the action capsule.
The panel entry is unchanged at 96 KB.

## Redesign — 2026-09-25

Field, list and action bar, with nothing else: the drag bar, the fixed
720 × 520 sheet and the hint footer are gone. The action bar names what Enter
does to the selected row and opens an actions sheet (`⌘K`): Open, Open in
Background (`⌘↵`, also `⌘`-click), Copy Link (`⇧⌘C`) and Add to Tasks (`⌥↵`).
Escape clears the field, then dismisses. Rows are 40 points with a plated glyph
and the detail following the title; note rows show their matching line, which
native now sends without a placeholder and reads at most three of.

Budgets: panel entry 96 KB JS (was ~137 KB), launcher graph 160 KB. The panel
and launcher limits were lowered to hold that. DownloadsList, LibraryPage,
DownloadsView and HistoryView limits were raised: with the panel no longer
sharing `$domain/downloads` and the downloads/history loaders, Rolldown hoists
them into the browser entry, so those graphs now count the whole entry. The
bytes the browser loads for them are unchanged.

### Not verified

- Windows was reviewed by reading only: the desktop crate cannot be
  type-checked for Windows from macOS, because a crypto dependency needs the
  Windows SDK's C headers. The card was checked in a WebKit render.
- No native interaction was qualified: activation from another application,
  return of activation on dismissal, content-height resizing, utility-window
  fade, placement under the pointer, and background opens need a hands-on run.
- No process-memory measurement yet. The structural change removes the second
  editor and task session from the panel; RSS for hidden, first-open and after
  20 cycles should be recorded before and after.
- Windows still runs a separate WebView2 user-data folder, and so a separate
  browser-process tree, for the panel.
