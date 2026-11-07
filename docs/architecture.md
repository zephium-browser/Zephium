# Architecture

Zephium is a native Rust browser with a Tauri + Svelte interface.
Windows and macOS have equal support priority. Linux is unsupported;
Linux-specific source and tool-host CI jobs do not establish browser support.

This guide maps ownership and the boundaries a contributor must preserve.
For setup and checks, use [CONTRIBUTING](../CONTRIBUTING.md).
For detailed trust rules, use the [security model](security-model.md).

## 1. Composition and ownership

`desktop/src/lib.rs` is the desktop composition root. It constructs native
services, opens storage, starts the application actor, registers IPC, and wires
native callbacks and shutdown. It is where Tauri meets the browser services.

`zephium-app` is the actor shell around `zephium-core`. UI intents and engine
callbacks enter its queue; the actor owns application state, applies domain
transitions, invokes ports, and publishes projections. Blocking I/O belongs in
adapters or workers, rather than in reducers or a renderer event handler.

`zephium-core` owns domain types, reducers, navigation policy, layout math, and
port contracts. It forbids unsafe code. Change the domain here when browser
meaning changes, then adapt the application, storage, engine, and IPC edges.
A frontend store is a projection or transient interaction state, not a second
source of browser authority.

## 2. Workspace map

The Cargo workspace includes `crates/*`, `desktop`, and `xtask`.
Platform implementations are generally selected inside their owning crates.
These groups locate work; they do not define a dependency graph.

| Area | Owners and entry points |
| --- | --- |
| Browser state and orchestration | `zephium-core/src/`, `zephium-app/src/` |
| Desktop and frontend | `desktop/src/`, `frame/src/`, `zephium-ipc/src/` |
| Native browser | `zephium-engine/src/host/`, `zephium-engine/src/platform/` |
| Persistence and private files | `zephium-store`, `zephium-private-fs` |
| User resources and import | `zephium-notes`, `zephium-import` |
| Content protection | `zephium-blocker`, `zephium-blocker-service`, `zephium-blocker-update`, `zephium-update-transport` |
| Extensions | `zephium-webext`, `zephium-webext-macos`, `zephium-webext-windows`, `desktop/src/webext/` |
| Browser execution contracts | `zephium-agentic`, `zephium-agent-controller`, `zephium-agent-runtime` |
| Work composition | `zephium-work-composition`, `zephium-app/src/work_*`, `desktop/src/work_*` |
| Provider and decision services | `zephium-agent-provider-transport`, `zephium-agent-model-catalog`, `zephium-decision` |
| Account and credentials | `zephium-account`, `zephium-credentials` |
| Sidebar assistance and voice | `zephium-page-assist`, `zephium-voice`, desktop adapters |
| External protocol integration | `zephium-mcp` |
| Development and qualification | `xtask`, `scripts/`, `zephium-terra-macos-probe` |

Read manifests and module headers before moving behavior across boundaries.
Qualification crates and probes do not establish shipping product behavior.
Vendored native dependencies have their own provenance and maintenance
requirements; see CONTRIBUTING before changing them.

## 3. Native window and process model

The Rust process owns application policy, persistence coordination, native view
lifetimes, and the event loop. System web engines supply their own renderer and
network processes. A logical tab is application state; it is not identical to a
live native WebView or an operating-system process.

Trusted chrome, overlays, and internal pages load bundled interface assets.
Remote browsing content uses raw Wry/native WebViews, not a privileged Tauri
content window. Remote pages must never receive the application IPC bridge.
Trusted UI and untrusted pages sharing a window does not make them peers in
capability or authority.

The native stage hosts content views and applies pane geometry. Rust owns view
attachment, focus, input routing, and retirement. Svelte cannot position remote
content by placing an iframe in its DOM. Overlays and menus must cooperate with
native focus and stacking rather than assume normal web-page layering.

macOS uses WKWebView and AppKit; Windows uses WebView2 and Win32/COM adapters.
Engine callbacks and native object access must respect their platform thread
and lifetime requirements. Keep platform-specific operations in the appropriate
`platform/` and `host/` modules, with application policy expressed through ports.

## 4. State, commands, and IPC

The typical path is:

1. A trusted surface submits a typed intent through its permitted transport.
2. Desktop authorization checks the calling surface and validates the request.
3. The application actor applies policy and transitions domain state.
4. Native or storage ports perform effects and return correlated outcomes.
5. Projections and events update the interface.

`zephium-ipc` owns shared DTOs. Rust command/type registration and generated
TypeScript bindings keep the interface aligned; follow the existing generation
workflow instead of editing generated bindings. See [frontend](frontend.md)
for interface structure and IPC consumption rules.

Authorization belongs at the Rust boundary even when a control is hidden or
disabled in the UI. Validate identifiers against the current profile, window,
item, and operation context. A typed payload alone does not prove authority.

Mutating APIs distinguish admission from disposition. `Deferred` means queued
work, not completed navigation or a successful durable write. Preserve both
admission failures and later backend failures; correlate asynchronous replies
rather than treating command return as proof of native completion.

## 5. Persistence and profile boundaries

`zephium-store` provides SQLite persistence behind a dedicated actor. It owns
blocking database work and forward migrations. Keep schema changes and migration
tests with the store; keep domain decisions in core/application code.

Shared metadata contains the profile registry, settings, and restorable
non-private session. History and favicon data use per-profile databases.
Profiles therefore have explicit logical and native-engine boundaries, but are
not separate operating-system users or wholly separate file-level principals.

The persistence adapter excludes private-session state from durable session,
history, and favicon storage. That guarantee does not describe every temporary
artifact a system engine or the operating system may create.

Restoration must preserve authoritative state or report recovery failure.
Do not silently repair unknown/newer schemas or recreate a failed ancillary
profile database. Profile deletion coordinates durable authorization, native
retirement, and file removal; bypassing that workflow can leave live engine
storage or remove data without settled authorization.

`zephium-private-fs` owns capability-oriented private filesystem operations.
Account secrets belong behind `zephium-credentials` and account services;
renderer state, general preferences, and IPC projections are not secret stores.

## 6. History, navigation, and launcher

History is a native-backed browser feature. `zephium-app` handles history
requests and completion, the store owns durable records and queries, and
`frame/src/features/history/` owns presentation and transient query state.
Launcher search ranks and deduplicates destinations; the history library pages
individual visits. Keep those store queries separate. Desktop/native adapters
own native list integration and history menus.
Distinguish durable browsing history, a tab's back/forward list, and closed-tab
session state when implementing navigation or deletion.

The launcher combines a trusted interface with native activation and focus.
`desktop/src/launcher_trigger.rs` and platform adapters own shortcut delivery;
`desktop/src/overlay.rs` owns presentation, focus, placement, and settlement;
desktop command handlers and the application own search/action dispatch.
Frontend selection or a result label must not become arbitrary command, path,
or navigation authority. Use validated targets and the command registry.

Launcher appearance follows application preferences; keyboard capture,
activation, dismissal, and return of focus require native coordination.
Keep UI interaction conventions in [frontend](frontend.md), and verify native
behavior on both supported hosts when changing the activation path.

## 7. Work and sidebar assistance

Work has a desktop composition and a Svelte workspace. Application modules own
sessions, planning, resources, execution, persistence, and recovery;
`zephium-work-composition` joins trusted native/store adapters without owning
Tauri UI. Runtime and controller crates provide reusable execution mechanisms.

Browser execution must retain native context identity, lifecycle ownership,
policy admission, and completion evidence. A provider reply is input to these
mechanisms, not permission to execute an arbitrary browser action.
See [Work](work.md) for product composition and [agentic browsing](agentic-browsing.md)
for the browser-execution substrate and qualification references.

The sidebar service lives in `zephium-work-composition/src/page_assist/`, with
`zephium-page-assist` contracts and native readers. It has its own conversation
and lifecycle. Do not infer Work resource or action authority
from access to sidebar text, or treat the two interfaces as the same session.

## 8. Native services and file workflows

Website file selection, downloads, permissions, authentication prompts, and
engine storage are native boundaries. The trusted interface may present choices
and operation IDs; Rust retains native handles, validated paths, and completion
ownership. Page callbacks do not gain general filesystem or application access.

Keep page-derived networking tied to the exact native profile/context.
App-owned transport services, such as update acquisition, have separate origin
and package policies; they are not a general page fetch API.

Native changes need real host evidence for focus, callbacks, teardown, storage,
and hostile-page behavior as applicable. Compilation and pure tests establish
different facts from packaged runtime qualification. Consult CONTRIBUTING and
the existing platform QA guides when choosing checks.

## 9. Choosing the owning layer

- Change browser invariants and transitions in core, orchestration in app.
- Change native view behavior in engine; desktop wires services and trusted IPC.
- Change durable formats and queries in store, preserving migration contracts.
- Change presentation in frame, using generated IPC and backend projections.
- Change extension or protection policy in its dedicated subsystem.

Cross-cutting work should trace one request through those layers. Avoid adding
a second state owner or a generic privileged bridge to make an edge convenient.

## 10. Userscripts and extensions

Userscript injection and installed browser extensions are distinct paths.
The Windows CDP module explicitly refers to this section: its isolated-world
userscript feasibility probe is gated and is not a shipping userscript fallback.
An unavailable or failed gate leaves Windows userscripts disabled; injecting
into a remote page's main world is not an acceptable substitute.

Any userscript enablement must establish native principal/document identity,
isolated-world messaging, frame coverage and ordering, bounded resource use,
navigation-generation binding, debugger coexistence, and terminal cleanup.
Live runtime evidence must cover isolation and performance, not just successful
compilation or a returned script result. The blockers and probe contracts live
beside `zephium-engine/src/platform/windows/cdp.rs`.

Installed extension management is wired into ordinary desktop startup on both
Windows and macOS: desktop registers management commands, constructs the
manager, restores enabled installations, and starts updates. It is not globally
disabled behind an opt-in desktop feature.

`zephium-webext` handles package verification, extraction, manifests, permissions,
and preparation. The platform crates and engine adapters own native runtime
integration; desktop owns install review and management. Compatibility remains
API-, extension-, runtime-, and platform-specific. Startup wiring does not prove
universal Chrome extension compatibility or every permission's availability.
See the [extension guide](extensions-user-guide.md) and
[release scope](extension-release-scope.md) for user-facing limits and evidence links.
