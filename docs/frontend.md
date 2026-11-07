# Frontend engineering

Zephium is a native Rust browser with a Tauri + Svelte interface. Windows and
macOS have equal support priority; Linux is unsupported. The interface projects
native state rather than owning browser authority. Read [architecture](architecture.md)
for native boundaries and [CONTRIBUTING](../CONTRIBUTING.md) for setup and host checks.

## Layers and imports

The frame uses Svelte 5 runes, strict TypeScript, Vite and Tailwind CSS.
`frame/architecture.config.js` enforces the dependency direction:

```text
app → features → session → domain → shared
```

- `app/` assembles composition roots and independent feature bodies.
- `features/` owns product views, local drafts and interaction behavior.
- `session/` owns shared transient state with one WebView-document lifetime.
- `domain/` admits Rust projections and exposes typed user intent.
- `shared/` provides transport, primitives, pure helpers and test support.
- `styles/` owns semantic tokens and shared styling.

Across modules use aliases and public `index.ts` APIs; use relative imports
inside a module. Consult the explicit composition exceptions in the architecture
configuration before adding a sibling-feature dependency. Do not broaden them
merely to avoid extracting a shared primitive. UI primitives receive props and
callbacks; they do not import native transport or domain stores.

Features keep rendered composition in `components/`, behavior in `lib/`, and
verification in `tests/`. Domain slices keep focused projection/admission/intent
files and `tests/`. Controllers do not import rendered components or their own
barrel. Create directories when they have real contents. Heavy views expose lazy
loaders rather than eager component exports.

## Ownership, IPC and lifetimes

Rust owns durable entities, permissions, secrets, native geometry and execution
outcomes. Feature-local interaction belongs in the feature; shared transient
interaction belongs in `session/`. User data drafts need the entity's native
save contract. A local checkbox or optimistic state change cannot prove a commit.

Send intent through generated `commands.*` in `shared/ipc`; do not import generic
Tauri invocation or event APIs in product modules. Scoped DOM projections preserve
caller and surface authority. Never invent parallel Work wire schemas: runtime
identities, revisions and transitions remain runtime-owned. Display-only chart and
component types are distinct from IPC. See [Work](work.md) for the product protocol.

`desktop/src/lib.rs::export_typescript_bindings` generates `bindings.ts`,
`work-constants.ts` and `theme-presets.json` under `frame/src/shared/ipc/`.
Change the Rust definitions and regenerate; never hand-edit these outputs.
The regeneration recipe and hosted drift-check scope are in
[AGENTS.md](../AGENTS.md#generated-files-and-forks).

Admission is not completion. `domain/operations/settle.ts` joins admission and
Browse dispositions; deferred confirmation still needs authoritative observation.
Its process-local operation ledger is separate from Work command receipts.
Aborting an observation does not cancel an admitted native effect. Keep operation
IDs and observer lifetimes intact when validating responses or exposing retry.

Stores expose idempotent initialization and disposal. Use `.svelte.ts` for runes
and plain `.ts` for framework-free models. Release subscriptions, retry timers and
partial registrations on disposal; use the shared lifecycle/observation helpers.
Newer events must outrank stale startup reads. Render/effect error boundaries do
not settle asynchronous commands: handle their failures and retries explicitly.

## Native surfaces and presentation

The chrome WebView cannot paint across its native rectangle. Page-overlapping
menus and context menus are native; DOM popovers stay inside their owning chrome
surface. Preserve CSP, caller scope and the native chrome/page separation.
Production `script-src` remains exactly `'self'`; do not add inline scripts,
`unsafe-eval`, `data:` or `blob:` script sources.

The native tab-presentation barrier inspects committed DOM synchronously before
revealing page content. Preserve these contracts when moving or restyling UI:

- Install scoped tab listeners before the first await; admit revisions through
  `TabProjectionModel` and publish presentation inside `flushSync`.
- Flush synchronously after mounting the browser entry. Do not defer barrier
  fields through effects, ticks, promises or animation frames.
- Keep one shell with a direct `data-zephium-active-tab` binding and ID-keyed
  authoritative rows binding tab ID, URL and projection revision sentinels.
- The title sentinel contains only the exact title text. The address remains a
  real input with one-way value and an explicit input handler that accepts native
  synthetic input and does not navigate.
- New-tab sentinels have conditional markup without an outro. Never duplicate
  sentinels in drag ghosts or responsive copies, or virtualize a presenting row.

Update `desktop/src/frame_sources.rs` in the same change whenever a source anchor
moves. `shared/ui/FavIcon` paints only Rust's bounded `rgba32:` raster payload to
its fixed canvas; do not substitute image URLs, protocols or privileged decoders.

Browser, panel and onboarding have separate entries. The launcher panel is
search-only and uses `panelReady()`, never `uiReady()`. Keep editor/tool views out
of its graph. Onboarding is built separately and must not enter the browser graph.
Theme/material subscriptions precede startup queries; settle style/layout before
announcing readiness. Keep bootstrap canvas colors aligned with tokens and avoid
inline style blocks in privileged HTML. Hidden views may suspend animation frames,
so readiness cannot depend on one. Global shortcuts remain native.

## Interface rules

Use the existing browser controls as the visual reference: quiet neutral surfaces,
clear hierarchy, optical alignment and restrained depth. Semantic colors, role
radii, type and shadow tokens live in `frame/src/styles/tokens.css`. Do not use raw
palette utilities or size-named radius classes. Prefer native material for glass;
bounded CSS blur needs a justified rendering cost and must not animate.

Keep the 8 px rhythm with 4 px subdivisions and compact chrome controls. Buttons
and inputs retain native semantics; submit buttons explicitly use `type="submit"`.
Associate stable IDs with labels, help and error text. Keep icon buttons named,
status text explicit, keyboard focus visible and interactive controls unnested.
Test light/dark, forced colors, RTL, text expansion and reduced motion. Use logical
CSS. Localized copy comes from message catalogs and compiled Paraglide messages;
do not hand-edit `shared/i18n/` output or persist fake locale settings.

Use shared motion tokens through CSS or `shared/lib/motion.ts`. Prefer transform
and opacity; preserve reduced-motion paths. Keyed list departures use inert ghosts
stripped of native sentinels. Reuse list motion, selection-plate and sidebar-shape
helpers rather than creating competing gesture or measurement loops.

Notes/Tasks hosts need explicit width, flex growth and zero minimum width with
inline-size containment. Test list/detail navigation in both actual ToolSlot hosts.
Only an open note mounts its lazy editor; list previews come from the native index.

## Charts

Use `loadChart` and `ChartSpec` from `shared/ui/data/Chart`. The kit draws SVG;
`chart.ts` owns parsing/formats, `plan.ts` data planning and `geometry.ts` paths.
Use `styleSpec` for supported style mappings. Keep missing values as gaps and retain
source display text/evidence. Name charts for assistive technology and provide exact
values through the kit's table or the owner's equivalent. Use semantic chart tokens,
respect reduced motion, and keep the chart behind its lazy loader. Display specs
must not become a second runtime artifact schema.

## Verification and emitted assets

Run from the repository root, following the same frontend commands as CI:

```sh
pnpm --dir frame run check
pnpm --dir frame run build
pnpm --dir frame run check:styles
```

`check` includes i18n generation/typechecking, ESLint, Stylelint, formatting, Knip
and unit tests. Browser-component tests are separate (`.component.test.ts` versus
unit `.test.ts`); use `shared/testing` mocks and fixtures. Production graphs must
never import tests, fixtures or development scenario adapters. Component engines
are WebKit on macOS and Chromium on Windows; see CONTRIBUTING for installation,
commands and hosted trigger coverage. They do not qualify native focus, CSP,
lifecycle, resources or cross-platform behavior.

The build checks complete emitted static graphs, lazy destinations and forbidden
imports. `frame/bundle-budgets.json` owns graph budgets; measure new destinations
and explain any reviewed increase. Do not raise limits to hide regressions.
The emitted-CSS gate follows the build, covering component-scoped styles too.
Migration exceptions are existing debt, not templates for new exceptions.
Report native Windows and macOS evidence separately when native contracts change.
