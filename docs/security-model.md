# Zephium security model

Zephium renders hostile, attacker-controlled code by design. This document describes
the guarantees implemented in the current tree, not planned product features. Clearly
labelled product-disabled contracts define additional gates that must hold before their
feature can be enabled; they are not current guarantees. If code and this document
disagree, the code is the evidence and the mismatch is a release blocker.

Zephium is a browser shell over OS WebViews. The application can harden its own
boundaries, data lifecycle, and native API surface, but it cannot turn WKWebView,
WebView2, or WebKitGTK into the same engine or promise controls that an engine does not
expose. In particular, a multi-process WebView is not automatically equivalent to
Chromium's full site-isolation model.

## Trust zones

1. **Rust/native process (trusted).** It owns authoritative application state,
   persistence, native windows, and the WebView lifecycle. A compromise here has the
   current user's OS privileges.
2. **Application WebViews (privileged, semi-trusted).** The `main` chrome and `panel`
   load bundled application assets and can invoke a deliberately small Rust command
   surface. Both are created as incognito/non-persistent WebViews so the application
   origin does not intentionally retain cookies, cache, service workers, or web storage
   between launches. An XSS in either view is still a security event; CSP and ephemeral
   storage are defense in depth, not reasons to trust its input.
3. **Tab WebViews (untrusted).** Raw Wry child WebViews render arbitrary web content.
   They receive no Tauri command bridge, no Wry IPC handler, and no Zephium custom
   protocol handler. The application injects fixed cosmetic CSS plus protected
   page-world bootstraps for discard-safety observation, bounded HTML extraction, and
   scripted-print denial. Those scripts expose no native bridge or application
   authority; all injected page-world code has exactly the page's trust level.
4. **Extension execution (unprivileged; ordinary-product-disabled).** The architecture reserves
   one non-reusable native principal per installed extension. Content scripts may run
   only in engine-provided isolated worlds scoped to that principal. Extension-owned
   background and UI surfaces do not inherit application-WebView privilege and receive
   neither generic Tauri/Wry IPC nor a generic custom protocol. Any enabled native
   registration must bind the profile, install, principal, and generation and never
   accept identity from a JavaScript payload; only capability-specific,
   permission-checked brokers may cross into zone 1. This is the required enablement
   contract, not a current product claim: production adapters remain disabled until
   platform-native hostile tests enforce it. A manifest-declared extension sandbox is
   not itself a fifth trust zone on macOS: the live WKWebExtension gate proves such a
   page retains its `webkit-extension` origin and `chrome` API, even when its extension
   URL is placed in an explicit `sandbox="allow-scripts"` iframe. A separate live gate
   proves the permitted replacement primitive: an authenticated content script fetches
   a public inert payload and instantiates it through a Blob-backed `allow-scripts`
   frame whose message origin is `null`, whose extension APIs are absent, and whose DOM
   is inaccessible to the parent. Product provisioning remains disabled until the
   pinned package deterministically adopts that topology and its authenticated message
   flow passes hostile tests. Native naming, CSP text, or the `sandbox` attribute alone
   is never accepted as isolation evidence.

   The explicit macOS staging build is the sole current exception to ordinary
   product provisioning, not an exception to this trust boundary. It uses an
   isolated bundle identifier and fixed embedded catalog transport. Extension
   action popups and settings views are native WKWebExtension surfaces with no
   application bridge. An options view admits only its exact extension origin;
   programmatic remote navigation is cancelled, while a user-activated HTTP(S)
   link may request one ordinary Shell tab through the bounded browser-request
   broker. Opening options consumes the existing popup-class native resource
   lease rather than allocating outside the process ceiling. If its WebContent
   process terminates, the weak navigation delegate schedules a later main-turn
   teardown and releases only the options owner whose exact context and
   WKWebView still match; a delayed callback cannot close a replacement.
   A same-principal `tabs.create()` request for an internal extension page uses
   a separate native extension-document host. The exact retained context and
   URL stay inside the controller delegate; Shell receives only a typed request
   to authorize the foreground profile, and `webkit-extension:` never enters
   the ordinary URL or persistence model. Presentation rejoins the context,
   principal, controller and the one foreground-extension resource lease before
   constructing a WKWebView from the context's own configuration. The bounded
   logical window/tab pair is visible only to that controller. Foreign
   principals, inactive or unselected configurations, invalid window/index
   targets, parent tabs, pinning, muting and reader mode fail closed. Internal
   navigation remains exact-origin only. A user-activated external link opens
   an ordinary Shell HTTP(S) tab; an allowed programmatic top-frame transition
   does the same and then retires the privileged extension page, while external
   subframe navigation is denied. Window close, renderer termination, context
   retirement and shutdown detach delegates on a later main-queue turn where
   necessary, balance WebKit close notifications and return the same lease.
   An opt-in compatibility receipt may select WebKit's nonpersistent document
   background for an authenticated module worker when the worker resource
   transport cannot complete. Manifest admission binds that environment into
   compatibility identity, and the transform retains no hidden `WKWebView`.
   The document receives only two service-worker-shaped failure facades: a
   frozen empty `clients.matchAll()` cohort, and native-messaging calls that
   disconnect or reject without resolving a host, serializing a payload, or
   contacting a process. Existing native-messaging functions make adaptation
   fail closed. The receipt discloses the missing WindowClient inventory,
   absent host, and callback denial without `runtime.lastError`; none of these
   facades grants native or page-world authority.
   WebKit's document-background fallback receives one additional native wake
   hint for an exact provisional top-level URL. The host first rejoins the
   physical view's event permit, current navigation epoch and canonical target,
   its profile partition, the complete published runtime fingerprint, and the
   exact retained native owner. It calls the public background-load API only
   when the authenticated manifest selected the document environment and
   WebKit reports injected content for that URL. Redirect replacement and
   per-view coalescing cannot move an old target forward. The hint grants no
   host permission, script execution, page-world channel, view allocation or
   persistence, and service-worker runtimes keep WebKit's native event path.
   Failure degrades the extension request without changing ordinary browsing.
   Native extension keyboard commands use no startup monitor. The observer
   exists only while a context is loaded, gives browser-menu accelerators first
   refusal, and accepts only the focused resident regular tab's WKWebView. It
   resolves every canonical context without executing, reauthenticates matches
   against published runtime ownership, and rejects collisions. An exact
   `_execute_action` match carries no geometry or native object across the
   engine boundary: Shell rejoins the current action revision and privileged
   chrome may return only its already-rendered browser-owned button rectangle
   through the ordinary action command. Actor revision ordering, a one-second
   gesture deadline, exact one-shot consumption, and the existing native
   revalidation prevent delayed or replayed shortcuts from minting popup
   authority. Monitor-removal failure poisons clean
   shutdown rather than leaving an untracked input observer.
   Extension page-menu integration runs only in Wry's native `menuForEvent:`
   callback for a controller-bound content view. The default `NSMenu` and
   extension `NSMenuItem` objects remain opaque; page context is never copied
   into Rust. Exact view/tab/runtime checks precede a fixed-cap merge, and the
   full submenu tree has identity, depth, count, and title-byte ceilings.
   Malformed or stale native inventory returns the unchanged default menu and
   poisons extension-controller integrity rather than presenting partial items.
   The opt-in long-duration alarm classifier uses only a Zephium-owned fixture
   and extension-local storage. It proves live `alarms.onAlarm` delivery, then
   verifies that current WebKit discards the next alarm when the exact context
   unloads/reloads before its deadline; this limitation is disclosed rather
   than repaired with a hidden view or an unbounded background worker.
   The native WebKit contract also exercises both sides of web-accessible
   resources from the same hostile product page: one declared dynamic script
   loads and executes, while an exact present-but-undeclared extension script
   is rejected and leaves no execution marker. A package file is never treated
   as page-readable merely because an isolated extension principal can derive
   its private URL.
   `unlimitedStorage` is product-prohibited on both native macOS grant schemas.
   WebKit exposes per-extension data-size records and deletion, but no quota
   setter; reactive inspection cannot prevent a granted extension from filling
   disk between observations. Feature-only probes retain the native permission
   enum for platform evidence, while product activation fails before removing
   WebKit's finite default quota.
   Manifest support for file URLs or private browsing does not imply product
   availability. The current management projection marks both unavailable and
   Shell rejects a forged grant selection before Store admission; those flags
   stay closed until their native data-isolation and execution gates pass.
   On macOS, three post-WebView contexts accept and read back an exact file
   match-pattern grant but do not execute their declared isolated script in a
   real local document. That negative live gate prevents permission metadata
   from being mistaken for usable file authority.
   A replacement adding required API/host authority or a new compatibility
   degradation cannot silently retire the old runtime. Shell retains an exact
   catalog-set/package/install/grant-bound review and gives chrome only an
   opaque echo token. Approval is freshly reauthenticated and Store advances
   package plus required grants atomically; optional, file, and private grants
   are never inferred from update consent. Closing the review leaves the old
   package live, and stale tokens cannot authorize a later replacement.
   Uninstall reauthenticates the stable Chromium identifier, retires matching
   runtimes, removes only that identifier's bounded WebExtension data record,
   and proves persistent readback zero before deleting the Store install. A
   timeout is not absence and prevents later management writes in the process;
   peer extension records are never included in the removal array.

The structural boundary between zones 2 and 3 is the most important application-owned
control. A tab is a separate raw WebView, never a navigation of the privileged chrome.
The boundary between zones 1 and 2 is caller-labelled IPC, Rust validation, and Tauri
capabilities.

Profiles are a **web-data privacy boundary**, not a separate OS principal or a defense
against a compromised Rust process, engine sandbox escape, or local account. Native
engine cookie/storage contexts are separated per persistent profile. History and the
favicon cache use per-profile databases, while the profile registry, settings, and
complete restorable session for all non-private profiles share `meta.sqlite`. There is
currently no application-level encryption at rest.

## Enforced invariants

Any change that breaks one of these invariants must fail review and release:

1. A raw tab WebView is built without privileged IPC or a content-reachable custom
   protocol. Page script cannot invoke Zephium commands.
2. `main` and `panel` are constructed at exact `about:blank`, receive all mandatory
   native hardening, and only then navigate to the bundled application origin. Debug
   builds additionally allow the exact local Vite origin. Other navigation is rejected.
   On Windows the same policy is installed on WebView2's separate subframe-navigation
   event, and every request to launch an OS-registered URI scheme is cancelled.
3. Application-requested tab navigation is checked in core and again before the native
   load; page-initiated navigation is checked by Wry's navigation handler. Only `http`,
   `https`, and exact `about:blank` are allowed. URLs are limited to 8 KiB; credentials,
   reserved application hosts, `file:`, `javascript:`, external app schemes, and other
   `about:` pages are rejected. Explicit malformed or forbidden URL-like omnibox input
   (including local path-shaped input) is not converted into a search request, and the
   final percent-encoded search URL must fit the same native limit. Rust records a tab's
   URL only from the native committed-source observer, including same-document History
   API changes; a stale explicit-navigation failure cannot cancel a newer request.
4. Privileged commands authorize the Tauri-injected caller label and validate or clamp
   IDs, URLs, strings, and coordinates in Rust. The panel has no generic Tauri
   capabilities; the main view has only its required window controls. Rust projections
   are delivered directly to a fixed target label, so the panel cannot select the main
   view as a generic event-listener target. The macOS browser-credential query is
   main-only, on-demand, and reads authorization state without enumerating credentials
   or relying parties. Passkey authorization requires a trusted UI request, is
   process-single-flight, executes on the native main thread, and settles only to the
   fixed main label.
5. Raw content permissions are denied by the pinned Wry permission callback.
   Privileged and agent downloads/new windows remain denied. Human macOS and
   Windows tabs opt into the native download and new-tab brokers described below;
   Linux retains denial. No new window is forwarded to an unmanaged OS browser.
   Linux additionally cancels file-chooser requests from both raw and privileged
   WebViews. macOS raw foreground views opt into a native upload picker bound to
   the exact view/navigation and labelled with WebKit's initiating-frame origin;
   privileged and agent views retain file-selection denial. macOS also denies
   privileged media capture and device motion natively.
6. Both privileged WebViews are created with incognito mode and their platform-native
   store is checked to be InPrivate/ephemeral/non-persistent. That check and every
   platform hardening hook must succeed before bundled application assets or page script
   load, or startup aborts. Release WebViews have developer tools disabled; macOS also
   explicitly clears `inspectable`. Privileged responses receive a restrictive CSP plus
   `Permissions-Policy`,
   `Referrer-Policy: no-referrer`, and `X-Content-Type-Options: nosniff`.
7. Incognito profiles are excluded from session snapshots, history, and persistent
   favicon records. A private tab may keep its fixed-size favicon raster in process
   memory for the current run; the persistent store rejects an incognito profile again
   at its adapter boundary.
8. Shutdown is an ordered durability barrier: commands already accepted by the shell
   precede the final session snapshot and SQLite flush. A flush failure occurs before
   native shutdown is invoked and leaves the reusable shell component available for a
   host-controlled retry. The desktop close path is still terminal: the same eight-second
   deadline covers storage admission, durability, native teardown, blocker
   compiler/updater/service termination, acknowledgment, and UI exit, so an unproven
   flush exits non-zero instead of leaving an unclosable window. The blocker service
   passes that unchanged absolute deadline through its layers and gives each worker only
   the remaining duration; it does not restart the timeout per layer.
   Once native shutdown is invoked, a negative native result, rejected main-thread
   dispatch, unproved blocker-worker exit/join, or missing acknowledgment is likewise
   terminal. On Windows that outer deadline contains the separate five-second WebView2
   process-group proof plus bounded private-UDF removal. An unproven outcome is never
   reported as clean teardown.
9. Page-derived values remain untrusted and bounded before crossing into application
   state. Titles are stripped of control characters and limited to 512 characters, and
   HTML extraction has character and serialized-result limits. The Wry adapter accepts
   only primitive-string IPC and enforces a shared 64-KiB UTF-16/UTF-8 ceiling before
   constructing a Rust `String`; raw tabs have no IPC handler at all. Privileged local
   protocol requests independently cap method, header count/name/value/aggregate size,
   and stream at most 64 KiB of request body before invoking Tauri. Native engines may
   materialize their own request objects first, but Wry does not duplicate an oversized
   value into Rust. Favicon fetching and image decoding stay in the untrusted page
   renderer. Candidate scanning and polling are bounded, including at most one fresh
   pass after the exact document reaches load completion. Rust accepts only an exact,
   canonical 32-by-32 RGBA raster for the current navigation epoch and origin; chrome
   paints that fixed raster without parsing a page-controlled image container.
10. Linux is not a supported platform at launch: `run()` in `desktop/src/lib.rs` prints
    that Zephium isn't available on Linux yet and exits with status 1 before any
    runtime is touched. The Linux engine code stays in the tree and keeps the
    admission described here, but none of it is exercised by a shipped build. On
    Linux, browsing requires WebKitGTK 2.54.0 or newer. Older builds,
    odd-minor development builds, and unrelated major lines fail closed. A
    newer stable even-minor WebKitGTK 2.x line is admitted with a visible
    unreviewed-runtime advisory rather than a numeric-version kill switch.
    Every Wry context, persistent or ephemeral, is constructed with top-level
    cross-site process swapping enabled. The
    requested Web-process sandbox flag and the construct-only swap policy are read back
    and asserted before the context can own a WebView. Those properties alone are
    configuration invariants, not confinement attestation; no in-renderer or
    equivalent-credential filesystem probe exists.
11. On Windows, privileged native hardening requires `ICoreWebView2Environment10`, and
    every privileged or raw view requires `ICoreWebView2_18` so external URI schemes can
    be cancelled natively. Raw views additionally require `ICoreWebView2Settings7` to
    remove PDF Save, Save As, and Print controls. Before any view is created, startup
    rejects every documented loader/browser-argument/channel and script-debugger
    environment override, including empty values, and the reported runtime must be a
    parseable Stable Evergreen build. A runtime below the reviewed security floor is
    admitted with an update-recommended advisory; preview-channel, overridden and
    unparseable runtimes are refused with a native alert and exit status 78. Startup
    aborts if privileged hardening cannot install; an individual raw view is rejected before its first load if any
    mandatory setting, navigation, external-URI, or process-failure handler cannot install.
12. On macOS, startup occurs before any WebView construction and refuses only an
    unsupported system: macOS older than 14, Safari older than major 26, or an
    unparseable version, with a native alert and exit status 78. The canonical
    system Safari bundle and the framework actually supplying `WKWebView` must have
    the expected identifiers. Supported releases below the reviewed Sonoma, Sequoia,
    Tahoe or macOS 27 recommendation (including Safari for that line), a Safari/WebKit build mismatch (typically Safari
    updated without a restart), and a newer stable OS/Safari major are admitted with
    an update-recommended or unreviewed-runtime advisory.
13. The content-policy lifecycle never treats absence as allow-all. Every profile must
    install an exact process-local generation—an explicit allow-all artifact while
    disabled, or a validated blocking artifact—before its first raw view/navigation is
    admitted. Compile and native callbacks are generation-checked; a replacement becomes
    authoritative only after native cohort installation succeeds. A clean failure may
    retain the exact previous generation, while an initial failure leaves raw navigation
    held. Unsupported or all-omitted rule sets cannot be published as enabled protection.

## Implemented controls

**Chrome/content wall.** The chrome and panel are Tauri-managed local-asset WebViews;
tabs are independently constructed raw Wry WebViews. No Tauri API is injected into a
tab. The privileged CSP has no remote script source, no object source, no form target,
and no release `unsafe-eval`. This reduces an application XSS's exfiltration options,
but its real blast radius remains the commands and data available to that caller. Both
privileged views request an incognito/non-persistent engine store; authoritative UI
state still lives in Rust and is persisted only through the validated store commands.

**Navigation.** Omnibox input is normalized by the core policy and every native load is
checked again. Explicit forbidden URL-like input is rejected locally rather than leaked
to the configured search engine; search expansion is bounded before admission.
Page-initiated top-level navigation uses the same allowlist. On Windows, raw subframe
navigation also uses that allowlist, privileged subframes remain on the app origin, and
WebView2's external-URI event is cancelled for every view. No blocked scheme is
automatically opened through the OS shell. Human macOS/Windows new windows require
native user-activation admission and become host-owned tabs as described below.
Back, forward, and stop use native WebView operations instead of page-overridable
JavaScript history/window calls. Native URL/history observers are mandatory on all three
platforms. They deduplicate bounded state and close the view if an engine source escapes
the URL policy, so chrome never presents a pre-navigation URL over a forbidden document.

**Permissions and downloads.** Geolocation, notifications and other permissions
routed through Wry's permission callback are denied in every view. Display
capture uses separate native paths: ordinary human OS-picker consent is not a
camera/microphone grant, and a deny callback alone does not prove agent display
capture containment. That boundary still requires native qualification. Camera and microphone are
the only exceptions, and only for ordinary human tabs. On Windows, those tabs defer
camera and microphone to WebView2's own origin-labelled prompt
(`PermissionResponse::Prompt`); Zephium stores nothing in the profile, and work,
extension and privileged views stay deny-only. On macOS, Zephium replaces Wry's
permission callback with a browser-owned broker that is on by default (the
`macos-page-permission-prompts` desktop feature). It uses WebKit's structured
security origin, retains the native completion on the main thread, and binds it to the
exact profile, item, physical view generation and committed navigation. It admits only
the focused resident tab, loads no catalog at startup, serializes one browser-owned
prompt, and has independent exact Shell, navigation, close/retirement, 25-second Shell
and 30-second native timeout paths. Malformed origin data, unsupported capabilities,
overflow, stale identities, callback panic, navigation, focus loss, window hide and
shutdown all deny. Decisions are one-time only: `remember_enabled` is false, so
nothing is written to the profile until the browser can list and revoke grants. The
store and its atomic remembered-choice path exist behind that switch and are covered by
tests, not by shipped behavior. macOS capture indicators observe native Active/Muted
state and stop the exact resident document through WebKit; provisional navigation
does not hide ongoing capture. Windows has no equivalent verified capture-state
projection. Packaged device-permission, stop and iframe-lifetime checks remain required. Extension pages and offscreen documents deny media
capture. The bundle carries camera and microphone usage descriptions and the matching
`desktop/Entitlements.plist` entries; those grant no authority by themselves.
[WebKit requests system validation before its UI-client policy
decision](https://github.com/WebKit/WebKit/blob/main/Source/WebKit/UIProcess/UserMediaPermissionRequestManagerProxy.cpp),
so even a native Deny can first prompt or wait on device consent, and the
[mock-capture setting used by WebKit tooling](https://github.com/WebKit/WebKit/blob/main/Tools/MiniBrowser/mac/WK2BrowserWindowController.m)
is not a shipping WKWebView API. The feature-only loopback probe is therefore compiled
and linted in macOS CI but not executed there. On macOS, privileged views use a retained
deny-only UIDelegate for media capture, device orientation/motion and file selection;
optional dialog and popup methods are omitted so WebKit takes its cancel/no-dialog
defaults. All privileged responses also deny ambient features through
`Permissions-Policy` where the engine supports each directive. A signed packaged
WKWebView/TCC check on the supported OS lines remains a release gate for the macOS
broker. Foreground human macOS tabs
use the native download broker described below. Foreground human Windows views
now opt into the WebView2 download adapter described below; privileged and agent
downloads remain denied (Linux code also denies them, but Linux does not start). Windows Attachment Services and Mark-of-the-Web
are implemented but await native Windows qualification. Linux also cancels privileged file-picker requests. Stable WebView2 exposes no supported file-chooser interception event, so a
raw Windows file input remains an engine-owned, user-selected native upload surface and
privileged Windows views have no equivalent native denial hook. This is an explicit
platform limitation, not a broker Zephium has implemented. Raw Windows views disable
browser accelerator keys and initially disable default context menus. Human views
re-enable native menus only after installing a bounded command allowlist and a
mandatory SaveAsUIShowing cancellation handler; privileged/agent views retain
menu denial. Native edit/copy, link opening and download save actions remain;
print, document Save As, sharing and unknown commands are removed. Built-in PDF
Save, Save As, and Print controls remain hidden before the first load. WebView2 likewise exposes no event that can cancel page-initiated scripted
printing. Zephium locks the page and `Window`-prototype print functions and wraps and
locks both `Document.prototype.execCommand` and the document's own `execCommand` before
any page-owned script. The wrapper coerces a command exactly once, rejects
normalized `print`, and delegates other commands using that same primitive. A trusted
chrome print command calls WebView2's native print API directly. This renderer-layer
guard is defense in depth, not a native denial guarantee; packaged hostile-page testing
(including `window.print()` and `execCommand('print')`) and explicit acceptance or a
future native broker remain stable-release gates.

`HiddenPdfToolbarItems(PRINT)` hides WebView2's built-in PDF toolbar control; it does
not document a denial of embedded PDF actions. A PDF `/Named /Print` or clickable print
action can bypass the DOM guard through the PDF viewer. Until Zephium can disable or
broker the built-in viewer with a supported native API, packaged malicious-PDF testing
and an explicit risk decision are separate Windows stable-release gates. Zephium does
not claim that the current source tree denies this path.

**Human native new tabs.** On macOS and Windows, native user-initiated links,
modifier clicks and script-created windows use the original native opener and
request. macOS disables automatic JavaScript windows in WKPreferences; Windows
requires NewWindowRequested.IsUserInitiated. Admission additionally checks the
source generation, navigation activity, presentation, foreground window, profile
lifetime, resource limits and a bounded burst of eight requests per second.
Raw page IPC cannot mint this authority. Unsupported URLs and construction or
adoption failures deny the request. macOS uses WebKit's supplied configuration
with a fresh per-view script controller; Windows uses the same environment and
profile and registers request filters after SetNewWindow, before completing its
deferral. The host never retries a POST/blob/navigation as a plain URL GET.

Rust owns each child through an exactly-once adoption lease. Abandoned adoption
retires the native controller. Foreground requests select their child only after
its exact committed-document presentation is admitted, and a newer user tab
selection cancels that deferred focus. Background requests keep the source
selected. The first native download chain has a one-shot, 30-second admission
allowance while its child is hidden; all destination consent and filesystem
checks still apply. Acceptance or terminal cancellation removes an uncommitted
transient child, including after a space switch. A child that committed a real
document is retained. Native script-close notifications only close host-owned
children; ordinary browser tabs remain host controlled.

macOS native image/link context-menu downloads use the optional private WebKit
`_webView:contextMenuDidCreateDownload:` callback to receive the original public
WKDownload. This is not a public API stability guarantee: supported macOS/WebKit
versions require native qualification, including the minimum OS. Without this
callback the browser does not synthesize a replacement network request. Native
context-menu wording is retained. A host-rejected new-tab request projects a
bounded popup-blocked status; popups rejected inside WebKit before delegate
invocation may have no host status event.

**Human file uploads on macOS.** Raw content construction opts into Wry's
native upload callback. The default remains denial before request metadata is
read. The engine retains one process-main-thread NSOpenPanel reservation, labels
it with the canonical HTTP(S) origin supplied by WebKit for the initiating
frame, and requires a live, presented, visible view in the key window before
opening it. WebKit owns input user activation and the original input/frame
association; Zephium does not infer authority from a recent generic click.
Single/multiple/directory modes come from native parameters. Selected NSURLs
stay native and are returned only to the original WebKit completion, with at
most 1024 selected entries and no filesystem paths in frontend IPC or storage.
A non-cloneable, non-Send responder cancels on drop. A unique request identity
prevents a late sheet callback from settling a newer request. Navigation start,
renderer exit, removal from the presented layout and view teardown cancel the
panel. Settlement rechecks the view permit, committed navigation, navigation
activity (including failed attempts), visibility and exact window attachment,
including after native sheet dismissal. There is no persistent filesystem grant.
The public pinned WKOpenPanelParameters API exposes selection modes but not
HTML accept filters; this adapter does not invent those filters with page JS.
Linux upload selection remains disabled; Windows selection retains the engine-owned
behavior described above. macOS human views opt into native WebKit file drops with
a 128-item bound, URL bounds, and document/presentation checks at entry and drop.
Privileged and agent views retain default denial.

**Human downloads on macOS.** Raw human views explicitly opt into a WKDownload
broker; the native network request retains its WebKit profile, cookies, POST body,
and blob ownership. No URL is replayed through a separate HTTP client. At most
eight transfers run; an accepted transfer survives source-tab closure. Destination
panels share the main-thread upload reservation. Pending decisions are bound to
the initiating view, navigation activity, intended presentation and key window.
A direct attachment response may arrive before a document commit; that download
intent does not grant permission to reveal page content.

Rust owns transfer state, bounded profile history and native paths. The trusted
UI receives typed metadata and issues ID-scoped actions. Private-profile records
remain in memory. Default behavior asks for a destination; a native directory
selection records its filesystem identity before automatic saves are permitted.
Same-volume private staging is journaled before native bytes are admitted.
Publication applies and checks quarantine, synchronizes the file, and uses an
exclusive rename with collision suffixes. Open/Reveal verify the recorded file
identity; interrupted staging cleanup requires the original directory receipt
and removes only the fixed payload and empty staging directory. Shutdown and
profile erasure drain native cancellation, filesystem work and Store replies.
Recovery marks abandoned transfers Interrupted; it does not claim resumability.
Migration 21 separates cleanup ownership from visible history, so forget/pruning
cannot discard a pending receipt. Startup enumerates active and deletion-pending
profiles without a history view. Recovery is paged, profile-scoped, identity-checked
and retriable. Tombstoned profiles permit only internal cleanup and terminal saves.
Private receipts stay in memory and drain on orderly closure; abrupt process death
can leave an incomplete hidden staging directory at the selected destination.
Quarantine is OS provenance, not malware scanning.

**Human downloads on Windows.** The adapter retains the original WebView2
DownloadStarting event/deferral and operation. Default cancellation and hidden
native download UI precede host admission. Cookies, POST bodies and blob/data
payloads remain native. Pending destination selection requires the exact live
view/navigation/presentation and foreground parent. Automatic
saving is not inferred from a recent click or a concurrent browser navigation:
the event has no initiating-frame activation/navigation identity proof, so every
Windows download requires native confirmation. HTTP origins
are displayed without URL secrets; origin-less data downloads label their top
page origin as context, not an initiating-frame assertion.

Directory handles pin canonical ancestors; a protected staging DACL admits only
the current user and SYSTEM. File identities retain all 128 Windows file-ID bits.
Publication invokes Attachment Services with the proposed filename/source origin,
requires an Internet zone stream, flushes the payload and moves it without a
replace/cross-volume-copy flag. Protection failure cannot produce Completed.
Recovery never recursively deletes and refuses an open native writer. Uncertain
shutdown preserves the writer PID plus creation time and its durable receipt.
Closing the logical tab revokes page authority, hides/disables scripts and parks
the retained controller at about:blank until its transfer ends. Native process
exit proof joins download drain during shutdown/profile retirement. These Windows
paths have been cross-checked, not yet qualified on a native Windows runtime.
See [Windows qualification and implementation notes](file-workflows-windows-qa.md).

**Profiles and storage.** Persistent profiles use distinct native engine data
partitions: profile paths/contexts on Windows and Linux and named WKWebsiteDataStore
identifiers on macOS. Windows also reuses one WebView2 environment per profile rather
than spawning an unrelated browser process group for every tab. SQLite runs in WAL
mode with `synchronous=FULL`; every opened connection also enables SQLite's defensive
mode and cell-size checking, disables trusted-schema behavior and double-quoted string
literals, and keeps temporary storage in memory. The complete restorable non-private
session is committed as one meta-database transaction. History is capped at 50,000 rows
per profile and the favicon cache at 512 origins per profile.

`user_version` is not treated as sufficient schema identity. Before migration DML and
after every committed migration step, the store compares a bounded inventory of every
`sqlite_schema` object (including indexes, triggers, FTS shadow objects, and their DDL)
with a reference manifest generated from the same immutable migration prefix. Unknown,
replaced, oversized, future-version, and non-boundary schemas fail closed before normal
writes begin. Settings admission likewise reserves each new key atomically before
reporting success, so the 128-key durable limit cannot accept an operation that the
storage actor later drops.

Database paths are derived from one canonical application-data root. Existing files
must be regular single-link files; new files are created exclusively with owner-only
permissions; SQLite opens the final component with `NOFOLLOW`; and the path's platform
file identity is checked again after open. These checks reject symlink and hard-link
aliasing present at admission. They do not create an OS security principal: another
malicious process already running as the same user can still race directory components,
SQLite WAL/SHM sidecars, or later filesystem operations. Eliminating that stronger
same-UID attacker requires descriptor-relative directory handles and durable ownership
identity throughout the store and erasure paths, and remains outside the current
website-threat guarantee.

A valid authoritative session is not held hostage by one damaged ancillary profile
database. Each existing registered profile file is first opened through the protected
read-only path and matched to an exact shipped schema before any read-write setup. A
future, altered, or unreadable schema is left untouched. Any profile that fails that
preflight—or later fails bounded migration/configuration/budget enforcement—is reported
in a sorted bounded degraded cohort and disabled for the rest of the process; it is not
retried, recreated, or used for reads or writes. The exact session and healthy sibling
profiles continue. A forged, duplicate, or foreign degradation report stops bootstrap,
while a profile without an authoritative session still fails closed instead of guessing.
The ordinary two-phase deletion journal may later remove that exact degraded file only
after the user-authorized profile deletion and native erasure proof.

The focused window's profile and space are an authorization scope for runtime tab,
split, popup, and native-layout operations. A same-profile favorite may appear across
that profile's spaces; ordinary tabs and every split leaf must belong to the focused
space. Restored state is checked against the same rule before any native view or engine
partition is created.

Session loading distinguishes a genuinely absent snapshot from corruption or I/O
failure. Failure leaves the shell uninitialized and unable to overwrite the recoverable
state. Authoritative JSON first passes an allocation-free lexical preflight that bounds
individual strings, scalar tokens, nesting, and total structural tokens. Schema-aware
visitors then bound profile, space, item, and split-tree allocation while rejecting
unknown fields. The decoded value must already equal its exact canonical form; Zephium
never uses canonicalization to silently drop damaged rows. A malformed, oversized,
noncanonical, or registry-mismatched snapshot preserves the source row, records a sticky
recovery marker (and the exact corrupt bytes when they fit the outer snapshot bound),
and leaves the store read-only until an explicit recovery flow exists. Profile-file
reconciliation or deletion is authorized only after an exact snapshot and its complete,
canonical, non-duplicate profile registry agree.

Incognito means no intentionally durable Zephium session, history, favicon record, or
privileged-UI website data. Each privileged WKWebView receives a newly created
non-persistent store, and each privileged WebKitGTK view receives a newly created
ephemeral context. Raw private tabs in the same profile share one retained
nonpersistent `WKWebsiteDataStore` on macOS and one explicitly ephemeral WebKitGTK
context on Linux; different private profiles cannot share either object. Every macOS
tab still receives a fresh configuration, and post-build attestation proves its
WebView uses the exact profile-owned store. The vendored Wry patch rejects a durable
supplied context when incognito mode is requested.

WebView2 requires a user-data directory even for an InPrivate controller. Each run uses
a fresh, non-guessable generation below Zephium's dedicated
`web-content/private-runtime` root. Privileged `main` and `panel` controllers use
separate UDFs in a fresh generation below `privileged-runtime`, so the less-capable
panel does not share the main view's cookies, storage profile, or WebView2 session. For
raw content and privileged chrome, cleanup of the current generation follows an exact
Environment5/PID/process-HANDLE proof. The privileged registry retains both environment
guards after its windows are destroyed, requires the exact `main`/`panel` label set and
two distinct browser PIDs, and gives apartment-affine exit callbacks a bounded two-second
message-pump tail after `run_return`. Missing, invalid, aliased, late, or HANDLE-
contradicted proof leaves the generation quarantined and makes a clean exit unsuccessful.

Both runtime roots use the same generation manager. It holds an exclusive root lease,
writes a versioned exact-generation marker, and creates a matching per-generation key
below a volatile HKCU registry key. [Windows discards volatile registry data when the
system shuts down](https://learn.microsoft.com/en-us/windows/win32/api/winreg/nf-winreg-regcreatekeyexw),
so a correctly marked generation whose volatile key is absent has crossed a reboot
boundary and may be reclaimed without inferring process death from filesystem access.
Pre-provenance Zephium directories are first bounded, marked, and classified as
same-boot; they cannot be reclaimed until a later reboot. Malformed or unowned objects
are never adopted or deleted. Startup is bounded to 32 same-boot generations, 128 total
including the new allocation, 2 GiB of quarantined logical file bytes, and 250,000
inspected entries. Saturation fails with an explicit full-Windows-restart remediation;
opaque data that remains after reboot requires the runtime directory to be moved aside
manually while Zephium is closed. Successful exact cleanup removes its volatile subkey,
so clean relaunches do not grow HKCU for the rest of the boot.

Separate UDFs cost an additional WebView2 browser session/process set and must remain in
resource benchmarks. Consequently, "incognito never writes a byte to disk" is **not** a
guarantee on Windows. Crash, power loss, swap, filesystem snapshots, and engine defects
remain relevant to private data on every OS. Windows does not provide the Unix directory
fsync durability used by Zephium's deletion journal. The cross-restart completed
tombstone prevents the application from forgetting cleanup authorization after a single
acknowledged unlink, but packaged NTFS power-cut erasure testing remains a release gate
rather than an asserted guarantee.

The engine now exposes a narrow, asynchronous profile-retirement primitive. It
tombstones the profile before teardown, rejects later view/navigation/spare work, drops
every known view and context, and invokes an exactly-once `Verified`, `Failed`, or
`TimedOut` completion. Linux captures and deduplicates strong website-data-manager
handles immediately after native construction and retains them across last-tab close
and failed post-build steps. It clears every retained manager and fetches all record
types afterward before removing the canonical profile directory. Opaque native-build
failure, manager mismatch, or failed clear/fetch creates sticky process-lifetime proof
debt; no later empty retry may authorize disk deletion, and successful handles are
released only for the exact attempt that verified disk absence. macOS clears and
fetch-verifies ephemeral stores and removes a named store before re-enumerating its
identifier. Windows closes the profile's controllers, requests an opportunistic
`ICoreWebView2Profile2` all-data clear, and requires the matching
`Environment5.BrowserProcessExited` event for the exact PID and monotonic environment
generation before deletion. The clear callback is not authoritative because WebView2
may release it without invoking it after controller close. At the Environment5 event,
a retained, non-reusable handle for that exact browser process must already be
signalled; only then are both UDF roots removed and verified. A
caller-visible timeout does not authorize an overlapping retry while native work might
still be running; the process-lifetime tombstone remains. Public attempts are admitted
once per profile before either the watchdog or native host queue is allocated, and both
the retirement/attempt maps and host tombstone/attempt maps are capped at the maximum
64 session profiles without eviction. Main-thread refusal, host-erasure refusal, or the
eight-second public timeout cannot stop an already-running page by changing Rust state,
so each reports its exactly-once outcome, seals event/content authority, and invokes a
mandatory composition-root fatal callback. The desktop callback terminates the process
immediately with a non-zero status; sealing alone is not described as native teardown.

`Verified` covers only the engine-owned native store and canonical engine directories;
it is not forensic RAM/media erasure. The application now coordinates this proof with
the SQLite phase for deletion of an inactive named profile. It first atomically commits
the exact canonical survivor session and a bounded authorization-journal row, then
applies the in-memory tombstone and starts native retirement. A deadline-ambiguous
authorization is reconciled against that journal. If reauthorization is legal, the
coordinator rebuilds the survivor snapshot from current aggregates and records a
process-local session revision, so mutations accepted while the earlier result was
unknown cannot be overwritten by stale deletion state; a newer survivor revision is
scheduled for a post-barrier persist.

After native `Verified`, finalization durably records that proof before scrubbing and
unlinking the exact journal-authorized profile database. Unix orders the unlink with a
directory durability barrier before clearing the row. Windows instead durably marks a
completed local tombstone and retains the authorization until a fresh process start,
after filesystem recovery, re-observes the canonical database plus WAL/SHM sidecars as
absent. Any observed artifact resets only the local phase while preserving native
proof. Startup resumes native retirement or local finalization from the same journal.
Profile deletion emits its correlated disposition only after the current process has
definitively completed both native and SQLite phases; retryable uncertainty remains
pending rather than being reported as success. Arbitrary orphan-shaped files are never
treated as authorized deletion work.
Packaged native tests across all engine data types and crash points remain a release
gate, so this coordinator is not yet evidence of complete real-OS erasure.

**Lifecycle and resource bounds.** Shell commands and engine events share one bounded,
ordered queue. Normal dispatch from native/UI callbacks is non-blocking. Replaceable
renderer-state events may be coalesced, displaced, or rejected under overload; ordinary
coalescing cannot cross a command or shutdown barrier. Normal work is capped at 960
entries. Native URL/failure/crash/profile-exit/split transitions use a reserved band up
to entry 4,095 and may replace an older fact for the same bounded native object or
displace ordinary work at that ceiling rather than leave Rust out of sync with native
state. Entry 4,096 is reserved for shutdown. Admitting that barrier
atomically seals further dispatch, so work behind it is rejected rather than falsely
reported as accepted and later drained. A failed pre-teardown store flush reopens
admission for a safe retry.

Native host work has a second bounded queue for main-thread reentrancy, notably when
WebView2 construction pumps the native event loop. Tasks are typed `Normal`,
`Maintenance`, `Observation`, `Lifecycle`, `Close`, `ProfileErasure`, or `Shutdown` and
keyed where one newer native fact subsumes another. Normal work is capped at 960;
lifecycle work has a bounded reserved band, one additional non-coalescible erasure slot
is reserved per maximum live profile, and entry 4,096 is a non-droppable shutdown slot.
Higher-priority keyed work may replace or displace lower-priority work, but the shutdown
barrier never evicts accepted lifecycle or erasure work. This gives teardown and
authoritative native state priority without permitting hostile callbacks to grow memory
or deadlock the active host borrow.

Every accepted privileged mutation reserves a slot in a separate 1,024-entry,
process-local result ledger before entering the shell FIFO. Pending and processed
entries are not evicted; capacity exhaustion rejects a new command before exposing an
operation id. The desktop records the actor's first disposition before fallible WebView
delivery. `Processed` means the shell actor made the named decision; it is not a claim
that WebKit/WebView2 finished later native work. Deferred dispositions name that
remaining boundary explicitly. Main-chrome-only status, reconciliation, and
acknowledgement commands let a reloaded UI subscribe first, recover missed dispositions,
and explicitly release them; acknowledgement is retried after transient IPC failures.
This ledger is not durable across process death. Only workflows with their own journal
have cross-restart continuation: profile deletion and the bounded extension
native-ownership ordering seam. The latter must durably enter `NativeMayOwn` before an
ownership-changing native call, treats both `NativeMayOwn` and `NativeOwned` as possible
ownership after restart, and clears only after definite native absence plus subordinate
resource release. An observed macOS or WebView2 owner identifier is durably attached by
exact CAS, is immutable for that native incarnation, and is mandatory before a native
row can claim `NativeOwned`; an identityless `NativeMayOwn` remains possible ownership
and must be reconciled conservatively. Its rows are profile-independent shared-meta state, so unresolved or
unreadable ownership blocks both profile deletion authorization and local purge. The
bounded extension coordinator now owns the Store projection, authenticated repository,
native-host authority, and bounded profile-retirement/shutdown drains. It refuses clean
extension-service evidence while a worker-owned runtime or attached authority remains
unresolved or the accepted/completed command counters differ. Production native
backends and product activation remain disabled and are release-blocking work.
`StoreShutdownOutcome::Clean` proves storage durability and actor/resource shutdown
only. Extension-service shutdown evidence proves its worker/resource drain and command
settlement; neither evidence proves durable rows or possible native owners absent.
Journal reconciliation must establish that native-absence claim separately.

The subordinate repository package pin is representation-exact as well. Its
durable identity includes browsing context, exact catalog-set record, historical
active/rollback role, exact package record, and the Store-native incarnation;
repository generation is an independent transition clock. Pin acquisition must
join a freshly product-verified package snapshot to the exact
`Acquire/NativeAbsentPreparing` Store row and currently rejects private context.
Idempotent replay requires equality of every persisted field. Same owner with a
different tuple is an ordinary owner conflict; a duplicate non-zero Store
incarnation or a missing/mismatched set row is corruption and fails recovery.
Removal compares the complete persisted identity, so a later acquisition of
the same package cannot satisfy a stale release. Owner pins retain their full
authenticated catalog set, and at most one exact set may exist outside the
candidate/current/previous slots. The 1,024-row durable ceiling covers bounded
reconciliation of both browsing contexts and does not authorize private
execution. Materialization schema v3 deliberately rejects the unreleased,
under-bound schema-v2 live-pin shape rather than guessing missing authority.
Post-reopen cleanup accepts only a Core-minted
`Release/NativeAbsentReleasePending` binding and grants no package or runtime
access. It first excludes a live same-open owner. After a classified frontier
preflight rejects in-memory or physical object-stage residue, it treats owner
absence as an idempotent crash-before-pin result without consulting a set
object. A present owner requires the complete persisted identity plus exact
set-row/package/backend join before the exact removal transition. A stale
incarnation, role, context, set, package, or backend is never approximated.
Bundled package settlement is source-free and precedes every fresh package
callback. A package-record final is the last-published commit marker. An intent
may be aborted only after repository-minted proofs establish that its marker
and every exact build stage are absent, with marker absence revalidated in the
abort transition. A present marker can never take the abort path, including in
the same process before its runtime map has been refreshed. Instead, stored
catalog and manifest bytes must pass current product admission and the complete
active or rollback object closure must verify from repository-owned bytes.
There is no source-backed repair after commitment. Missing or corrupt closure
members, a corrupt marker, or an unrooted package-record final fail closed with
path-free public errors. Tests cover no-build, pre-marker abort, active and
rollback completion, source-callback counts, publication/completion crash
frontiers, stale same-process state, missing closure members, corrupt markers,
and unrooted markers.
Bitwarden source acquisition is not delegated to the build adapter. The
offline source-admission command requires the exact reviewed commit and tag, a
clean non-sparse checkout, and exact SHA-256 plus marker cardinality for every
compatibility preimage. It emits no package or authority. Git output and source
file reads are bounded, and canonical file paths must remain beneath the
admitted root. This protects release operators from building a drifted or
partially inspected tree; it is not confinement against a malicious process
running as the release user.
Chrome-package acquisition is likewise separate from archive materialization.
The pure CRX3 boundary bounds the signed header, protobuf fields, proof count,
proof components, and total payload before cryptography; verifies every
recognized RSA/ECDSA proof; and requires one unambiguous developer proof whose
key digest derives both the signed CRX identifier and the catalog-expected
identifier. A valid unrelated publisher proof cannot substitute for that
developer identity. The legacy 1024-bit RSA verifier is selected only for that
identity-deriving developer proof; unrelated RSA proofs retain the 2048-bit
minimum. The offline probe materializer consumes only the verified
ZIP payload and preflights the complete central-directory inventory before
writing: encryption, links, special entries, nonportable names, duplicate or
case-colliding paths, file/directory shape conflicts, and file/tree expansion
budget violations fail closed. A deflated empty directory must declare zero
expanded bytes, contain at most 64 compressed bytes, reach decompressor EOF,
and emit no byte. Its private output retains an incomplete marker
until every bounded ordinary file is durably written. These commands remain
diagnostic and cannot mint a catalog release, materialization receipt, lease,
grant, or runtime authority.
Release-side CRX assembly uses the same verifier rather than a second trust
definition. Its preceding release-preparation boundary first verifies an exact
closed MV3 tree, rewrites the manifest to the external public-key identity,
removes upstream update authority, and removes only two named Store-generated
metadata files; an unknown Store metadata member is a refusal rather than a
silently unbound transform. It rebuilds the closed-tree index and a
platform-independent deterministic ZIP, then cross-checks the ZIP's CRX3
signing identity against the rewritten manifest before publishing. Publication
reserves a new private directory without replacement and keeps a durable
incomplete marker until the tree, index, ZIP, and non-authorizing evidence are
settled. A source race, malformed MV3 manifest, unsupported key shape, output
race, or size violation cannot produce a completed artifact.

The encoder accepts only one canonical P-256 public SPKI and a bounded ZIP,
retains no private key, and exposes a scatter/gather signature preimage so
release infrastructure may sign outside the repository process.
Finalization accepts only a raw ASN.1 ECDSA signature, constructs exactly one
developer proof, verifies the complete result through `VerifiedCrx3Package`,
then applies the hostile-ZIP preflight before no-replace publication. A wrong
key, signature, archive, output race, or unsupported key shape returns no CRX.
This proves package integrity only; it does not authorize distribution or
substitute for manifest-key, tree, catalog, license, compatibility, Store, or
runtime admission.
The first macOS vertical-slice overlay is deliberately non-product. It fixes
the absent `ExecutionWorld` enum and offscreen fallback, but removes public
inline-menu pages and forces that UI path closed because WebKit does not enforce
their sandbox. Its atomic output metadata records both the Chrome MV3 build
target and `product_authority=false`; package authority must reject any artifact
derived from this probe overlay. It is evidence-gathering infrastructure, not a
temporary permission bypass.
The corresponding artifact finalizer is also non-authoritative. It admits only
the reviewed build inventory, strips the exact debug and disabled privileged
outputs, adds explicitly identified diagnostics/canaries, and writes an atomic
closed tree plus canonical path/length/SHA-256 index. It records
`build_toolchain_attested=false` and cannot mint catalog, install, grant, or
runtime authority. The debug native consumer distrusts even that output: it
requires exactly two bounded root files plus one extension directory, reparses
the canonical index, rejects links/special files/path escapes, and reads and
hashes each bounded regular file before WebKit receives the root. Probe failure
at popup startup therefore cannot be converted into product authority, and
probe diagnostics can never enter a release package unnoticed.
The repository can recover at most eight sealed catalog-set finals. Production
reaches its bounded collector only through the serialized service worker, at
most one batch per existing one-minute Shell heartbeat. Collection runs after
interrupted-build settlement and preserves every candidate/current/previous
and owner-pin closure; an atomic permit coalesces duplicate wakeups, and
neither `more_garbage` nor transient pre-commit filesystem failure creates a
hot retry. Integrity failures fail closed and disable later periodic turns for
that process. Repository mutation/recovery and authenticated service admission
are automated gates; repeated-catalog endurance at the physical ceiling
remains required release evidence.
The Core acquisition and release bindings prove only a complete structural
join; they are not Store freshness capabilities. Acquisition is linear at the
repository boundary: the move-only binding is consumed into the live lease and
cannot be replayed after conversion to cleanup-only release. The extension
service exclusively owns the Store journal and repository, freshly revalidates
the exact row and CAS in the same serialized actor turn before each repository
call, and exposes neither bindings nor raw repository mutation over its
mailbox. Internal-only authenticated tests exercise that boundary through
activation, exact retirement, profile retirement, shutdown, and post-drain
Store/repository reopen while denying stale authority. They use a bounded
native fake; production adapters remain disabled, so the low-level and service
APIs are still not a release-enablement claim.

Logical content views have a warm set and two watermarks. The most recently used hidden
pages (four by default, two to save memory, ten to keep tabs ready) stay resident; every
older hidden page becomes a discard candidate after the chosen idle grace, whatever the
tab count. Above the pressure watermark of 24, under critical OS memory pressure, or while
a foreground page waits for a slot, the least recently used eligible page is probed
without waiting. A system memory warning shortens the grace to two minutes. Kept-awake
sites never sleep, and with sleeping off only critical pressure discards. The absolute
application admission ceiling is 64 live views, a backstop rather than a memory budget:
a foreground create waits briefly for a verified close, and an unsafe page is never
force-discarded to make room. A discard probe protects only state a reload would lose: a
page that is itself a POST result, audible media, capture, a child frame the user has
focused, and, once the user has entered input or drawn in that document, dirty forms,
edited rich text, beforeunload handlers, and any inspection that could not prove those
clean. The
engine independently caps native resources at 75 (83 with agentic contexts, plus nine
extension pages on Windows) while counting live views, a warm
spare, in-construction reservations, and WebView2 cleanup debt that may still own a
controller. These are deterministic resource bounds, not evidence that the resulting
RSS, CPU, wakeup, or battery budgets have passed.
The non-shipping `measure-macos-extension-product` gate now captures optimized
authenticated-path latency plus main-process `RUSAGE_SELF` evidence, but it
explicitly excludes WebKit helper processes, idle/battery behavior, tab-scale
campaigns, and endurance and therefore does not close those release gates.
The separate non-shipping `measure-macos-process-family` command attaches to one
unambiguous LaunchServices application coalition and samples the app plus its
re-parented WebKit XPC services through kernel resource counters. It rejects
identity drift, duplicate or oversized coalitions, unreadable live members, and
counter rollback. Its local JSON evidence still does not establish release
budgets until signed builds run the named clean-machine scenarios and endurance
matrix.

Every item-attributed native callback, native error, navigation observation, accelerator,
and asynchronous JavaScript result carries the immutable permit identity of its native
WebView generation plus the outer item token. A warm spare may bind its permit once;
revocation is terminal and the permit cannot be rebound to a later same-id view. Each
native generation also owns a non-wrapping navigation epoch. Adoption and explicit
loads pre-arm a fresh epoch; main-frame load URLs and queued source/history observations
must match both the native generation and the exact current epoch. Thus late
`about:blank` callbacks from a spare cannot be attributed to its adopted item. The host
also compares queued observation/crash work with the currently installed native permit.
For renderer crashes it revokes and physically removes the exact native view before
emitting `Crashed`; the shell therefore never sends a delayed id-only cleanup close that
could destroy the replacement generation.

URLs are policy facts, not navigation identities. The Wry adapter now carries one opaque
native identity across provisional start, every HTTP redirect, commit, finish, and failure:
WebView2 preserves `NavigationId`, WebKitGTK owns a bounded non-wrapping load-sequence id,
and WKWebView binds the exact returned `WKNavigation` (or the exact page-driven commit)
without a mutable global pending URL. At native commit, Wry first revokes a per-physical-view
atomic presentation permit and hides/makes transparent the raw surface before calling Rust.
The host accepts the final allowed redirect destination under that identity, then sends the
URL fact through the shell's reserved critical band before a matching opaque presentation
token. A fresh tab retains its real privileged New Tab UI and native frame during provisional
loading; there is no painted native placeholder. The exact presentation eval atomically applies
the URL/title projection, removes and verifies the active New Tab surface, and returns its
fixed-width monotonic revision. The actor accepts that callback only while it remains the last
emitted revision for the same item, then queues privileged-frame/content geometry before raw
reveal. A later same-tab or full projection invalidates an already-queued success callback;
unrelated-tab churn does not. Native finish idempotently re-drives the same fact if queue
coalescing replaced the commit notification. A two-second absolute bound applies only to
callback/native-dispatch retries and retires an unproven hidden view; page loading never creates
an intentional blank delay or timeout reveal.

Every native reveal revalidates both that exact atomic permit and the latest tab/split layout
revision immediately before and after re-entrant AppKit, COM, or GTK visibility calls. A
superseded pass hides first and retries only the newest retained model. Titles do not have a
portable native navigation id, so a cross-origin URL commit first replaces the old title with
a neutral host label; the finished document title is then queried from the native view only
after exact URL/epoch attribution. This is the browser-chrome anti-spoof boundary for redirects
and overlapping loads; packaged hostile redirect/re-entry tests on all three engines remain a
release gate.

Event delivery and profile/close retirement share a writer-preferring lifecycle barrier.
A waiting retirement writer blocks later events, waits for every earlier caller-sink
delivery to return, installs the tombstone, and only then lets delivery resume. The
caller sink must enqueue lifecycle commands rather than synchronously invoke `close` or
profile erasure. Same-thread lifecycle reentry is detected without waiting on itself,
seals the engine, and invokes the mandatory fatal callback instead of deadlocking.
Ordinary non-lifecycle engine calls may still be made by the sink because no retirement
mutex is held while external code runs.

Main-thread dispatch returns whether scheduling succeeded. Shutdown converts a rejected
dispatch into an immediate negative native result instead of waiting for a callback
that cannot arrive. After the durable store barrier, the composition root waits at most
eight seconds for native teardown. Windows spends at most five seconds of that budget
on its internal whole-process-group proof before bounded private-root cleanup. A
negative result or timeout is logged and permits forced process exit with a non-zero
status; the sealed actor never resumes. On Windows, a failed cleanup leaves that
generation quarantined. A later startup never reuses it: the shared bounded generation
manager reclaims only exact marked prior-boot data, baselines legacy data for one reboot,
and keeps same-boot or opaque state quarantined. Hitting a crash-loop budget produces a
recoverable full-Windows-restart instruction instead of an irreversible assertion.
Store worker queues are also bounded. Page-controlled history growth and extraction
results are capped. These are denial-of-service controls, not proof that arbitrary web
pages cannot exhaust an engine renderer.

**Page-derived native networking and images.** There is no generic native HTTP client or
page-derived fetch port. Such a client could not share the exact profile's proxy, DNS,
cookie, partition, and shutdown policy, so page-derived network access remains inside
the exact profile-scoped native engine session. Favicon network access and codec parsing
run inside the exact sandboxed site renderer. The host polls an asynchronous,
same-origin image load under the current immutable WebView generation and navigation
epoch, then accepts only a canonical base64 encoding of exactly 32x32 RGBA pixels
(4,096 bytes). Rust validates and stores only that fixed raster; privileged chrome
paints it with `ImageData` on a canvas and never invokes an `<img>` decoder or custom
protocol. Persistent caches are capped at 512 decoded origins; private-profile icons
are bounded in memory and never sent to SQLite. A page can choose or forge its own
pixels, but malformed containers, SVG/ICO/PNG parsers, cross-origin late results, and
unbounded payloads do not cross into the privileged renderer. Candidate discovery
considers at most 32 declared icon links, fetches at most four same-origin candidates
plus the same-origin `/favicon.ico` fallback, and uses seven timed polls. If that budget
expires while the document is still loading, the application retains one bounded marker
and makes one final discovery pass at the authoritative load-complete edge before
caching a negative result. Cross-origin/CDN-declared icons are intentionally unsupported
until a profile-scoped network broker can preserve the exact proxy, cookie, DNS, and
shutdown policy; such sites may still show a fallback icon.

Future application-owned downloads, including maintained filter lists and signed update
metadata, are distinct from browsing traffic. They require purpose-built bounded
components with explicit source, redirect, proxy, integrity, retention, and shutdown
policies; they must not restore a generic fetch capability for page-controlled URLs.

**Native network blocker (release seed active; TUF unprovisioned).** The tree contains a
bounded authenticated source updater, network-rule compiler, persistent caches,
coordinator, and native enforcement adapters. The desktop embeds exact immutable EasyList
and EasyPrivacy snapshots whose compressed/raw bytes, upstream revisions, approved
CC-BY-SA-3.0 metadata, and compiler reports are authenticated by the signed application
release. It deliberately embeds no production TUF root or repository origins/identity, so
bundled mode performs no source-network request and exposes distinct `release_bundle`
provenance. Profile preferences remain disabled by default. Disabled profiles install an
explicit allow-all generation; enabling requires the exact authenticated and installed
non-empty catalog and does not let an initial profile browse under a fictitious enabled
state.

The compiler worker accepts only pre-authenticated immutable source material, applies
source/rule/line/artifact budgets, rejects mutation and cosmetic capabilities outside the
audited network-only scope, and records platform omissions. Core admits a blocking artifact
only when its coverage equations hold and its explicit post-control native blocking-rule
entry count is nonzero; this structural count does not claim every entry remains reachable
after exceptions, and exceptions/platform omissions are not counted. Partial coverage is
typed separately for resource contexts, native request-source kinds, and document
attribution, while the aggregate counts each affected source rule once. Windows publishes a
precompiled frozen matcher to document-sourced WebView2 request contexts. Service- and
shared-worker requests remain unsupported until one profile/environment owner can avoid
WebView2's per-view callback fanout. The synchronous callback
performs no I/O or actor dispatch and fails open on malformed/oversized native data, lock
contention, matcher failure, or the per-request filter-evaluation ceiling. Because WebView2
does not supply the exact initiating frame URL, only source-independent blocks are applied;
a potentially applicable source-sensitive exception makes a normal generic block fail
open. macOS and Linux install digest-exact
native WebKit declarative filters and validate native cache identities. The application
holds first raw navigation until native installation settles, retains a proved previous
generation on replacement failure, and exposes only a trusted in-process typed status and
bounded exact-generation retry seam. Declarative compiler timeouts fail logical waiters and
queued jobs. Linux also cancels its retained `GCancellable`, but releases the one physical
slot only at the exact GLib callback; macOS has no native cancellation handle and keeps that
slot quarantined until its exact callback.

The release-seed loader verifies deterministic gzip identity and performs bounded lazy
inflation plus exact raw length/digest/header validation after a compiled-cache miss. It
drops inflated source strings after compilation. EasyList's source-refresh timestamp is
reported as advisory `source_refresh_due` for release-bundle provenance; it cannot
invalidate the signed release's immutable policy or degrade an exactly installed native
generation. Bundles install the exact attribution notice and license legal text.

The optional `tuf` feature admits only a fixed-origin licensed TUF package, persists a
monotonic rollback/clock high-water and content-addressed current/previous/candidate state,
and keeps the durable candidate distinct from current until its exact compiler artifact is
prepared. The feature, including HTTP/TLS transport, update worker, timers, and source
cache, is absent from the desktop release-bundle dependency graph; an independent CI
feature matrix prevents that retained implementation from rotting.
Its redirect-free HTTPS boundary is shared through
`zephium-update-transport`, which owns no trust root or package policy and
exposes only untrusted streams. Both origins are exact HTTPS directories;
credentials, redirects, compressed responses, queries, fragments, encoded
paths, and cross-origin finals are refused, and diagnostics retain only the
origin. The ordinary desktop graph still links no network updater through that
extraction.

Curated acquired extensions reuse that network boundary without inheriting its
TUF adapter. `zephium-extension-distribution` cannot be constructed until the
compiled product extension authority is valid. It authenticates the exact
fixed-name catalog before any secondary request, requires a complete ordered
runtime projection, and derives immutable CRX/legal object paths only from the
authenticated package key, revision, inner-ZIP digest, or legal digest; the
catalog's provenance URL and legal target path are never fetch authority.
Bounded object reads require one nonzero canonical `Content-Length`, refuse
transfer encoding, allocate fallibly, and check the observed length. The client
then independently verifies the CRX signature, developer key, inner ZIP
length/digest, and legal bytes before constructing the existing path-free
move-only service request. The service reauthenticates the same evidence before
private staging and source-free catalog activation. No endpoint or package is
currently provisioned. The separately audited worker contains no endpoint or
automatic polling policy, and the ordinary desktop graph excludes the worker,
distribution client, and HTTPS transport. No generic network capability is
reachable from pages or extensions.

The distribution coordinator is a one-run, one-request-at-a-time state
machine. Network execution talks to Shell only through non-blocking move-only
submission callbacks. The Shell ingress is an opaque shared one-shot solely so
its cloneable command vocabulary cannot duplicate move-only authority. Only the
first actor consumer can extract the request; later copies are inert. Refusal
before mailbox admission drops the callback without claiming it, while an
admitted command receives an explicit unavailable or failed-closed settlement
if Shell cannot reach the lifecycle. Product callbacks are invoked outside the
one-shot lock and panic-contained. The service drops the admitted request's exact byte
charge before invoking settlement, and the coordinator never fetches a later
package before observing that settlement. Only one exact retry follows a
completed outcome-unknown result. A missing or late accepted callback is not a
retry signal: it quarantines the coordinator until restart because the prior
request may still own memory or mutation authority. Async cancellation during
that interval follows the same quarantine path. Submission panics,
repeated unknown outcomes, service invariant failures, and accounting failures
also quarantine. This bounds overlap and prevents a transport retry loop from
amplifying memory, disk, or network work under uncertain durable state.
The dormant worker adds one bounded command slot and a dedicated current-thread
async runtime only when product composition explicitly constructs it. Shutdown
cancels network acquisition or settlement observation before joining the
thread; it does not claim service cleanup, which remains ordered under Shell.
The worker binds one immutable runtime selection derived from product-sealed
manifest profiles at launch. Refresh admission carries no caller-selected
package, backend, profile, endpoint, or catalog bytes, so a compromised UI
command cannot retarget the authenticated transport.
The process launch claim survives shutdown and quarantine, preventing in-process
replacement from clearing a restart-required coordinator state.
Run completion publishes its terminal status before admission can reopen, and
the worker settles that admission under the same gate used by cancellation. A
shutdown that wins the gate cannot be overwritten by a late success or
retryable failure; any other contradictory state quarantines fail-closed.
Status is a fixed-size generation-checked value, coalesced by Shell and mapped
to a closed privileged-UI vocabulary without URLs, package identities, error
strings, or native authority.
The optional desktop composition graph owns the unique worker only after a
suspended Shell callback exists and before startup admission becomes
irrevocable. Terminal shutdown consumes one absolute deadline: it cancels and
joins distribution first, then submits Shell's extension-service, Store,
native, and blocker teardown with the remaining time. The sealed endpoint slot
is empty, and the default desktop graph excludes the worker and its HTTP/TLS
dependencies entirely. Only privileged main chrome may request a refresh, and
that request carries no caller-controlled endpoint, package, profile, runtime,
or selection. Unique-owner removal and refresh admission share one lock, while
terminal intent is rechecked inside it, so shutdown cannot lose or bypass an
admitted worker request.

The blocker update coordinator then durably commits its candidate before
activation revalidates the exact prepared recovery state. A stale identity or
newly unavailable candidate leaves compiler authority unchanged; interrupted
transitions remain explicit and recoverable. Compiled/source caches and native
WebKit namespaces have bounded identity-safe garbage collection.

Only privileged main chrome receives the revisioned focused-profile diagnostics and bounded
enable/disable and exact-generation retry controls; refresh is exposed only for TUF
provenance. The DTO contains aggregate coverage and bounded public package
revision/SHA-256 identities, never profile IDs, URLs, request decisions, filter text, or
native/parser strings. Raw pages have no blocker command surface. Before claiming stable,
continuously maintained blocker protection, Zephium still needs production TUF trust
material, packaged hostile enforcement tests on every supported OS, legal approval,
external review, and recorded resource/endurance budgets. Full capability and failure
details are in `docs/adblock.md`.

**Dependency policy.** Tauri, Tauri Runtime Wry, Wry, and adblock-rust are vendored from
the immutable upstream revisions recorded in their respective
`vendor/*/UPSTREAM.md` files. The Tauri patch preserves pathless ephemeral storage for
implicit Linux incognito WebViews and rejects contradictory explicit persistent storage
before touching the filesystem. The runtime patch acknowledges native construction
before publishing a detached handle and propagates the exact failure instead of creating
ghost runtime state. Relative to Wry's recorded revision, its patch covers constructing
WebKitGTK contexts with process swapping, enabling/asserting the sandbox before WebView
creation, allowing an incognito view to use only an explicitly ephemeral supplied
context (including related-view construction), and the documented Windows staged-
construction/IPC/callback hardening.
The direct Tauri and Wry dependencies are exact-versioned, and Cargo source policy
rejects unknown registries/git sources and requires revision-pinned git
dependencies. CI treats frontend high-severity audits and Cargo
license/advisory/source policy as gates. Every external GitHub Action is pinned to a
40-character commit and weekly action updates are configured. This reduces supply-chain
risk; it does not make upstream code trusted or mean the graph has no known advisories.
`deny.toml`
explicitly allowlists exact, currently unavoidable GTK3 and Tauri/Specta transitive
advisory IDs with reasons. Unmaintained and unsound advisories otherwise fail, every new
advisory fails until reviewed, and an obsolete ignored ID also fails the policy so the
exception must be removed. The allowlist is accepted debt tied to the GTK4/WebKitGTK 6
and upstream-dependency release gates, not a declaration that those dependencies are
safe.

## Inherited engine properties and non-guarantees

Same-Origin Policy, CORS, CSP enforcement, cookie semantics, certificate validation,
HSTS, mixed-content behavior, renderer sandboxing, and process allocation are supplied
by the installed native engine. Zephium does not reimplement them and must not disable
them. It also does not currently install a certificate-bypass callback.

These inherited properties must not be overstated:

- Zephium does **not** guarantee a renderer process per origin or Chromium-equivalent
  full site isolation. WebView2, WKWebView, and WebKitGTK have different process models,
  and their behavior can change with the installed runtime. Profile data partitioning
  is not proof of renderer/site isolation.
- TLS errors use the engine's default behavior. There is no Zephium certificate
  interstitial, HTTPS-only mode, certificate pinning, or independently tested TLS
  policy yet.
- Cookies, cache, service workers, storage quotas, and clearing semantics are chiefly
  engine-owned. Per-profile partition construction and a verified engine-store
  retirement primitive are implemented. A crash-resumable coordinator now spans
  full-profile native retirement, the authoritative session/registry, and the
  journal-authorized profile database. Cookie inspection/editing and origin-selective
  clearing are not implemented, and the full-profile coordinator still requires the
  packaged native data-type/crash matrix described in the release gates.
- "Zero telemetry" currently means no first-party Zephium telemetry client is wired
  into this tree. It does not mean pages make no requests or that OS WebViews and
  services make no vendor requests. For example, WebView2 content protection remains
  enabled and may follow Microsoft/OS policy. Zephium configures custom crash
  reporting to prevent WebView2's automatic crash upload; this does not disable
  separately governed required diagnostics or SmartScreen. A production privacy claim needs a
  platform-by-platform traffic audit.
- A sandbox limits impact; it does not make engine vulnerabilities impossible. Native
  engine security updates are part of the product's security boundary.
- Hard runtime admission, recommended maintenance, and review age are separate
  states. An unsupported or unparseable engine (macOS older than 14, Safari
  older than 26, a preview, development or overridden runtime), a
  security-relevant environment override, or a missing native capability still
  fails before WebView construction; the first group shows a native alert and
  exits 78. Falling below the reviewed security floor, falling behind the
  newest reviewed patch, running a newer stable release line, or crossing the
  review SLA produces an independent sanitized warning, shown as a dismissible
  card in the sidebar, instead of blocking startup. The wall-clock rollback
  checks were removed: no installed build uses the clock as a kill switch. Rust
  carries the closed vocabulary as a canonical allocation-free set, so a
  newer runtime cannot hide an overdue review or update recommendation. CI
  and every release remain fail-closed on an overdue review.
- Runtime assessment is local and bounded. Startup reads only the native
  versions/provenance/capabilities it already needs and performs no vendor or
  Zephium network request, timer, or background task. Vendor HTML and release
  pages are human/build-review evidence, never client policy inputs. A future
  remotely refreshed policy must arrive through Zephium-signed,
  rollback-resistant update metadata and may update advisories
  asynchronously; it must not delay first paint or weaken the embedded hard
  floor.

## Platform-specific posture

### Windows / WebView2

- Browser extensions remain disabled in every ordinary WebView2 environment.
  A direct `browser_extensions_enabled(true)` request and every unmanaged
  extension path fail before COM/HWND construction. The only enabled path is
  an engine-owned Wry startup gate invoked after exact controller retention but
  before WebView initialization or initial navigation. It pointer-attests the
  controller environment through canonical `IUnknown` identity, the exact UDF,
  and a non-private `ICoreWebView2Profile7`.
- The first enabled environment uses a short-lived 1x1 hidden controller with
  no IPC, DevTools, focus, autoplay, download, permission, popup, or arbitrary
  navigation authority. It is explicitly closed before the lifecycle call
  continues. Construction and close failures retain the exact process and
  controller cleanup obligations and make admission fail closed. The extension
  service's durable journal and authenticated package state must already have
  selected this native operation: WebView2 can make persisted extensions
  available when the environment starts, before an embedder can enumerate the
  profile, so the bootstrap controller is deliberately page-inert and no raw
  content controller is admitted at that point.
- An allocation-free, eight-slot engine-owned mode registry records every captured Windows profile
  environment as ordinary disabled or extension preparing/ready/failed. The
  extension profile object is published only after native hardening, explicit
  controller close, cleanup-debt collection, and deadline checks succeed. A
  configure failure, close failure, late deadline, interrupted preparation, or
  mismatch between mode/environment/profile maps leaves a sticky refusal; the
  captured enabled environment can never fall through the ordinary gate-less
  content path. Only exact browser-process exit or process restart removes that
  generation's mode state.
- Content inventory comparison includes exact published owners from both fresh
  activation and restart recovery bindings. A Windows recovery binding must
  resolve its catalog and prior adapter identity to one owner and its published
  evidence must match. Conflicting, identityless, duplicate, or over-capacity
  recovered cohorts fail the registry invariant and admit no content view.
- Before any later extension-enabled content controller initializes, the host
  enumerates the complete bounded native extension snapshot and requires exact
  equality with the published runtime-owner cohort. Activation, removal, and
  recovery consume callback HRESULTs and native objects under one absolute
  deadline and a reentrancy-bounded UI message pump. Missing or duplicate
  identities, late callbacks, inventory overflow/mismatch, cleanup debt, or a
  changed environment/profile identity blocks new content views and prevents a
  clean shutdown claim.
- WebView2 fixes extension enablement at environment creation. If a profile
  already owns an extension-disabled browser-process generation, Zephium does
  not attempt to change it in place or enable all profiles preemptively. The
  current Windows adapter returns a retryable native pre-entry
  `RestartRequired` refusal. That reason is projected through typed management
  settlement and bounded IPC into restart copy in the Extensions Center; it
  does not trigger automatic teardown or weaken tab/session ownership.
  Restart re-enters cleanup-first authenticated startup. Exact live-profile
  reconstruction and live Windows validation remain release gates.
- No reviewed Windows catalog/classification profile is currently sealed. The
  macOS staging and private-lab feature graphs remain macOS-only at the desktop
  build boundary, and non-macOS target configuration is empty. Windows must not
  reinterpret a macOS compatibility digest as WebView2 evidence; enabling the
  target requires an atomic reviewed profile plus native-runner proof.

- Raw persistent and private-content controllers use profile-specific WebView2
  contexts; the privileged Tauri user-data folder is not reused by raw content.
- The main and panel are InPrivate and use distinct UDFs inside a fresh per-run
  generation. Native hardening requires Environment7 and Environment10, verifies that
  each environment's
  reported UDF canonically equals its assigned direct (non-reparse-point) directory,
  admits that environment's own browser-version string, disables default script dialogs
  and context menus, verifies the resulting profile is actually InPrivate, disables
  password autosave/general autofill, and installs permission denial. Any failure aborts
  startup.
  Raw content requires Settings4, disables password autosave and general autofill, and
  reads both values back before navigation. It also disables browser accelerator keys
  and default context menus and requires Settings7 to hide PDF Save, Save As, and Print.
  Privileged downloads remain denied; raw human views use the native download adapter. Both view classes replace Wry's
  broader default browser arguments with only the `msWebOOUI`/`msPdfOOUI` suppressions,
  so Zephium does not deliberately disable SmartScreen.
- Every view also requires CoreWebView2_18. Raw and privileged subframe navigations are
  checked by native handlers, and OS-registered external URI schemes are always
  cancelled. HTTP Basic Authentication and client-certificate selection are also
  cancelled before WebView2 can fall back to native credential/certificate dialogs. A
  missing interface or failed registration rejects the raw view or aborts
  privileged startup before the window is shown. Raw process-failure event registration
  is also mandatory rather than a best-effort observability hook.
- Raw handlers install before the first content load. Privileged views start at the
  browser-generated exact `about:blank`; Tauri's IPC/document-start plumbing is already
  registered, but no bundled application asset or page script loads until all native
  handlers install and the backend explicitly navigates to the validated app URL.
- Every raw-controller build attempt creates a construction proof obligation before Wry
  can allocate native state. The fork exposes the selected environment before controller
  creation; at that boundary the engine retains an exact browser-process handle and
  installs the Environment5 observer. A Wry construction guard owns the parent-window
  subclass registration, controller, and child container HWND through every fallible
  step. It attempts all outstanding releases on failure and returns a typed, retryable
  cleanup debt if any detach, `Controller::Close`, or `DestroyWindow` contract remains.
  Ordinary drop also retains bounded fallback debt instead of forgetting native
  ownership. The engine associates debt with the exact profile, retries it, counts any
  still-owned controller against the native-resource ceiling, and quarantines/fails
  closed rather than creating around unproven teardown. The engine then verifies
  Environment7 before any content load. Its actual
  reported UDF must canonically equal the engine-owned per-profile directory, every
  path component must be a direct directory rather than a symlink, junction, mount
  point, or other Windows reparse point, and its environment-local browser-version
  string must independently parse as an admissible Stable build. Only complete
  attestation clears the construction obligation. A failure before environment creation
  has no native process obligation; a later failure retains exact provenance and can
  retry only the controller. Missing or contradictory environment/process proof remains
  terminal and prevents an empty in-memory map from being mistaken for native absence.
- Before Tauri creates a view, the runtime version must parse as a stable four-component
  WebView2 version. The reviewed security floor and latest recommendation are
  `154.0.4258.62`, published October 5, 2026. An older Stable runtime is admitted
  with an update-recommended advisory. A newer stable major receives
  an unreviewed-runtime advisory. Preview-channel and malformed strings are
  refused (exit 78 with a native alert).
  The process also rejects documented WebView2
  environment overrides that
  can replace runtime/UDF selection, append browser flags such as `--no-sandbox`, select
  another channel, or attach script debuggers. CI and release publication
  expire this review after October 13, 2026; runtime reports an overdue-review
  advisory instead. Per-view Environment7/UDF/runtime, Environment10, Settings7, and
  CoreWebView2_18 checks remain independent capability gates.
- Microsoft acknowledged on July 14 that additional Chromium security fixes
  were not yet available in Edge/WebView2 Stable. Stable `150.0.4078.80`
  incorporated the update on July 16. Microsoft listed CVE-2026-85046 as
  exploited in the wild in Stable `152.0.4191.62` on September 2, then published
  Stable security updates through `154.0.4258.62` on October 5.
  Microsoft publishes no WebView2-specific per-CVE applicability matrix;
  Zephium therefore treats the shared runtime release as a conservative floor
  rather than claiming each listed CVE applies to WebView2. The October 2 review
  independently confirmed matching x86, x64, and ARM64 WebView2 packages in the
  Microsoft Update Catalog and raised both the floor and recommendation; the
  October 6 review raised them to `154.0.4258.62`. On October 6 Microsoft
  again acknowledged a pending Chromium security fix, so production
  publication is blocked until a later Stable release is reviewed. See
  [the review evidence](windows-webview2-security-review-2026-10-06.md).
  The release gate preserves the
  historical notice and requires both a cleared blocker and a floor published
  after it, so changing a boolean cannot turn a known vendor patch gap into
  release evidence. That gate is a CI and release check; it does not stop a
  user's machine from starting.
- Every retained raw WebView2 environment owns one deduplicated RAII
  `NewBrowserVersionAvailable` registration. The first callback sets a sticky,
  queryable backend `restart_required` state, emits one typed engine event, and is
  replayed to reloaded privileged chrome through the typed `zephium:runtime-status`
  event. It does **not** recycle raw profiles or claim that the newer runtime was
  adopted: Tauri's privileged environments still run the old generation, so the only
  valid transition is the ordinary ordered whole-application shutdown followed by a
  restart. Automatic restart remains deliberately outside this callback path.
- A warm spare is admitted only while its profile still owns a real live
  controller. Closing the profile's final real view synchronously closes a
  matching spare as well, including typed cleanup-debt capture. WebView2 then
  performs normal process-group shutdown; Zephium retains the Environment5
  observer and exact process HANDLE until `BrowserProcessExited` and the
  signalled HANDLE agree on the same PID/generation, at which point the idle
  environment stops counting toward the eight-group ceiling. A new profile is
  rejected during that asynchronous overlap rather than briefly exceeding the
  native process/RAM bound. Packaged Windows tests must still measure exit
  latency and rapid close/open behavior under load.
- Main and panel Tauri environments install the same update event through a bounded
  UI-thread-local registry keyed by their fixed window labels. Each entry retains its
  exact environment, Environment5 exit observer, process HANDLE, and update token. Window
  destruction removes only the update handler; the exit observer remains on the same
  apartment through `run_return` and the bounded callback pump. Cleanup requires exactly
  both labels, distinct PIDs, matching exit events, and signalled exact HANDLEs before
  registrations are released. Each hardening call also verifies its Environment7
  `UserDataFolder` against the distinct assigned `main` or `panel` directory; a runtime
  that aliases those environments/processes therefore fails closed. COM references never
  cross apartments and no token is leaked.
  A sticky pre-shell bit replays a callback that fires during privileged hardening into
  the engine's global dedupe gate once startup reaches the shell. Queue saturation is
  repaired from that queryable gate on the bounded maintenance tick.
- InPrivate still creates runtime files. Raw and privileged current-generation cleanup
  is explicit and uses the same Environment5/PID/HANDLE gate. A failed generation stays
  quarantined and is never reused. Volatile per-boot provenance permits bounded
  prior-boot reclamation; same-boot state cannot be deleted and exceeding its budget
  requires a full Windows restart. Exact markers, root leases, reparse rejection, and
  byte/entry/count budgets apply equally to `private-runtime` and `privileged-runtime`.
- Engine profile retirement releases every controller and environment and arms the
  authoritative Environment5 exit continuation independently of Profile2's
  opportunistic all-data-clear callback. At that exit event the exact browser-process
  handle captured while the process was alive must already be signalled; the engine
  then verifies the persistent and private UDF directories are absent. An
  unproven process identity or process-exit timeout remains fail-closed for the process.
  Every engine-owned UDF root and typed profile directory rejects Windows
  `FILE_ATTRIBUTE_REPARSE_POINT` objects (including junctions/mount points), not only
  symbolic links. Every existing ancestor, root, and profile component must be a direct
  directory. Root/profile identity and that condition are read again immediately before
  every recursive-deletion retry, including engine shutdown cleanup and
  privileged-runtime cleanup after exit.
- **Release gates:** keep the expiring security floor aligned with Microsoft's current
  Stable security release, handle adoption of a newly downloaded runtime for long-lived
  processes, test InPrivate and native navigation-denial behavior for raw and privileged
  views, test privileged main/panel exact exit proof and separate-UDF cleanup after
  crashes/restarts/power loss on packaged Windows, and make an explicit ship/withhold or
  documented-risk decision for WebView2's missing supported file-picker interception
  surface. Environment-local UDF and runtime postconditions cover
  those registry/group-policy overrides, but neither they nor environment-variable
  rejection prove the absence of policy-injected browser arguments such as
  `--no-sandbox`: a native hostile-startup test must inspect the actual spawned
  browser/renderer command lines, tokens, job membership, and process mitigations before
  Zephium claims sandbox attestation. The current stable WebView2 COM
  surface has no supported file-chooser interception/cancellation event; do not replace
  this gate with DOM monkey-patching or experimental DevTools-protocol interception.

### macOS / WKWebView

- Persistent content profiles use named website-data stores; private content profiles
  and both privileged WebViews use non-persistent stores. Bundle metadata requires
  macOS 14.0 or newer.
- Before Tauri constructs any WebView, runtime admission requires macOS 14 or newer
  with Safari 26 or newer. The reviewed recommendation is Sonoma 14.8.9 with
  Safari 26.6.1, Sequoia 15.8.1 or Tahoe 26.7.1 with Safari 27 (which carries
  WebKit fixes those point releases do not), or macOS 27.0.1 (September 14/28
  releases). A supported system below that, or one whose canonical Safari bundle
  build differs from the loaded `com.apple.WebKit` framework build, starts with
  an update-recommended advisory. Newer major lines (macOS 28, Safari 28)
  receive an unreviewed-runtime advisory. The review expires for CI/release
  after October 13, while runtime
  keeps starting and reports review age to privileged chrome.
- Overlay configuration keeps Tao's allocated `TaoWindow` class and instance layout
  intact. Zephium does not use `object_setClass` to turn that live object into an
  unrelated `NSPanel`; true non-activating panel behavior remains deferred until an
  allocation-time Tao/Tauri constructor seam exists.
- Privileged chrome layout no longer stores an unretained global native pointer. The
  main thread owns a retained `WKWebView` with a non-reusable generation. Queued layout
  work resolves that retained state only when it executes, rechecks the generation and
  window attachment, and the destroy/drop path unpublishes the generation before
  releasing the retain. Chrome-coordinate state carries the same generation, so an old
  adapter cannot publish geometry for a replacement view.
- Raw content has the pinned Wry native permission denial. For each privileged view,
  Zephium verifies that the native website data store is non-persistent and immediately
  replaces Wry's permissive delegate with a retained native UIDelegate that denies media
  capture, device motion, and file selection. Optional dialog/popup delegate methods are
  omitted to retain WebKit's cancel/no-dialog default. `Permissions-Policy` remains an
  independent script-facing layer.
- Raw content also sets Wry's macOS fullscreen and Picture-in-Picture private
  preferences to false per view. Tauri's `macos-private-api` feature may enable the
  compiled fullscreen path for privileged chrome through Cargo feature unification;
  that compile-time availability is not inherited as authority by raw child views.
- Native delegate/inspector hardening must complete for the main and panel or startup
  aborts. Both views contain only the browser-generated blank document while the
  delegate is attached; application navigation occurs afterwards. Attachment is still
  not atomic with native construction.
- WKWebView does not expose Safari's complete browser UI or Safari-only Safe Browsing
  behavior to Zephium.
- Arbitrary relying-party passkey registration and assertion require Apple's
  managed `com.apple.developer.web-browser.public-key-credential` entitlement.
  The credential boundary reads the current task's exact signed Boolean before
  constructing AuthenticationServices state. Missing, false, malformed, or
  absent or false entitlement state fails closed as entitlement-required;
  malformed or unreadable state fails closed as unavailable. Neither can
  present the authorization action. No entitlement is currently configured or claimed;
  Apple account approval plus signed/notarized packaged WebAuthn workflows are
  release gates. Extension-mediated provider behavior does not substitute for
  this browser-native capability.
- Engine profile retirement clear/fetch-verifies retained non-persistent stores, removes
  the profile's named store only after releasing its WKWebViews, and re-enumerates data
  store identifiers before reporting native-store verification.
- **Release gates:** attach the deny-only UIDelegate during privileged WebView
  construction rather than immediately post-build, define migration/purge behavior for
  any legacy default WK data store, and test permission/file-panel denial, data-store
  isolation, and deletion on supported macOS versions.

### Linux / WebKitGTK

- The loaded library must be WebKitGTK 2.54.0 or newer. Odd-minor
  development builds, older versions, and unrelated major lines are rejected.
  Newer stable even-minor WebKitGTK 2.x lines are admitted with an
  unreviewed-runtime advisory until the review catches up. The raw content WebContext sandbox
  flag and top-level cross-site process-swap policy are enabled and read back during
  context construction, before any WebView can launch a Web process. A rejected or
  disabled configuration therefore fails closed, but the public property is not an
  attestation of the confinement actually applied to a spawned process. The newest
  stable release reviewed in this pass is 2.54.1 (October 2, a bug-fix release); the
  enforced 2.54.0 boundary is the first stable release fixed for WSA-2026-0006
  (September 29), and no later 2.52 release carries those fixes, so the reviewed
  line moved from 2.52 to 2.54. The official advisory index and release feed were
  re-reviewed on October 4, 2026; 2.53.92 remains an odd-minor development
  release. CI/release review expires after November 3, 2026; runtime reports that
  expiry without disabling an otherwise admitted engine.
- Both privileged WebViews request non-persistent contexts. Their native permission and
  file-chooser denial handlers must install successfully, and the native context is
  checked to be ephemeral, or startup aborts. The vendored Wry adapter separately
  cancels file-chooser requests from raw tabs as well; file upload remains disabled
  until Zephium has an origin-labelled broker.
- Persistent profiles have separate on-disk contexts. Private tabs in one profile share
  a single verified in-memory context, so their cookies/storage form one process-lifetime
  incognito session without sharing across profiles.
- Download denial is installed once on each raw WebContext before its first load;
  opening and closing tabs does not accumulate context-global denial callbacks.
- The current integration uses the GTK3/WebKit2GTK 4.1 bindings. Top-level cross-site
  process swapping reduces process reuse but is not WebKit full site isolation, does not
  promise a process per origin/frame, and remains engine-controlled. Other WebContext
  callbacks are context-global and require explicit single-owner lifetime management.
- No Linux package is built or published: the release workflow produces macOS and
  Windows installers only, and CI compiles and tests the Linux code on Ubuntu as a
  source gate. The earlier Fedora RPM path and its real-WebProcess confinement CI
  were removed with Linux support, so nothing here is runtime sandbox attestation.
  A future Linux release must restore a package that carries a
  `webkit2gtk4.1 >= 2.54.0` dependency, with runtime admission as the final gate, and
  re-establish confinement evidence.
- **Release gates:** rerun the confinement probe through the packaged application on
  supported hosts and inspect its actual process tree. Add an in-renderer or
  equivalent-credential host-file denial probe and identify the filter policy before
  claiming filesystem confinement or filter provenance. Also validate private-context and
  context-global-handler lifetime. Native erasure tests must write
  cookies, local storage, IndexedDB, Cache API data, and cacheable responses; destroy
  every view/context; clear and drain GLib callbacks; reopen the exact profile; and prove
  both API-level absence and that delayed processes do not recreate the directory. Keep
  distro/runtime floors synchronized with WebKit advisories and define a supported
  portable package path.
  The GTK4/WebKitGTK 6 migration must retire the explicitly allowlisted GTK3 advisories
  and re-audit the vendored Wry patch.

### Offline macOS extension adaptation

The package-neutral macOS compatibility materializer is offline and
non-authorizing. It accepts only an already authenticated closed MV3 tree plus
its canonical index, rejects source drift and reserved-path collisions, and
emits a distinct closed tree and metadata with `product_authority=false`. It
cannot mint a catalog row, install, profile grant, runtime witness, controller,
or redistribution decision. Generated resources live under a reserved
namespace; source and output manifest/tree/index identities are all rebound.

Compatibility release preparation must consume that complete artifact, not an
extension directory detached from its receipt. The release boundary requires
the exact root inventory, revalidates the receipt's non-authorizing header and
closed output identity, copies the captured receipt without re-reading it, and
binds its digest, target, and input identity into deterministic CRX3 release
evidence. It also inserts those exact bytes at a digest-derived reserved path
before re-indexing and zipping the release tree. This closes receipt-loss and
tree-substitution seams but still grants
no signature, legal, catalog, install, runtime, or product authority.

Catalog authority is a separate mandatory transition. Schema 2 signs a
strictly ordered, unique compatibility-receipt cohort per package and binds
each receipt's exact bytes and adapted pre-signing
manifest/tree/index/count/size identity. The containing package row separately
binds the post-rewrite release tree. Receipts are capped at 64 KiB and eight
targets per package. Schema 1 cannot carry them, and schema-2 inventory hashing
uses a new domain that covers every receipt field. Brokered macOS manifest
admission fails closed unless the catalog contains its exact versioned target
and the canonical release tree contains its exact digest-derived resource. The
product gate verifies those embedded bytes before repository activation; a
digest-only row cannot stand in for the receipt body. These checks authenticate
reviewed adaptation evidence but do not make that evidence a signature,
redistribution decision, user grant, live lease, or native controller
capability.

The adapter preserves native extension principals and namespace objects. It
does not expose a generic page-world API, fetch port, or native-message channel,
and it leaves upstream `MAIN` scripts unchanged. File match patterns are
removed until a separate per-extension file-URL grant exists; exact removals
are part of the sealed surface contract. The webNavigation endpoint is injected
only into an isolated content world and supplies only two absent same-document
event objects to that extension's own worker. Messages have a fixed channel and
kind, bounded URL, validated native tab/frame identities, and a restricted URL
scheme; reported changes come from the endpoint frame's actual location. A
hostile page may dispatch the fixed DOM signal, but it cannot supply a URL or
cross the isolated-world runtime boundary, and unchanged locations are ignored.
The authenticated macOS product gate independently serves a hostile raw page
before its document-end content script. The page requires Tauri, Wry,
principal-handler, and privileged extension APIs to be absent, then installs
forged `chrome`/`browser` objects and poisons its own DOM setter. WebKit's raw
page does expose the restricted external-messaging
`browser.runtime.connect/sendMessage` pair. The lower live gate has both that
raw page and a separate extension address the exact known extension context
through message and port APIs, then has a delayed isolated content script read
the target worker's delivery counter. Only exact zero passes; no timeout is
interpreted as refusal. Zephium preserves any `externally_connectable`
declaration as unmodeled authority and cannot classify it runnable.
Extension execution must still settle through its isolated world without
touching either forgery. This is real product-path boundary evidence, not a
claim against a native WebKit sandbox escape.
The production History API signal may originate only from Zephium's native
committed-URL observer. The macOS host emits the constant, payload-free event
only after the exact current main-frame epoch accepts a changed same-origin
URL, and does so independently of extension presence. It evaluates the event
in WebKit's default client world so page code cannot replace the constructors
used to dispatch it. The authenticated product gate proves both the native URL
observation and cross-world DOM delivery in the native and brokered runtimes.
Same-URL `pushState`/`replaceState` calls remain unsupported: allowing a
page-dispatchable event to report an unchanged location would let hostile
content manufacture extension navigation events. Subframe History API changes
that do not update the top-level WKWebView source remain limited to native
`hashchange`/`popstate` delivery inside the injected frame.

Installed optional-authority edits are browser-owned transactions, not a
generic permission bridge. Privileged chrome echoes only a bounded optional
declaration index and exact install/catalog/grant revisions from its current
authenticated projection. The serialized extension service resolves that
index from a freshly authenticated manifest and refuses required, missing, or
stale declarations. A changed edit first retires every native context for the
install, commits one grant CAS only after the Store proves native ownership
absent, and reactivates only the contexts that were live. A no-op does not
touch WebKit or durable state. Store uncertainty quarantines management until
restart; a definite pre-commit refusal restores the old runtime. Page content
cannot name a permission, package, profile, runtime, or native object through
this path.

Profile-wide safe mode and site denials are a separate revisioned Store
authority, never implicit grant mutations. The complete policy is loaded in
the same profile transaction as installs/grants and is bound into runtime and
native-plan identity. A changed write cannot commit while any native ownership
row for the profile exists; the serialized service retires the whole bounded
profile cohort before CAS and restores only the exact previous live keys on a
definite refusal. Outcome uncertainty closes management until restart.
Privileged IPC carries only the exact policy revision and booleans. Shell alone
derives a canonical HTTP/HTTPS DNS-or-IPv4 whole-host scope from the focused
committed URL, so page/chrome JavaScript cannot submit a URL or pattern.
Core document witnesses and transient `activeTab` redemption independently
reject a profile-denied site before any engine scripting permit is issued.

The macOS live gate proves that an explicit exact-host denial overrides a
broader granted host pattern while a different host remains granted, and that
content-script execution follows the effective denial and later restoration.
The probe applies and exactly reads back both native dictionaries. Authenticated
native and brokered product gates then prove the durable policy transaction,
native rebind, denied execution, restoration, uninstall erasure, and clean
restart. File, wildcard, path-specific, userinfo, and IPv6 scopes remain
unavailable rather than being approximated.

### Extension compatibility broker

The macOS native extension compatibility broker is a narrow zone-1 service,
not native-messaging authority for installed extensions. It accepts only the
fixed internal identifier `app.zephium.extension-broker.v1`, a canonical
versioned string request, and a loaded context belonging to the exact retained
controller. Product authority must have admitted the distinct brokered
compatibility profile, and the published runtime must consume an operation-
specific witness proving the effective API grant before Shell sees a request.
The closed operations are read-only recent history, browser-default search in
the current or a new tab, and restoration of the newest closed current-space
tab. They carry no SQL, path, provider, arbitrary URL, hostname, window/native
tab identity, arbitrary application identifier, generic fetch, or page-world
payload.

Native, Shell, and Store enforce independent bounds: 1,536 request bytes, a
1,024-byte search query, 64 KiB response bytes, 100 history rows, 32 durable
recently-closed regular tabs, a five-second native deadline, 8 pending requests
per profile, and 32 process-wide. History reads remain profile-scoped, reject
degraded/recovery-required storage, inspect only a bounded recent window,
deduplicate URLs, validate navigation schemes, and sanitize titles. Search
passes only through the browser's default-provider classifier and existing
navigation lifecycle. Restore requires the exact focused profile and space,
excludes incognito state, validates the durable record again, and creates a
fresh item/native identity. Retirement and shutdown cancel retained callbacks;
late Store results can settle only the exact runtime generation and request
identity. The ordinary
`macos.wkwebextension.v1` profile still prohibits `nativeMessaging` and cannot
mint broker witnesses. Persistent ports and arbitrary native hosts remain
unsupported.

This boundary has a source-free live product gate for both sides of the policy
split. The ordinary native profile proves that its denied broker-only grants do
not reach the channel. The distinct brokered profile proves authenticated
repository selection, durable grants, native controller/context binding, exact
runtime-witness consumption, a real profile-scoped Store read, bounded JSON
delivery back to extension JavaScript, runtime retirement, repository cleanup,
and clean Store restart. The non-shipping gate coordinator uses a dedicated
bounded reader rather than the product Shell's fair read queue; Shell settlement
and queue fairness remain independently tested. No extension compatibility
claim may rely on these operations until an exact reviewed adapter, packaged-app
coverage, and release-build resource/endurance measurements also pass.

## Features deliberately not claimed

Public extension metadata parsing, store-listing recognition, original CRX
authentication, and upstream checkpoint comparison are non-authorizing
boundaries. Neither a parsed policy nor a recognized store URL may mint an
existing Verified manifest witness. The new conditional metadata transport
replays only content-derived ETags; an HTTP 304 does not authenticate cached
bytes, advance trusted time, or extend signed policy expiry. Production Beta
installation remains disabled until separate admission authority, signed
policy receipt consumption, repository/transform reauthentication, and native
recovery are composed and tested. Store now commits bounded source provenance
and monotonic publisher history atomically with installation and grants;
complete expected provenance is rechecked on cohort reads, and native grant
reads validate its output-manifest and high-water joins. Neither these rows nor
their constructors authenticate package bytes or mint native ownership.
Uninstall retains upstream history; profile erasure scrubs it.

The opt-in public-policy client now verifies the complete TUF chain and exact
shared target, then compares role/policy/time high-water marks and preserved
publisher revocations. It only accesses fixed versioned metadata and the one
content-addressed policy target. Every response has a hard byte ceiling before
duplicate-key parsing; remote lengths cannot override it. Delegated roles and
extra targets are rejected before following them. Root keys require 2-of-3,
and decoded public keys cannot be shared across signing roles.

Durable acceptance uses one private-file record for checkpoint and target,
exact prior-state comparison, synced atomic replacement, and fail-closed
recovery. An unsettled mutation returns no accepted receipt. Old receipts are
invalidated on replacement or owner teardown; monotonic time also prevents a
wall-clock change from extending their lifetime. Disk contents alone cannot
recreate an accepted receipt: reopening recovers structural high-water state,
then fresh signature verification is required. No production root is compiled,
no desktop loop consumes these receipts, and Windows private-cache support is
still absent. There is no remote revocation-reversal authority in this client;
a reviewed reversal requires a separate explicit client contract.

The separate Beta source authority now consumes complete authenticated CRX/tree
receipts and fresh durably accepted policy. It requires an exact compiled
target/version opt-in, rejects revoked publisher keys/original CRX digests,
checks supplied upstream high-water state, and applies a closed declaration
policy to required and optional authority and nested execution semantics.
An absent upstream manifest key is permitted only on the structural source
entry point; a supplied key must match the original CRX publisher. The existing
Verified parser entry point still requires its exact reviewed manifest key.

`ProductAdmittedBetaSource` cannot be cloned, deserialized, or passed to a
Verified native plan. It owns the original tree receipt and rechecks policy
expiry/supersession when read. Its local package revision and previous upstream
checkpoint must still be rejoined with Store at commit. It grants no provider,
transformation, output-tree, permission, filesystem, or native execution authority;
those subsequent authorities must bind its exact source identity. In particular,
copying authenticated input to a sink is sufficient to analyze source but never
proves durable materialization. No public Beta installation is enabled by this
new source witness.

The separate prepared-artifact boundary now authenticates exact on-device
output for the initial key-identity transformation. It rechecks original CRX
identity and length, uses the authenticated developer SPKI (never a co-signer
or metadata-supplied replacement), preserves other file bytes and the complete
admitted permission/execution declarations, and seals/re-hashes the closed
filesystem tree. Stored JSON is only evidence: reopening recomputes it from
fresh admitted source and the original CRX. No path is exposed by the artifact
type. Provider eligibility, user grants, installed-repository authority, and
native ownership remain independent; structural provenance cannot supply them.
Interrupted prepublication stages are removed only within a known bounded
namespace. Published output survives reopening but must pass full verification.
Mutations are single-flight and invalidate prior receipts before they begin;
unknown settlement keeps the owner poisoned until recovery.

The following are roadmap items or disabled backends, not current security guarantees:

- remembered per-origin camera and microphone grants (the macOS broker and the
  WebView2 prompt are one-time decisions; the catalog behind `remember_enabled` stays
  off until the browser can list and revoke grants), and any permission beyond camera
  and microphone;
- Linux as a supported platform: the code stays in the tree, but the app refuses to
  start there;
- native Windows release qualification of downloads/protection,
  guaranteed malware detection, and automatic transfer resumption;
- Chrome/Firefox extension compatibility beyond the subset in
  `docs/extension-release-scope.md`;
- continuously maintained online blocker sources or full EasyList semantics: the usable
  network-only release seed is bundled, but production TUF trust is unprovisioned and
  packaged enforcement/endurance proof is still a release gate;
- a custom certificate-error interstitial or anti-phishing service;
- rollback-resistant update metadata: the updater (`desktop/src/updates.rs`) verifies a
  minisign signature on artifacts referenced by HTTPS `latest.json`, including the
  exact bytes consumed at installation. Metadata is not independently signed, and
  there is no durable highest-accepted-sequence beyond comparing versions. Payloads
  are size-bounded and held through anonymous owned files. Windows retains a bounded
  checkpoint before shutdown; recovery requires fresh HTTPS release metadata and
  payload verification with the embedded key. The macOS installer
  restores the original on failed publication, retains its backup if rollback
  fails, and requires manual installation when app-folder write access is denied;
- application-level encryption of profiles or session data;
- Chromium-equivalent full site isolation on every platform.

Keeping a feature disabled is the required safe posture until its complete broker and
tests exist. Documentation and marketing must not present these items as implemented.

## Production release gates

Before describing Zephium as a production-hardened privacy browser, all of the following
need an explicit implementation, native integration tests, or a documented accepted
risk. The recurring engine-floor, advisory, and fork-review procedure is defined in
`docs/security-maintenance.md`:

1. Close the platform integration gaps above: independently test the blank-bootstrap
   privileged construction path and native deny handlers, attach the macOS privileged
   UIDelegate at construction, decide and document the Windows file-picker and
   page-initiated print limitations, obtain and verify the macOS browser public-key
   credential managed entitlement before enabling browser-native passkeys,
   establish privileged process-group teardown seams, continuously refresh all three
   engine security floors, provide a release/update SLA before their review expiries,
   and handle WebView2 runtime replacement for long-lived processes.
2. Run packaged hostile-page tests on real supported Windows and macOS systems
   for IPC absence, custom-protocol absence, navigation/popup/download/permission denial,
   privileged file-picker policy, scripted print denial (top-level, initial blank,
   `srcdoc`, dynamic, and cross-origin frames), malicious PDF `/Named /Print` and
   clickable print actions, cross-profile cookies and storage, privileged-view
   non-persistence, renderer crashes, queue overload, process mitigations/confinement,
   and ordered shutdown. Source-level/unit CI is not a substitute for these tests.
3. Exercise the implemented crash-resumable profile-deletion coordinator through native
   engines. Write and then verify removal of cookies, cache, service workers, IndexedDB,
   local storage, WebSQL where present, HTTP authentication state, and engine caches for
   persistent and incognito profiles. Inject crashes and ambiguous storage outcomes at
   every authorization, native-proof, filesystem, and journal-finalization boundary.
4. Keep unsupported download adapters and remembered permission grants disabled until
   their brokers are complete and adversarially tested. Qualify the macOS file broker
   on signed release artifacts. The ad blocker ships on by default with the bundled
   seed; calling its protection stable and continuously maintained additionally
   requires provisioning and exercising the production trust domain and exact
   licensed list package, plus the packaged cross-platform enforcement, external-review,
   and endurance gates in `docs/adblock.md`.
5. Rehearse the production workflow with protected publisher credentials and verify
   its signed/notarized artifacts, SBOM attestations, encrypted symbols, and signed
   rollback-resistant metadata on real installer hosts. Exercise the updater's
   background download and relaunch-to-install path on both signed platforms, and decide
   whether a durable sequence check is needed beyond version comparison. Track every
   explicit Cargo advisory exception to removal; adding one is a reviewed risk
   acceptance, not a routine way to make CI green.
6. Pass recorded 1/10/50/100-tab and 24-hour endurance budgets for RSS, process/handle
   growth, crash loops, database growth, CPU wakeups, battery use, startup, and shutdown
   latency under adversarial workloads. Queue and view-count bounds alone are not a
   performance or denial-of-service proof.
7. Audit network traffic and native-engine policy on each OS before making stronger
   privacy or telemetry claims.
8. Obtain an independent audit of the native/Wry/OS boundary and remediate its release
   blockers before calling the browser stable or trusted for a large daily audience.

## Threat model

In scope are hostile pages attempting to reach native APIs, privileged-view XSS,
cross-profile data leakage, malicious navigation and protocol handling, permission and
download abuse, page-controlled resource exhaustion, persistence of private state, and
unsafe teardown/recovery.

The shared extension native-ownership journal remains cleanup-authoritative
during session recovery. Recovery mode rejects every new ownership `Begin` and
every acquire-directed `Transition`, but it must continue accepting exact-CAS
release-directed `Transition` and `Clear` mutations for already-journaled rows;
otherwise recovery could permanently strand a native owner and block safe
profile deletion.

Native-ownership clock and phase validation detects internally inconsistent,
torn, or corrupted journal histories before reconciliation. It is not an
external anti-rollback mechanism: an attacker who coherently restores the
entire shared database to an older valid state is outside that guarantee.

Not defeated by this architecture are a native engine sandbox escape, compromise of
the Rust process or shipped bundle, an already-compromised OS/user account, physical
access, swap/hibernation/backup forensics, traffic analysis, fingerprinting, or an
upstream supply-chain compromise. Runtime floors, dependency policy, signing, and rapid
updates mitigate some of these risks but do not remove them.

## Per-PR security checklist

- [ ] No raw tab gains Tauri IPC, a Wry IPC handler, a privileged initialization
      object, or a content-reachable custom protocol.
- [ ] Every new privileged command has an explicit caller policy, Rust-side input
      validation/bounds, and the smallest Tauri capability set.
- [ ] Every new navigation path uses the core and native scheme policy; no blocked URL
      is forwarded to the OS without a separate, user-gesture-aware broker.
- [ ] Page-derived strings and bytes stay tainted, bounded, and out of privileged HTML,
      command strings, filesystem paths, and in-process decoders.
- [ ] Profile context changes include cross-profile cookie/storage tests and document
      exactly which data remains shared in `meta.sqlite`.
- [ ] Incognito changes prove that session/history/favicon persistence is rejected and
      that privileged UI storage is non-persistent; tests account for engine-created
      temporary files and abnormal termination.
- [ ] Permission, download, popup, extension, content-blocking, and favicon code preserves
      its documented boundary behavior unless its complete policy and native tests land
      together; do not call a fail-open per-request blocker decision “fail-closed.”
- [ ] Release devtools remain disabled; privileged navigation lock, CSP, response
      headers, incognito construction, retained fail-closed native delegates/handlers,
      mandatory hardening installation, and label-scoped projection delivery remain
      intact.
- [ ] Queue producers remain non-blocking, memory remains bounded, overload behavior is
      tested, and accepted work cannot reorder across lifecycle barriers.
- [ ] Mutating IPC reserves the process-local result ledger before FIFO admission;
      pending/processed dispositions remain queryable until main chrome acknowledges them,
      and no process-local operation result is described as cross-restart durable.
- [ ] View lifecycle changes preserve the warm set, the 24/64 application watermarks, the separate
      75-resource native ceiling (83 with agentic contexts), visible-leaf protection, exact discard probes, and the
      rule that unsafe pages are not force-discarded to admit new work.
- [ ] Reentrant native work keeps typed normal/maintenance/observation/lifecycle/close/
      shutdown priority, keyed displacement rules, and a reserved non-droppable shutdown
      barrier.
- [ ] Linux WebKitGTK, macOS/Safari/WebKit, and Windows WebView2 capability/version floors
      remain aligned with supported packages and current security advisories; their
      review deadlines fail CI when stale.
- [ ] Profile-erasure changes preserve process-lifetime tombstones, exactly-once public
      completion, native in-flight state after a caller timeout, symlink-safe typed
      paths, and platform-specific post-clear verification. Do not call engine-only
      verification a complete profile deletion.
- [ ] Shutdown changes preserve command ordering, the durable snapshot/flush barrier,
      result-bearing main-thread dispatch, the eight-second outer application deadline
      around Windows' five-second process-group proof and bounded UDF removal, terminal
      post-teardown behavior, and fresh per-run private generations. Unproven older
      generations are never reused; only exact marked prior-boot data is reclaimed.
      Same-boot budget exhaustion remains recoverable by restarting Windows, and unknown
      objects require explicit manual remediation rather than inferred ownership.
- [ ] Dependency/source-policy and full frontend/build lockfile audits remain blocking CI
      gates; exact advisory exceptions stay justified, reviewed, and tied to removal work.
- [ ] This document is updated when a guarantee or accepted platform limitation
      changes.
