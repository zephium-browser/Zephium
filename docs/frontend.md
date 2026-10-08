# Frontend architecture and conventions

Read this before touching `frame/`. It is the working contract for UI work;
`architecture.md` covers the system and `security-model.md` the trust rules.

## Stack (fixed, do not swap)

- Svelte 5 runes + strict TypeScript + Vite 8.
- Tailwind CSS v4, token-first. Bits UI supplies narrowly adopted accessible
  behavior; Zephium owns every visual component.
- Icons come only from the MIT/free packages `@hugeicons/svelte` and
  `@hugeicons/core-free-icons`, wrapped by `src/shared/ui/Icon`.
- Inter Variable are bundled through Fontsource and
  imported by the separate main and panel app roots.
- ESLint with the Svelte and TypeScript configs, plus Prettier with the Svelte
  and Tailwind plugins. `pnpm run check` is the frame gate: Svelte/TypeScript
  diagnostics, dependency boundaries, Stylelint, formatting, Knip, then unit tests.
- Vitest covers framework-free state and the small security-sensitive Svelte
  seams. Browser component tests run separately with Playwright WebKit on macOS and
  Chromium on Windows. They approximate the renderer; they do not qualify native
  focus, CSP, lifecycle, IPC or resource use.

Use platform-native controls and surfaces whenever they fit. Bits UI is not a
default component catalog: add a primitive only when native HTML cannot provide
the required behavior. Popovers must remain inside their owning chrome WebView.

## Visual system

- Premium and quiet, near-native in the macOS Graphite sense: neutral, near-white
  emphasis, no default accent color. Hierarchy comes from native material, spacing,
  typography and restrained neutral surfaces. Depth is one bounded glass recipe
  (`--shadow-control`, `--shadow-primary`, `--shadow-recess`): a soft top-lit gradient
  fill, a hairline rim of light on the upper edge and a 2px contact shadow. Real soft
  shadows exist only on things that physically float (`--shadow-float`,
  `--shadow-thumb`, `--shadow-popover`, `--shadow-overlay`). Prefer native material
  for window glass. CSS blur is allowed when it improves the interface and its
  bounded rendering cost is justified; avoid gratuitous or animated blur. This
  supersedes older blanket blur prohibitions in design documents.
- Colors and shadows come only through semantic tokens in
  `src/styles/tokens.css`, shared by both app roots. Never use raw palette utilities such as `white/10` in
  component markup. Every component must work in dark, light, reduced-motion,
  and forced-color modes.
- Use the 8 px spacing rhythm, compact 28–36 px chrome controls, and clear
  focus-visible states. Animate only transform/opacity or a narrowly justified
  color change; never animate browser geometry or startup.
- Customization is token- and domain-driven. Do not spread user preference
  conditionals through components or turn transient component state into a
  second settings system.

## Trust and native-boundary rules

1. **The frame is a projection, never authority.** Domain events update stores;
   user intent goes through generated `commands.*`. Do not optimistically claim
   native or durable success.
2. **The chrome cannot draw over page content.** Its WebView ends at the
   configured rectangle. Menus, context menus, and page-overlapping surfaces
   are native. A DOM popover is allowed only when fully contained in the
   sidebar; do not portal one across a WebView boundary.
3. **`src/shared/ipc/bindings.ts` is generated** by `cargo test -p zephium-desktop
export_typescript_bindings`. Never edit it by hand.
4. **Favicons stay fixed raster.** `shared/ui/FavIcon` accepts only the bounded
   `rgba32:` payload produced by Rust and paints a 32 × 32 `ImageData` canvas.
   Never replace this with `<img>`, a data URL, a custom protocol, or a
   privileged image-format decoder.
5. **Operation settlement stays exact.** `domain/operations/operations.ts` listens before
   reconciling, deduplicates process-local dispositions, and retries native
   acknowledgements. Accepted admission is not success, and the ledger does not
   survive process death.
6. **Production CSP is a release invariant.** `script-src` remains exactly
   `'self'`; production must never gain `unsafe-eval`, inline script, `data:`, or
   `blob:`. Svelte production output needs none of them. Dynamic native geometry and dropdown style restoration require style
   `unsafe-inline`;
   changing that is a separate hardening project, not migration cleanup.

## Native presentation barrier

Rust dispatches `zephium:presentation-tab` and inspects committed DOM in the
same synchronous expression before revealing raw page content. This is a
security boundary, not a rendering optimization.

- `domain/tabs/tabs.svelte.ts` installs all three scoped projection listeners before
  its first `await`, admits revisions through the framework-free
  `TabProjectionModel`, and publishes a presentation inside `flushSync`.
- `src/entries/browser.ts` calls `flushSync()` immediately after Svelte `mount()` so
  initial DOM and subscription work exist before bootstrap returns. Do not add
  an application re-entrancy guard; Svelte 5 supports nested synchronous
  flushing and a guard could reject a valid newer presentation.
- Keep exactly one shell with a direct
  `data-zephium-active-tab={active ?? ""}` binding.
- Render tab rows in an ID-keyed `{#each}` block. Every authoritative row
  directly binds `data-zephium-tab-id`, `data-zephium-tab-url`, and
  `data-zephium-projection-revision`.
- The label sentinel's sole child is `{tab.title}`. Do not add hidden text,
  icons, badges, or literal whitespace inside it.
- The address is a real `HTMLInputElement` with one-way `value={...}` and an
  explicit `input` handler. Accept native synthetic input; never gate it on
  `event.isTrusted` and never navigate from that handler.
- `data-zephium-new-tab` is conditional markup with no outro. Never leave a
  transitioning or hidden sentinel behind.
- Do not virtualize away an active/presenting row or create drag/responsive
  duplicates carrying any sentinel attribute.
- Never move a barrier field through `$effect`, `tick`, a microtask, an
  animation frame, or an awaited callback.

## Startup and surface lifecycle

- `browser.html` and `panel.html` load their own `src/entries/` modules. Each
  validates its native window label, mounts its composition root and calls
  `flushSync()` synchronously. The panel uses `panelReady()`, never `uiReady()`.
- See `design/launcher.md` for the launcher: search-only host, native glass
  shapes, trigger settings, lifecycle and verification status. Production builds
  emit and enforce `frame/reports/bootstrap-report.json`, which also fails if the panel can
  reach an editor or tool view at all; do not add browser features to the panel.
- `onboarding.html` is a first run's page in the main window. Native picks it
  over `browser.html` before either loads (`zephium_app::onboarding_due`), and
  `onboarding_finish` replaces it with the browser in the same window: the
  chrome view stays hidden until the browser passes the same `uiReady` gate as a
  normal launch. It is built separately (`vite.onboarding.config.ts`) so it never
  splits code out of the browser graph; the build fails if the browser can reach
  any onboarding module, and reports to `frame/reports/bootstrap-report.onboarding.json`.
- `frame/bundle-budgets.json` records each page's startup graph and what each
  lazy destination adds on top of what is already loaded when it opens: its
  page and every destination that must have run to open it. The build fails only
  when one grows more than 16 KB JS or 8 KB CSS past its recorded size, or a new
  destination adds over 48 KB without one. Shrink it first; if the growth is
  intended, run `pnpm run budgets` and say why in the commit.
- Theme initialization applies the system mode and subscribes to theme commands
  before its first native query. The main surface installs projection listeners,
  resolves theme/material, synchronously forces style/layout, and only then
  calls `commands.uiReady()`.
- Keep the opaque bootstrap colors in `src/styles/axes/bootstrap.css` byte-exact with
  the canvas tokens (`#1a1a1d` dark, `#f1f1f3` light). Both root stylesheets import
  this file. Do not add inline `<style>` blocks to either privileged HTML entry:
  Tauri adds style nonces, which override `unsafe-inline` and prevent dropdowns
  from restoring pointer input. A hidden WebView
  may suspend `requestAnimationFrame`; startup must not depend on one. Native's
  15-second watchdog intentionally fails closed instead of revealing partial
  privileged chrome.
- Every domain store exposes an idempotent `init()/dispose()` pair. Runes stores
  use the `.svelte.ts` suffix; pure admission/reconciliation models stay normal
  `.ts` and are tested without a framework.

## Structure and ownership

```text
src/
  app/            composition roots, assembling independent features through snippets
  features/       product slices: sidebar, tabs, essentials, address, spaces,
                  extensions, blocker, permissions, launcher, newtab, settings, tools, library, split, notes, tasks, work
  session/        shared transient state with one WebView-document lifetime
  domain/         Rust projections and typed intent: tabs, layout, surface,
                  preferences, appearance, operations, runtime, permissions, credentials, extensions, blocker, resources
  shared/         IPC transport, UI primitives, pure helpers, platform traits, testing
  styles/         semantic tokens and the global styles being migrated to component ownership
```

Imports flow `app -> features -> session -> domain -> shared`. Features generally do not
import sibling features. Explicit composition exceptions in `architecture.config.js`
allow the utility host to load Notes/Tasks and a Work host to consume entity public
APIs. Entity features never depend on Work.
ESLint resolves TypeScript aliases and enforces these directions and public entry
points. Cross-module imports use `$app`, `$features`, `$session`, `$domain`,
`$shared`, or `$styles`; imports within a module stay relative.

Every consumed feature/domain module exports its public API through `index.ts`.
Lazy surfaces expose loader functions so the public API does not turn navigation
into eager loading. `Sidebar` owns its frame, header, footer and resize handle;
`app/browser/Shell.svelte` supplies its product bodies. New Tab receives Settings preview
values as props; it never imports the Settings feature.

Rust owns persistent state, security decisions, native geometry, operation outcomes,
and product state shared between windows. The frame owns interaction state whose
lifetime is one document. Feature-local drafts stay with their feature; transient
state shared by multiple features belongs in `session/`. Real entity editor drafts
are user data and must autosave to Rust when those entities are implemented.

`domain/operations/settle.ts` consolidates admission and disposition waiting.
Its `deferred` outcome is not completion: callers requiring a durable or native
effect must observe the authoritative projection. `shared/lib/lifecycle.ts` supplies
generation guards and partial-registration cleanup; presentation barriers keep their
synchronous admission and `flushSync` behavior.

Observation lifetimes are explicit. `shared/lib/observe.ts` bounds native promise
observation and releases retry timers on abort. Aborting observation does **not**
cancel an admitted native effect. `settle` retains the native operation ID when
confirmation is unavailable; its `observation_aborted` reason is not a runtime
cancellation outcome. Ledger responses must match the requested ID and current
observer lifetime before admission.

Preferences and browser-surface initialization are single-flight. Preference startup
reads and write readback are bounded, disposal invalidates their observations, and
newer events take precedence over older reads. The readback fallback remains until
Rust supplies a truthful committed preference projection. An inability to confirm a
write is not proof that the write did not happen.

The Browse disposition ledger is not the Work command protocol. Work uses the
runtime-owned profile/Work scope, decimal revisions and replayable command receipts
specified in [agent-work-persistence.md](agent-work-persistence.md); do not route Work command IDs through the
Browse operation-ID parser.

The session sidebar/tools modules and the string UI-command transport are transitional
while the unified typed surface migration remains incomplete. Do not use them as patterns for new
product state. Placeholder tool views stay in `features/tools`; they are not Tasks,
Notes or Work implementations. No placeholder entity directories are created.

## Notes and Tasks host behavior

The Notes/Tasks root is both a flex item and an inline-size query container. Give
it an explicit width, flex growth and a zero minimum width; size containment
otherwise collapses it inside the native sidebar, and without it a long title or
preview widens the whole panel. Test both ToolSlot hosts with production sidebar
CSS, including list/detail navigation and launcher return.

Notes save on their own cadence: a background save is one
write, explicit navigation and app close drain everything typed, and conflicts,
retries and read-only notes are shown in place. Hidden hosts stop observation and
release saved text, retaining the open note's ID for a fresh read on return.

Only the open note mounts an editor, in its own lazy chunk with `marked`. Lists,
the panel and the page never parse Markdown: titles and previews come from the
native index, and the list derives only the open note's row as it is typed.
Notes-only icons are imported per icon (`@hugeicons/core-free-icons/<Name>`) so
they stay in notes chunks instead of the startup icon chunk.
The 1,800-paragraph projection test is a local CPU sample, not native latency or
process-memory qualification.

## Tests and migration gates

Features use `components/`, `lib/` and module-local `tests/`, with only `index.ts`
at the root. Domain slices keep focused source files and `tests/`. Do not create
empty role directories. Browser tests use `.component.test.ts`; other `.test.ts`
files run in the unit project. App scenarios stay with their composition root.

`shared/testing` provides typed binding mocks, scoped event emission and fixtures.
Unexpected native commands fail tests. Production builds reject testing and gallery
modules from every emitted JavaScript chunk, including lazy chunks.

`pnpm -C frame check` runs types, ESLint, Stylelint, formatting, Knip and unit tests.
`pnpm -C frame test:component` runs real browser-component interaction tests.
`pnpm -C frame build` enforces the startup graph; `cargo xtask check-frame-styles`
checks emitted CSS after a build. The local unit gate does not replace native QA.

Knip and Stylelint migration exceptions list exact existing files in
`knip-migration.ts` and `stylelint-migration.js`. They are temporary debt,
not permission to add new exceptions. Remove each with the corresponding migration.

Rust markup contracts reference `desktop/src/frame_sources.rs`; update its anchor
in the same change as a source move. The CSS tonal gate checks built styles so
component-scoped CSS will remain covered. Bootstrap-paint and presentation-sentinel
contracts still have independent Rust tests.

Global shortcuts remain native. The small set consumed by the chrome WebView is
matched in `app/browser/BrowserApp.svelte` and dispatched through the Rust Commands registry.
Theme/material attributes are currently owned by `domain/appearance/theme.ts`; appearance remains
persisted and allowlisted on the Rust side.

## Shared interface system

See [the interface system](design/system.md) for the visual contract. Settings is
the current product surface for inspecting controls; Interface Studio is removed.
The old unused `shared/ui/work` kit has been removed. Future entity presentations
share one UI kit and gain folders only with their real runtime contracts.

The retained logo asset is `assets/brand/zephium-logo.png`, outside the shipped bundle. Its placement in the
product belongs to the visual refinement work; this migration does not substitute
it for the existing wordmark or change its pixels.

`UiInfo.material` reports a per-window enum. Native material updates still use the
scoped `material.*` UI-command transport until typed appearance projections land.
Preserve startup-query versus live-update ordering. CSS effects must preserve
native surface boundaries and meet the visual/performance guidance above.

## Work presentation foundation

Use `features/work` through its lazy loaders. Shared semantic renderers live in
`shared/ui/data`; their display types are not Work IPC contracts. XYFlow, SVG
LayerChart and the constrained Tiptap editor load on demand. Tables remain lightweight.

Open Work using the browser sidebar mode switch. Rust admits the internal surface
and suppresses native page WebViews; returning to Browse uses the existing verified
chrome-restoration handshake. `app/browser/WorkWorkspace.svelte` mounts only an empty canvas foundation. It loads
no Notes/Tasks stores and makes no decisions about the final Work product layout.
There is no standalone Work demo or production fixture transport. Agent execution
is unavailable until runtime integration. Deterministic scenarios remain tests only.
The copied runtime contract was removed; the frontend does not depend on a local
Work wire-schema copy. Runtime-owned definitions remain with the runtime stream.
