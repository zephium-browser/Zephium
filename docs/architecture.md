# Zephium Architecture

FOSS browser on the OS-native webview (Tauri + Wry), Rust-heavy, with a Svelte 5
UI and no application-owned telemetry. This document is the single source of
truth for structure and boundaries. Revised 2026-07 during the foundation
review; it reflects both what is built and the agreed target. Security
enforcement lives in
`docs/security-model.md`.

Rigor is spent only where a decision is expensive to reverse; everything else
stays flexible.

---

## 0. North star and constraints

**Power, polish, and customization without shipping another browser engine.**
The distribution target is roughly 15–20 MiB, but size never overrides native
safety, data durability, or release authenticity. A dependency must justify
both its shipped bytes and its runtime behavior; large engine/runtime payloads
are not bundled merely to meet a feature deadline.

The constraints that remain hard:

1. **Runtime cost**: baseline memory, CPU, startup latency. The browser must
   feel weightless and instant on weak machines. Judge every addition by hot-path
   and baseline-memory cost, not by disk: a Rust crate is ~0 at runtime (add
   freely); an extra live webview is real RAM (count it).
2. **Security** (priority #1, see security-model.md).

Mental model: Arc (and Zen) for product shape, Vivaldi for customization depth,
plus our own: Raycast-style global launcher, notes/tasks/easel, site-as-app
windows, local data visualization. Feature surface is intentionally large; the
architecture below exists so features land as "just work" on top, not as
rewrites.

---

## 1. Architecture style (named honestly)

A composite of independent patterns, each applied where it pays:

1. **Functional core, imperative shell (FCIS).** The domain is pure data + logic
   with zero I/O. All side effects live at the edges.
2. **Single source of truth + unidirectional flow.** Authoritative state lives in
   the Rust core; every UI surface (chrome, overlays, internal pages) is a
   projection that sends intents. Multi-window and session restore fall out for
   free.
3. **Command/event core with domain slices.** Core state is composed of domain
   slices (see §6), each with its own commands, reducer and events. One actor
   loop in `zephium-app` owns the composed state: commands in (queued messages),
   pure reduction, effects out through ports, events out to projections. Adding a
   feature = adding a domain slice; existing domains are not touched.
   Cross-domain operations (close tab -> Items + Splits + Session) compose
   reducers inside one command, atomically, on the single owner thread.
4. **Ports and adapters, applied pointwise and lazily.** Traits abstract I/O
   edges where a port earns its keep (§4).
5. **Event sourcing: deferred.** One cheap discipline is kept: every mutation to
   a persistable aggregate goes through its reducer (no out-of-band writes). That
   chokepoint is where a change-log for sync attaches later, without a rewrite.

The expensive decisions are 1-3 (purity, ownership, command/event shape).
4-5 evolve with the code.

---

## 2. Process and trust model

The native webview delegates renderer parsing/JIT and process sandboxing to the
maintained OS engine: untrusted web content normally runs in engine-owned
content processes we did not build. Zephium still has to prove each supported
runtime's confinement and keep page data out of its Rust/native capabilities;
using a multi-process WebView is not itself a site-isolation guarantee. Rust
requests page-world scripts for tightly bounded observations, but never treats
their results as trusted code or data.

The current three webview classes map to trust zones 1-3 in
security-model.md:

```
+- OS-sandboxed content processes (UNTRUSTED, zone 3) ----------------+
|   raw wry child webviews. web pages. ZERO IPC bridge by construction |
+---------------^------------------------------------------------------+
                | FFI (engine adapter, the only unsafe surface)
+---------------+------------------------------------------------------+
| Main process (TRUSTED, zone 1) - Rust core                           |
|   core domains . actor shell . ports . adapters . composition root   |
+---------------^------------------------------------------------------+
                | typed IPC (specta-generated) / typed overlay bridge
+---------------+------------------------------------------------------+
| Our UI webviews (SEMI-TRUSTED, zone 2)                               |
|   chrome (Tauri main webview) . overlay surfaces . internal pages    |
|   local bundled assets ONLY; no authoritative state                  |
+----------------------------------------------------------------------+
```

Attack surface we own: the IPC/bridge boundary, parsers of semi-trusted input
(filter lists, URLs, themes, config, extension manifests), FFI to the webview,
and any purpose-built application downloaders added for trusted browser data
such as filter lists or signed updates.

Extension execution does not inherit the application-WebView trust zone. Each
installed extension has one non-reusable native principal. Content scripts may
execute only in engine-provided isolated worlds scoped to that principal.
Extension-owned background and UI surfaces form a separate unprivileged trust
zone: they receive no generic Tauri/Wry IPC bridge and no generic custom
protocol. Any enabled native registration must bind the profile, install,
principal, and generation; JavaScript payloads can never select or assert that
identity. Only capability-specific, permission-checked brokers may cross into
the trusted process. Raw page worlds remain bridge-free. This contract is
ratified for the extension architecture, but ordinary product builds keep
extension execution disabled until every platform adapter and hostile native
gate enforces it.

v1 is a single main process with async tasks; the untrusted code is already
process-isolated by the engine.

---

## 3. Native shell: tiling, stage, windows, overlays

These decisions are proven in code and are load-bearing. Do not revisit without
re-reading this section.

**The cursor-fight iron law (macOS).** Two overlapping interactive WKWebViews in
one window fight over the cursor: NSTrackingArea fires regardless of z-occlusion
(clicks hit-test with z; cursor tracking does not). Therefore:

- **Tiling, never overlap.** Chrome = one region rectangle (sidebar or top bar),
  content = another, gaps = window vibrancy. `core::layout` computes both rects
  from (window size, mode, metrics).
- **Native content stage owns content geometry.** A custom NSView subclass
  (objc2 `define_class`) hosts content webviews as subviews and lays out the
  split tree inside AppKit's layout pass (`resizeSubviewsWithOldSize:`), so
  window/split resize is smooth with zero Rust per frame. Split dividers are
  dragged natively in the stage; ratio changes sync back to core via
  `EngineEvent::SplitChanged`. Windows/Linux get the equivalent (§13).
- **Resize/layout is latest-value and revision-exact.** The shell coalesces
  window-size facts, the engine retains at most one newest layout per bounded
  window, and each stage hides removals before additions. Windows caches native
  geometry/visibility and applies only deltas; COM visibility is revalidated
  around each re-entrant call. Linux performs at most four immediate GTK
  convergence attempts and owns one coalesced idle retry. macOS gives every
  content update a container epoch before the first AppKit call, while its
  native split tree recomputes pane frames during resize. A stale pass may hide
  content, but it may never reveal content for an older tab/split revision.
- **Chrome is positioned natively** (setFrame + autoresizing mask), NOT via
  Tauri multiwebview `add_child` (behind the `unstable` flag). Boring permanent
  OS API over unstable framework feature.
- **Window content size is read from the chrome webview's superview bounds**,
  not `inner_size()` (contaminated after the chrome webview is shrunk).
- Non-interactive visuals over content (drop indicator, zone highlight) are
  plain layer-backed native views: no tracking area, no cursor fight. Proven by
  the DnD indicator.

**Multi-window is first-class.** `Window` is a core entity; each window has its
own chrome presence, stage, mode and split tree, all projecting the one
authoritative state. Window kinds: `Main` (full chrome), `App` (site-as-app,
minimal/no chrome), `Little` (ephemeral Arc-style link window).

**Overlays.** DOM cannot render over a content webview, so anything that
visually crosses the chrome/content boundary is its own surface. Preference
order: native menu > overlay panel > DOM.

- **Native first for menu-shaped UI.** Context menus, dropdowns and selects are
  native menus (Tauri Menu popup): zero webview cost, correct z-order, system
  behavior. Simple confirmations are native dialogs/sheets. DOM renders only
  what stays fully inside the chrome region (tooltips, hovers).
- **One reusable overlay panel** hosts the launcher/palette (and later find-bar,
  rich modals): currently a pre-warmed hidden Tauri WebviewWindow configured
  as a floating, all-Spaces auxiliary NSWindow on macOS. It is never changed
  into an unrelated Objective-C class after allocation. Being a Tauri window,
  it reuses the specta IPC and asset protocol as-is; the frame routes by
  window label. The pool grows to 1-2 recycled surfaces only when two must
  coexist.
- **The launcher is ONE surface with one behavior**: a reusable auxiliary
  window shown by both focused and global shortcuts. X11, Windows and macOS
  place it near the top-center of the active monitor. Native Wayland exposes
  no global coordinates, so the compositor owns its final placement.
  Allocation-time non-activating NSPanel semantics require a supported
  Tao/Tauri constructor seam and are not claimed by the current NSWindow.
- Anchored overlays (find-bar, modals over content) attach the same panel as a
  child window of the main window and position relative to its frame.
- Per-platform backends: macOS auxiliary NSWindow (with a future
  allocation-time NSPanel seam); Windows tool window plus the native global
  shortcut adapter; Linux uses a separate GTK Tauri window. X11 uses an
  asynchronously installed, reply-checked direct grab and free positioning.
  Its worker verifies detectable XKB repeat behavior, derives active lock masks
  from the server keymap, and publishes capability only after every grab is
  acknowledged; connection or mapping changes immediately retire the stale
  grab and restore the focused fallback. Direct X11 global registration is
  currently limited to layout-independent Space/Tab keys. Configured text keys
  remain available through the focused GTK path rather than being guessed
  across XKB groups/levels.
  Native Wayland uses the XDG GlobalShortcuts
  portal, accepts only the exact returned binding/session, and consumes the
  portal activation token before presenting the window. The worker watches
  the portal owner before Registry admission, pins subsequent calls to that
  exact unique D-Bus owner, and retries a bounded three times on restart or a
  bounded non-interactive call timeout. An explicit Registry policy or
  identity rejection is terminal for that worker run: replaying the same
  identity against the same owner cannot repair missing desktop metadata and
  would only delay the focused fallback. A user-interactive Bind response is
  never timed out. `ShortcutsChanged` is subscribed before the authoritative
  List/Bind exchange; removal or malformed state revokes the global-capability
  bit before parsing so the focused fallback resumes immediately. If the
  portal is absent, rejects the exact Registry identity, is denied, times out,
  restarts, or closes the session, the
  focused-window shortcut remains available in both the main and panel
  windows and no background-shortcut capability is claimed. Passing an XDG
  `parent_window` handle remains future, device-tested permission-dialog UX
  hardening; it is not required for capability correctness.
- Packaged Linux identity is one exact value: Tauri's Linux-only product name,
  installed desktop basename, GTK application id, and host Registry id are
  `app.zephium`; a custom desktop template keeps the visible `Name=Zephium`.
  No Linux package is built, so the release workflow does not verify it. A raw
  development binary has no installed desktop-entry proof, so its attempted
  Registry identity may be rejected and cannot be treated as proven. Zephium
  never falls through to automatic cgroup identity after such a rejection; the
  focused fallback is the dependable development path.
- A future true NSPanel must be allocated with a Tao-compatible subclass and
  layout from the start; changing a live TaoWindow to a sibling Objective-C
  class is forbidden. A raw zone-2 Wry view in an owned panel remains another
  possible implementation, with local assets only and Rust-validated commands.

**Internal pages** (history, settings, graphs/data viz, notes editor, easel):
zone-2 webviews placed INTO the stage content region in place of a content view.
The stage hosts either zone-3 content or a zone-2 internal view for a given
pane; internal views are navigation-locked to bundled assets. This gives
arbitrarily rich internal UI with no new mechanism.

---

## 4. Workspace layout

Single Cargo workspace monorepo, frontend included. Per-platform native code is
`cfg`-gated inside the owning crate, not split into per-OS crates.

```
/
├── Cargo.toml                   [workspace]
├── deny.toml                    FOSS license allowlist, bans, advisories
├── rust-toolchain.toml          pinned toolchain
│
├── crates/
│   ├── zephium-core             domains (slices + reducers + commands/events),
│   │                            layout/split math, navigation policy, ports.
│   │                            pure, #![forbid(unsafe_code)], zero I/O.
│   ├── zephium-agentic          pure browser-execution identity, lifecycle,
│   │                            semantic, action, policy, and probe contracts;
│   │                            native fixtures remain release-excluded.
│   ├── zephium-agent-model-catalog
│   │                            product-owned immutable Terra provider model,
│   │                            tokenizer, and standard-rate pricing entry.
│   ├── zephium-work-composition optional trusted macOS engine/Store adapter to
│   │                            durable Work admission; no UI or task authority.
│   ├── zephium-ipc              DTOs + specta/tauri-specta TS codegen.
│   ├── zephium-engine           Wry adapter + native stage per platform
│   │                            (stage_macos / stage_windows / stage_linux,
│   │                            same inherent surface, cfg-selected).
│   │                            unsafe allowlisted here.
│   ├── zephium-blocker          bounded adblock-rust compiler worker and
│   │                            immutable platform artifacts.
│   ├── zephium-update-transport shared redirect-free fixed-origin HTTPS
│   │                            boundary; no package or catalog authority.
│   ├── zephium-blocker-update   TUF authentication, private
│   │                            package storage, rollback/clock authority.
│   ├── zephium-blocker-service  candidate preparation, durable commit, and
│   │                            exact compiler activation coordinator.
│   ├── zephium-extension-package bounded manifests, catalogs, tree indexes,
│   │                            portable paths, and CRX3 authentication.
│   ├── zephium-extension-acquisition catalog-bound hostile ZIP preflight and
│   │                            bounded streaming decompression.
│   ├── zephium-extension-authority product-sealed catalog/manifest policy.
│   ├── zephium-extension-distribution fixed-origin authenticated catalog and
│   │                            one-at-a-time CRX/legal object acquisition.
│   ├── zephium-extension-repository immutable package materialization, atomic
│   │                            catalog sets, leases, rollback, recovery, GC.
│   ├── zephium-extension-runtime-api move-only native host authority.
│   ├── zephium-extension-service serialized Store/repository/native lifecycle
│   │                            and management transactions.
│   ├── zephium-store            SQLite actor, migrations, FTS5, fakes.
│   └── zephium-app              the actor shell: owns composed core state,
│                                drives ports, builds projections.
│
├── desktop/                     Tauri composition root. windows, menus/hotkeys,
│                                overlay backends, platform wiring. ONLY crate
│                                that touches tauri.
│
├── frame/                       Svelte 5 app (Vite, Tailwind v4). One app,
│                                entry-routed: chrome | overlay | internal
│                                pages | onboarding.
│
└── xtask/                       dev automation. `cargo xtask ci` = the one gate.
```

**Dependency rules (CI-enforced):** core depends on no internal or I/O crate;
only `desktop` knows tauri; `unsafe` only in engine/native adapters with
`# Safety` notes. Page-derived network access stays inside the exact
profile-scoped engine session; the application has no generic URL-fetch port.

---

## 5. State and data flow

One direction. UI surfaces never own truth.

```
UI intent -> IPC/bridge -> command queue -> actor: domain reducer (pure)
                                              |
                              +---------------+----------------+
                              v               v                v
                        store persists   port effects    domain events
                                        (engine, ...)         |
                                                    UI surfaces reconcile
```

Engine callbacks (title, url, loading, favicon, nav state, new-window intent,
permission request, download start, crash, native-runtime restart requirement)
enter the same queue as commands. Runtime restart state is process-sticky and
is replayed on chrome bootstrap; it is never cleared by recycling one content
profile.

Main-frame navigation uses an opaque native identity rather than requested-URL
equality, so same-origin, cross-origin, and multi-hop redirects retain one
ordered lifecycle. The raw view is hidden at native commit before Rust can be
re-entered. Privileged chrome atomically applies and verifies the exact
revision-bearing final URL/title projection; the actor correlates that revision
again before it queues first-content geometry and returns the same opaque
presentation id. A fresh tab's real New Tab surface remains in place until this
transition, with no synthetic native placeholder. Native title attribution
begins only after the exact navigation finishes.

- **Projection is granular per domain** (`items.*`, `spaces.*`, `downloads.*`),
  NOT one monolithic snapshot: no re-render storms at scale. A fresh surface
  gets a domain snapshot, then ordered deltas.
- High-frequency streams (load/download progress) use a channel, not event
  spam; frequent deltas (title/favicon/loading) are coalesced.
- **Durability tiering.** Ephemeral session state persists debounced/async.
  Durable user data (bookmarks/pins, documents, settings) persists BEFORE the
  UI is told the action succeeded.
- Optimistic UI (drag-reorder) is transient view state, reconciled against the
  projection.

Privileged mutations first reserve a bounded process-local operation id, then
enter the non-evictable command FIFO. The actor records a typed disposition in a
1,024-entry desktop ledger before event delivery. Chrome subscribes before
calling `operations_reconcile`, deduplicates results, and retries explicit
acknowledgement; the backend refuses new admission rather than evict an
unacknowledged result. This repairs WebView reloads and missed events, but it is
not a durable cross-process queue. Only operations with their own persistence
journal, currently profile deletion, resume after process death.

**Sync patterns by cardinality:** bounded state (items, spaces, windows) =
snapshot + deltas; unbounded (history, downloads) = query + paginate, never
pushed; small config (settings, theme) = load once + change events.

---

## 6. Domain model (the part to get right)

Aggregates in `zephium-core`, invariants enforced inside the aggregate.

**ID strategy (irreversible).** Persistent/syncable entities (Profile, Space,
Item, Document, Setting, Theme, SearchEngine, permission grants) get ULIDs so
future sync never collides and the FK graph never needs rewriting. Runtime-only
entities (View, Window) use local `u64`. Deletes of syncable entities are
tombstone-capable through the reducer chokepoint.

**The sidebar is a tree of Items, not a flat tab list.** Arc model: a pinned
tab IS a bookmark that can be alive. This unifies bookmarks, pinned tabs,
favorites and folders into one aggregate; there is no separate bookmarks system.

- **Profile** - isolation root. `{ id, name, color, kind: Default | Named |
  Incognito }`. Owns a store partition (own SQLite) and a webview data
  partition (WKWebsiteDataStore / WebView2 user-data-folder /
  WebKitNetworkSession). Incognito is ephemeral, never persisted.
- **Space** (workspace) - `{ id, profile, name, theme, order }`. Switching
  spaces swaps the visible item set and the theme.
- **Item** - sidebar node. `{ id: Ulid, container, parent: Option<ItemId>,
  order, kind: Folder { name } | Tab { url, title, favicon } }`.
  Containers/sections: `Favorites(space)` (the space's Essentials),
  `Pinned(space)`, `Today(space)` (ephemeral tabs, auto-archivable).
- **View** (runtime only) - a live webview bound to an item:
  `{ view: u64, item: ItemId, profile }`. Many items, few views (§7).
- **Window** - `{ id, kind: Main | App { origin } | Little, profile,
  active_space, mode, splits: Option<Pane> }`.
- **Layout / Splits** - `Mode` (Sidebar now; Horizontal/top reserved) + the
  split `Pane` tree (built, proven). Layout owns geometry; the engine
  positions native views to match.
- **Session** - snapshot of windows/spaces/splits/active per profile for
  restore. Restore rebuilds aggregates and replays the same projection path.
- **History** - append log per profile. Queried, not pushed.
- **Documents** - `{ id, kind: Note | Task | Easel, content (typed JSON),
  refs: [ItemId | Url] }`. Notes/tasks/easel/research live here; easel cards
  can hold page captures (§8 engine `capture`).
- **Downloads** - entries with state/progress; supervised subsystem.
- **Permissions** - page permissions are per `(profile, origin)`; extension API,
  host, temporary, and scheme grants are a separate per-`(profile, extension)`
  authority model. Everything defaults deny. A Chrome match pattern is never a
  permission grant, and local-file access requires an independent explicit
  grant even when the pattern is `<all_urls>`.
- **SearchEngines / Settings / Themes** - data-driven; themes are token maps
  validated against a typed schema.
- **Commands** - registry with stable string ids (`tab.close`, `space.next`,
  `page.copy-markdown`). One registry powers native menus, accelerators, the
  palette, the launcher and the user-configurable keymap (Vivaldi-grade).
- **ContentRules** - compiled native network-policy generation per profile
  (§9).
- **SearchIndex** - FTS5 across items, history, documents and commands; feeds
  the launcher and palette (§8).

---

## 7. Tab lifecycle and view binding (un-retrofittable seam)

A logical tab (Item) is not a webview. Mapping is many items -> few views,
governed by a scheduler; 200 tabs must never mean 200 content processes.

```
New(empty) -navigate-> Active <-> Inactive -idle-> Hibernated -> Closed(restorable)
```

- New/empty tab has no webview (zero engine cost); the chrome renders the
  new-tab view. The webview is created lazily on first navigation (built:
  `view: bool` on Tab).
- Three tiers, built (shell = policy via idle clock + maintenance tick;
  engine = mechanism). Hidden views drop their tiles and are throttled at once
  (WebView2 low-memory hint; WebKit's throttle scheduling policy). Hidden AND
  idle (5 min) views are suspended losslessly: WebView2 `TrySuspend`, or
  WebKit's suspend scheduling policy, which still runs audible or capturing
  pages; both resume when shown. Older hidden pages beyond a small warm set of
  recent ones (4, or 2 to save memory, 10 to keep tabs ready) are discarded
  after the chosen idle grace, whatever the tab count; a hidden page whose
  renderer exceeds a heavy threshold (256 MB, or 160 MB to save memory)
  sleeps after three minutes (one) unless it is the most recent hidden page,
  and urgent pressure takes the heaviest first. Discard follows an exact
  generation/navigation discard-safety probe. Above the pressure watermark of
  24 or under critical OS pressure they are probed without that grace; a
  memory warning shortens it to two minutes. At most four probes run at once,
  visible split leaves are protected, kept-awake sites never sleep, and an
  unsafe page is never force-discarded. A successful discard drops the view
  but keeps the item; activation recreates it, restores native back/forward
  state where it was safely captured (showing the tab's last frame until the
  restored document paints), and brings the view to its site's page zoom.
- The application admits at most 64 live views, a backstop for kept-awake and
  protected pages rather than a memory budget; a foreground create waits
  briefly for a verified close at that ceiling.
  The native engine has an independent ceiling of 75 (83 with agentic contexts), counting live views, a
  warm spare, construction reservations, and WebView2 cleanup debt that may
  still own a controller. These constants are admission bounds; the packaged
  1/10/50/100-tab and 24-hour resource measurements remain release work.
- Discard fidelity upgrade is planned per-engine: WKWebView
  `interactionState`, WebView2 resume-state, WebKitGTK
  `WebKitWebViewSessionState` (back/forward list). Until then a discarded
  tab restores by URL.
- Inactive non-discarded tabs keep their webview hidden (media, sockets,
  scroll survive a switch).
- Site icons cross as a reference (origin plus content revision), never as a
  raster. Pixels travel on their own projection, sent once per privileged
  surface and only for references that surface does not already hold. See
  `docs/design/history.md`.

---

## 8. Engine port v2

The full surface is declared now; implementations land incrementally. Grouped:

- **lifecycle**: `create_view(view, profile, url, opts)`, `close`. Views are
  created against the profile's data partition.
- **nav**: `navigate`, `reload`, `stop`, `back`, `forward`.
- **layout**: `set_content(window, tree, region)`, `set_drop_indicator(zone)`.
- **page ops**: `capture -> png` (easel, previews), `extract_html` (html->md,
  research), `find`, `zoom`, `mute`, `print_pdf`.
- **injection**: `set_user_content(scope, generation, content)` where
  `ContentScope = Global | Profile` is ownership, not URL targeting. `Global`
  is host-only; runtime callers may replace only a profile scope. Every
  registration has a stable owner-qualified `(owner, ScriptId)` identity,
  bounded `MatchSet`, and frame policy; scripts additionally carry `World` and
  `RunAt`. `MatchSet` expresses bounded URL eligibility, not authority; the
  permission broker must intersect host and scheme grants before installation.
  A terminal settlement reports the requested generation as applied, retained
  with the prior generation, or unavailable. The current adapters fail closed
  for live-view mutation and all principal-owned content until exact pre-source
  per-frame match enforcement, installed-state preservation, and isolated-world
  gates pass. Dormant macOS/Linux per-principal handler primitives are not
  retained by product construction; page worlds receive no native bridge.
- **rules**: `install_content_rules(profile, generation, compiled)` with an
  exact settlement event. Windows installs a synchronous request matcher;
  macOS/Linux install declarative native rules. Core models both as immutable
  policy payloads and never performs per-request matching.
- **events**: Title/Url/Loading (built), SplitChanged (built), Favicon,
  NavState{can_back, can_forward}, NewWindowRequested{url, disposition},
  PermissionRequested, DownloadRequested, Crashed.

The macOS page-permission seam is origin-labelled and asynchronous without
blocking WebKit's main thread. The pinned Wry adapter bounds native origin
components before copying them, preserves a combined camera-and-microphone
request as one atomic completion, retains at most four callbacks per WebView,
and denies malformed metadata, overflow, handler panic, unknown settlement,
and delegate teardown. Engine admission is stricter: at most one request per
physical view and eight process-wide, bound to the exact profile, item,
physical-view permit, and committed navigation epoch. Shell settlement,
navigation, close/profile retirement, and a 125-second watchdog use an
independent fixed terminal channel; an `Allow` is downgraded to denial after
any identity or epoch change.

The Shell-side coordinator is on by default in desktop builds (the
`zephium-desktop/macos-page-permission-prompts` feature is a default feature and
forwards to the app feature). On Windows, ordinary human tabs defer camera and
microphone to WebView2's own prompt and Zephium stores nothing; geolocation,
notifications and screen capture are denied everywhere, and extension pages and
offscreen documents deny media capture.
The macOS coordinator loads the exact profile catalog only after a supported request from the
focused resident tab, retains at most the two relevant rows, and serializes one
browser-owned process-wide prompt. While chrome reports the expanded sidebar's
address field on screen (never on a browser page such as Work), the prompt is a
card under that field, where no page view overlaps, and the page stays on
screen; otherwise it is a window-modal sheet and the content stage is removed
while it shows. Opening a browser page over the asking tab dismisses it.
Navigation to another document, tab/profile
loss, window hide, shutdown, a two-minute Shell deadline, Store refusal, or
identity mismatch all deny; the same document finishing its load or changing
its address through the History API does not. One-time choices never write
policy. Allow this time and Don't allow are held in memory for the exact tab,
origin and committed document that asked, so its later requests settle without
a prompt until the tab shows another document or closes, or the private session
ends. A saved decision still wins, and changing the site in Site settings
forgets them. Escape refuses that one request and remembers nothing. A
remembered or one-time denial dominates an atomic camera-and-microphone request; a remembered Allow reaches WebKit only
after an atomic CAS mutation and a second exact catalog read observe every
requested capability as allowed. Conflict and outcome-unknown results are
reconciled but never blindly retried. The prompt offers Allow while visiting
the site, Allow this time and Never allow; only the first and last are written.
Geolocation uses the same coordinator and catalog. Settings ▸ Privacy ▸ Site
settings lists and changes the catalog's rows through the same CAS path. The
frame receives opaque echo identities, a canonical origin, and a closed
camera/microphone/location vocabulary; it cannot supply an origin or permission
name. Incognito profiles bypass Store entirely, expose
only one-time choices, and cannot be coerced into durable policy through IPC.
Pre-bootstrap and quarantined-profile events still
deny every request. Unit, actor, IPC-binding, and frontend projection tests
cover the remembered-choice path. The feature-only `macos-page-permission-probe` provides a loopback-origin
WKWebView gate that observes one atomic camera-and-microphone request, defers
it, resolves exact Deny once, rejects a duplicate settlement, and requires
JavaScript `NotAllowedError`. It is deliberately compiled and linted, but not
executed, by `cargo xtask ci`: [WebKit requests system validation before it
enters the UI-client policy decision](https://github.com/WebKit/WebKit/blob/main/Source/WebKit/UIProcess/UserMediaPermissionRequestManagerProxy.cpp),
and its [mock-capture setting is test tooling rather than a shipping
WKWebView API](https://github.com/WebKit/WebKit/blob/main/Tools/MiniBrowser/mac/WK2BrowserWindowController.m).
An unattended run can
therefore prompt, wait on, or mutate camera/microphone consent before Zephium
receives the request, even though Zephium later resolves Deny. The focused
command is a developer diagnostic only when the responsible host process is
already TCC-authorized; it is not packaged-release evidence:

```sh
cargo run --locked -p zephium-engine --features native-page-permission-probes --bin macos-page-permission-probe
```

The macOS bundle carries human-readable `NSCameraUsageDescription` and
`NSMicrophoneUsageDescription` values, but metadata is not capability. A
signed packaged build must still be checked for camera, microphone, and atomic
combined Allow/Deny, OS consent ordering, navigation and tab-close
invalidation, and device-track cleanup (`desktop/Entitlements.plist` grants the
two device entitlements and location). Remembered-policy reload and incognito
non-persistence are covered by Shell tests and still need a packaged check.

Session restore state (§7) rides on lifecycle (`opts` carries restore state).

The favicon path never polls. When a page commits, the shell asks the engine
to watch that document's icon links: one host-initiated evaluation waits inside
the page, answers with the addresses its `icon`, `apple-touch-icon` and
`mask-icon` links name (cross-origin included) as soon as its head has any, and
again if they change until a moment after load, then stops. The page names
addresses only; it never supplies an image. The shell ranks them (kind first,
then the side nearest 32 px without upscaling, the site's own `/favicon.ico`
last) and asks the desktop prober for the best three. The prober is the one
anonymous client the Work origin probe already used: HTTPS to public addresses
only, resolved by a resolver that refuses private ones, no cookie store, no
credentials, no referrer and nothing of the page, two redirects, five seconds,
256 KiB. Its decoders are the bounded memory-safe ones; only a fixed 32x32
RGBA raster leaves it, so a page-controlled image is still never parsed by the
chrome. What the prober cannot use (vectors, plain-http or private hosts, icons
that need the page's cookies) is decoded by the page itself from one address
the host names, same-origin first. A private profile never asks the prober for a
page's addresses: all of its icon fetching stays inside its own partition and is
kept in memory only. Where WebView2 reports the icon it chose (`FaviconChanged`
and `GetFavicon`), that PNG is the primary source and the declared addresses
wait for the load and back it up. A tab shows its page's icon, then its
origin's, then the other spelling of its host (`www` and apex only). While a
page of the site it was just on loads it keeps the icon it had until the new one
is known or thirty seconds pass; a tab that moved to another site shows its
loading state. An address that gave nothing is not asked for again for an hour,
and the oldest of those give way first, so a new site always gets its turn.

Future application-owned downloads, such as maintained filter lists or signed
updates, are not page-derived browsing traffic. Each must use a purpose-built,
bounded component with its own source allowlist, redirect, proxy, integrity,
retention, and shutdown policy. Apart from the icon prober above, a generic
native fetch seam must not be reintroduced for page-controlled URLs.

---

## 9. Blocker (bundled network policy implemented; TUF next)

The native network blocker is implemented behind the core policy ports. The
desktop embeds an immutable, release-authenticated EasyList + EasyPrivacy seed
with exact license/provenance and compiler-quality manifests. Raw list bodies
are inflated only after a compiled-cache miss, and every new profile still
defaults to disabled. Enabling requires the exact authenticated catalog and
holds first navigation until a non-empty native artifact is installed; it
cannot present an empty policy as protection.

The production TUF trust domain is intentionally not provisioned yet.
Bundled mode is network-inert, exposes `release_bundle` provenance, and has no
manual source-refresh action. TUF will be a distinct, independently
authenticated authority rather than an environment-variable or writable
configuration override.

One bounded compiler worker parses already-authenticated ABP/uBlock-style or
hosts-format sources with the vendored `adblock-rust` fork. It publishes only
one native artifact for the current platform. The shell assigns an exact,
non-wrapping policy generation, holds first view creation/navigation until an
explicit allow-all or blocking policy is installed, and generation-checks
every worker and native settlement. A replacement is installed across the
profile's live-view cohort before the previous registration is retired;
ambiguous cleanup is terminal native-accounting failure.

| Platform | Implemented network mechanism | Deliberate v1 boundary |
|---|---|---|
| WebView2 (Windows) | frozen in-process matcher from synchronous `WebResourceRequested`; a block receives an empty, no-store 403 response | only document-sourced native stylesheet, image, media, font, script, XHR, fetch, and ping contexts; no document/subdocument/WebSocket/object/other interception and no service/shared-worker request interception; missing initiating-frame attribution uses conservative source-independent matching |
| WKWebView (macOS) | canonical WebKit JSON compiled/cached as `WKContentRuleList` and installed per raw view | conversion losses are counted; `$important`, method predicates, full regexes, and other non-equivalent rules are omitted rather than approximated silently |
| WebKitGTK (Linux) | the same canonical JSON compiled/cached as `WebKitUserContentFilter` and installed per raw view | the same declarative coverage boundary as macOS |

v1 is network-only on every platform. Cosmetic filtering, scriptlets,
redirect resources, CSP mutation, URL-parameter rewriting, generic-hide
controls, and tag-driven policy are rejected before artifact publication.
There is no second Rust proxy or local-root-CA MITM path.

Published blocking artifacts carry a checked coverage report and an explicit,
nonzero post-control native blocking-entry count. This is a structural
admission invariant, not a claim that exceptions leave every entry reachable;
exceptions and platform-omitted rules are not counted as blocking entries.
Approximation is also typed by resource reachability, native request-source
kind, and initiating-document attribution, with a deduplicated aggregate.

The purpose-built updater and coordinator are implemented. Release mode
validates deterministic embedded gzip, canonical manifests, exact raw
length/digest/header provenance, and the approved license before compilation.
TUF mode authenticates a fixed-origin, licensed package into a private
content-addressed store with a durable clock/revision high-water, then keeps a
candidate distinct from current while the compiler prepares its exact
artifact. Only that exact prepared candidate may be durably committed and
activated; current/previous known-good state and candidate recovery survive
crashes. Compiled artifacts and native WebKit content-rule namespaces have
bounded, identity-safe caches and garbage collection.

Privileged main chrome receives only a revisioned focused-profile status and
bounded refresh, preference, and exact-generation retry commands. It exposes
public package revision/digest and aggregate coverage diagnostics, not profile
IDs, URLs, per-request decisions, list text, or native/parser strings. Raw
content views have no blocker command surface. TUF trust material, packaged
native enforcement tests, legal approval, external review, and
resource/endurance evidence remain stable-release gates. Exact limits,
release-seed measurements, failure semantics, native coverage, and release
gates are documented in
[`adblock.md`](adblock.md).

---

## 10. Extensions (bounded MV3 subset; currently disabled)

The WebExtensions API is a browser-layer compatibility product, not a feature
the native engines provide uniformly. Zephium does not claim extension
installation, API mediation, userscripts, or Chrome/Firefox compatibility in
the current tree. The product target is a curated, package-neutral MV3
compatibility surface. Pinned **Bitwarden Core** and stock third-party
artifacts are adversarial acceptance contracts used to expose platform gaps;
they are not product dependencies and no target-specific branch belongs in
ordinary browser code. The agreed public target now combines a small
recommended cohort with policy-eligible Compatibility/Beta installation from
the Chrome Web Store or an approved publisher. Original package bytes come
directly to the user's device; Zephium serves shared signed compatibility and
revocation metadata, not third-party packages. Recommendations retain exact
test history while eligible upstream versions update independently. This does
not relax the existing exact Verified manifest authority or promise general
API parity. Public provider composition and Beta activation remain unfinished
and disabled.

The public metadata format and backend handoff are defined by the
metadata service contract. Its bounded parser and read-only xtask
validator produce structural data, not authenticated policy or installation
authority. Upstream intake can authenticate and preflight an original CRX
without inventing a reviewed catalog row; complete tree receipts preserve its
original CRX digest and can bind its exact original manifest to a numeric
upstream checkpoint. The checkpoint has a fixed durable codec and rejects
downgrades, publisher changes, and equal-version byte changes.

The opt-in `zephium-extension-distribution/public-policy` client now verifies
the fixed shared target with Tough 0.24.0. It pins the channel and bootstrap
root, bounds every metadata response before parsing, rejects delegations and
unexpected targets, requires 2-of-3 root keys and separate signing roles, and
compares all four role versions/digests plus the exact policy revision/digest
against durable history. Publisher-wide revocations cannot disappear. Both
compiled root slots remain empty, so this client is not enabled in the desktop.

`ExtensionPolicyCache` uses the existing exclusive private-filesystem namespace
to atomically commit the target bytes and checkpoint in one bounded record.
Reopening retains expired high-water history and rejects corrupt state instead
of resetting it; uncommitted staging is discarded only after current-state
validation. Receipts become stale when superseded or when their cache owner is
dropped. Freshness checks include the earliest signed role/policy deadline,
durable known time, and a process-local monotonic deadline. These are signed
policy receipts, not Beta manifest or native-runtime authority. The filesystem
adapter supports this cache on macOS/Linux. Windows has implemented handle-relative
operations and shared recovery behind a debug-only validation gate; default
activation still fails closed pending live Windows validation.

The response cache is bounded to 2 MiB/32 entries and sends only content-derived
conditional ETags. A 304 still re-enters full signature and freshness validation.
Requests have a 20-second deadline, complete refreshes a 60-second deadline,
and at most 16 root updates are processed per refresh. No worker or scheduling
loop is started; jitter/backoff and desktop composition remain integration work.

PROFILE schema v14 adds an immutable provenance row per installation and a
bounded publisher high-water history. The source-aware Store authority methods
commit package selection, source/transform/output/compatibility/policy evidence,
grants, and upstream history in the same transaction. Updates compare the
complete expected current provenance and reject source-provider changes,
policy rollback/equivocation, and upstream rollback/equivocation. Existing
reviewed installs receive no inferred source data. Snapshot reads require
matching independently reauthenticated provenance when a row exists; legacy
bindings cannot silently omit it. Native Begin/MayOwn and grant writes also
rejoin the stored output descriptor and high-water state through the existing
grant codec. The native journal retains its existing exact package and
install/grant revision bindings rather than duplicating mutable grant/native
identities in provenance. Uninstall removes the installation's provenance but
retains its upstream maximum; complete profile erasure scrubs both tables.

The empty schema adds 8 KiB (two 4-KiB SQLite pages) per profile in the measured
migration. Each encoded record is bounded to 1 KiB (the current maximum shape
is 812 bytes), and history is capped at 128 publishers per profile. The path
uses the existing Store actor and starts no worker, timer, or native view.
These are structural persistence guarantees, not Beta authentication.
The opt-in `zephium-extension-distribution/beta-admission` path now consumes an
authenticated complete CRX tree and a live accepted policy to mint the separate
`ProductAdmittedBetaSource` witness. Both macOS and Windows native targets are
assessed independently of the build host. The shared manifest parser retains
the exact Verified entry point; the new upstream entry permits an absent
manifest key while rejecting a supplied key that differs from the CRX publisher.
No reviewed catalog row is synthesized. Compiled source rules, exact remote
target/version opt-in, revocation, and upstream high-water comparison all apply.
The witness rechecks live policy on use and cannot enter a Verified activation
path.

The opt-in `BetaPreparationWorkspace` now turns admitted source into a private,
sealed output artifact. It reauthenticates the original CRX, performs the
compiled manifest-key identity adaptation, streams and re-hashes the closed
tree, and atomically publishes an uninstalled `ready` slot with the original
archive, canonical index, and deterministic transformation evidence. Reopen
recomputes the transformation before accepting stored output. Private filesystem
primitives own identity/mode checks, sealing, no-replace publication, and bounded
recovery; operation serialization and receipt epochs prevent discard/read races.
`PreparedBetaArtifact` exposes no native path or lease and cannot enter a
Verified activation path. Its structural provenance requires separately proven
provider classification and later Store/native joins.

Approved provider acquisition, broader compiled compatibility adaptations,
publication into the installation repository, permission consent, native
admission, and public install/update
orchestration remain necessary before exposing public installation. The exact
Verified constructors remain unchanged.

The synchronized macOS private-directory publication hardening also requires
GC to account for one additional root-reseal directory sync and two mode
changes per freshly retired tree. The pre-existing maximum-cohort filesystem
tests reproduced five failures on unchanged `1db685c`; correcting the accounting
makes measured macOS pending cleanup 102 syncs against a 104-sync cap, preserving
two syncs of headroom. Fresh cleanup is 115/128. Other platforms retain the
existing 94/96 pending and 107/128 fresh accounting. This changes no native
filesystem operation and does not remove the required publication hardening;
the no-garbage path still performs zero durability writes.

Delivery is layered and measured:

- declarative data uses native rules/storage and no persistent JS runtime, but
  still has compile, match, and memory cost;
- userscripts/userstyles lazily require a native world/handler registration per
  active principal in each eligible live content controller/view, plus an
  isolated JS context in each eligible frame/document; both cardinalities are
  explicitly capped; and
- the Linux MV3 compatibility subset requires a bounded Zephium-owned
  event-runtime view/context plus any chosen extension-UI or capability-specific
  offscreen resources; WKWebExtension and WebView2-native extension workers are
  engine-managed and require separate count admission and measured
  process-resource gates.

On macOS 15.4 and newer, Apple's public `WKWebExtensionController` stack is the
preferred native MV3 candidate. A feature-gated live probe proves controller
attachment before view construction, explicit host/private-data grants,
per-extension isolated worlds, frame matching, exact context unload/reload,
preservation of Zephium's protected scripts, MV3 background execution, and
same-principal extension-storage isolation across two persistent controller
namespaces and fresh controller instances. A second behavioral gate constructs
two regular profiles through the exact product registry and one
nonpersistent private session. It pointer-attests each view/controller/store
binding, proves mutually exclusive cookie and extension-storage state, proves
regular reconstruction and private noninheritance, and publishes distinct
tab/window/delegate graphs to demonstrate that each controller exposes only its
own profile surface. The probe retires each namespace independently, reopens
both to verify zero persistent extension bytes, and requires every native
view/controller/context/store and routing object weak reference to release.

The delegate always enumerates the complete logical tab set, including
discarded tabs, but it returns no `WKWebView` for a non-resident tab. That
callback cannot carry a typed error, so the refusal increments one saturating,
URL-free per-profile diagnostic instead of inventing a generic mutation
failure. The native gate verifies that the physical-view resolver is never
called for the discarded tab and that the diagnostic advances. Extension
enumeration therefore cannot resurrect a renderer or defeat §7's memory
policy.

Extension-driven tab reload and back/forward traversal use the same bounded
native-completion broker as create, activate, navigate, and close. Shell owns
the mutation decision and dispatches only to an already-resident tab in the
focused profile scope; a discarded or in-flight-discard tab receives a typed
`TabDiscarded` refusal and is never recreated. History traversal also requires
the latest Shell-owned native history flag. Cache-bypassing reload remains an
explicit refusal because the cross-platform Engine contract currently exposes
ordinary reload only; approximating it would report semantics the browser did
not apply.

Native grant replacement is an exact, bounded main-thread operation. The pure
grant compiler supplies the complete allowed API/host set; the adapter clears
all four WebKit permission dictionaries, revokes every bounded prior key
through the per-key status API, applies every new key through that same API,
and accepts the generation only after exact dictionary/status/private-access
readback. This distinction is behavioral: bulk dictionary assignment alone
does not recompute content-script eligibility for a loaded context. The live
probe therefore proves host-grant activation, revocation, and restoration on
subsequent navigations, then unloads the context before clearing and verifying
final absence. The production macOS adapter reaches this boundary only through
authenticated package, grant, ownership-journal, and operation authority
reconstructed inside the serialized extension service.

The same identity rule applies to runtime `permissions.request()`. Grant
revision and digest are inputs to the runtime fingerprint, durable native
ownership row, native grant snapshot, and operation authority. A delegate may
therefore never persist a grant and patch only `WKWebExtensionContext`, nor may
it return an allowed set before those authorities agree. Runtime optional
permission requests remain denied until the serialized coordinator implements
a crash-consistent upgrade protocol. The manual native gate now proves the
platform settlement semantics with genuine clicks in an extension-origin page:
WebKit invokes the API-permission and host-pattern callbacks separately with no
tab for this context-scoped request; complete denial leaves all four native
grant/denial dictionaries empty; partial approval of a combined API-plus-host
request settles JavaScript as denied and retains neither subset; and complete
approval settles JavaScript as allowed and retains exactly both grants. The
delegate must therefore collect and authorize the complete request without
depending on callback order or tab presence. The gate runs explicitly with:

```sh
cargo run --locked -p zephium-engine --features native-web-extension-probes --bin macos-web-extension-probe -- --interactive-permission-gate --require-supported-runtime
```

Normal unattended CI compiles this path but reports it as
`interactive-not-run` because WebKit rejects `permissions.request()` without a
trusted user gesture. The upgrade protocol must either atomically supersede
every fingerprint-
bound authority around a verified complete live native replacement, or retire
the old owner before persistence and prove that WebKit can truthfully settle
the originating request across replacement. Live context grant replacement
remains useful for native behavioral verification, but is not durable product
authority by itself.

The controller registry now separates persistent namespace identity from
quiescence. View construction and exact controller borrowing revalidate the
profile identifier, persistent controller configuration, and the exact
`WKWebsiteDataStore` pointer while allowing that controller to contain active
contexts. Creation, profile erasure, and clean shutdown additionally require
the controller to contain no contexts or extensions. Controller preparation is
not a startup side effect: the native lifecycle must supply the exact durable
`MacosControllerV1` namespace scope, preparation is bounded to the profile
ceiling and idempotently reuses an existing entry, and a borrow never creates a
missing namespace.

The native activation boundary constructs one move-only macOS owner from a
provider-validated package-root lease, a complete compiled grant snapshot, an
exact catalog-authenticated 32-byte Chromium identifier, and the retained
profile controller. The root path is borrowed exactly once; only a byte-for-byte
read-back file URL crosses the callback boundary, while the package lease stays
retained by the host reservation. Activation accepts only MV3 with zero parse
errors, assigns and re-reads the exact native identifier, applies the complete
grant set, and pointer-attests the context, extension, and controller after
load. Teardown unloads the exact context, clears every bounded native key seen
at cleanup (not merely keys from the last plan), and mints the macOS absence
audit only after controller-membership and unloaded-state readback. A native
exception while entering asynchronous parsing is ownership-uncertain, never a
never-entered rejection. The ordinary macOS factory joins activation,
retirement, same-process reconciliation, restart reconciliation, and shutdown
drain as one recoverable lifecycle. Startup first settles cleanup, then loads a
bounded canonical inventory of enabled regular runtimes and replays the same
authority transaction before Shell may construct any profile webview. Hydration
retries resume monotonically without re-entering cleanup around a runtime
already acquired by the same worker; strict runtime-capacity deferrals and
exact rejections are retained and counted in readiness evidence.

An extension-origin page is not navigated in a normal profile view. WebKit
requires the loaded context's customized
[`webViewConfiguration`](https://developer.apple.com/documentation/webkit/wkwebextensioncontext/webviewconfiguration),
so toolbar popups use WebKit's native action popover and a separately budgeted
popup view rather than navigating, replacing, or overlaying a content tab. One
private extension context per installed extension lives for the private-session
lifetime; its UI views may be recreated from that context, while destroying the
session context, controller, and nonpersistent store is the storage-erasure
boundary.

The same rule now covers extension-owned full documents. A controller callback
may retain one exact same-principal internal URL while Shell authorizes only the
foreground profile through `OpenExtensionPage`; the URL itself never crosses
into ordinary navigation authority. After that typed settlement, the host
rejoins the request with the context/controller identity and the existing
process-wide foreground-extension lease, builds the view from
`WKWebExtensionContext.webViewConfiguration`, and publishes one bounded native
window/tab pair back to that controller. Configuration shape, target window,
append index, active state and principal are revalidated before any view exists.
WebKit maps Chrome's ordinary `tabs.create({ url })` default to an active tab
without adding it to an existing multi-selection; the dedicated extension
window has exactly one tab, so both values of that inert selection flag are
accepted without manufacturing selection semantics. Parent/opener, pinned,
muted, reader-mode, and inactive requests remain refused. Same-origin routing
remains inside the extension document. Allowed
external links become ordinary Shell HTTP(S) tabs, and an allowed programmatic
main-frame transition hands off to an ordinary tab before closing the
privileged document; external subframes and all non-web schemes remain denied.
The page, options view and action popup intentionally share the one
foreground-extension pool, so extension UI cannot consume tab or background
runtime capacity. Close, process termination, context retirement and shutdown
release the same lease; renderer callbacks defer teardown so their native
delegate cannot be destroyed reentrantly.

The macOS Bitwarden contract gate additionally resolves its tab-specific
`WKWebExtensionAction`, verifies label/enabled/popup state, and opens the
declared popup `WKWebView`. That view must point to the exact extension
controller and profile `WKWebsiteDataStore`; the gate proves load,
`closePopup`, reopen, and final wrapper release when the context retires.
The contract runs on a persistent regular profile prepared through the product
controller registry, not on a nonpersistent stand-in: Wry's constructed view
must pass the normal controller/store attachment attestation, the Shell-shaped
browser surface supplies the existing resident tab, and teardown closes that
surface before unloading the context, clearing grants, erasing the exact probe
principal's data record, and releasing the registry. This distinction is
behavioral. WebKit treats a nonpersistent website data store as private even
when a test delegate labels its logical window regular, which suppresses
content injection without the independent private-data grant.
WebKit may cache the wrapper and its last URL after `closePopup`, so production
holds the process-wide `ExtensionPopup` resource lease only for the presented
interval and treats the documented close call—not wrapper deallocation—as the
presentation-resource release boundary. Only one such lease exists inside the
same hard native-resource ceiling (75, or 83 with agentic contexts).

Toolbar projection is a replaceable Shell-owned cohort keyed by the exact
profile, logical tab, browser-surface generation, runtime generation, and
monotonic native action revision. The engine enumerates only operation-authority
published runtimes in stable native `Owned` state, resolves the already-created
logical `WKWebExtensionTab`, and calls `actionForTab:` without reading the tab's
weak webview or accessing `popupWebView`. Labels and badges cross the boundary
only after fixed byte validation. WebKit decodes the declared icon natively;
the adapter exception-contains and rasterizes its `NSImage` into one exact
32x32 straight-alpha RGBA buffer, so privileged chrome never decodes an
extension-controlled image format or retains a native image graph. Unchanged
effective state reuses its prior revision. An applied empty cohort removes stale
buttons, while a rejected or stale refresh retains the last known-good cohort.
`didUpdateAction` is accepted only from the exact profile controller, loaded
context, and (when present) known logical tab, then reduced to one coalescible
profile invalidation fact. Scheduled reads remain in a fixed-size pending set
until an exact applied settlement arrives. The existing low-frequency
maintenance tick rereads each active profile to repair a rejected or lost
settlement/invalidation without adding a timer, renderer, or idle wakeup.
Profile retirement erases both the logical surface and its action cohort. This
read path is not invocation authority. A toolbar invocation is independently
versioned by request id, surface generation, runtime generation, and action
revision; Shell derives the current tab rather than accepting one from IPC.
The native host revalidates residency and action state, optionally joins a
declared `activeTab` witness to the exact currently-presented HTTP(S) document,
then revalidates again before `performActionForTab:`. Missing `activeTab` or a
restricted document never suppresses the separate click event, while capacity
or identity contradictions fail closed. Non-popup actions are dispatched;
popup actions first reserve the one process-wide `ExtensionPopup` lease and an
exact profile/controller/context/tab callback expectation. A programmatic or
mismatched WebKit callback is rejected. A matching loaded popup is shown as a
transient native `NSPopover` relative to the privileged Shell anchor, with
finite clipped anchor geometry, continuously clamped 64x48--800x600 content,
outside-click/Escape dismissal, a ten-second load watchdog, and exact-once
terminal settlement. Before acquiring another lease, a second trusted
invocation reauthenticates the complete action cohort and toggle-dismisses only
a pending or visible popup owned by the exact same runtime context and native
tab. A pending first request settles as superseded and the second settles as a
typed dismissal; a different extension or tab still receives the one-popup
capacity refusal. The authenticated native and brokered product probes require
open, toggle-dismiss, reopen, discard-close, and final reopen in one lifecycle.
Diagnostic builds retain normal transient behavior by default; setting
`ZEPHIUM_EXTENSION_LAB_RETAIN_POPUP=1` is the explicit opt-in that keeps a
popup alive while Safari attaches to its WKWebView. Surface replacement closes
a popup when its tab becomes
discarded, inactive, or absent; runtime retirement, profile erasure, and host
shutdown cancel loading and close the native popup before owner release. The
lease covers only the pending/presented interval and is released even though
WebKit may cache its popup wrapper. Zephium-created options WKWebViews also
handle `webViewWebContentProcessDidTerminate:`. The weak native delegate defers
teardown to the next main-queue turn, revalidates the exact context and WKWebView
generation, detaches and closes the window, and releases the shared
popup/options lease instead of leaving a blank resource owner. A real injected
renderer-crash run in a signed packaged application remains a release gate.

Native extension commands use one on-demand AppKit local key-down monitor. It
is installed only for an actual native activation attempt and removed exactly
when the last loaded context retires, so inert startup and a fully disabled
cohort pay no per-keystroke cost. The macOS main menu receives first refusal for
browser-owned accelerators. Remaining events must originate from a content
WKWebView whose logical item is the Shell-focused, active, resident regular tab.
The controller then bounds and sorts at most eight canonical context identities,
uses `commandForEvent:` without executing, reauthenticates every match against a
currently published runtime, and performs only one unambiguous non-action
command. Cross-extension collisions are not dispatched. An unambiguous
`_execute_action` match emits only the exact runtime, tab, and surface
generation into Shell. Shell rejoins the current action revision and projects
one actor-ordered, one-shot request to privileged chrome; chrome can contribute
only the already-rendered browser-owned action button's current rectangle
through the ordinary action invocation. A newer action/failure projection or
the one-second gesture deadline invalidates the request, exact consumption
prevents remount replay, and the
normal native path reauthenticates every identity before WebKit executes.
Browser-owned shortcut remapping/conflict UX and packaged physical-key evidence
remain separate product work.

Page context menus are composed at Wry's native `menuForEvent:` boundary only
for views constructed with an authenticated profile controller. WebKit creates
the default `NSMenu`; Rust receives it as an opaque retained object and never
copies page URL, selection, link, editable-field, or title metadata. The host
revalidates the exact view generation, resident logical tab, surface generation,
and currently published runtime contexts. Contexts are ordered by canonical
principal in fixed storage; each extension may contribute at most 16 top-level
items, the merged cohort at most 32, and the complete submenu graph at most 64
unique items, 32 unique menus, depth four, and 512 UTF-8 title bytes per item.
Cycles, aliases, inconsistent native counts, stale contexts, and over-budget
trees fail closed to WebKit's unchanged default menu. A live gate preserves the
default prefix, including a nested `AutoFill -> Passwords…` stand-in with the
exact original item and submenu identities, adds the native separator/item,
performs that item through `NSMenu`, observes the exact
`contextMenus.onClicked` tab, and then proves removal. A real Extension Lab run
also observed WebKit's native AutoFill submenu beside 1Password's extension
menu. Packaged physical right-click and broader page-context variants remain
release evidence rather than an architecture gap.

The privileged frame receives an actor-revisioned exact-replacement action
cohort only for the focused profile and active logical tab. Fixed RGBA icons
use canonical base64 rather than a 4,096-element JSON integer array. The frame
joins the projected profile/tab to the current Items snapshot, rejects delayed
action or failure evals against the same monotonic revision floor, and echoes
only install/runtime/action revisions plus the clicked button's CSS viewport
rectangle. Desktop translates that rectangle through the generation-checked
chrome origin into window-logical coordinates, then accepts canonical ULIDs,
fixed lowercase nonzero hex, a fully visible finite anchor, the main-window
caller, and a live shutdown state;
Shell and native code still derive and revalidate every authorizing fact.
Synchronous and asynchronous refusal map to a closed, profile/tab-bound
user-visible taxonomy without exposing native strings, URLs, extension content,
or runtime identity. A surface-generation replacement emits an empty cohort
before its asynchronous native refresh, so even a same-tab reconstruction
cannot preserve a stale button.
Toolbar rendering is allocation-bounded by the eight-install profile ceiling;
disabled actions remain visible but inert, badges are clipped, and no idle
animation, renderer, timer, or popup resource exists until an actual failure or
trusted click.

The repository/service tests prove authenticated package admission and
startup/crash reconciliation against a bounded native fake, while the live
WebKit probe proves the platform adapter. The authenticated product probe now
joins package authority, native ownership, controller activation, the
Shell-owned window/tab delegate, and real content-script execution. Its raw
HTTP document first proves that Tauri, Wry, principal-handler, and privileged
extension APIs are absent, then installs page-world `chrome`/`browser`
lookalikes and poisons the DOM setter used by the document-end fixture. WebKit
does expose the restricted page-side `browser.runtime.connect/sendMessage`
shell; a separate live gate has both the raw page and a second extension
address the exact known context identifier through message and port APIs when
`externally_connectable` is absent. A delayed isolated content script asks the
target worker for its delivery counter; only exact zero passes, so a timeout is
never treated as refusal. Zephium preserves that manifest declaration as
unmodeled authority, so no product profile can classify it runnable. The
authenticated content script must still complete through its isolated world
without touching either forgery. Production
provisioning remains deliberately empty, so ordinary release builds expose no
extension runtime. The opt-in acquired-package service does now own a bounded
provisioning ingress: one move-owned, path-free request may be retained at a
time; its actual vector capacities are charged against the shared ceiling; and
the worker reauthenticates the sealed catalog, CRX, manifest, legal artifact,
and complete extracted tree before publication. Complete catalog activation is
separate and source-free. `zephium-extension-distribution` now owns the narrow
product-neutral source for that ingress. Construction first requires a valid
compiled product authority; it then fetches one fixed `catalog-v1.json`,
authenticates the exact bytes before requesting any package object, binds one
complete ordered runtime selection, and derives CRX and legal URLs only from
authenticated package/revision/digest fields. The legal object is fetched
before the larger CRX, each response requires one bounded exact
`Content-Length`, and CRX signature, developer identity, inner ZIP identity,
and legal digest are checked before a path-free service request is created.
The serialized service deliberately reauthenticates all of that evidence at
the durable boundary. A single-flight distribution coordinator now joins the
two without borrowing Shell across network awaits: it fetches and submits one
move-owned package, waits for the non-blocking service callback, and only then
fetches the next. One completed outcome-unknown settlement permits one exact
refetch/retry; a repeated unknown, callback loss, callback timeout, submission
panic, or service invariant failure quarantines that coordinator until process
restart. The service releases its one-request retained-byte permit before the
callback can reenter, so retry and forward progress cannot overlap owned CRX
buffers. Dropping the async run during an accepted but unsettled callback also
quarantines; dropping it during network acquisition safely releases the
single-flight slot. Complete activation remains source-free and occurs only
after every selected row settles. The application actor now provides the
non-blocking end of that port: an opaque shared one-shot envelope moves each
bounded request through the existing Shell mailbox, Shell consumes it exactly
once, and a shared settlement cell preserves exactly-once completion across
service refusal or boundary panic. Network work and package parsing never run
on Shell, and the mutable lifecycle owner never crosses an await. Mailbox
refusal does not claim callback ownership; an already-admitted command drained
during terminal shutdown settles unavailable rather than disappearing. The
separate `zephium-extension-updater` now owns the dormant product scheduler:
one explicitly constructed current-thread async runtime, one bounded request
slot, cancellation-aware shutdown, and a fixed-size monotonic status stream.
Its launch plan binds the authenticated client to one complete immutable
runtime selection derived from sealed manifest profiles. Refresh callers carry
no package key, backend, profile, endpoint, or catalog bytes; they can only ask
the worker to synchronize that reviewed plan.
Its process-singleton launch claim is never released by shutdown or quarantine,
so a failed worker cannot be replaced to bypass restart-required state.
It performs no automatic polling and issues no I/O before an explicit request.
Invalid or overallocated runtime selections fail during plan construction,
before worker launch or catalog acquisition;
retryable failures reopen admission, while coordinator quarantine remains
terminal until restart. A run publishes its terminal generation before
reopening admission, and settlement shares the shutdown gate so it cannot
overwrite a terminal state with `Idle`. Status is coalesced into Shell and
projected with a closed redacted vocabulary to privileged extension UI;
ordinary builds that never construct the worker correctly expose no
distribution state. There is
still no product endpoint/configuration, platform runtime-target policy, or
refresh schedule. The opt-in desktop `curated-extension-distribution` graph now
constructs a dormant worker only after the suspended Shell callback exists,
linearizes its unique owner against Shell admission, and cancels plus joins it
before Shell can retire the extension service under the same absolute shutdown
deadline. Native-failure exit drains it independently. The feature's sealed
configuration slot remains empty, and the default desktop graph still has no
dependency on the updater, distribution client, HTTP, or TLS stack; neither
graph currently supplies package bytes or network authority. Privileged main
chrome exposes a refresh control only after that worker projects its presence.
The command is argument-free: it can request the already sealed plan once, but
cannot select a profile, package, runtime target, URL, or artifact. Its bounded
admission result is distinct from the generation-ordered completion status.

The redirect-free fixed-origin HTTPS boundary shared with the blocker lives in
`zephium-update-transport`. It accepts only plain-ASCII paths below exact HTTPS
directories, refuses redirects, compression, credentials, queries, fragments,
and encoded path components, and redacts request failures. Its stricter bounded
object operation additionally refuses missing/ambiguous lengths and transfer
framing, reserves the declared capacity fallibly, and verifies the final body
length. TUF adaptation is optional and absent from the extension-distribution
graph. The shared crate grants no catalog, filesystem, install, or runtime
authority and neither it nor the distribution client is exposed to extension
or page content. Authenticated catalog management and install UX,
native action projection, and transient popup hosting are implemented; the
install review always includes required API/host authority, defaults every
optional declaration to denied, and can return only bounded indexes into the
exact canonical candidate retained by Shell. The serialized service
reauthenticates that package, resolves the selected declarations from its
manifest, and atomically initializes the grant cohort; local-file and private
browsing access remain separate explicit decisions. Reviewed degraded
declarations also flow from that exact admitted manifest into a bounded typed
disclosure cohort. API names are revalidated tokens; structural declarations
collapse into browser-owned feature categories; compatible rows cannot carry
limitations and degraded rows cannot omit them. The install review renders the
complete cohort before consent. Installed rows retain the exact canonical API
and host grants from the same authenticated Store authority and disclose them
on demand; their counts are derived in chrome rather than projected as a
second source of truth. Optional declarations are projected as separate,
canonically ordered arrays. Privileged chrome may return only one array index,
the exact install/catalog/grant CAS revisions, and a desired boolean;
permission text never returns through IPC. The serialized service
reauthenticates the package and manifest, resolves the index, and compares the
complete grant cohort before mutation. A changed edit retires every
regular/private native context, applies one bounded Store patch only after
native-owner absence, then restores only contexts that were live. No-op edits
perform no native work; uncertain commits stop further management writes until
restart. Required declarations are not editable from this surface. Neither
view receives native error strings or invents compatibility from a package
name. Legacy `options_page` and MV3 `options_ui` declarations are
schema-validated and bound to the exact authenticated page resource and option
flags; they no longer survive as unmodeled authority. Dynamic optional-grant
requests now traverse one bounded native
prompt per admitted runtime, a Shell-owned consent projection, an exact
generation-bound service mutation, and native settlement; absent, stale,
over-capacity, or unavailable paths deny rather than grant. This extension
grant broker is distinct from page-origin permission events; camera/microphone
prompts now have their own Store-backed Shell policy and UI.

Profile-wide extension safe mode and exact-site pause now use a separate
durable policy aggregate rather than rewriting per-install grants. Profile
schema 13 stores one monotonic pause revision plus at most 128 canonical HTTP
or HTTPS DNS/IPv4 whole-host denials. The Store loads that policy in the same
SQLite snapshot as the install/grant cohort; its revision and digest enter the
runtime fingerprint and the native grant snapshot. Changed policy writes are
refused while any profile runtime ownership row exists. The serialized service
captures the bounded live-key cohort, retires the complete profile, commits one
policy CAS, and restores only prior live runtimes; resuming safe mode
deterministically activates enabled installs within the existing global runtime
ceiling. Definite pre-commit refusal restores the old cohort, while uncertain
commit quarantines management until restart.
Core URL decisions intersect the same policy before durable host or transient
`activeTab` authority reaches the engine, so a native denied dictionary is not
the sole enforcement layer for Zephium-owned scripting brokers.

Privileged chrome echoes only the policy revision and a boolean. Shell derives
the current site's scope from its own focused committed URL; URL, host, path,
query, and browsing data never cross the IPC command. The Extensions Center
exposes profile pause and current-site controls and reprojects them on active
tab/committed-URL changes. Unsupported schemes and IPv6 have no site toggle.
One native gate gives a context a broad
`http://*/*` grant and an exact explicit `http://127.0.0.1/*` denial in the
same complete replacement. WebKit retains both dictionaries, reports the
overlapped broad pattern as implicitly denied, blocks real content-script
execution on the exact host, and keeps a different HTTP host granted. Removing
the denial restores execution without changing controller-owned script
inventory. The authenticated native and brokered product gates additionally
commit the policy through Store/service, rebind the real runtime, prove absence
on a new denied navigation, remove the denial, and prove execution returns.

Installed-package updates compare the exact current grant root and reviewed
compatibility map with the authenticated replacement before native retirement.
New required API/host authority or a newly introduced degradation produces one
bounded Shell-retained review; the old package and runtime remain live while
the review is visible or dismissed. Privileged chrome receives only an opaque
subscription-local review token plus bounded browser-owned display data. An
approval reauthenticates the catalog-set digest, replacement package, profile
catalog/install/grant revisions, manifests, and changed cohort inside the
serialized service. Store then commits the package replacement and all exact
replacement-required grants as one transaction and one grant revision. It
preserves only still-declared optional grants and never enables new optional,
file, or private authority. Stale or contradictory reviews conflict without
mutation; uncertain commits enter restart reconciliation rather than replaying
consent. Permission-neutral and non-degrading updates retain the automatic
atomic path.

Uninstall is also a cross-system transaction rather than a Store-row delete.
The service first loads the profile's durable native-namespace obligation and
reauthenticates the current package's Chromium identity, then retires every
regular/private runtime owner. When a macOS namespace exists, the unique host
factory may reopen its deterministic controller without loading an extension,
fetches the bounded record cohort, selects exactly one matching 32-byte native
identifier, removes all three WebExtension data types for only that record,
and polls local/synchronized persistent bytes to zero. Other extension records
are validated and preserved. Only an `Erased` or `NotPresent` settlement
allows Store to delete the install and grants; native timeout becomes
outcome-unknown and blocks further management writes until restart, while a
definite pre-erasure refusal restores the retired runtime. The authenticated
native and brokered product probes write real `storage.local` data and require
this erasure before uninstall, clean service shutdown, and Store/repository
reopen. Reinstall therefore cannot recover the prior extension-origin data.

Remaining release gaps include product-sealed distribution endpoints, the
complete permission and API matrix, quotas, complete
cross-process release-build resource evidence, and endurance. Older admitted
macOS versions and Linux require a Zephium
compatibility runtime only after per-principal world/handler isolation, exact
match enforcement, protected-script installed state, and native hostile tests
pass. Windows has two separate candidates: curated MV3 packages through a
production wrapper around
`AddBrowserExtension`, whose environment-level enablement is a startup-time
decision, and a CDP isolated-world probe for first-party userscripts. The CDP
probe is excluded from normal product builds and has no page-world fallback;
its lifecycle, removal, navigation, resource, debugger-coexistence, and live
performance gates remain open.

`cargo xtask measure-macos-extension-product` is the non-shipping optimized
macOS product-path measurement gate. It rejects ambient internal authority,
enables the sealed fixture and a separate measurement cfg only for the probe
crate graph, compiles private-filesystem operations without their debug
counters, runs the bounded process-usage conversion tests, and then executes
three alternating repetitions of both native and native-brokered authenticated
paths. Each measurement-only run compares a five-second live-extension window
with a five-second same-process control after the native extension owner has
retired while the same page and engine remain alive. These idle windows are
compile-time absent from the ordinary CI probe. Application code still
compile-time rejects the base fixture authority. Two consecutive arm64 macOS
26.6.1 campaigns on 2026-08-16 passed the full package-to-popup lifecycle. The
native path measured 79--83 ms authenticated startup, 257--291 ms profile-view
creation, and 276--288 ms popup presentation; the brokered path measured
78--79 ms, 246--261 ms, and 282--291 ms respectively. Across all four short
processes, `RUSAGE_SELF` reported 92,438,528--93,732,864 bytes peak main-process
RSS and 601--660 ms combined user/system CPU. These are preliminary,
machine-local observations, not release budgets: `RUSAGE_SELF` excludes WebKit
helper-process RSS and the campaign does not measure steady-state idle wakeups,
battery impact, tab-scale behavior, or 24-hour endurance.

A three-pair arm64 macOS 26.6.1 campaign on 2026-08-17 also passed. Native
measured 75--82 ms authenticated startup, 262--266 ms profile-view creation,
270--294 ms popup presentation, and 94,240,768--94,781,440 bytes peak
main-process RSS; native-brokered measured 76--83 ms, 260--270 ms, 275--291 ms,
and 94,273,536--94,502,912 bytes respectively. The live-extension intervals
consumed 155--171 ms native and 154--168 ms brokered main-process CPU versus
102--104 ms and 101--103 ms after runtime retirement. They also recorded
1,304--1,379 and 1,313--1,361 additional involuntary context switches versus
their sequential controls. This establishes a repeatable measurement gate and
a measurable live-runtime delta; it does not yet attribute cause or define an
acceptable product budget. The active AppKit harness, sequential control,
machine contention, WebKit helper processes, idle wakeups, and energy impact
still require clean release-runner and process-family instrumentation.

A three-pair optimized campaign on 2026-08-19 passed after the packaged
install/update/options lifecycle landed. Native measured 78--87 ms
authenticated startup, 268--318 ms profile-view creation, 287--297 ms popup
presentation, and 93,601,792--94,863,360 bytes peak main-process RSS;
native-brokered measured 75--88 ms, 266--268 ms, 284--291 ms, and
93,716,480--94,666,752 bytes respectively. Five-second live-runtime windows
used 177--186 ms native and 183--190 ms brokered main-process CPU, versus
119--141 ms and 115--120 ms after retirement. The delta remains measurable and
is not yet a release threshold; the probe intentionally reports only
`RUSAGE_SELF`, so WebKit helper RSS/CPU and energy remain outside this evidence.

A three-pair optimized campaign on 2026-08-20 passed after the mixed native /
native-brokered catalog and deterministic extension origin landed. Native
measured 120--142 ms authenticated startup, 206--254 ms profile-view creation,
275--305 ms popup presentation, and 90,832,896--92,209,152 bytes peak
main-process RSS; native-brokered measured 122--126 ms, 214--221 ms, 280--309
ms, and 91,406,336--92,274,688 bytes respectively. Five-second live-runtime
windows used 202--205 ms native and 204--216 ms brokered main-process CPU,
versus 135--154 ms and 144--156 ms after retirement. The 51--72 ms paired CPU
delta remains visible after origin stabilization. These machine-local runs are
release-path regressions and trend evidence, not permission to retain disabled
contexts or a substitute for process-family RSS/CPU, idle-wakeup, energy, and
endurance budgets.

A three-pair optimized campaign on 2026-08-21 passed after profile execution
policy, hostile page/cross-extension denial, and exact options-view crash
cleanup landed. Native measured 120--144 ms authenticated startup, 220--241 ms
profile-view creation, 286--303 ms popup presentation, and
96,010,240--97,042,432 bytes peak main-process RSS; native-brokered measured
117--139 ms, 212--223 ms, 267--290 ms, and 95,911,936--96,714,752 bytes.
Five-second live-versus-retired main-process CPU deltas were -7--26 ms native
and 22--50 ms native-brokered. The reversed native pair demonstrates the noise
floor of this sequential machine-local measurement; these ranges are trend
evidence, not budgets or a system-wide energy/process-family claim.

`cargo xtask measure-macos-process-family --bundle-id ID
--duration-seconds N` is the complementary packaged-application sampler. It
binds exactly one running LaunchServices application and refreshes its bounded
process coalition throughout the campaign. This is essential on macOS because
WebKit WebContent, networking, GPU, and auxiliary XPC services are re-parented
to launchd and therefore are not discoverable through a parent-PID walk. Each
sample reads `proc_pid_rusage(RUSAGE_INFO_V4)` under the process UUID and start
time, converts Mach CPU time through the current kernel timebase, accumulates
monotonic CPU/wakeup/I/O/instruction counters across helper churn, and records
simultaneous resident and physical-footprint peaks for the application,
WebContent, networking, GPU, and auxiliary roles. Bundle-ID ambiguity, a changed
root identity, an oversized/duplicate coalition, unreadable live members, and
counter rollback all fail the campaign. Output is one versioned JSON object on
stdout plus a bounded human summary on stderr, so release infrastructure can
retain raw evidence without granting measurement authority to product code.
The V4 billed/serviced energy fields remain explicitly raw counters; release
energy budgets require a calibrated supported-machine campaign rather than an
invented unit conversion. A successful short attachment proves only sampler
integrity, not a browser resource budget.

One ten-second arm64 macOS 26.6.1 attachment on 2026-08-24 exercised the release
staging application with one enabled extension after native commands and
context-menu routing landed. Six processes reached 118,133,224 bytes peak and
118,067,688 bytes terminal aggregate physical footprint; the observation
interval accumulated 2.72 ms user CPU, 1.07 ms system CPU, 12 package-idle
wakeups, and 15 interrupt wakeups. Peak application and combined two-WebContent
physical footprints were 35,832,888 and 55,920,224 bytes respectively. This is
a correctly attributed short sample, not a before/after causal claim or release
budget; clean-runner multi-extension, churn, energy, tab-pressure, and endurance
campaigns remain required.

### macOS extension handoff (2026-08-24)

The reusable macOS implementation boundary is stable enough to branch Windows
work without freezing ordinary browser development. This means the current
declared support contract is implemented, fail-closed, resource-bounded, and
green; it does not mean every Chrome API, third-party extension, or external
release dependency is finished.

| Area | Current macOS state | Work that deliberately remains outside the stable handoff |
|---|---|---|
| Package authority, install lifecycle, updates, rollback, revocation foundations, uninstall, restart, profile deletion | Implemented and source/native gated | Provision the production KMS/origin/catalog/rollout authority; ordinary builds remain intentionally inert until then |
| Browser UX | Extensions Center, reviewed install, required/optional grants, enable/disable, options, toolbar/popup, profile safe mode, exact-site pause, update/degradation consent, and uninstall are implemented | Physical VoiceOver and supported-machine packaged workflow matrix |
| Native runtime security | Per-profile data-store/controller binding, stable principals, hostile page/peer denial, protected scripts, undeclared-resource denial, discarded-tab non-resurrection, exact erasure, bounded commands/context menus, and crash cleanup are implemented | Signed-app renderer-crash injection and long hostile/endurance campaigns |
| Verified staging cohort | Revision 9 carries Vimium 2.4.2 and Dark Reader 4.9.129 with revisions 7/8 as immutable rollback; artifact bytes, classifications, and reissue evidence are exact | Add a publisher-supported authenticated cloud/security extension and define operational support windows |
| API compatibility | The declaration table below is authoritative; live alarms work, while context-restart persistence is disclosed as degraded | Notifications, idle/system-lock, managed storage, offscreen, arbitrary native messaging, blocking request mutation, and broader browser APIs remain absent or degraded |
| File/private contexts | File execution is negative-gated and the unavailable control cannot be forged; native private-store isolation is proven | No product file-access claim and no extension-enabled private-window claim until WebKit execution and a separate product runtime pass |
| Password managers | Platform diagnostics, popup/content/background primitives, sandbox replacement primitive, explicit failure classifications, and a signed stock 1Password run now cover authenticated vault UI, inline fill, synthetic save, publisher-app item update plus browser-extension sync/refill, extension-mediated passkey registration, desktop-unlock recovery, forced background reload, completed-profile restart, and an explicit degraded HTTP Basic-auth contract | No password manager is Verified: browser-inline changed-password update, passkey assertion, Apple browser-passkey entitlement approval, longer endurance/resource budgets, and publisher/legal gates remain |
| Performance/release evidence | Optimized product campaigns and a real process-family sampler are implemented; short release samples are recorded | Dedicated clean-runner budgets, maximum-cohort/tab pressure, energy calibration, sleep/wake, multi-profile, and 24-hour endurance |
| External compatibility mode | Package-neutral offline transformation and authenticated acquisition primitives exist | Chrome Web Store/AMO/Safari/file/developer acquisition UX, legal adapters, diagnostics, and public compatibility policy are not shipped |

Windows now has a compile-time-complete native ownership/admission seam around
WebView2's `ICoreWebView2Profile7` APIs. Direct builder enablement and unmanaged
extension paths remain refused. An authenticated lifecycle reservation may
instead create one 1x1 hidden, unfocused, non-inspectable, page-inert
controller through Wry's pre-initialization startup gate. The gate binds the
exact UDF, environment, controller-reported environment, non-private profile,
and canonical COM identities before Wry initializes or navigates the view. The
controller is explicitly closed immediately; its transient native-resource
lease transfers to the existing teardown-debt registry if close is not proven.
Only the attested environment/profile pair remains, alongside the existing
browser-process exit and runtime-update observers.

Environment mode is not inferred from either native map. A separate
allocation-free eight-slot registry, sharing the native process-group ceiling,
records ordinary `Disabled`, extension `Preparing`,
extension `Ready`, or sticky extension `Failed`. The host records ordinary
disabled environments when their exact process capture succeeds. Extension
profile authority is published only after environment/profile attestation,
controller hardening, explicit close, cleanup-debt collection, and the caller's
deadline all succeed. Any failure removes the provisional profile and records
`Failed`; a captured environment without matching mode state, or a `Ready`
state without both environment and profile authority, is an invariant failure.
`Preparing`/`Failed` refuses later content and extension preflight until exact
process exit or restart clears that generation.

Activation consumes the authenticated package-root lease only after the
durable lifecycle has entered its native phase. It checks every asynchronous
HRESULT, retains the returned extension object, reads back its exact Chromium
identifier and enabled state, and verifies a bounded complete profile
inventory. Removal and crash recovery use the same deadline-bounded UI message
pump and can mint absence only from a complete exact-profile inventory in
which the reservation's owner is absent. A late callback fail-stops subsequent
extension and content-view admission while retaining its bounded COM owner.
Every extension-enabled content controller must then pass a pre-initialization
comparison between the complete native inventory and the exact published
runtime-owner cohort; an extra, missing, duplicate, timed-out, or mismatched
owner prevents page initialization. Profiles without product authority still
construct the default extension-disabled environment and allocate no native
extension host.
The published cohort includes both fresh activation reservations and recovered
reservations reconstructed after restart. A recovery row contributes an owner
only when its catalog-expected and previously adapter-observed Windows IDs
resolve unambiguously and the published ownership evidence matches that exact
ID. Conflicting anchors, duplicate owners, or capacity overflow fail the whole
registry invariant instead of projecting an empty cohort that can never match
WebView2.

This is not yet a Windows product-support claim. A profile whose ordinary
extension-disabled WebView2 environment is already running cannot switch the
environment option in place; the current adapter returns a retryable native
pre-entry `RestartRequired` refusal rather than enabling globally or silently
mixing modes. That closed reason survives service activation, install/update/
enable/grant settlements, Shell's bounded operation ledger, generated IPC, and
the Extensions Center, which tells the user to restart Zephium. It is not a
restart command and grants no permission to tear down a live profile behind
Shell's tab/session authority. Exact in-process reconstruction remains future
work; restart re-enters ordinary authenticated startup before any content view.

The sealed staging and local-lab authorities still carry only reviewed macOS
profiles, and the desktop build boundary still limits those feature graphs to
macOS. Non-macOS configuration additionally resolves to no distribution target
rather than relabelling macOS classification as Windows evidence. A
`WindowsNative` catalog profile must land atomically with its own reviewed
classification/admission digest and Windows runner evidence. The Windows
toolbar/popup host, packaged live install/restart/remove/crash probes, and
measured process-family budgets remain release gates. Linux still needs
the package-neutral compatibility runtime built on the already-proven
principal and injection boundaries.
Distribution reuses the blocker stack's generic package-authority model—fixed origins,
authenticated content-addressed packages, monotonic candidate/current/previous
state, staged activation, rollback, and crash recovery—without coupling
extensions to blocker compilation.

The shared meta schema carries a separate bounded native-ownership journal.
Each path-free row binds one `(profile, install, regular/private context)` to
the exact package, active/rollback catalog-set digest, store/grant revisions,
backend target, persistent native incarnation, intent, and conservative phase.
Native backends also bind an optional fixed-width, backend-specific owner
identifier once it is observed. The identifier is immutable for that
incarnation and is required before a native row may claim `NativeOwned`;
identityless `NativeMayOwn` and legacy cleanup rows remain conservative rather
than guessing an owner identity. Compatibility runtimes cannot persist a
platform-native identifier.
A complete-cohort global CAS must commit `NativeMayOwn` before any native call
that could create, retain, or remove an owner. `NativeOwned` is preserved
exactly on disk but treated as may-own after restart; a row clears only after
definite native absence and subordinate resource release. Unknown, corrupt,
duplicate, or over-limit state fails the complete load. The journal survives
profile ancillary degradation/removal and blocks profile deletion until its
rows settle. Session recovery rejects new `Begin` operations and every
acquire-directed `Transition`, while exact release-directed `Transition` and
`Clear` operations remain available to retire existing native owners. The
bounded extension service now owns this persistence and ordering seam. Its
serialized actor joins the Store projection, authenticated repository lease,
native-host authority, profile retirement, and shutdown drain. It refuses clean
worker evidence while a worker-owned runtime or attached authority remains
unresolved or accepted/completed command counts differ. That evidence proves
only worker/resource drain, not durable-journal or native-owner absence.
The move-only application lifecycle is also the only app-facing activation and
exact-runtime retirement ingress. Inputs are identity selectors, not authority;
successful settlements carry the complete active-profile routing cohort from
the same serialized worker turn. Shell therefore never guesses whether a
sibling runtime still keeps a profile active, and cloneable status handles
remain observation-only.
The macOS native adapter is enabled and the acquired service ingress exists
behind an opt-in feature, but sealed product transport/provisioning remains
empty and the Windows/Linux adapters remain unavailable, so this coordinator
is not a release-enablement claim. Reconstruction also rejects
unreachable clock histories: operation and incarnation high-water
marks are equal, every row binds the same operation/incarnation, phase and row
revision agree, and with `C = high_water - live_rows` plus
`S = sum(live_row_revisions)`, the global revision is within
`1 + S + 3C ..= 1 + S + 7C`. The upper bound includes the one-shot identity
attachment that may be durably fenced before a native call. These internal
consistency checks prevent clock
reuse from a torn or corrupted cohort; they cannot detect a coherent rollback
of the entire database without an external anti-rollback anchor.

The product-sealed catalog authority now distinguishes payload origin at the
capability boundary. `AdmittedBundledCatalog` can contain only `BundledTree`
rows; `AdmittedAcquiredCatalog` can contain only exact `AcquiredZip` rows and
requires every row to carry a complete expected Chromium key identity. Both
paths authenticate the same exact active catalog anchor and product-owned
license policy, but their non-cloneable witnesses are nominally distinct and
cannot cross into the wrong materializer. Acquired manifest admission has the
same separate typed ingress. This adds no catalog, download, or startup work to
ordinary builds: production sealed provisioning remains empty, arbitrary
caller-parsed catalogs remain non-authoritative, and an acquired witness grants
no network, archive, repository, profile, or native-runtime authority.
Consumers that accept either active representation use `AdmittedActiveCatalog`:
the authority checks length and digest, parses canonical metadata, binds policy,
and classifies the homogeneous payload inventory exactly once, then returns the
original nominal witness as a closed enum variant. Mixed bundled/acquired
inventories fail admission. This dispatch does not create a representation-
neutral materializer or permit fallback from one payload parser to another.

The extension repository's materialization schema v3 retains the subordinate
side of that cross-store join. Every durable package pin records the complete
`(profile, install, regular/private context)` owner key, the exact catalog-set
record digest, its historical active/rollback role, the exact package-record
digest, and Store's persistent native incarnation. Repository generation is a
separate transition CAS clock and is never reused as native incarnation.
Store incarnation therefore supplies reopened ABA protection, while a consumed
materialization runtime plus its generation and directory identity protect an
in-process transition. Candidate/current/previous remain the three atomic set
slots; all owner pins retain their complete named set, with at most one
additional owner-only drain set. Recovery validates the package as a row of
that exact set and retains the whole set closure instead of reconstructing a
partial drain from catalog anchors. The durable owner inventory is bounded to
1,024 exact context keys (64 profiles x 8 installs x 2 contexts). That is
crash-recovery capacity, not private-browsing permission: acquisition remains
explicitly regular-only until the private runtime contract is implemented.
The unreleased schema-v2 pin shape lacks these join fields and fails closed.
Cleanup after repository reopen is a separate, access-free path. It accepts
only a Core-minted Store binding for `Release/NativeAbsentReleasePending`,
grants no package or runtime access, and rejects a live lease for the same
owner in the current repository-open epoch. After a classified frontier
preflight proves there is no in-memory or physical object-stage residue, an
absent owner settles before catalog-set lookup so crash-before-pin replay
remains idempotent. A present owner must match the complete persisted
owner/set/role/incarnation identity
and the exact set-row/package/backend join before the existing exact removal
transition may run.
Interrupted package builds settle at a source-free boundary before any new
bundled-resource adapter callback. The package-record final is published last
and is the durable commit marker for its complete object closure. While that
marker is freshly proven absent, recovery may remove only the exact stages
owned by the one durable intent, reprove both marker and stage absence, and
abort the intent. Once the marker is present, abort is permanently forbidden:
the repository rereads its own authenticated catalog object, re-admits the
catalog and stored manifest through product policy, freshly verifies the
index, legal artifact, tree, and package record, and consumes a role-specific
active or rollback completion transition. The public settlement API accepts no
source, and a retry of that same catalog/package/runtime returns after
settlement without consulting its source. Missing, corrupt, or mismatched
post-marker state fails closed and remains unrepairable by the package source.
Every package-record final discovered during recovery must be rooted by either
the exact sole build intent or the completed ledger; unrooted commit markers
are ambiguous durable state, not inert garbage.
The physical recovery inventory currently permits at most eight sealed
catalog-set finals. The repository implements bounded, pin-rooted,
crash-recoverable closure garbage collection ordered after interrupted-build
settlement and preserving every candidate/current/previous and owner-pin root.
The serialized service schedules at most one collector batch from Shell's
existing one-minute maintenance heartbeat. A process-local atomic permit
coalesces duplicate wakeups before mailbox admission, the worker applies the
same absolute deadline and repository serialization as other operations, and
`more_garbage` never creates a hot follow-up. Callback loss releases the permit
with the command, transient pre-commit filesystem outages wait for a later
heartbeat, and integrity failures stop further periodic admission until
restart. The inert lifecycle reports maintenance unavailable before Shell
allocates a callback, so an extension-free launch still creates no worker,
timer, repository, or maintenance work. Repository tests prove real garbage
mutation/recovery; an authenticated service test proves production admission,
coalescing, an empty fresh-inventory turn, and clean ordered shutdown. A
repeated-catalog endurance campaign remains release evidence rather than an
assumption derived from those component gates.
Core package-pin bindings are structural joins, not proof that a Store row is
still current. Fresh acquisition consumes its move-only binding into the live
repository lease; the lease exposes eligibility only by borrow and destroys
the acquisition authority when converted to cleanup-only release. The runtime
service is the exclusive owner of both the Store journal projection and
`ExtensionRepository`: immediately before every repository mutation it
revalidates the exact current row and CAS in its serialized actor turn. Neither
raw bindings nor repository methods cross its bounded mailbox. Internal-only
authenticated authority tests exercise real Store and repository state through
activation, exact retirement, profile retirement, shutdown, and post-drain
Store/repository reopen; stale or caller-synthesized rows cannot reach a
repository transition. These tests use a bounded native fake and do not
substitute for a live platform adapter.

Durable package identity is representation-exact. A bundled authenticated tree
is tagged `BundledTree` and carries no synthetic archive evidence; an acquired
ZIP is tagged separately and binds both a non-zero bounded byte length
and its SHA-256 before materialization. The profile schema stores the same tag
and nullable/exact ZIP evidence redundantly in install and grant rows, and the
bounded codecs reject any disagreement. Profile schema v12 is a deliberate
fail-closed epoch: the unreleased v9-v11 shape recorded only an archive digest,
so migration preserves the catalog revision and monotonic install-ID floor but
invalidates those inexact install/grant rows. They must be reinstalled through
the exact package authority and their old identities can never be reused.

`zephium-extension-acquisition` owns the first acquired-package boundary and is
linked into the service only by the opt-in `acquired-packages` feature, never by
the ordinary inert product graph. It accepts
only CRX3 bytes whose signed developer key derives the product-expected
Chromium id and whose inner ZIP byte length and SHA-256 exactly match an
`AcquiredZip` catalog identity. The production-shaped release-row constructor
also requires the complete 256-bit CRX developer-key digest from the row's
Chromium identity; the shorter derived extension id is not treated as complete
key authority. Chrome Web Store packages signed by legacy 1024-bit RSA
developer keys remain admissible only when that exact proof derives the signed
CRX id; unrelated RSA proofs retain the 2048-bit floor, and ECDSA behavior is
unchanged. Before constructing the ZIP parser, an
allocation-free terminal-record preflight bounds the entry count and central
directory. Complete preflight then admits only stored or deflated ordinary
files/directories, canonical ASCII portable paths, one root `manifest.json`,
and the existing per-file, aggregate-tree, entry, depth, and retained-memory
ceilings. It rejects ZIP64/multi-disk framing, encryption, links and special
files, duplicate/case/device/file-directory aliases, ambiguous local headers,
and overlapping payload regions. Files can leave the boundary only through a
bounded streaming copy that reaches EOF and therefore checks decompression,
exact length, and ZIP CRC; the boundary opens no paths itself. A Chrome-produced
empty deflated directory is accepted only with a zero declared expansion and at
most 64 compressed bytes, and its reader must reach EOF without producing a
byte. Every successful file copy returns one non-cloneable receipt containing
the digest of the exact byte
prefix the destination writer accepted. An exact, duplicate-free receipt cohort
derives canonical tree-index bytes, reparses them through the shared bounded
canonical index boundary, and retains both forms under one named memory
ceiling. Binding that completed tree to a release row rechecks the acquired
payload, full CRX key digest, derived id, manifest digest, tree digest, index
digest/length, file count, and aggregate bytes.

The offline CRX probe materializer consumes this same implementation and
completes the stream receipt after per-file synchronization, so diagnostic and
product definitions of safe extraction cannot drift. The repository's opt-in
`acquired-packages` boundary now streams each authenticated archive file once
directly into a create-new private tree, retains only the bounded root
manifest, binds the complete receipt cohort to the release row, bottom-up seals
the stage, and then independently enumerates and re-hashes every closed file.
When the declared content-addressed tree already exists, the same archive
authentication, decompression, CRC, digest, and receipt work runs once with
non-manifest bytes sent to a zero-retention sink; no duplicate tree is built.
No stream callback or receipt alone is publication authority, and the ordinary
product graph does not link the archive stack. The prepublication tree uses one
canonical `.acquiring` name distinct from both content objects and durable
build stages. The outer catalog high-water transition completes before this
name is created, because that transition reopens the materialization namespace
and correctly removes every unowned acquisition stage. The subsequent package
build intent may preserve exactly one acquisition stage only when its digest
and `AcquiredZip` record match. A source-free restart with no package-record
commit marker removes that exact stage and aborts the intent; a marker-committed
restart reconstructs acquired catalog and manifest authority from repository
objects and completes only after re-verifying the entire final closure. Missing
controls, multiple stages, a bundled or mismatched intent, or an object/stage
digest alias fail closed without deleting ambiguous evidence.

Acquired publication consumes the stage with the same-parent no-replace
primitive, persists the canonical index and authenticated legal artifact, and
publishes the package record last. Only then may the completed ledger advance.
On macOS, publication consumes the sealed root under the existing namespace
operation lock/no-path-pins gate, changes only that root from `0500` to `0700`,
performs one native `RENAME_EXCL`, immediately restores `0500` and syncs, and
freshly validates the destination identity, mode and parent before returning a
capability. Descendants remain sealed. This is an explicit private transition,
not a claim that the root stays read-only during rename: macOS 15 requires
source-directory write permission even for a same-parent rename. A refused
rename returns the source only after reseal, sync and fresh unchanged-boundary
proof; ambiguous mode/rename/reseal/sync settlement quarantines the namespace.
No check-then-rename, overwrite, retry or fallback is used. A crash can leave an
owner-only `0700` stage or destination; dedicated sealed admission rejects it.
The existing package-record-last protocol and fresh final-tree verification
remain authoritative, while mixed-mode stage cleanup remains explicit recovery,
never an implicit seal or a successful publication receipt.
The public materializer accepts borrowed CRX bytes and the existing
capability-limited legal-resource adapter; it performs no network request,
accepts no host path, and grants no install, profile, activation, or native
controller authority. The ordinary build remains archive-free unless the
`acquired-packages` feature is explicitly enabled.

The CI-only acquired-package gate replaces the product-sealed active fixture
under a second debug-only compiler configuration; it does not add shipping
catalog bytes or product transport authority. A self-authored MV3 archive is signed at
test time by a fixed test-only developer key and traverses the public catalog,
CRX authentication, extraction, manifest admission, repository publication,
selection, package lease, resource read, and recovery path. The repository gate
requires exact in-process and post-reopen replay, clean recovery from a
legal-source failure after its callback, marker-committed crash completion with
neither CRX bytes nor a legal callback, source-free candidate promotion, and
fresh post-reopen install-candidate and lease authentication. A second gate
runs 16 real Store/repository service scenarios against the acquired
representation. One starts from an empty repository and traverses the bounded
service ingress, exact replay, source-free catalog activation, management
candidate, install, runtime publication, shutdown, and post-reopen audit. The
remaining scenarios cover hydration, runtime planning, activation/publication,
grant rebind, management, retirement, reconciliation, profile cleanup, and
shutdown. Both inventories are counted
explicitly by `cargo xtask ci`; the acquired configuration requires the base
internal authority, and that combined authority is rejected from optimized
builds and application linkage. The ordinary authority fixture remains
independently exercised. `acquired-packages` remains opt-in, so the archive
stack is absent from the ordinary inert product graph.

Permanent ceilings include Manifest V2, persistent backgrounds, blocking
`webRequest` on public WebKit, devtools extensions, browser-identity overrides,
native messaging and unbounded extension storage in the initial target, and an
open catalog. Unsupported or
degraded APIs must fail deterministically and be disclosed; they are never
silently approximated. Permanent security invariants and release gates live in
[`security-model.md`](security-model.md); implementation sequencing is not part
of this architecture contract.

The pinned Bitwarden Core `browser-v2026.7.0` contract treats `webRequest` as
required. Its [exact background implementation](https://github.com/bitwarden/clients/blob/browser-v2026.7.0/apps/browser/src/autofill/background/web-request.background.ts)
uses that namespace only for HTTP Basic-auth autofill: `onAuthRequired` is
registered with `asyncBlocking`, while completion/error observation only
retires pending request identities. The macOS live gate now proves that WebKit
parses the permission, exposes `chrome.webRequest`, starts the MV3 background
worker, accepts that exact listener-registration shape, and invokes the
non-blocking `onCompleted` listener after the already-registered worker observes
a real HTTP navigation in an existing regular product tab. The content script
must also run in that document before the callback observation is accepted.
This proves observation, not blocking semantics: WebKit does not provide the
blocking behavior the Basic-auth workflow requires, and the upstream
`webRequestAuthProvider` declaration is filtered by the native parser. The
reviewed macOS classification is therefore:

| Bitwarden surface | macOS native classification | Evidence / boundary |
|---|---|---|
| MV3 background startup with declared `webRequest` | Compatible | Persistent product-profile live background registration gate |
| Toolbar action and declared popup page | Native-brokered | Exact action/icon projection, privileged gesture admission, native transient popup presentation, bounded failure UX, resource admission, and teardown are implemented; release-build end-to-end UX/RSS/endurance evidence remains required |
| Non-blocking request observation | Compatible on the exercised runtime | Exact upstream-shaped `onCompleted` registration observes a real regular-tab HTTP navigation; the supported macOS floor still requires the same release-runner evidence |
| HTTP Basic-auth autofill | Degraded | Startup survives, but the required blocking callback semantics are unavailable |
| Network blocking/modification through `webRequest` | Unsupported | Zephium never emulates synchronous request control through a generic bridge |

The same product-shaped background gate records a closed runtime-namespace
inventory, not just the permissions WebKit retained while parsing. On the
current native gate WebKit exposes `alarms`, `commands`, `contextMenus`,
`scripting`, `storage.local`, `tabs`, `webNavigation`, and `webRequest`, while
`idle`, `notifications`, `offscreen`, `sidePanel`, and `storage.managed` are
absent. A namespace being present is not a behavioral compatibility claim;
each API that affects the pinned workflow still needs an exact live gate. The
product-tab gate additionally proves a main-world file injection when the
literal `"MAIN"` value is supplied, a top-level `webNavigation.onCommitted`
callback, execution through an opaque dynamic web-accessible-resource URL, and
the candidate sealed-Blob sandbox boundary. It also verifies alarm create/read/clear,
exact manifest-command enumeration plus native-to-background dispatch, and
context-menu create/update/native projection/remove.
WebKit omits the `ExecutionWorld` enum even though literal `"MAIN"` main-world
injection succeeds. The typed runtime classification is therefore
`literal-main-only`, not main-world unavailability; a pinned extension that
dereferences the absent enum still needs an exact reviewed source adaptation.
A manifest-declared sandbox page,
including one placed in an explicit `sandbox="allow-scripts"` iframe, retains
both a `webkit-extension` origin and the `chrome` API. The viable adapter gate
therefore removes the leaf HTML and privileged intermediate page from public
resources, exposes only an inert payload, and lets the authenticated content
script create a Blob-backed `allow-scripts` frame. The live gate observes a
`null` message origin, no `chrome`, `runtime`, or `storage.local`, and no parent
DOM access; direct leaf exposure is absent. This proves the platform primitive,
not the still-unimplemented pinned-source transform. The release runner at the supported
OS floor must reproduce the closed inventory and these exact behavioral
classifications before they become floor-wide claims.

The native gate also carries a separate Zephium-owned ordinary-extension
contract. It declares `bookmarks`, `favicon`, `history`, `search`, `sessions`,
`storage`, and `webNavigation`, mirroring only the browser-owned API delta present in the
official [Vimium manifest](https://github.com/philc/vimium/blob/master/manifest.json);
it does not download or execute Vimium and is not a package-compatibility
claim. On the exercised runtime WebKit parses that MV3 manifest with no errors,
publishes `storage` and `webNavigation` in `requestedPermissions`, and exposes
the `action`, `runtime`, `storage.local`, `storage.session`, `storage.sync`,
`tabs`, and `webNavigation.onCommitted` JavaScript controls. The storage
session get/set/access-level and storage sync get/set methods are functions, as
is `action.setIcon`. However, `webNavigation.onHistoryStateUpdated` and
`webNavigation.onReferenceFragmentUpdated` are absent, while
`bookmarks`, `history`, `search`, and `sessions` are `undefined`. The
`favicon` token has neither a native permission key nor a namespace. The probe
uses a distinct nonpersistent controller and store, applies only the exact
native `storage` and `webNavigation` grants, validates controller/store
binding, and requires its view, context, controller, and store to release
during the same bounded teardown gate.
An independent product-tab behavior gate executes a real main-world
`history.pushState`. `tabs.onUpdated` accepts registration but emits no
same-document URL update on the exercised WebKit runtime. A content-script
message reaching the worker does carry a non-negative integer `frameId` plus
HTTP `sender.url` and `sender.tab.url`; the generic endpoint therefore has
native sender identities to bind, but cannot inherit same-document observation
from `tabs.onUpdated`.

This closes an important admission ambiguity: parser acceptance is not runtime
support. The current `macos.wkwebextension.v1` grant schema therefore rejects
unknown non-representable permissions. The known broker-only `history` token is
more precise: an unrequested optional declaration may remain denied, while any
effective grant is product-prohibited. It is never silently dropped or treated
as supported. A distinct
`macos.wkwebextension-brokered.v1` compatibility profile now exists for exact,
reviewed package adapters. Both profiles use the same durable `MacosNative`
backend and native owner identity because they own the same WKWebExtension
controller/context resources; the profile is retained independently in the
authenticated package record and admitted manifest. It is not a fifth
persistence backend or controller namespace. A generic page-world or generic
native-messaging bridge remains prohibited. Packages that need broader emulated
APIs still belong on the separate macOS compatibility-runtime target. The
ordinary native profile does not classify Vimium as compatible. The exact
authenticated Vimium gate described below is a non-authorizing compatibility-
artifact result; it does not silently broaden the native profile or establish
its workflows. The separate brokered gate proves only its explicitly named
history, default-search, and most-recent-session compatibility slices;
bookmarks remain empty/read-only and notifications remain unavailable with an
explicit degraded classification.

Product contexts also bind one deterministic extension origin before load.
Both `uniqueIdentifier` and the host of
`webkit-extension://<chromium-extension-id>/` come from the
catalog-authenticated Chromium signing identity; the adapter verifies both
before and after controller load. WebKit's random default `baseURL` is never
accepted for a product runtime. Reconstructed contexts for one installed
package therefore keep the same extension-page origin, while the independently
verified per-profile `WKWebsiteDataStore` and controller preserve profile
isolation even when the public extension identifier is the same. The packaged
Dark Reader gate reads the exact deterministic popup origin after restart and
re-enable. Stable origin does not change WebKit's repeated `onInstalled`
behavior described below, so it is origin/persistence hardening rather than a
claim that the lifecycle degradation is closed.

WebKit does expose a narrower compatibility seam through
`WKWebExtensionControllerDelegate`, and the live gate now classifies it without
enabling it in ordinary product policy. A separate Zephium-owned extension
declares `history` and `nativeMessaging`, receives the one native grant only
inside the feature-gated probe, and loads the exact package-neutral read-only
history adapter. `chrome.history.search` sends the fixed internal identifier
`app.zephium.extension-broker.v1` and the closed recent-history operation. The
delegate compares both controller and `WKWebExtensionContext` identities before
reading the bounded string payload. The returned two-row response is strictly
validated and mapped into `HistoryItem` values before the page accepts the
round trip. Mutation methods remain absent. This proves the facade-to-host-to-
facade primitive; product authority still comes only from the distinct
brokered runtime profile and its operation witness.

The corrected persistent-port gate is bidirectional on the exercised runtime.
The delegate receives the exact context-bound connection and one extension
message; a retained native object reply is dispatched non-reentrantly on the
main queue, the extension's registered `port.onMessage` listener observes the
exact value, and both sides then disconnect. The native handler, port,
delegate, view, context, controller, and store all release. An earlier gate
incorrectly reported `accepted-unobserved` because its JavaScript never
registered an `onMessage` listener; that claim is withdrawn. The ordinary
native grant schema still prohibits `nativeMessaging`. A separate sealed
publisher-host requirement must bind the exact package, upstream Chromium ID,
host name, and macOS Team/signing identifiers before the publisher schema can
grant it; arbitrary native application names remain unavailable.

The first production-shaped operations on that new profile are bounded recent
history, default search, and restoration of the newest closed tab. The
extension side can send only `v1/history.recent/<limit>`,
`v1/search.default/{current,new}/<canonical-base64url-query>`, or the fixed
`v1/sessions.restore/recent` string to the internal application identifier.
History accepts `1..=100`; search accepts a nonempty UTF-8 query of at most
1,024 bytes and no control characters; the complete request is capped at 1,536
bytes. Native code joins the callback to the exact controller and loaded
context, resolves the exact published runtime, and consumes a move-only witness
for the operation's independently effective `history`, `search`, or `sessions`
grant before emitting a typed Shell request.

History performs a profile-scoped Store read through a bounded fair queue. It
considers at most the newest 4,096 rows, deduplicates by URL, validates URLs,
sanitizes titles, and returns at most 100 entries. Search enters Shell's normal
default-provider classifier and current/new-tab navigation path only when the
requesting profile owns the focused window; it carries no provider, arbitrary
URL, window, or native tab identity. Session restore consumes only the newest
regular tab owned by the focused profile and space from a browser-owned durable
cohort capped at 32. Unsafe URLs, incognito state, invalid ownership, control
characters, and invalid zoom are removed during canonicalization, and restore
always allocates a fresh item/native identity rather than reviving stale
authority. It does not expose session enumeration or cross-space restore.

The native reply is capped at 64 KiB, every request has a five-second watchdog,
and pending work is capped at 8 per profile and 32 process-wide. Context
retirement, timeout, shutdown, stale runtime settlement, overload, and malformed
responses all complete the retained one-shot callback with an explicit error.
Four extension history reads yield to a pending browser-owned history/favicon
read, so an admitted adapter cannot starve ordinary browser UX; search and
restore execute synchronously through the already-bounded Shell operations and
create no worker or polling loop.

This is not a complete `chrome.history` implementation or a compatibility claim.
The source-free product gate now runs the same sealed fixture in separate
ordinary and brokered processes. Ordinary authority keeps `history` and
`nativeMessaging` denied and must observe native-channel rejection. Brokered
authority grants only that exact pair, then proves authenticated repository and
service startup, native witness admission, a two-row profile-scoped Store read,
bounded JSON delivery to extension JavaScript, popup continuity, runtime
retirement, repository cleanup, and clean Store restart. The gate deliberately
does not instantiate the product Shell actor: it performs the real Store read on
a dedicated bounded worker, while Shell settlement, stale-result rejection, and
four-to-one queue fairness remain component/integration gates. The package-
neutral adapter and native WebKit seam are independently live-gated. The exact
Vimium artifact and its real history-dependent Vomnibar workflow now close the
first package-level behavioral gate; that result does not generalize to other
packages. Packaged release-build latency/RSS/endurance evidence remains
mandatory before Vimium is presented as compatible.

| Bitwarden surface | macOS native classification | Evidence / boundary |
|---|---|---|
| MV3 backup-localStorage through `offscreen` | Requires a reviewed Bitwarden Core adapter before release | The pinned entrypoint always selects [`OffscreenStorageService`](https://github.com/bitwarden/clients/blob/browser-v2026.7.0/apps/browser/src/platform/storage/offscreen-storage.service.ts) for MV3. Primary writes survive because the upstream [`PrimarySecondaryStorageService`](https://github.com/bitwarden/clients/blob/browser-v2026.7.0/libs/common/src/platform/storage/primary-secondary-storage.service.ts) settles both writes, but a primary miss reaches the absent API. Zephium must select a deterministic primary-only/recovery-compatible adapter in its sealed build and test migration, missing-key, and recovery behavior; it must not inject a page-world shim. |
| Idle and system-lock integration | Degraded | The pinned [`IdleBackground`](https://github.com/bitwarden/clients/blob/browser-v2026.7.0/apps/browser/src/background/idle.background.ts) returns immediately when the namespace is absent. System-lock vault timeout and idle-driven notification reconnect/disconnect are unavailable; ordinary timer-based vault locking remains a separate behavioral gate. |
| System notifications | Degraded | The pinned composition selects `UnsupportedSystemNotificationsService` when `chrome.notifications` is absent. In-extension auth-request flows may remain available, but OS notification presentation/click handling is unavailable. |
| Unlimited local storage | Unsupported in the initial target | WebKit can report per-extension stored bytes but exposes no quota setter. Zephium therefore refuses `unlimitedStorage` in both product grant schemas; the feature-only platform probe may exercise the native token, but no product runtime can remove WebKit's finite default quota without a separately reviewed bounded storage design. |
| Chrome side panel | Degraded | The pinned [`BrowserApi`](https://github.com/bitwarden/clients/blob/browser-v2026.7.0/apps/browser/src/platform/browser/browser-api.ts) capability-checks the namespace and makes side-panel operations no-ops. Zephium's native toolbar popup remains the primary extension UI. |
| Enterprise managed storage | Degraded to an empty read-only compatibility surface | WebKit exposes no enterprise policy. A package-neutral facade supplies `get()` with an empty frozen result and an inert `onChanged` event so optional enterprise features can initialize, while preserving any future native members. It never approximates managed policy with writable extension storage. |
| Native messaging | Native-brokered for exact sealed publishers; otherwise unsupported | `macos.wkwebextension.v1` prohibits the permission by default. A package-specific sealed publisher requirement may select the narrower publisher schema, after which the broker reauthenticates the live context, exact host name, upstream extension allowlist, fixed Chromium registration root, and Apple Team/signing identity before launching one bounded cold stdio worker. The corrected live gate proves bidirectional persistent ports. Arbitrary native hosts remain unavailable, and each password-manager integration still requires a real signed-app interoperability gate. |
| Programmatic main-world scripting | Native literal supported; pinned source requires a sealed adapter | `scripting.executeScript` injects the exact extension file into the product tab when passed literal `"MAIN"`, but WebKit exposes no `chrome.scripting.ExecutionWorld` enum. The sealed build must substitute the absent enum access without adding page-world privilege or a generic bridge. |
| Runtime ports | Registered routing compatible; pre-listener connection not queued | A Zephium-owned extension page opens a named port only after the MV3 worker registers `runtime.onConnect`; the worker receives the port and completes an exact message round trip. A separate connection created immediately before listener registration returns a port, then disconnects without `runtime.lastError` and is not delivered after registration. Extensions that depend on Chrome queuing that startup race require a reviewed compatibility decision; ordinary registered port routing does not. |
| Non-blocking top-level navigation observation | Compatible on the exercised runtime | A background `webNavigation.onCommitted` listener observes the real regular product-tab HTTP navigation. Frame/detail behavior remains outside this gate. |
| Dynamic web-accessible resources | Compatible on the exercised runtime | A content script resolves an opaque runtime URL for a `use_dynamic_url` resource; the page loads and executes the declared resource. The same isolated script places the exact URL of a present but undeclared extension file into page DOM; WebKit emits the error path and the private script never executes. Revocation across a complete browser session and multi-frame behavior remain separate gates. |
| Manifest sandbox pages | Sealed adapter primitive compatible; pinned transform still required | WebKit does not enforce the declared sandbox and also ignores the intended isolation when the extension URL is placed in an explicit sandboxed iframe. The live gate proves the reviewed replacement topology: an inert public payload is fetched by the authenticated content-script host and instantiated through a Blob-backed `allow-scripts` frame; the child has a `null` message origin, no extension APIs, and no parent DOM authority, while the privileged leaf URL is not public. The exact Bitwarden button/list bundle transformation and authenticated message flow still require deterministic source/build adaptation and hostile end-to-end tests. |
| Private extension-resource WebAssembly startup | Release-blocked on the exercised runtime | The pinned 7,378,704-byte SDK module is returned as `application/octet-stream`. Native streaming compilation rejects that MIME, the vendor `arrayBuffer()` fallback promise does not settle, and the popup event loop stops advancing before compilation starts. A separately labelled probe that wraps the same private response body with `Content-Type: application/wasm` in both popup and background also stalls during the streaming retry. This closes a header-only runtime workaround; it does not authorize embedding bytes, adding a generic resource bridge, or distributing a modified package. |
| Alarms | Live delivery compatible; context-restart persistence degraded | The ordinary gate creates an exact future alarm, reads it, clears it, and proves post-clear absence. The opt-in `cargo xtask classify-macos-extension-alarms` gate additionally arms the Chrome minimum 0.5-minute delay, observes `alarms.onAlarm` from the worker, then arms a second alarm and unloads/reloads the exact native context before its deadline. On macOS 26.6.1 the live alarm fires, while immediate post-reload `alarms.get` reports the second alarm absent with no delivery record. Zephium therefore does not claim Chrome-compatible alarm persistence. OS sleep/wake behavior and long-duration drift remain release gates. |
| Commands | Native event routing compatible for unambiguous commands | `commands.getAll` returns the exact six pinned command names. A real `NSEvent` is matched through `commandForEvent:`, the context is exact, and `autofill_login` reaches `commands.onCommand`. Browser-menu precedence, focused/resident-tab binding, current-runtime authentication, collision refusal, and monitor retirement are implemented. `_execute_action` crosses one actor-ordered, one-second request and reuses the exact visible browser-action button as its trusted popup anchor; the ordinary Shell/native action path revalidates the complete runtime/tab/revision cohort. Browser-owned remapping/conflict UI and packaged physical-key evidence remain release gates. |
| Context menus | Native merge and click routing compatible on the exercised tab context | The background creates and updates one tab-context item; Zephium preserves an opaque default-menu prefix, appends the bounded native item, performs it through `NSMenu`, observes the exact `contextMenus.onClicked` tab, then removes it and verifies native absence. Packaged physical right-click, frame/editable/link variants, dynamic enablement, and navigation-time teardown remain release gates. |
| Clipboard read/write | Behavior unassessed | A production gate must be driven by a trusted popup gesture and preserve the user's prior clipboard contents on every success, refusal, timeout, and crash path. Automated tests must not destructively overwrite ambient clipboard state. |

This is one reviewed slice of the required declaration-by-declaration matrix,
not a claim that the complete Bitwarden workflow is compatible. Product
authority remains unprovisioned until every declared authority is classified
and the exact sealed package/catalog digests are compiled in.

The release-side source boundary is offline and separate from package
authority. `cargo xtask check-bitwarden-core-source --source PATH` accepts only
an ordinary, non-sparse, clean checkout whose `HEAD` is the exact
`browser-v2026.7.0` commit and tag. It additionally hashes every reviewed
compatibility preimage and checks exact marker cardinality for the offscreen,
main-world, manifest, inline-menu, template, and webpack seams. The command
does not clone, fetch, adapt, build, or authorize a package. Git output and
individual source reads are bounded; a partial inspection checkout, local
edit, untracked input, symlink escape, or source drift fails before adaptation.
The native vertical slice builds the OSS production **Chrome MV3** target. The
pinned Safari manifest transform deletes the service-worker `background`
declaration and is therefore not the input to Zephium's WKWebExtension
controller runtime. This target choice is compiled into generated overlay
metadata rather than left to a release-shell environment default.

`cargo xtask materialize-bitwarden-core-macos-probe-overlay --source PATH
--output PATH` emits an atomic, no-replace source overlay only after that exact
admission succeeds. It substitutes literal `"MAIN"`, selects primary storage
when `chrome.offscreen` is absent, classifies the unbranded WK service-worker
environment as Safari-extension shaped, guards the absent notification
subscription surface, and fail-closes the unsafe inline-menu path while removing
its sandbox and public resource declarations. Popup stage markers are explicitly
probe-only. The metadata states `product_authority=false` and
`inline-menu-disabled`; this overlay exists to run the first real native vertical
slice and cannot be sealed as the release package. The production overlay must
replace that degradation with the self-contained inert-payload renderer proven
by the live native gate.

`cargo xtask finalize-bitwarden-core-macos-probe-artifact --build PATH --output
PATH` is the next offline boundary, not a release sealer. It accepts only the
exact reviewed Chrome-MV3 output shape, rejects links and special files, removes
exactly nine source maps and nine disabled inline-menu outputs, injects four
explicit probe resources, and atomically emits a closed 173-file extension tree.
Every retained byte is bound into the same canonical path/length/SHA-256 tree
format used by package authority. The sibling metadata binds the source commit,
target, manifest, tree/index hashes, adaptation inventory, stripped counts, and
the negative claims `product_authority=false` and
`build_toolchain_attested=false`.

The debug-only `macos-bitwarden-core-probe` independently reopens that artifact,
checks its closed root inventory, reparses the canonical index, and re-hashes
every extension file before invoking WebKit. It then exercises exact native
grants, real content registration, action-owned background/popup startup, an
extension-page canary, and full native teardown. As of the August 12, 2026 gate,
the exact build reaches registered content and a real Bitwarden popup with every
static/dynamic script loaded, but fails the 12-second executable-popup deadline
while importing the 7.04 MiB SDK WebAssembly module. WebKit serves the private
extension resource with a MIME type that rejects streaming compilation; the
vendor webpack fallback is allowed to run, but does not settle within the UX
deadline on the reviewed machine. Bounded phase evidence on August 14 sharpens
that result: the 7,378,704-byte resource returns status 200 and
`application/octet-stream`; streaming rejects immediately, `arrayBuffer()`
returns its promise immediately, that promise never resolves, and
`WebAssembly.instantiate` is never called. A 100 ms probe timer also stops near
188 ms, so the failure is resource-body/event-loop progress rather than slow
compilation. Three uncontended release-build repetitions
on August 13, 2026 reproduced the same `sdk-import` / `streaming-rejected`
terminal state in 12.92–13.32 seconds, at 115–119 MB maximum resident set and
approximately 110.5–111.0 MB peak footprint for the probe process. The blocker
is therefore stable on the exercised machine rather than a contention
artifact. It is a release blocker and resource-budget input, not authority to
increase the timeout, preload a hidden view, install a global fetch shim, or
claim Bitwarden compatibility.

The separate feature-gated `macos-web-extension-resource-probe` closes the
public `baseURL` plus `WKURLSchemeHandler` hypothesis with a Zephium-owned MV3
fixture. An ordinary nonpersistent `WKWebView` routes its HTML, JavaScript, and
empty WASM module through the exact native handler, preserves
`application/wasm`, and passes `WebAssembly.instantiateStreaming`. A loaded
`WKWebExtensionContext` accepts and reads back the custom principal-bound base
URL, retains its extension runtime identity, and returns a configuration to
which the same handler is attached. Nevertheless, its private extension
resources never dispatch to that handler: WebKit's internal extension loader
serves the WASM as `application/octet-stream`, and streaming compilation is
rejected. The gate requires zero extension-handler callbacks, denies a foreign
principal, and proves controller, context, view, data-store, and handler
teardown. This rules out that public resource override on the exercised runtime;
it does not claim that future WebKit releases cannot add a supported transport.
The separately labelled response-MIME diagnostic also retries native streaming
with the original private body wrapped in a new `Response` carrying
`application/wasm`. With that adaptation active in both the popup and MV3
background, the runtime reaches `streaming-mime-retry` near 178 ms but neither
the retry nor its timer advances before the same 12-second deadline; native
extension objects also remain retained on that error path. The artifact records
the adapter in both its compatibility and limitation contracts, remains
`product_authority=false`, and cannot be confused with the baseline artifact.
This closes a header-only response adaptation as well as the public scheme
handler route. The stock Bitwarden deadline therefore remains blocked unless
upstream output changes, WebKit changes its private loader, or a separately
approved sealed adaptation removes the dependency on that resource behavior.

The August 25, 2026 stock 1Password 8.12.32.33 gate also closes the remaining
"intercept before native streaming" variant. Its exact 17,466,756-byte module
can be read and compiled from an extension popup in 15–17 ms. A package-neutral
background prelude that recognizes only same-principal `webkit-extension:`
`.wasm` responses with status 200 and `application/octet-stream`, avoids the
native streaming call entirely, and invokes `arrayBuffer()` first still makes
no body progress in the MV3 worker. The native MIME warning disappears, but the
worker remains at `WASM: Initializing`, never publishes `Finished initializing
1Password`, and the stock popup remains at its loading state. The adapter was
therefore rejected rather than shipped. This separates CPU/compile cost from
the WebKit worker-resource transport ceiling and rules out a global fetch shim,
a `WebAssembly.instantiateStreaming` shim, or a longer popup timeout as valid
product fixes.

The package-neutral Chrome acquisition boundary now authenticates CRX3 before
materialization. It bounds the package and protobuf header, verifies every
recognized RSA/ECDSA proof, requires the signed developer proof to derive the
declared and expected Chrome identifier, and exposes only the authenticated ZIP
payload. The offline probe materializer then preflights every archive entry,
rejects encryption, links, special files, nonportable paths, cross-platform
collisions, file/directory conflicts, duplicate entries, and decompression
budget violations before writing into a private, incomplete-marked,
no-replace tree. It now completes the canonical stream receipt, but that receipt
still does not prove a filesystem snapshot: catalog release authority, staged
tree re-verification, atomic repository publication, and a live lease remain
separate requirements.

Adapted packages no longer require release tooling to hold a private signing
key in Zephium code. Before signing,
`cargo xtask prepare-extension-crx3-release-archive` accepts only an exact
closed MV3 tree and its canonical index. It binds the external P-256 public
SPKI into the manifest's canonical `key`, removes the extension's upstream
`update_url`, strips only the two known Chrome Store verification files, and
rejects every unknown `_metadata` member. It then re-indexes the output and
emits a deterministic ZIP in canonical file order with fixed compression,
timestamp, platform, and mode metadata. Its private staged output is published
through a no-replace directory reservation whose incomplete marker is removed
only after every component is durable. The accompanying evidence records both
tree identities, the ZIP identity, the derived extension id, and the exact
rewrites while explicitly declaring that signature, legal policy, catalog, and
product authority remain unsettled.

Adapted trees use the distinct
`cargo xtask prepare-extension-compatibility-crx3-release-archive` boundary.
It accepts the complete compatibility-artifact directory rather than caller-
selected extension/index paths, requires the exact three-entry root inventory,
revalidates the non-authorizing receipt and both source/output identities,
reopens the closed output tree, and then runs the same deterministic CRX3
release preparation. The exact compatibility receipt is copied byte-for-byte
beside the release archive and inserted into the staged extension at the sole
digest-derived path
`__zephium__/compatibility-receipts/<sha256>.json` before the release tree is
re-indexed and zipped. Its length, SHA-256, target, and adapted pre-signing
manifest/tree/index identities are bound into release evidence. Receipt capture
does not settle signing, licensing, catalog membership, redistribution, or
product authority. Ordinary unadapted release preparation remains unchanged,
and later Windows/Linux adapters can consume the same receipt-bound release
boundary without becoming macOS package-name special cases.

Release-catalog schema 2 closes the next authority seam. Each package may bind
an ordered, unique cohort of at most eight compatibility targets. Every row
fixes the receipt byte length and SHA-256 plus the exact release-input
manifest, tree, canonical index, file count, and byte count. The package row
separately binds the post-rewrite release tree, because stable-key insertion and
store-metadata removal legitimately change that identity. Receipt bodies have
a separate 64 KiB ceiling. Schema 1 remains readable for unadapted legacy
catalogs but cannot carry receipts, so changing a schema number cannot smuggle
adaptation authority. The redundant bundled-inventory digest uses a new domain
for schema 2 and covers the complete receipt cohort while preserving the fixed
schema-1 golden. The sealed macOS brokered manifest profile requires the exact
`macos.wkwebextension-brokered.v1` receipt before it can mint an admitted
manifest. The non-shipping product gate additionally authenticates the exact
receipt body and requires the digest-derived inert resource in the canonical
release index before repository publication, then
proves the ordinary and brokered native lifecycles independently. This fixture
authority does not provision ordinary product builds; production still needs a
signed catalog, authenticated receipt transport, legal policy, and packaged
application configuration.

`Crx3SigningRequest` borrows that bounded ZIP plus the canonical P-256 public
SPKI, derives its stable Chromium identity, and exposes the CRX3 signature
preimage as four ordered slices without copying the archive.
`cargo xtask prepare-extension-crx3-signing-message` streams that exact
preimage into a private no-replace file for an external signer.
`cargo xtask assemble-extension-crx3` accepts only the resulting ASN.1 ECDSA
signature, constructs one proof, re-enters the ordinary CRX verifier, runs the
complete acquired-ZIP preflight, and atomically publishes only after all of
those checks pass. Neither command accepts a private key or overwrites an
output. All three commands remain `product_authority=false`: reviewed
license/legal artifacts, exact catalog rows, sealed product anchors, and
transport are still separate release work.

The second stock-manager gate uses the unmodified Chrome Web Store CRX for
[Proton Pass 1.39.0](https://chromewebstore.google.com/detail/proton-pass-free-password/ghmbeldphafepmbegfdlkpapadhbakde)
(`ghmbeldphafepmbegfdlkpapadhbakde`). Its authenticated CRX
SHA-256 is
`bbdb442bbc9d1232650bcef18ae586d72d929c3eb8bc4415049054c4cac99916`;
the exact 276-file, 21,512,233-byte tree is fixed by the probe contract. WebKit
parses and loads the manifest, creates the context and action popover, and
executes enough stock popup JavaScript to populate the application root.
However, it reports `WKWebExtensionContextErrorDomain` code 6 (background
content failed to load), the stock content scripts produce no login-field or
inline-root effect, and the popup does not establish a usable runtime contract
under the probe. Controller, context, page, popup, and routing objects release
after unload, but the nonpersistent website data store remains retained beyond
the bounded error-path teardown budget; repeated failing activation must
therefore stay disabled until that lifecycle is bounded. This proves that this
raw stock Chrome package is not usable on the exercised WKWebExtension runtime.
It does **not** prove that Safari-authored packages or a reviewed
package-neutral compatibility transform are impossible.

The next stock gate authenticates
[1Password 8.12.32.33](https://chromewebstore.google.com/detail/1password-%E2%80%93-password-mana/aeblfdkhhhdcdjpifhhbdiojplfjncoa)
from the Chrome Web Store. The CRX SHA-256 is
`3bd61e220683d0549a4f5b0d9632e2015d5a3a809bc12e9a832e8bc0630600f6`;
three valid CRX3 proofs derive `aeblfdkhhhdcdjpifhhbdiojplfjncoa`,
and the complete developer-key SHA-256 is
`041b53a7773239f8577138e9fb59d2e02599f358bd24094b9e875402b0cc7927`.
The exact 998-file, 45,115,978-byte tree has SHA-256
`873bd553f05fda33c2227b40682cfa4ff2988e0e53944291432e4869f6caaa4e`
and canonical-index SHA-256
`633f2cbe9ce15b12e89e6276c76565834618fb5403d5e6969b7f488122291de4`.
No package byte or manifest is committed. The diagnostic pins only those
identities and the reviewed manifest shape.

The package is genuine MV3, has no manifest sandbox, and its unmodified action
popup executes with both native `chrome.runtime` and `browser.runtime`
identities. Its module background nevertheless fails to complete WebKit's
public load callback within 12 seconds, records
`WKWebExtensionContextErrorDomain` code 6, and leaves the content/autofill
orchestrator unconfigured. The package contains seven WASM resources; the
largest is 17,466,756 bytes, and its generated loaders try streaming before an
`arrayBuffer` fallback. WebKit returns status 200 and
`application/octet-stream`. The complete body arrives and
`WebAssembly.compile` settles in 14–17 ms in an extension popup, proving raw
compilation is cheap on the reviewed machine; the same response body does not
make progress in the MV3 worker, which is the startup blocker.

The popup surface also exposes a separate API gap. WebKit provides `action`,
`alarms`, `commands`, `contextMenus`, `declarativeNetRequest`, `scripting`,
`storage`, `tabs`, `webNavigation`, `webRequest`, and `windows`, but omits
`downloads`, `idle`, `management`, `offscreen`, and `privacy`. Its
`notifications` surface is absent in some extension realms and partial in
others. The Chrome build installs notification and navigation listeners during
background initialization. Declared-permission-gated, package-neutral adapters
therefore preserve valid native members, fill only missing notification
members with inert/no-delivery behavior, expose empty read-only managed
storage, and add an inert `onCreatedNavigationTarget`. Reconciliation is
bounded to startup microtask/task frontiers; no poller, hidden view, network
port, or page-world authority remains resident.

The worker-resource failure is now bypassed by a narrower standards-shaped
gate rather than a byte or MIME workaround. Apple documents nonpersistent
background pages as a supported alternative to MV3 service workers. The
opt-in transform retains the module service worker for source identity, adds
the same wrapper to `background.scripts`, and selects
`preferred_environment: ["document", "service_worker"]`. The environment is
typed into manifest admission and compatibility identity and is bound in the
receipt as a degradation. Classic workers, existing page/script declarations,
and ambiguous environment declarations fail closed. A document background is
still event-managed by WebKit; Zephium does not create or retain a hidden
`WKWebView` for it.

The independently indexed current 1Password compatibility artifact has 1,005
files and 45,140,692 bytes, manifest SHA-256
`901937289daf1f75c4a48507e8023af312d52144cf61cb6000fb6f913c0bebf0`,
tree SHA-256
`e1271e34edbf7c9a79c09da93b4e8a73b427d3eefe270af7a570627864ca1388`,
and index SHA-256
`287673c89c85059bdffa67f4e514fef80d5f68e67ecbc886f0da9326980ece51`.
The native probe reports the background-load callback in approximately
175–192 ms with zero context errors; the 17,466,756-byte WASM fallback compiles
in 16–17 ms. In the real browser, the generated background document reaches
1Password's `Finished initializing 1Password` milestone, analyzes the active
page, and returns a successful `get-popup-config` payload. This proves that the
WASM worker transport was the startup blocker and that document execution is a
viable compatibility lever on the reviewed WebKit runtime.

Two deny-only facades preserve safe failure semantics without granting the
missing authority. A document-scoped `clients.matchAll()` returns one frozen
empty cohort because a background document has no ServiceWorker WindowClient
inventory and Zephium independently enforces the single-popup lease. Native
messaging permission is removed; `connectNative()` returns a bounded Port that
disconnects on the next microtask, while `sendNativeMessage()` rejects (or
settles its callback with no response). No identifier is resolved, no bytes are
serialized, and no process is contacted. 1Password consequently records
`PortClosed`, resets its desktop-connection state, and continues its
extension-first initialization. Callback denial cannot expose a native
`runtime.lastError`, and the receipt discloses that degradation.

This is a substantial positive gate, not yet a compatibility claim. The real
browser now executes the package's no-popup action, receives its exact internal
`tabs.create()` target, and presents the full same-principal welcome document
through the bounded extension-UI host. Continue reaches Start setup. Sign in
then performs the package's programmatic HTTPS transition; Zephium creates a
normal profile tab at `my.1password.com/signin?auth-only=1`, retires the
privileged extension document, reopens it on the next action, and exits cleanly
when that document is still open. This closes onboarding action routing and the
pre-authentication handoff on the exercised packaged lab. The isolated probe
still does not observe the static content-prelude marker, and account
completion, vault unlock, autofill, save, passkeys, offscreen-storage
migration, complete installed-restart behavior, and release resource/endurance
budgets remain unproven. Neither stock nor transformed 1Password may enter a
shipping catalog until those workflows pass and publisher/legal distribution
approval exists.

The August 26 signed-account restart gate narrows that remaining blocker. A
preserved profile proves that onboarding and the HTTPS account handoff can
durably create the account database, but a cold process restart does not
complete 1Password's exported `initializeFinishedPromise`; its popup and the
account-completion page remain at their loading states. The background module
graph evaluates and its document reaches `complete`. A lab-only bounded trace
proved that both private WASM responses finish `arrayBuffer()` in 1--8 ms,
both fallback instantiations settle in 9--22 ms, local and session extension
storage settle, startup runtime messages receive callbacks, and queued
microtasks and timers run. The trace observed no pending IndexedDB, Web Locks,
WebCrypto, extension-storage, or WASM loader operation. 1Password's signed-
account B5X request remains pending inside its own JS/WASM transport, and
WebKit later unloads the nonpersistent background. Attaching Web Inspector is
the only exercised condition under which a manual background reload completed
that initialization; an uninspected reload did not.

Surface-before-runtime ordering, a replayed `load` event, a `scheduler.yield`
fallback, an explicit generated WASM `start()` call, an uninspected reload,
and a self-connected runtime port were each tested and falsified, then removed.
No timer, hidden view, private WebKit background handle, persistent-background
conversion, source rewrite, or 1Password identifier branch remains in the
browser. This exact Chrome build therefore remains **not compatible** for
signed-account daily use. A future target needs either publisher cooperation,
a separately reviewed and legally authorized package adapter with an explicit
readiness/keepalive contract, or an upstream WebKit/package lifecycle change;
the current evidence is not authority to keep the background resident.

One independent compatibility repair remains valid. The authenticated native
grant projection now retains whether the admitted background is WebKit's
document fallback. On each exact top-level Started or Redirected epoch, macOS
attempts `loadBackgroundContent` only for published document runtimes whose
native extension reports injected content for that exact provisional URL.
Service-worker runtimes and nonmatching extensions pay no native wake call.
Stale, replaced, terminal, disallowed, cross-profile, or unowned callbacks are
rejected before URL construction or WebKit entry; the bounded runtime cohort
uses fixed slots, so the ordinary extension-free navigation path performs no
temporary heap allocation. The call uses WebKit's public event-background API,
creates no view or timer, and remains a usability hint rather than navigation,
permission, or content-script authority. In the signed-account gate it removed
the earlier undefined content-message responses during direct navigation, but
it does not claim to repair 1Password's separate cold-start core blocker.

The August 26, 2026 refresh separately pins the then-current stock 1Password
8.12.34.34 Chrome package. Its authenticated CRX SHA-256 is
`7f0b3a8f66469091c74dc6943d77d01fda657a0eb3ff3247afc3be8b841edb75`;
the developer-key SHA-256 and Chrome identifier remain unchanged. The exact
1,000-file, 45,151,539-byte source tree has SHA-256
`7a0ba69b85aed523e22b7e4e63c002cdf12bfd77156186a02ecefedba3e20aad`,
canonical-index SHA-256
`45a749822287874b93f147ae2b26720342c25e7fe4a8a940fe1efc35b2b8c31d`,
and manifest SHA-256
`388326228cad52ddc0bc860101a27d571ca52736018526a2b45ddae05ed1622d`.
The release adds a required `nativeMessaging` declaration; the ordinary native
target removes it and retains the same explicit no-process deny facade. No
third-party package byte is committed.

The clean package-neutral document transform now produces 1,008 files and
45,186,434 bytes with manifest, tree, and index SHA-256 values
`08caefed1cd5c8945a60799257b41aa8586be0e2a5a5860285af06d4c7bbb169`,
`e56cb28e0cbdb7ebb26d52d181f7a222b701af532ca7604b58245086fd2fd77d`,
and `82532476eb19d8ccc2b6bfccc33809b7342884e961d82c64898b1acbb6a68c66`.
The stock worker still reaches WebKit context error 6 after the bounded
12-second load, while the clean document background loads without a context
error in about 172 ms. The isolated-content compatibility marker is still not
observable through `scripting.executeScript`, so this remains diagnostic
evidence rather than a compatibility claim. In a fresh real-browser profile,
the clean 8.12.34.34 popup also remains at its loading surface and does not
expose onboarding.

The distinct non-authorizing `webkit-macos-native-publisher-v1` transform
preserves that release's required `nativeMessaging` declaration without
installing the ordinary deny facade. Its current 1Password lab output contains
1,007 files and 45,181,908 bytes; manifest, tree, and canonical-index SHA-256
values are `35a3ee69fb4caeb27db4be80b4fa68ee66f904145ee1ef0109fc2ba1f007980c`,
`beefd591018b35405bb0b6488f869a3e84e377ee362ad8332692983be9a3238d`,
and `f83a7a3682384b42a064d6af7e7db179fc22e964414dc8cb2c720e1ae836e8b5`.
The receipt explicitly requires a sealed publisher-host policy and a
publisher-signed executable. It mints no such authority itself. The private
lab profile now compiles the inspected 1Password host, Team, signing, and
upstream-extension identities and activates them only from a correctly signed
Zephium bundle; the generic transform remains non-authorizing and cannot make
an arbitrary package or native host eligible.

A separate non-authorizing experiment bound the module's exported
`initializeFinishedPromise` into a generated wrapper with an eight-second
one-shot deadline. WebKit's public background-load callback returned in about
89 ms, before that promise could serve as host-visible readiness, and the real
product popup still remained at its loading surface. The experiment was
rejected and removed: no readiness-export adapter, timer, hidden view, or
package-specific runtime branch remains. The result proves that module
top-level completion cannot be treated as the settlement contract for
`loadBackgroundContent` on the exercised WebKit runtime.

The August 27 signed product gate advances that evidence with the exact stock
8.12.34.34 package and publisher-native document artifact. The installed
1Password BrowserSupport helper verified Zephium's Developer ID team, connected
to the desktop app over its own XPC boundary, and exchanged framed native
messages through the sealed upstream Chromium principal. The authenticated
vault popup rendered real account content, and a public HTTPS login form
exposed 1Password's inline field affordance. No 1Password package byte or
package-specific execution branch is compiled into Zephium.

The August 28 follow-up identified one package-neutral startup failure rather
than treating the intermittent result as a timeout problem. The stock
background rejected initialization while evaluating
`chrome.privacy.services`: current WebKit exposes no such namespace, while
1Password reads `passwordSavingEnabled` and conditionally disables browser
password, address, and card autofill. A declared-`privacy` compatibility asset
now preserves every valid native member and supplies only those four settings
when absent. Zephium has no competing browser password/address/card service, so
each value is truthfully fixed to `false`; disabling and clearing are accepted,
enabling fails with `NotSupportedError`, and change events are inert. The
5,813-byte asset has no credential/passkey access, network API, native message,
hidden view, worker, polling loop, or idle timer. Its permission gate, exact
receipt surface, disabled-only semantics, native-member preservation, and both
service-worker and document-background execution are automated release
contracts.

With that exact publisher artifact in a fresh private profile, the native port
callback arrived at about 233 ms and the worker was ready at about 334 ms. The
publisher onboarding completed against the installed desktop application, the
real vault popup rendered, the content script exposed its field affordance on
public HTTPS login forms, and an explicit popup `Autofill` action filled a
synthetic username/password pair and dismissed the popup. This is the first
complete real-vault fill through Zephium, but it is not yet a Verified-package
claim. In that first candidate, clicking the in-field icon changed 1Password's
logical open/close state without presenting its inline menu, and invoking the
already-visible toolbar action did not toggle-dismiss the popup. Save/update, lock/restart,
passkeys, request authentication, long-running resource behavior, and upstream
support remain open gates. No real credential or Apple Passwords entry was
used by this test.

The next package-neutral slice closes both browser-side causes disclosed by
that run. Native toolbar actions now reauthenticate and toggle-dismiss only an
exact same-context, same-tab popup before acquiring another resource lease;
competing targets still receive the one-popup capacity refusal. Ordinary lab
runs also use production transient outside-click/Escape behavior, while
`ZEPHIUM_EXTENSION_LAB_RETAIN_POPUP=1` is the explicit inspector-only opt-in.
Native and brokered product probes require open, toggle-dismiss, reopen,
discard-close, and final reopen in one lifecycle.

The offline transform also adapts manifest-declared web-accessible HTML pages,
instead of limiting the native-preserving prelude to the action popup. Resource
patterns are bounded portable ASCII with `*` as the only wildcard; expansion
runs against the already-authenticated closed tree, deduplicates exact paths,
and fails a missing literal HTML declaration. Declared sandbox pages remain
byte-identical, undeclared extension pages remain explicitly unsupported, and
the receipt binds the exact adapted-page count. A Zephium-owned fixture embeds
one such page in a closed-shadow extension-origin iframe, authenticates its
response by child-window identity and a random nonce, requires both native
runtime aliases plus the compatibility marker, keeps the page world without
extension APIs, and releases every native owner. Both service-worker and
document-background WebKit runs pass. The current 1Password artifact expands
exactly four declared pages: inline menu, notification, modal, and universal
sign-on. Its newly signed private candidate installs, reconnects the publisher
host after restart (native callback about 325 ms; worker ready about 434 ms),
renders the real vault popup, and toggle-dismisses correctly. The field icon is
present.

The signed August 28 follow-up closed that inline-menu gate and located the
browser-side defect at the native navigation-policy seam. The exact stock
1Password artifact can load both its declared inline stylesheet and
`inline/menu/menu.html` from a controlled cross-origin page, and the
package-neutral fixture also passes with `<all_urls>`, nested paths, wildcard
resources, an omitted `use_dynamic_url`, a stable custom context base URL, and
a named persistent controller. Splitting web-accessible HTML into another
manifest group did not repair the product and was removed. The product tab was
instead sending every `WKNavigationAction`, including child frames, through
Wry's host callback. Zephium's callback intentionally admits only canonical
browser-level targets, so it cancelled the `webkit-extension:` child
navigation before WebKit could apply its controller- and
`web_accessible_resources`-bound authorization.

The pinned Wry adapter now delegates only exact non-main-frame
`webkit-extension://` and `chrome-extension://` navigations to WebKit's native
policy; the latter is the base URL the runtime assigns every context, so
omitting it cancelled every extension's in-page frame (issue #160). Main-frame and
target-less actions still require the host callback, other schemes receive no
bypass, and WebKit remains responsible for proving a loaded context plus the
manifest resource grant. A pure policy test covers those boundaries. The live
compatibility fixture installs a hostile host callback that would reject the
extension URL, proves the callback still handles ordinary navigations, proves
it never receives the extension child navigation, and completes the
closed-shadow resource round trip through both service-worker and document
background artifacts. A Developer-ID-signed lab build then rendered the real
1Password inline menu, selected the synthetic QA item, filled the controlled
login form, and dismissed the menu; no Apple Passwords item was used. This
closes inline fill as an engineering gate, while the later signed workflow
evidence below narrows the remaining password-manager gates further.

The same signed build produced a controlled 60-second process-coalition
comparison with three resident test tabs. With 1Password active, the coalition
ended at 12 processes and 363,520,880 physical-footprint bytes, consumed about
632 ms of combined user and system CPU, and reported 811 package-idle plus
2,002 interrupt wakeups. Pausing extensions in the same profile ended at 10
processes and 189,455,800 bytes, consumed about 24 ms of CPU, and reported 51
package-idle plus 154 interrupt wakeups. The paused projection removed the
1Password BrowserSupport helpers and extension-owned WebContent while retaining
the three browser tabs, so the inert path does reclaim the optional runtime.
These numbers are machine- and state-specific release inputs, not universal
budgets: they show that this publisher extension has a material opt-in cost and
must receive an explicit resource classification and endurance gate before a
Verified badge. They are not authority to keep a hidden background view,
multiplex the publisher's per-port native-host contract, or charge this cost to
profiles with no active extensions.

A later August 28 signed candidate closed the first durable save and passkey
workflows. The publisher artifact's unmodified inline save UI accepted one
generated synthetic login, and a fresh vault search found the exact item after
the dialog retired. Three
consecutive completed-onboarding process restarts restored the inline field
surface and authenticated vault popup; action-to-render time under the UI
harness was approximately 1.2--1.3 seconds. The user separately confirmed that
unlocking the installed 1Password desktop application unlocks its browser
extension. A synthetic passkey registration completed through 1Password and
remained visible in the vault after the package update and clean restart. No
real credential or Apple Passwords item entered these gates. At that point,
browser-inline saved-item update, passkey assertion, and HTTP
request-authentication behavior remained separate.

An August 30 follow-up separates two different update claims. Editing the
original synthetic login in the publisher's desktop application advanced its
durable modification time; the already-running Zephium extension synchronized
that change, its isolated inline UI filled the exact replacement password on
the public HTTPS test form, and 1Password auto-submitted the form. This closes
publisher-app update through browser-extension sync/refill. It does not close
browser-inline changed-password update: the inspected inline configuration had
`inlineSavingEnabled=false`, matching the package's publisher default; its
explicit Save action created a second synthetic item rather than replacing the
existing one. The package's full settings application contains the publisher
toggle for inline save behavior and opens it through ordinary
`tabs.create({ url: runtime.getURL(...) })`.

That workflow exposed one package-neutral configuration error in Zephium's
already-authenticated extension-page boundary. WebKit presents Chrome's
default active tab with `shouldAddToSelection=false`; Zephium incorrectly
required that independent multi-selection flag to be true even though its
dedicated extension window contains exactly one tab. Apple documents that an
active tab is already selected. The boundary now accepts either inert
multi-selection value while retaining every principal, URL, parent/opener,
pinned, muted, reader-mode, active-state, index, window, resource-pool, and
teardown check. Unit and signed-build gates cover the correction; a physical
publisher-settings interaction remains required before browser-inline update
is claimed.

HTTP request authentication is resolved as an explicit degradation rather
than an open workaround target. Public WebKit cannot enforce the blocking
`onAuthRequired` response used by password managers. The raw WKWebView
delegate hands a person's tab its main-frame HTTP sign-in challenges for
Zephium's own prompt and cancels every other non-server-trust challenge before
native credential UI can bypass browser policy. The browser therefore discloses HTTP
Basic-auth autofill as unavailable; it does not add a generic credential
bridge, expose a native dialog, or retain credentials outside the publisher.

That run also made the intermittent infinite-loading popup deterministic.
After WebKit reloaded the nonpersistent document background, 1Password aborted
initialization while evaluating `browser.storage.managed.onChanged`. Zephium's
empty read-only facade had run before the package's browser polyfill, but the
polyfill replaced the nested `storage` object after the facade's original
microtask/task reconciliation had already completed. The compatibility asset
now preserves complete native managed-storage objects, fills only missing
`get`/`onChanged` members, and reconciles across four fixed microtask plus four
fixed task frontiers. It installs no interval, proxy, worker, hidden view,
network path, or idle keepalive. Node contracts prove native identity
preservation, incomplete-native completion, replacement after both frontier
classes, and bounded termination. A Developer-ID-signed package revision then
survived a forced background reload, popup close/reopen, a detached 60-second
idle interval, and a clean process restart; the post-idle popup rendered in
about 1.45 seconds and the clean-restart popup in about 1.05 seconds.

Testing that revision also exposed a missing lab-only authority invariant. The
private lab previously sealed only its new active catalog, so an existing
profile on the prior exact generation correctly failed repository recovery as
unrecognized. The generated lab can now carry one separately validated older
generation. Core authority authenticates its catalog and manifest only through
rollback APIs, while distribution still serves only the active generation. In
the tooling, `prepare-local-extension-lab-generation` produces a non-launchable
reviewed seed, while `stage-local-extension-lab` requires that exact older seed
and is the only command that emits the fixed launchable layout. In
the live run catalog revision 7/package revision 2 recognized revision 6 as
rollback, installed atomically, retained the previous catalog set, and
restarted with the signed-in WebKit data intact. This is explicit rollback
authority, not a recovery bypass or mutable local trust input.

The same lifecycle campaign found a synchronous AppKit recursion in popup size
clamping: `NSPopover::setContentSize` can emit the observed frame-change
notification before Auto Layout returns, re-entering the setter until stack
overflow. A main-thread reentrancy guard now rejects only that nested clamp and
reopens immediately after the outer call. Dynamic later frame changes remain
eligible. The guard has a focused unit contract; native and brokered product
probes remain the behavioral regression gate.

Resource measurements remain deliberately state-labelled. The update plus Web
Inspector reload interval reached 604,826,496 bytes terminal physical footprint
across 14 processes and was treated as transient diagnostic state. Pausing the
profile removed both publisher helpers and ended a 30-second control at
266,494,040 bytes with about 9 ms combined CPU. After a clean restart, the
active 30-second interval ended at 425,502,032 bytes across 12 processes with
about 181 ms combined CPU. The approximately 159 MB active-versus-paused
difference is consistent with the earlier password-manager delta, while the
absolute browser baseline changed with page/process state. These observations
prove reclamation and reject the transient 605 MB number as a steady-state
budget; clean-runner energy, tab-pressure, sleep/wake, multi-profile, and
24-hour gates still define release readiness.

An August 30 same-process A/B on the signed optimized lab provides a newer
lifecycle-labelled input. Both intervals used the same completed-onboarding
profile, three restored resident tabs, closed Extensions Center, no Inspector,
and 60 one-second coalition samples. With 1Password active, the run ended at
11 processes and 394,060,152 physical-footprint bytes, used about 204 ms user
plus 79 ms system CPU, and recorded 542 package-idle plus 1,912 interrupt
wakeups. Pausing the profile removed one WebContent and one auxiliary process,
ended at 9 processes and 212,949,880 bytes, used about 42 ms user plus 18 ms
system CPU, and recorded 187 package-idle plus 606 interrupt wakeups. The exact
terminal footprint difference was 181,110,272 bytes. This confirms prompt
reclamation and a material publisher-extension cost on this machine; raw
energy counters remain uncalibrated, and these observations are not release
budgets or authority for a hidden keepalive.

That gate also found and fixed two package-neutral lifecycle defects. WebKit
delivers an extension's first `Port.postMessage` only after the delegate
completion, while stock MV3 code may post immediately after `connectNative`.
Zephium now arms the retained port and completes that logical connection
before native process startup, holds at most the existing two-frame outbound
capacity before the signed host is ready, and drains in order only after exact
runtime, registration, origin, and publisher admission. Separately, WebKit can
report a successful document-background load while exposing no background
target when the context was activated before a published resident browser
surface. The runtime records that ordering race and performs one authenticated
unload/load reconciliation only after the first real surface and legitimate
navigation or toolbar wake. Service workers, discarded tabs, extension-free
profiles, and already-ready document runtimes pay no reconciliation.

One fresh, incomplete-onboarding data-store run reached the first native port
at 60.786 seconds; after that callback, the signed worker was ready in 52 ms
and the first host response arrived 45 ms later. The background's own WASM
marks completed in about 31 ms on that run, so the minute was not attributed
to Zephium's native transport or to WASM compilation. This remains a cold UX
and readiness input, not permission to add a hidden view, permanent worker,
global fetch shim, or blind timeout. Password-manager verification still
requires repeatable completed-onboarding restarts, save/fill/passkey workflows,
resource/endurance budgets, and publisher/legal release gates.

The same gate found that current WebKit lacks `scheduler.yield()`, which modern
Chromium extensions use to split long tasks without timer clamping. The
package-neutral extension-world prelude now preserves a native implementation
when present and otherwise installs only `yield()`: a lazy fixed 128-slot
MessageChannel ring whose two ports close immediately when the queue drains.
It creates no idle timer, worker, hidden view, or persistent wake source;
overflow rejects with `QuotaExceededError`. The receipt explicitly discloses
that a compatibility continuation cannot reproduce native priority and abort
inheritance. A live Zephium-owned WebKit fixture proved resolution, isolated
world confinement, bounded mode reporting, credential round trips, and full
native teardown. A stock 1Password A/B still reached its first native port at
about 60.8 seconds, so this primitive is an API/responsiveness improvement and
is not claimed as the password-manager startup fix.

A separate Zephium-owned lifecycle gate now removes the browser action seam as
an explanation for that stall. The exact compatibility fixture has a real
action popup that sends a runtime message at module evaluation; its background
returns the response asynchronously after 750 ms. The native probe executes
the same artifact as both a module service-worker wrapper and WebKit's
nonpersistent module-document wrapper. Both modes complete the popup-to-
background response, preserve isolated extension identity, and release the
controller, context, page, popup, data store, and browser-surface graph. CI
materializes and runs both variants, so a regression in event-scoped popup
readiness cannot hide behind the simpler content-script round trip.

The public persistent-background alternative was also exercised and rejected,
not merely dismissed for resource cost. Apple allows [persistent extension
background content](https://developer.apple.com/documentation/webkit/wkwebextension/haspersistentbackgroundcontent)
on macOS in general, but WebKit rejects `persistent: true`
for this Manifest V3 artifact with `WKWebExtensionErrorDomain` code 8: MV3 must
remain nonpersistent. The experimental target was removed. These two results
bound the current engineering posture: Zephium must preserve event-managed MV3
semantics, and the stock 1Password initialization needs a publisher-owned
event/readiness contract or an upstream lifecycle change; it is not authority
for a timer, hidden view, permanent worker, or package-identifier branch.

The private lab now admits one initial synchronization from its immutable
embedded plan, making an empty repository reproducible without hand seeding;
staging and shipping workers remain dormant until their own product policy
admits a refresh. The revision-2 private run synchronized one candidate,
presented the full limitation review, installed it, created the background,
connected BrowserSupport, and rendered the authenticated vault popup. Two live
native ports used separate publisher helper processes, as Chromium semantics
require; the observed helpers were approximately 29 MB RSS each and 0.0% CPU
at the sample. That cost remains a cohort/endurance input rather than authority
to multiplex a publisher protocol behind its back.

Authenticating this real package exposed one legitimate package-admission
ceiling: a current major extension contains a single resource slightly above
16 MiB while remaining well under the 128 MiB aggregate tree bound. The exact
per-file and mirrored runtime-reader ceilings are now 32 MiB. Acquisition and
repository access remain streaming, resource-plan memory is independent of
file bodies, and the per-file ceiling remains one quarter of the aggregate
tree ceiling; this does not authorize a runtime to retain 32 MiB buffers.

This extension result does not remove the platform credential baseline. Apple
documents that [`WKWebView` handles website credential challenges and works
with keychain and third-party credential managers](https://developer.apple.com/documentation/authenticationservices/password-use-in-web-browsers).
Zephium now preserves that division explicitly. Password AutoFill remains
entirely WebKit-owned. An on-demand AuthenticationServices boundary reads only
the browser's passkey authorization state; it never enumerates credentials or
relying parties, and a future native enum value fails closed as unknown. Before
touching that manager it now reads the current task's exact signed
[`com.apple.developer.web-browser.public-key-credential`](https://developer.apple.com/documentation/bundleresources/entitlements/com.apple.developer.web-browser.public-key-credential)
Boolean. Absence or false returns the distinct `EntitlementRequired` product
state; an unexpected Core Foundation type, lookup failure, or native exception
returns `Unavailable`; a request fails as `MissingEntitlement`. Apple restricts arbitrary relying-party registration and
assertion to this reviewed managed capability, so adding a plist key without
account approval is not an implementation. The Extensions Center starts the
query only while visible and offers the platform authorization request only
from a trusted user click in an entitled build. Rust authorizes the main caller,
dispatches the request on the native main thread, admits one request at a time,
and projects settlement only to the fixed main label.

An earlier Developer-ID-signed lab build without the entitlement reported
`notDetermined`; that platform value was not evidence that WebAuthn was usable.
The exact signed-entitlement guard now reports that the build requires the
managed capability instead of blaming the user's system. A
self-authored `cargo xtask serve-password-manager-webauthn-qa` loopback gate
uses random one-use challenges, exact Host/Origin checks, strict CSP, bounded
HTTP/JSON, P-256 registration evidence, and server-side assertion-signature,
relying-party, user-presence, and user-verification checks. The unentitled lab
reached WebKit's `NotAllowedError` before provider UI, which is retained as a
negative entitlement result rather than attributed to 1Password. Once Apple
grants the managed capability, the same signed/notarized packaged gate must
pass without Apple Passwords or real credentials. The earlier 1Password
passkey-registration evidence remains extension-mediated evidence on a
supported HTTPS site, not proof of the browser-native baseline. Physical
password fill/save and passkey registration/assertion remain release gates.
This native baseline is not a substitute for a manager's full popup, vault,
save, inline-menu, settings, or desktop-integration workflows.

A separate authenticated-tree diagnostic now identifies two narrow WebKit API
compatibility requirements without changing production authority. It copies
only the indexed Proton tree into a private no-replace stage, re-hashes every
source while copying, and adds a Zephium-owned prelude and background wrapper.
The prelude preserves native `chrome` and `browser` identities, creates only a
missing alias, locks both global properties around the same native objects, and
provides an inert `runtime.onUpdateAvailable` event because Zephium's
authenticated catalog owns updates. That diagnostic imports the real
background, executes the isolated content-script prelude, renders the popup,
and releases every native object.
It records `offscreen` as absent and `user_workflows=unassessed`; it neither
proves Proton login/autofill compatibility nor authorizes a package. The result
shows that the stock failure includes a missing catalog-update event and an
unstable namespace-global seam, rather than requiring a Proton-specific product
runtime.

All stock third-party contracts stay feature-gated and confer
`product_authority=false`. Zephium must not hardcode any manager into the
production runtime or redistribute modified third-party bytes by accident.

The representative-extension gate applies the same rule to ordinary major
extensions without adding a package-name branch. `cargo run --locked -p
zephium-engine --features native-web-extension-probes --bin
macos-representative-extension-probe` accepts either one exact canonical stock
tree/index or one non-authorizing compatibility artifact. It grants only the
closed native permission cohort required by its page-theme/action scenario,
loads a light top document plus same-origin and `about:srcdoc` descendants,
requires dynamic CSS transformation, proves that the page world receives no
extension authority, executes the native action popup, and waits for the
controller, context, page, popup, store, and delegate graph to release. The
input identities are emitted with every result; the probe has no downloader,
catalog, installation, or product-authority path.

The first external input exercised through that gate is unmodified
[Dark Reader 4.9.129](https://github.com/darkreader/darkreader/releases/tag/v4.9.129).
The official Chrome MV3 release ZIP has SHA-256
`20e7993eee8015f7db18748eea366616dfd05ec477efb7be6ae52d2b221b0a64`.
The Chrome Web Store CRX has SHA-256
`9dcb1bd6dcad43892ddc028ff65f982df3edba135a9a6445e4fa71671e37aa13`,
developer-key SHA-256
`48c03f1215dc1aefee954a7da73184471db3f77fc2b34c9cb65e40cbc60e87d2`,
and materializes to the exact 89-file, 3,143,625-byte tree
`3a132ead591fcd395ff4090d888d0637715c253efe1508bbaffa04ff5164ee93`.
On the exercised WebKit runtime, the stock package and the package-neutral v3
transform both darken the top document, dynamic styles, and a same-origin
frame; render an executable popup with native runtime identity; report zero
context errors; and release every native owner. Stock therefore needs no
Zephium adapter for that core workflow. An `about:srcdoc` descendant remains
light even though the source requests `match_about_blank`; the result is
`usable-with-about-srcdoc-frame-degradation`, not full parity. No Dark Reader
byte was admitted by that external probe, and its evidence alone grants no
redistribution or catalog authority. Reviewed bytes are present only in the
explicit non-shipping mixed staging catalog described below.

The later mixed-catalog packaged gate adds redistribution and staging authority
without changing that core result. It also exposes a native lifecycle
degradation: unloading the context on disable and constructing a replacement on
re-enable, or reconstructing it for a package-revision update, makes WebKit emit
`runtime.onInstalled` with reason `install` again, so stock Dark Reader opens
another help tab even with its deterministic origin. Restart-only hydration
does not. Zephium does not suppress arbitrary `tabs.create` calls or branch on
Dark Reader's package identity; the background declaration remains degraded
as an intentional WebKit semantic difference. WebKit's own
[`InstalledEvent` test](https://github.com/WebKit/WebKit/blob/main/Tools/TestWebKitAPI/Tests/WebKit/WKWebView/WKWebExtensionAPIRuntime.mm)
expects the event after every unload/load, while
[`determineInstallReasonDuringLoad`](https://github.com/WebKit/WebKit/blob/main/Source/WebKit/UIProcess/Extensions/Cocoa/WebExtensionContextCocoa.mm)
selects `install` whenever an unchanged extension loads into a controller that
is not freshly created. The same source clears that transient reason on unload
and recalculates it on the next load. A live package-neutral negative probe also
showed that `unsupportedAPIs` retains the requested event-member string but
still exposes native `runtime.onInstalled`; directly replacing the property
instead makes the background fail to load. Keeping a context loaded would
contradict disabled-state native-owner absence. Closing this gap therefore
requires a future host-controlled lifecycle API or deeper compatibility-runtime
ownership, not a dormant-context cache or suppression of extension-created
tabs.

That run also settles the `fontSettings` declaration boundary. WebKit parses
the required token without error, omits it from `requestedPermissions`, and
exposes no `chrome.fontSettings` namespace. macOS therefore treats it as a
manifest-only degraded declaration: consent remains mandatory when required,
but no fictitious native grant key is applied. Unknown tokens remain rejected.

One macOS catalog may contain both unmodified `MacosNative` packages and
reviewed `MacosNativeBrokered` packages. Product composition requests a sealed,
strictly ordered target cohort and the authority derives one bounded canonical
selection across it. A duplicate package key across selected targets is an
invalid product configuration, because one durable install identity cannot
activate competing runtime profiles. This keeps runtime choice in compiled
product authority while allowing packages such as stock Dark Reader and
brokered Vimium to share one distribution worker and one catalog transaction.

The first package-neutral macOS transform now exists as an offline,
non-authorizing boundary. `cargo xtask
materialize-macos-extension-compatibility --extension PATH --tree-index PATH
--output PATH` reopens an exact closed MV3 tree, rejects source drift and
reserved-namespace collisions, and emits a separately indexed artifact. Its
`webkit-macos-native-v3` adapter preserves and locks native namespace identities,
supplies only the inert catalog-update event owned by Zephium, wraps classic or
module background workers, prepends isolated content scripts, leaves `MAIN`
scripts unchanged, and inserts the local prelude into an explicit leading
action-popup `<head>` plus manifest-declared web-accessible HTML pages selected
from the authenticated closed tree. Sandboxed and undeclared pages remain
unchanged and disclosed. A declared `notifications` permission conditionally adds
the separately typed native-preserving/inert fallback described above; packages
without that declaration receive no namespace or extra resource. It does not
proxy extension APIs: live WebKit evidence showed that proxy replacement
accepts listener registration but breaks native message delivery. Ambiguous
HTML, nonportable or absent resources, links,
special files, unsupported worlds, and every file/tree budget violation fail
closed.
Because file-URL access requires a separate user grant that does not yet exist,
the transform removes only `file:` match patterns and omits an entry only when
that leaves no runnable match. Exact removed-pattern and omitted-entry counts
are sealed into the artifact metadata.

When a background extension declares `webNavigation`, v3 also emits one
deduplicated document-start isolated endpoint for each retained content route,
including routes mirrored from untouched `MAIN` or CSS-only entries. It imports
the same endpoint before the original worker and supplies only WebKit's missing
`onHistoryStateUpdated` and `onReferenceFragmentUpdated` event objects. The
endpoint uses no polling, generic native messaging, network shim, or page-world
extension API. It derives changes from the frame's real location, bounds and
validates extension-local messages plus native tab/frame sender identities,
and accepts only HTTP(S) or exact blank/srcdoc URLs. Hash and popstate changes
are native DOM inputs. History API changes require a fixed browser-owned DOM
signal from Zephium's already-observed committed URL. The macOS host now emits
that payload-free signal only for a changed same-origin URL in the exact
current main-frame epoch, from WebKit's default client world and independently
of extension presence. The authenticated native and brokered product probes
require both the native URL event and cross-world DOM observation. This closes
changed-URL top-frame History API signaling without claiming full parity:
same-URL `pushState`/`replaceState` and subframe History API changes invisible
to the top-level WKWebView source remain explicit limitations.
The emitted metadata binds source and output manifest/tree/index identities and
states `product_authority=false`; it is not a release sealer, catalog entry, or
redistribution decision. A Node contract gate exercises native identity,
locked-global and alias behavior, idempotence, unadaptable-native fail-closure,
and the deliberate absence of page/network bridges.

A separate offline target, `cargo xtask
materialize-macos-extension-compatibility-brokered`, is deliberately not an
implicit upgrade of v3. It accepts only an MV3 background extension that already
requires both `history` and `storage`, refuses source `nativeMessaging`
declarations, and adds the internal native channel required by
`macos.wkwebextension-brokered.v1`. Its background and every non-sandbox
extension page receive the exact versioned compatibility assets; ordinary
content scripts receive neither history nor extension-page message-response
authority. Sandboxed pages remain byte-identical. Requests are
clamped to the newest 100 deduplicated rows, response shape and byte ceilings
are revalidated in JavaScript, time and text filters run only over that bounded
cohort, and mutating methods remain absent. `onVisited` and `onVisitRemoved`
accept a bounded listener cohort so dependent extensions can initialize, but
do not emit until a future browser-owned delta operation is explicitly
designed.

WebKit returns `undefined` for asynchronous `runtime.sendMessage` responses to
an extension-origin iframe even while the same request reaches the MV3 worker.
The brokered profile therefore uses a narrow hybrid transport only in extension
pages: a reserved same-extension runtime port carries the request and preserves
WebKit's native `port.sender`; a random-ID response is JSON-normalized under a
1 MiB ceiling, placed temporarily in the extension's in-memory
`storage.session`, consumed through `storage.onChanged` plus bounded-backoff
reads, and removed on receipt or timeout. At most 128 listeners and 256 pending
requests exist per realm, each request has a 30-second deadline, cross-extension
signatures stay native, internal wake/port traffic is hidden from extension
listeners, and no hidden WebView or persistent worker is created. Promise and
callback call forms are supported; callback failures cannot synthesize
`runtime.lastError` and are disclosed as a degradation. The transform rejects
packages without a declared `storage` permission rather than silently adding
that authority.

When a brokered source declares `bookmarks` on a WebKit floor where that
namespace is absent, the artifact exposes an explicit empty read-only tree so
aggregate completion handlers terminate; bookmark mutations and events remain
unsupported and no Zephium item is invented. When it declares `favicon`, only
Chrome's unsupported `_favicon/` pseudo-URL is mapped to a sealed transparent
SVG; all other `runtime.getURL` calls remain native. Both adaptations are typed
in the receipt and install disclosure. The artifact metadata records all of
these degradations and the fixed internal-only native channel. Declared
`search` installs only `search.query` with current/new-tab dispositions and a
canonical bounded query; explicit window selection is rejected. Declared
`sessions` installs only `sessions.restore` for the most recent tab and reports
`MAX_SESSION_RESULTS = 1`; enumeration and explicit session identities are
absent. Both facades require their distinct effective permission and exact
native broker operation, validate the typed response, and create no timer,
worker, hidden view, or page-world bridge.
The ordinary `webkit-macos-native-v3` output and its authenticated third-party
hashes remain unchanged.

The exact Proton 1.39.0 source transformed through that generic boundary
produces a 279-file, 21,520,099-byte tree with tree SHA-256
`faa9115baeaabe8206168b9896dde0136e4e76ca05abaa07c7d636320692653f`.
The feature-gated native probe independently pins the source and output
identities before WebKit. After publishing the exact native window/tab surface,
it requires WebKit's public background-load completion, executes the real
popup, and uses the popup's granted `scripting.executeScript` API to inspect the
same extension's isolated world in the active tab. That attestation observes
the package-neutral prelude, mode `native-preserved`, and both native runtime
identities without exposing a marker to the page world. The popup independently
observes the package-neutral compatibility world and renders, and every native
object releases. The v3 artifact contains two deduplicated isolated navigation
routes because the untouched MAIN-world WebAuthn entry has a narrower exclusion
set than the ordinary orchestrator. The synthetic unauthenticated login page
still shows no Proton inline-autofill effect. The extension's own orchestrator beyond prelude
initialization, login, vault, save, autofill, and user workflows therefore
remain unassessed; this result proves reusable background, isolated-content,
and popup adaptation seams, not Proton compatibility.

A separately indexed, Zephium-owned MV3 fixture now gates the package-neutral
path without third-party bytes. `cargo xtask ci` materializes its exact source
tree through the same offline transform and executes the result in public
WKWebExtension APIs. On the reviewed macOS 26.6.1 runtime, native namespace
identity remained intact and locked in both the isolated content world and a
module background worker; a bounded background readiness barrier completed in
88 ms; `runtime.sendMessage` returned a synchronous response; a separate
`tabs.sendMessage` reached the content listener; the native sender tab was
present; the page world could not observe the adapter marker; and controller,
context, page, data store, and routing objects all released. The barrier uses
Apple's public
[`loadBackgroundContent`](https://developer.apple.com/documentation/webkit/wkwebextensioncontext/loadbackgroundcontent%28completionhandler%3A%29)
completion contract and does not retain a hidden WebView or prevent ordinary
MV3 suspension. This closes the transformed content/background execution gate;
the same source-free gate also exercises the reusable credential seam without
shipping or adapting a password manager. The isolated content host discovers
one exact username/current-password pair, creates a closed-shadow sandbox leaf
from a bounded public inert payload, authenticates its one-use selection by
exact child-window identity plus a random nonce, and obtains the synthetic
credential only through a sender-tab-bound background message. It fills through
the native input setter and the page observes the expected `input` and `change`
events. A page-world selection forgery is ignored, and the page observes
neither the closed leaf nor extension APIs. The CI gate waits for the exact
bounded leaf geometry, sends AppKit mouse-down/up events through the native
window, and accepts the selection only when WebKit exposes the resulting DOM
click as `isTrusted`. This proves native pointer routing, transport, isolation,
fill semantics, and teardown; it is not evidence of physical input hardware,
real vault behavior, or stock-extension compatibility. Real stock login,
vault, save, autofill, and complete user workflows remain product evidence
before any package is presented as installable.

The first authenticated ordinary-extension workflow uses stock
[Vimium 2.4.2](https://chromewebstore.google.com/detail/vimium/dbepggeogbaibhgnhhndojpepiihcmeb)
from its signed Chrome Web Store CRX. The external CRX SHA-256 is
`3198c26aa719be462dea585050fbed9b8b80628d57ea88a113babf5334c5517c`;
its authenticated source contains 79 files and 558,837 bytes with tree SHA-256
`5015a2e84b2007f0e9cfc670c06b327787bf55a3129fa382534e8a462748eb23`.
The generic v3 transform emits 82 files and 566,373 bytes with tree SHA-256
`63726bb7feb7195bcafb9d605abf3fc48b56f79eafc1c44fbcd1ce7dfe0563ec`.
The distinct brokered transform emits 90 files and 614,936 bytes with manifest
SHA-256 `c2b503f1593b173305889abbe7c06eb0bf060c1d038aa4434a05a0564433c8b3`,
tree SHA-256
`9285fa9ad16e220a366644355f1d2434dfba05dcab921765936b295723d05ff6`,
and canonical index SHA-256
`88917d255a2914fdc0a2a6f9e7fdb3faa46523be1ccda5c73331a127775da286`.
The transformed source tree is not committed or downloaded by ordinary product
code. An explicit non-shipping `staging-extension-catalog` build instead embeds
the externally signed deterministic Vimium CRX3
(`5bf9a4d8916959fca6b441b1ded47fa8dd615dffa2d1fb80314e5f9f4ec3187b`)
and the separately signed Dark Reader CRX3
(`523516c9550b15fd2cd97ce722348313da32deccb777caae4dc0a127e13d2c1a`)
under mixed catalog SHA-256
`8462dad95e46ed68bf03760607a293c99afd45f9b7b0a9cb98942ba5c858f0bd`.
The catalog selects Dark Reader's `MacosNative` profile and Vimium's
`MacosNativeBrokered` profile as one canonical cohort. That feature has a
separate application identifier and a fixed five-object in-process transport;
ordinary builds compile neither the catalog/package bytes nor a distribution
worker. On macOS, `cargo xtask ci` separately authenticates both runtime
selections and every embedded authority/distribution object, then lints the
complete staging desktop feature graph under the exact isolated Tauri
configuration.

The feature-gated release probe independently reopens both exact identities,
loads the module worker through WebKit's public background completion API, and
publishes a real native window/tab/controller surface. A separate plain WebView
first proves that the queued AppKit key becomes a trusted DOM event without
letting the diagnostic input alter Vimium state. In the extension view, Vimium
then intercepts the stock `j` command, drives its real link-hint selection into
an HTTP navigation, and executes its native action popup through the same
controller and profile store. The generated adapter and privileged extension
API remain absent from the page world. The link click is deliberately observed
as untrusted because Vimium dispatches it from JavaScript; WebKit must not
upgrade that synthetic click even though the initiating key was trusted.
Popup, page, control view, context, controller, data store, and native routing
objects must all release after the Objective-C autorelease pool drains.

The separately pinned brokered artifact follows the same native lifecycle and
opens Vimium's real extension-origin Vomnibar iframe with a trusted AppKit `o`
command. A deterministic probe input then exercises Vimium's stock input
handler and aggregate completion pipeline. The worker requests the exact
bounded history cohort through the principal-bound native delegate; the hybrid
extension-page response path returns the sanitized completion set; and the
rendered rows contain the exact brokered title and URL alongside any legitimate
tab completion. The same run proves action-popup execution, all seven
non-sandbox extension pages adapted, the empty-bookmarks degradation, the
transparent favicon fallback, exact native-message cardinality, and bounded
native teardown. The same exact callback sequence invokes Vimium's adapted
`search.query` current-tab path, `sessions.restore(null)` facade, and closed
`v1/options.open` operation before the history request. Shell and the native
context rebind the options request to the exact live runtime and transfer the
already-admitted popup lease into one standalone unprivileged settings
WKWebView. The adapter converts the exact late-bound manifest options URL into
an inert keyboard-accessible control. Ordinary DOM and keyboard activation use
the closed compatibility broker. The exact authenticated href remains on that
control as an accessibility fallback because WebKit can service `AXPress`
without delivering the page's DOM listener; the controller delegate admits
only `context.optionsPageURL()` into the same settings surface and rejects the
synthetic tab request. The settings view has no Tauri/Wry bridge, permits
navigation only within its exact `webkit-extension` origin, and routes only
user-activated HTTP(S) links through the ordinary bounded tab-creation broker.
Their real Shell effects are covered separately by
profile-bound integration tests: the accessory WebKit probe returns typed
success settlements and does not mutate the developer's browser session. The
deterministic input event is not evidence of physical
typing; the trusted `o` command and real rendered result are separate gates.

The bundled staging artifact now keeps the exact options URL on a genuine link
instead of rewriting the control's accessibility role. Its real popup keyboard
route (`Return` on the focused Options control) reaches Shell authorization,
opens the settings window, saves a changed scroll-step value, and reads that
value from a newly opened settings view. Computer Use `AXPress` on WebKit's
link still dismisses the native popup without delivering a DOM broker request
or a link navigation on the exercised runtime. That remains an accessibility
gap inside the extension-owned popup. The browser-owned management surface now
projects `has_options_page` from the authenticated manifest and exposes
Settings only for an active exact runtime. Shell rejoins catalog, install, and
runtime generation; Engine rejoins the native owner, controller, parent view,
and bounded popup-pool lease; and the same unprivileged settings window opens
and reopens after close. This is the supported assistive route until a physical
VoiceOver gate proves the extension-page path. A source-asset change never
mutates an already authenticated catalog package: the link-semantics change
was republished and re-signed under the same stable staging key.

The isolated packaged staging application exercises the complete profile path:
first-run profile registration, authenticated catalog synchronization,
permission review, install, enable, content/action execution, popup and options
presentation, disable/re-enable, uninstall, clean shutdown, and restart.
Revision 9 is active, with revisions 7 and 8 retained as the two bounded
rollback generations. Revision 7 kept Vimium's exact revision-6 package bytes
and added Dark Reader revision 1, proving that one catalog transaction can grow
from one brokered package to a mixed native/brokered cohort. Revision 8 keeps
both byte streams and advances Dark Reader to package revision 2 so its newly
observed background-lifecycle degradation cannot mutate an already admitted
profile in place. Revision 9 reuses every revision-8 package byte but advances
Dark Reader's update-line revision from 2 to 3 so the reviewed `alarms`
degradation reaches the normal consent transaction after the long-duration
native gate proved live delivery but no context-restart persistence. Its
metadata-reissue review, pre-review profile inputs, final
review, classified output, and hashes are retained together; revision 8 remains
immutable as rollback. Revisions 5 and 6 deliberately reused the same Vimium package
bytes to isolate the live runtime-generation update contract. The service
retires every regular/private native owner, Store atomically advances the
catalog, stable install, and package-bound grant revisions, and only then
reactivates the same install identity. Existing authority is reconciled by
exact declaration name/pattern; a newly required API or host permission returns
`AdditionalConsentRequired` with no durable write. An ambiguous SQLite commit
is resolved by reloading the exact install row before activation or rollback.
The reviewed migration advanced catalog 5 to 6, install 4 to 5, and grant 1 to
2, bound both durable rows to release tree
`1aeb98ac8f13495c52a7e8c0d5719c2b40eceaaf3ab8297f05d82eab9891c429`,
kept the runtime active, and reopened the popup after a full process restart.
The newer live revision-5 to revision-6 packaged gate additionally proves that
activation schedules one bounded post-load action invalidation, the trusted
toolbar replaces its stale runtime generation, and the popup opens immediately
without a process restart. A `RuntimeUnavailable` action settlement also
requests a metadata refresh as a fail-soft repair path. Install, enable,
disable, and uninstall settlements additionally compare the logical browser
surface generation: when the tab/window surface is unchanged but the runtime
cohort changed, Shell clears the prior action snapshot before requesting its
replacement. A disabled extension therefore cannot remain visible or
actionable merely because no tab mutation occurred or the replacement read is
delayed.

This gate establishes a reusable stock Chrome-extension path and core Vimium
keyboard/link/action behavior on the exercised macOS runtime. It does not yet
provide real bookmark results: bookmarks are explicitly empty/read-only, and
page favicons are transparent. Search is deliberately limited to the browser's
default provider in the current or a new tab, and session support restores only
the newest current-space tab; enumeration and explicit session identifiers are
unsupported. Notifications are absent/degraded, while multi-profile packaged
management behavior remains a release gate. The exact native Vimium gate
now edits, saves, closes, recreates, and rereads a real option through WebKit's
extension storage before proving both settings views release with the context.
Enable/disable, package update, popup/options presentation, shutdown, and
restart are covered by the isolated packaged gate. The unbundled
accessory probe is not granted foreground animation frames consistently, so it
records whether smooth scrolling was visibly observed but gates on ordered
trusted delivery plus Vimium interception; a packaged product-app E2E gate must
cover foreground smooth scrolling before Vimium is presented as fully
compatible.

Install candidates project file-URL and private-window availability separately
from manifest declaration. Both controls remain visibly unavailable on the
current product runtime, and Shell rejects forged `true` selections, because
post-WebView file-scheme execution fails its live gate and the browser has not
yet provided a separately isolated private browsing context. The macOS gate
creates three distinct extension contexts only after the product-shaped
WKWebView exists. Each exact `file:///*` match-pattern grant is accepted and
read back, but an `<all_urls>` isolated content script never executes in a real
local document during the bounded settle window; unload and cleanup also prove
no stale execution. A future WebKit runtime that begins executing fails this
negative gate so availability must be reviewed deliberately. Persisting either
flag and failing only during native activation is not an accepted degradation.

---

## 11. Frontend (frame)

Svelte 5 runes + TypeScript + Vite. A pure projection; only transient view
state.

- **One app, entry-routed by surface**: chrome (sidebar/topbar), overlay
  (launcher, palette, find, floating panels), internal pages (history,
  settings, graphs, notes, easel), onboarding. All share components, tokens
  and stores.
- **Styling:** Tailwind v4, token-first. Design tokens are CSS custom
  properties and the single source of truth; runtime theming (Vivaldi-grade
  customization) = swapping variable values, driven by the Themes domain.
  Bits UI for narrowly adopted headless accessible primitives; the look is
  ours. Native HTML and platform-native surfaces remain the default.
- **State:** store-per-domain, updated by granular domain events (§5).
- **Command mirror:** the frontend consumes the Commands registry; palette,
  launcher, menus and keybindings all dispatch the same stable command ids.
- Structure by feature: `app/ features/ ipc/ (generated) theme/ ui/ styles/`.

---

## 12. IPC contract

Typed end-to-end with **specta + tauri-specta** (commands, events, types
generated from Rust; CI fails on drift). Namespaced by domain (`items.*`,
`spaces.*`, `downloads.*`), not a flat list. Overlay surfaces use the thin wry
bridge with the SAME specta-generated types (one contract, two transports).
Commands validate inputs in Rust; errors are a serializable `AppError`.
Adding a command touches the capability manifest: a small, reviewable security
checkpoint.

Mutating commands return an `OperationAdmission` and later emit a correlated
`OperationDisposition`; IPC must not discard either admission failure or a
backend/store rejection. `Deferred` means the actor queued native/store work,
not that navigation, reload, or rendering completed. Native-runtime update
state uses the stable typed `zephium:runtime-status` event. Its security facts
are a bounded canonical set rather than one lossy highest-priority message, so
UI presentation can evolve without changing the native admission boundary.

---

## 13. Platform layer

Linux is not a supported platform at launch: `run()` in `desktop/src/lib.rs`
prints that Zephium isn't available on Linux yet and exits 1. The Linux code
stays in the tree and CI keeps it compiling, but no Linux build ships. Native
code is written against the OS it runs on; no blind ports. CI keeps all
targets compiling (§14). The current source-level/native checks do not replace the
packaged hostile-page and endurance matrix still required on real machines.

Per-OS map for the native seams (each a `cfg`-selected module with the same
inherent surface; no `dyn`):

| Seam | macOS (built) | Windows | Linux |
|---|---|---|---|
| Webview handle | `WebViewExtMacOS` -> WKWebView/NSView | `WebViewExtWindows` -> ICoreWebView2Controller | `WebViewExtUnix` -> webkit2gtk WebView |
| Chrome positioning | setFrame + autoresizing mask | `put_Bounds` on WM_SIZE | `gtk::Fixed` move + size |
| Content stage | NSView subclass, in-pass layout | child HWND + WndProc subclass on WM_SIZE | `gtk::Fixed`/`Layout` + `size-allocate` |
| Divider drag | mouseDown/Dragged/Up | WM_LBUTTON*/MOUSEMOVE | button/motion events |
| Drop indicator | layer-backed NSView | layered child HWND | GtkDrawingArea overlay |
| Window material | NSVisualEffect (window-vibrancy) | Mica/Acrylic (window-vibrancy) | solid (no portable blur) |
| In-app overlay | auxiliary NSWindow | Tauri tool window | separate GTK Tauri window |
| Global launcher | auxiliary NSWindow + native shortcut | tool window + native shortcut | X11 direct grab / Wayland XDG portal; compositor-owned Wayland placement |
| Content size | superview bounds | client RECT | GTK allocation |
| Per-pane corner radius | layer cornerRadius | skipped (no clean per-control rounding) | GTK CSS / cairo clip |

Bindings, direct per platform (no cross-platform UI abstraction layer):
objc2 + objc2-app-kit/foundation/web-kit/quartz-core (built); `windows-core` +
`webview2-com` matching wry's version; `gtk` + `webkit2gtk` matching wry's.
All already in the dependency tree transitively via wry; declared explicitly,
target-gated. Linux caveat: content views on Linux are created via
`WebViewBuilderExtUnix::build_gtk` into a `gtk::Fixed` owned by the
composition root (wry positions children only inside a Fixed;
raw-window-handle child-building is not the GTK path).

Current foundation status: seams live in `zephium-engine/src/platform/` and
`desktop/src/platform/` (cfg-selected modules, one per OS, same inherent
surface). All three implement chrome positioning, content layout, native
navigation probes, and per-profile engine partitions. On macOS the overlay
keeps Tao's allocation-time Objective-C class, while queued chrome layout uses
a main-thread-retained, generation/attachment-checked WKWebView that is
unpublished before release. On Windows, every fallible controller build owns a
typed cleanup plan for the parent subclass, controller `Close`, and child HWND;
unresolved steps return as retryable profile-attributed debt and consume native
resource budget. On Linux (not shipped), context properties fail closed. Packaged real-OS hostile tests are still a
release gate. Split-divider drag/drop indicators on Windows/Linux and per-view
rounded corners remain later platform work (rounded corners are intentionally
skipped on Windows for now).

---

## 14. CI

`cargo xtask ci` is the local gate (fmt, clippy, tests, frame check).
`.github/workflows/ci.yml` is the per-push gate: workflow lint, the frontend
gate (`tsc --noEmit` + biome + unit tests), and native Rust jobs on {linux,
macos, windows} runners so `cfg`-gated code cannot rot. `nightly.yml` adds the
fuzz, extended, component and coverage jobs; `security-review.yml` checks
review deadlines weekly; `release.yml` builds, signs and drafts a release from a
`v*` tag (see `.github/RELEASE.md`). `deny.toml` at the root governs licenses,
advisories and sources.
Later: sustained fuzzing for new parsers (filter lists, themes), `cargo geiger`,
packaged hostile tests, and endurance/resource gates.

---

## 15. Build order

Phase 0 is the only "rewrite" step in the plan; it exists so phases 1-5 do not
rewrite anything. Everything already proven (split math, stage, tiling, DnD,
chrome positioning) carries over as-is.

- **Phase 0 - foundation:** actor core + domain slices; ULIDs + Item tree +
  Window entity; migrations + FTS5; specta + granular per-domain events;
  Engine port v2 signatures; CI matrix + platform seam stubs.
- **Phase 1 - input and overlays:** Commands registry + native menus/
  accelerators + keymap; overlay system (macOS backend first); launcher v1 +
  palette.
- **Phase 2 - product model:** Item tree UI (pinned/folders/favorites),
  Spaces, Profiles (incl. incognito).
- **Phase 3 - cross-platform:** Windows adapter complete (chrome positioning,
  stage, dividers, indicator, overlays), then Linux (GTK stage + GtkOverlay +
  Wayland specifics), each verified on real hardware.
- **Phase 4 - table stakes:** downloads + MOTW, find-in-page, permissions
  prompts, history UI, full session restore incl. splits.
- **Phase 5 - differentiators:** Boosts + userscripts/userstyles, notes/
  tasks/easel, site-as-app windows, blocker source packaging/enablement per §9,
  Tier 1 extensions (Windows), data viz internal pages, html->md and clipboard
  commands.
- **Later, own track:** Tier 2 extensions compat, sync (change-log at the
  reducer chokepoint), SQLCipher/SecretStore.

Website file workflows are implemented for macOS foreground human tabs:
(see [file-workflows-windows-qa.md](file-workflows-windows-qa.md) for the shared implementation notes). Native
selection stays in the engine, with no durable upload manager or frontend file
path authority. A profile-scoped native WKDownload coordinator owns transfers,
Store-backed metadata/preferences and bounded progress snapshots. Private transfers
remain memory-only. Filesystem workers mediate staging, quarantine, exclusive
publication and file-identity checks. Trusted UI actions carry download IDs;
paths and native completions stay in Rust. Linux download adapters remain outstanding (Linux does not ship). Local-document viewing is outside this delivery's scope.

---

## 16. Reversible vs irreversible

**Irreversible - get right now:** domain model shape incl. Item tree and
Window (§6); state ownership (§5); logical tab != webview, scheduler seam
(§7); per-profile isolation + schema (§8 persistence, §6 Profile); ID
strategy (§6); tiling + stage + overlay physics (§3); injection pipeline
with isolated worlds + messaging (§8).

Each irreversible decision gets an ADR in `docs/adr/` so a future contributor
or agent does not unknowingly revert it.

**Reversible - evolve freely:** number of ports, crate boundaries, module
splits, CI depth, exact reducer composition.

---

## 17. Persistence

`zephium-store`, SQLite via rusqlite (the plain `bundled` feature ships
FTS5; a test guards it).

- **Storage is an actor**: rusqlite is blocking; a storage actor owns the
  shared metadata connection and per-profile connections and serializes
  writes; the async Store port is a message send.
- **The isolation scope is explicit**: durable history and favicon data live
  in per-profile databases. The registry, application settings, and complete
  restorable non-private session share `meta.sqlite`; profiles are therefore
  not separate file-level principals.
- **Incognito is rejected at the persistence adapter**: it is absent from the
  registry, session snapshot, history, and favicon databases. This is not a
  claim that native web engines, swap, crash artifacts, or the OS never write
  temporary private-session bytes to disk.
- **Forward-only versioned migrations** (`user_version`) from day one, each
  with an up-test on representative data. A schema newer than this binary is
  rejected, so rollback cannot write through a future format. The claimed
  version must also match a bounded full `sqlite_schema` manifest generated
  from the exact immutable migration prefix before migration DML and after
  every step; injected/replaced triggers, indexes, views, tables, and FTS
  shadow objects therefore fail closed instead of executing during migration.
- **Database-path admission rejects existing aliases.** The store pins its
  canonical data root, requires regular single-link files, creates new files
  exclusively with owner-only permissions, opens SQLite with `NOFOLLOW`, and
  rechecks platform file identity after open. This is not confinement against
  a malicious process already running as the same OS user: closing all
  directory-component and WAL/SHM races would require descriptor-relative
  filesystem operations and persistent ownership identity end to end.
- **Ancillary corruption degrades one profile, not the browser.** Once an
  exact authoritative meta snapshot exists, every registered profile database
  is securely opened read-only and schema-verified before read-write setup.
  A future/altered schema is left untouched; any failed ancillary database is
  explicitly reported by profile ID and disabled without retry or recreation.
  The session and healthy siblings remain usable. Without authoritative session
  truth, the same condition still stops startup. Only the crash-resumable
  profile-deletion journal can later authorize removal of the preserved file.
- **Authoritative snapshots are exact, not repaired on read.** An
  allocation-free lexical preflight bounds JSON strings, scalar tokens,
  structure, and depth before schema-aware bounded visitors allocate profile,
  space, item, or split collections. Unknown fields and any difference from
  the canonical session are quarantined; the original row is preserved and
  the store stays read-only pending an explicit recovery flow.
- **Profile deletion is a two-phase, crash-resumable application workflow.**
  One SQLite transaction commits the exact survivor snapshot and authorization
  journal before the in-memory tombstone/native retirement. Ambiguous
  authorization rechecks the journal and rebuilds from the current
  process-local session revision before a legal retry. Native verification is
  durably recorded before the authorized profile database is unlinked; the
  journal clears only after success and startup resumes unfinished phases.
  Packaged tests of all engine storage types and crash boundaries remain a
  production gate.
- The current schema contains the shared registry/settings/session snapshot
  and blocker preferences, plus per-profile history/favicons with FTS5,
  source-authoritative userscripts, remembered page permissions, and bounded
  structural extension install/grant authority. Those ancillary catalogs are
  durable input only: they do not prove native activation, package
  authentication, or live permission enforcement. The conservative native
  reconciliation journal is separate shared-meta ordering state. It does not
  itself authenticate a package or prove native activation. The implemented
  bounded service coordinator joins that journal to authenticated repository
  authority and lifecycle drains, while production native adapters and product
  activation remain disabled.

---

## Non-negotiables

1. The domain stays pure and `unsafe`-free.
2. No UI surface ever holds authoritative state.
3. All mutations to persistable aggregates go through their reducer.
4. Only `desktop` knows Tauri. Page-derived network access stays inside the
   exact profile-scoped native engine; app-owned downloads require dedicated
   policy components rather than a generic fetch port.
5. Content page worlds are NEVER given a bridge (see security-model.md). Any
   future content-native channel must be isolated-world-only, per principal,
   capability-scoped, natively identity-bound, and disabled until the
   architecture/security documentation and native hostile tests prove it.
6. `unsafe` only in engine/native adapters, each block documented.
7. In-app overlays position relative to the main window, never absolute.
8. A change is not done until `cargo xtask ci` is green on all three targets.
