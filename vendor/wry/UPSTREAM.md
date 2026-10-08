# Vendored Wry provenance

This directory is Wry 0.55.1 from the immutable upstream revision
`fe9e7fb73bb6ad2637cb0b4b1685676c86970aeb`:

<https://github.com/tauri-apps/wry/commit/fe9e7fb73bb6ad2637cb0b4b1685676c86970aeb>

`FORK.toml` is the machine-readable companion to this document. This is a
reviewed native security adapter under active hardening, not an unmodified copy
of the crates.io package and not yet an externally audited production boundary.
Its security-relevant deltas currently enforce these invariants:

- macOS file selection is deny-by-default with an explicit embedder callback.
  Native frame origins are bounded before Rust allocation. The single-use,
  non-Send responder cancels on drop, including unwind, and returns only native
  selected URLs to WebKit. The embedder owns picker admission, visible origin
  labelling and navigation/view teardown; no Tauri or agent builder opts in.

- Permission callbacks are available on every supported platform and unhandled
  permission requests fail closed.
- No script-message/IPC bridge is registered when the embedder supplied no
  handler.
- GTK regular and ephemeral `WebContext`s opt into cross-site process swapping,
  enable the Web-process sandbox, and verify both results before a WebView is
  constructed. Construction is fallible when either postcondition cannot be
  proved; it never asserts in a release process. `try_new` reports this at
  context creation, while source-compatible `new` is revalidated by
  `build_gtk` before WebView/WebProcess creation. Pathless GTK contexts are
  ephemeral from initial native allocation in this fork; persistent browser
  contexts require an explicit profile data directory. Incognito views reject
  a supplied persistent context rather than reclassifying it after potential
  filesystem access. Incognito related views must use the exact supplied
  ephemeral context.
- WebView2 construction exposes a pre-controller hardening hook and tracks
  partially constructed controller ownership so failures remain cleanable.
  `native_cleanup.rs` models the ordered, retryable ownership debt. The Windows
  adapter keeps parent-subclass state, controller `Close`, and Wry-owned HWND
  destruction explicit; successful steps are terminal. Incomplete COM/HWND
  debt is transferred immediately into a bounded apartment-local registry.
  The public error carries only a `Send + Sync` incident identifier and
  failure description, never an apartment-bound native object.
- `native_bounds.rs` bounds untrusted native title, URL, version, and filename
  strings before copying them into Rust. Privileged IPC accepts only primitive
  strings up to 64 KiB (checked in both native units and UTF-8 bytes) on all
  three desktop engines; non-string and oversized messages are dropped before
  a Rust payload allocation. On WebKitGTK, the native message handler exists
  only in an isolated script world. A page-world DOM bridge feeds an isolated
  listener that checks both ceilings before asking WebKit to materialize the
  native `GBytes`, so hostile page code cannot bypass the pre-allocation
  check through `window.webkit.messageHandlers`. Privileged custom-protocol requests additionally
  admit at most a 64-byte method, 128 headers / 64 KiB of aggregate header
  bytes (with 1 KiB names and 16 KiB values), and a 64 KiB body. Native engines
  may materialize their own request/string objects before exposing a length;
  the adapter prevents a second attacker-sized Rust allocation and stops body
  streaming at the limit. `native_admission.rs` additionally allows at most 32
  asynchronous custom-protocol requests per WebView (per verified WebKitGTK
  context on Linux); on macOS the bound is per scheme of each WebView, so IPC
  and assets never hold each other back. Every accepted request carries a
  unique non-cloneable RAII permit through response, cancellation, timeout, or
  teardown. On macOS a request past the bound waits, in arrival order within
  its scheme, in a main-thread wait list of at most 512 tasks and starts as a
  permit is released; a stopped task leaves the list, and teardown clears it.
  Only a request past both bounds (and, on other platforms, past the in-flight
  bound) completes synchronously with an empty 503 response and never acquires
  a native deferral. Teardown seals and drains accounting, and late permit drops
  cannot underflow or reopen it. Zephium's page-evaluation integration accepts
  primitive-only contracts: fixed-size discard and favicon results and one
  explicitly bounded HTML string. Page objects and attacker-controlled object
  serialization are not accepted as host protocol messages.
- A successfully constructed macOS view retains its proven host `NSWindow` and
  updates that ownership only after successful reparenting. The source-compatible
  `ns_window()` API therefore does not manufacture a static borrow or unwrap a
  transiently detached AppKit relationship.
- WebKit native file, credential/client-certificate, media, popup, navigation,
  download, drag/drop, and custom-protocol callbacks fail closed on malformed
  optional native values. Standard TLS server-trust challenges retain the
  system's default certificate validation and never accept a custom credential.
  Page-facing callbacks do not use panic-based control flow.
- WebView2 construction requires both the basic-authentication and
  client-certificate event interfaces and registers deny handlers before the
  initial navigation. Basic authentication is cancelled; certificate
  selection is cancelled and marked handled, so neither can fall back to an
  unowned native dialog. It also requires `ICoreWebView2_25` and cancels plus
  suppresses `SaveAsUIShowing`, closing the independent native Save As surface
  that context-menu, accelerator, PDF-toolbar, and download policy do not
  comprehensively cover.
- WebView2 and WebKitGTK page-driven close requests default to a no-op.
  Destroying Wry's child container/widget is an explicit builder policy for
  embedders that can also reconcile native and logical-view ownership; a page
  cannot otherwise leave the host tracking a stale HWND or GTK widget.
- WebKitGTK container routing uses GTK subtype checks rather than exact runtime
  class names. Security-owned `GtkFixed` subclasses therefore retain native
  fixed positioning and bounds updates instead of falling through to generic
  one-child container behavior. Reparenting publishes geometry mode before
  reentrant GTK signals and verifies the actual native parent afterward.
- Raw WebKit link previews, Picture-in-Picture, macOS fullscreen, and
  WebKitGTK fullscreen are disabled until the embedder has an
  origin/gesture-labelled native-surface broker. The macOS fullscreen and PiP
  preferences are per-view attributes: Cargo feature unification may make the
  private fullscreen API available to privileged chrome without granting it
  to untrusted child views. A macOS file drag with no explicit handler is
  rejected before Wry reads pasteboard entries; handler-enabled drags cap both
  file count and each native file-URL string before allocating Rust paths.
- Asynchronous custom-protocol responders are structurally `Send`; the fork has
  no blanket unsafe `Send` implementation for them. Apartment/main-thread-only
  WKWebView and WebView2 objects remain in UI-thread registries while workers
  carry only opaque tokens and owned HTTP responses. Dropped responders complete
  exactly once with an internal-error response instead of leaking a native
  request or deferral.
- WebView2 validates every COM `IStream::Read` byte count against the supplied
  buffer before budget arithmetic or slicing. Its trusted print method calls
  `ICoreWebView2_16::ShowPrintUI` directly instead of evaluating the same
  page-controlled scripted-print primitives an embedder may deny at document
  creation. Disabling browser accelerator keys is a construction postcondition:
  the fork requires `ICoreWebView2Settings3`, applies the setting, and reads it
  back before an untrusted page can load.
- Desktop navigation callbacks expose one opaque per-WebView identity across
  provisional start, every redirect, commit, successful finish, and failure.
  WebView2 retains its native `NavigationId` and a bounded map of the last
  admitted URL instead of reading mutable global `Source` during overlapping
  callbacks. WebKitGTK assigns a non-wrapping identity to its documented
  ordered load sequence. For programmatic WKWebView loads, the adapter binds
  the exact `WKNavigation` returned by `loadRequest` to the requested URL
  before any asynchronous lifecycle callback can be accepted. Apple does not
  expose an action/response-to-`WKNavigation` correlation for page-driven
  loads, so those loads are not guessed through a global pending slot: their
  exact callback identity and the now-current bounded `WKWebView.URL` are
  emitted together at `didCommit`. `WKWebView.URL` is never treated as a
  provisional-navigation URL. All retained identity state is bounded and
  native-policy admission remains fail-closed when the bound is exhausted.
- The same identity callback owns a commit-time presentation and input guard.
  Before the embedder sees `Committed`, WebView2 hides both controller and
  child HWND, WebKitGTK revokes mapping, paint, and input, and WKWebView hides
  the native view. A guarded WebKitGTK child also remains unmapped throughout
  construction; Zephium's stage performs its first map at validated offscreen
  geometry and is the only code allowed to reveal it after exact attribution.
  The embedder's synchronous callback can revoke its own per-view permit before
  those native calls pump re-entrant layout. Wry also exposes a bounded
  current-document title query so the embedder can attribute title only after
  its exact navigation lifecycle is complete.
- Linux CI constructs a guarded WebKitGTK view inside an already-mapped
  `GtkFixed`, drives a real local document through commit while mapping, paint,
  focus, and input remain revoked, then proves the Stage-compatible offscreen
  first-map and attributed reveal produce a rendered native snapshot. Source
  ordering tests supplement this native gate; they do not replace it.

The standalone `Cargo.lock` is intentional. Fork CI invokes this manifest with
`--locked` so it cannot silently resolve a graph different from the reviewed
one. Application builds still use the workspace lockfile at the repository
root.

The Windows patch adds a pre-controller environment-observation hook so the
embedder can retain the exact environment, process HANDLE, and Environment5
exit registration even when a later controller initialization step fails. A
construction guard attempts to remove any installed parent subclass, close a
partially initialized controller, and destroy its child HWND on error. Each
  step can be retried independently; incomplete work remains typed cleanup
  debt in the creating COM apartment and the error exposes a send-safe incident
  descriptor rather than describing the native state as closed. Callback state takes a temporary strong
reference on entry so reentrant removal cannot free it while USER32 is still
executing the callback. Wry no longer injects/registers its web-message IPC
surface when no IPC handler was supplied, malformed WebMessage source data is
dropped instead of panicking, and a popup callback with a missing native sender
is denied instead of unwrapped. No-callback popup denial sets `Handled` before
reading metadata or taking a deferral, and the process-wide posted-closure
registry has an exact bound. These changes make the fork expose a staged
native-ownership boundary without adding a page capability.

The same no-handler rule applies to WebKitGTK: raw views no longer register an
unused IPC script-message listener. Handler-enabled views register the native
endpoint only in Wry's isolated world; registration failure aborts
construction. The signal closure holds only a weak WebView reference, avoiding
a manager/WebView retention cycle. Malformed/missing native URI or message
values (including load notifications) are dropped/defaulted rather than
unwrapped in the UI process.

Every Wry update follows the complete [`REBASE.md`](REBASE.md) procedure: diff
the full upstream range, disposition every `FORK.toml` patch set explicitly,
regenerate both lockfiles deliberately, run native hostile-page tests on all
three platforms, update the immutable revision, and rerun dependency, license,
and provenance gates. A version bump without that review is not an accepted
update procedure.

### macOS website file workflows

The opt-in upload responder owns the main-thread native completion and cancels on
drop. An opt-in download callback hands the original WKDownload to the embedder
before installing the legacy delegate; explicit DownloadPolicy::Deny always wins.
Renderable HTTP attachment responses use the native download policy too. Native
failure callbacks accept absent resume data. Handler-enabled file drops validate
the complete pasteboard cohort (128 items and bounded file URLs), preserve WebKit's
actual drag operation, and reject hidden views before native fallback. Default
upload/download/file-drop denial remains available to privileged/agent views.
See the application file-workflow record for broker ownership and native evidence.

Native cancellation retains the exact WKNavigation identity. The public legacy
WebKit policy-interruption error (`WebKitErrorDomain`, 102) and Foundation request
cancellation (`NSURLErrorDomain`, -999) emit Cancelled rather than Failed. Neither
outcome implies a committed document or completed download. The application keeps
an uncommitted cancelled controller hidden/reusable so pending native download
save sheets retain their owner. See Apple's [policy-interruption constant](https://developer.apple.com/documentation/webkit/webkiterrorframeloadinterruptedbypolicychange).

### Native links and context-menu downloads

The native new-window hook carries user-activation evidence and foreground intent.
WebKit's automatic-script-window preference is disabled. Modifier-click interception
loads the original NSURLRequest into the supplied host-owned controller. Ordinary
new-window requests preserve WebKit's supplied configuration and native WindowProxy.
Windows adds a guarded Create response: the embedder installs request filters after
SetNewWindow and before the native deferral completes, or closes the child. Callback
panic denies. An opt-in page-close callback leaves controller destruction to the host.

The optional private WebKit navigation delegate selector
`_webView:contextMenuDidCreateDownload:` hands a context-menu WKDownload to the same
native broker used for navigation downloads. Explicit denial still wins; a missing
handler or panic cancels. This selector requires native testing on each supported
macOS/WebKit version and must be reviewed during upstream rebases. It is not a
public API guarantee or a URL replay fallback.

Windows native context menus are opt-in and host filtered, and require a separate
SaveAsUIShowing cancellation registration because document Save As is distinct
from DownloadStarting. Privileged/agent defaults remain unchanged. See the
[application qualification record](../../docs/native-links-implementation.md).

### Page dialogs and failure categories (macOS)

`alert`, `confirm` and `prompt` show as an `NSAlert` sheet on the view's window,
titled by the initiating frame's security-origin host ("example.com says", or
"An embedded page at … says"). A hidden view, or a window that already shows a
sheet, gets the dismissed answer, as upstream's no-UI default did. From the
second dialog within ten seconds the sheet offers to silence the page until its
URL changes. Messages are bounded to 1,024 characters.

`navigation_failure_handler` reduces a failed main-frame navigation's
`NSURLError` code to a category the host can explain (offline, host not found,
unreachable, timed out, insecure, other). Native error text never crosses it.

On Windows the same handler is fed from `NavigationCompleted`'s
`WebErrorStatus` (cancellation and download conversion stay silent). The
`ContentLoading` of WebView2's built-in error page is hidden like a commit but
not reported as one, so the failure lands on the document the person asked for
and the host can show its own explanation.
