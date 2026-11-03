# Ad and tracker protection

## Status and release scope

The desktop implements network blocking, static cosmetic hiding, per-site pause,
and personal element hiding on macOS and Windows. The quick menu exposes site
controls and the picker; Privacy settings owns the live profile toggle, source
refresh, and retry. Rust owns durable state and exact operation settlement.

macOS native fixtures and the isolated browser QA build have been exercised.
Windows protection has been measured on physical hardware; the Windows engine
still needs per-release WebView2 qualification. Linux is not supported.

Protection defaults on for new profiles. META migration 23 enables existing
profiles once, preserving site pauses and personal hides; subsequent explicit
opt-outs remain durable. Both enabled and disabled profiles receive a locally prepared,
explicit empty policy at startup without joining the compiler queue. Enabled
profiles prepare real protection in the background. Browsing is available as
soon as the empty native policy is acknowledged; the UI never describes that
policy as active protection. Updates retain the installed policy throughout.
Already issued requests cannot be blocked retroactively.

Scriptlets, procedural cosmetics, resource replacement, response-CSP mutation,
URL-parameter rewriting, and YouTube-specific ad blocking are out of scope.
There is no proxy, TLS interception, local root certificate, or replacement
HTTP stack. Supported syntax and losses are reported, not advertised as complete
uBlock/Safari parity. The pinned adblock-rust fork is documented in
[`vendor/adblock/UPSTREAM.md`](../vendor/adblock/UPSTREAM.md).

## Ownership and startup

- `zephium-blocker-update` validates immutable catalogs and owns the official
  HTTPS source store, conditional fetches, candidate staging and recovery.
- `zephium-blocker-service` coordinates source preparation, native preflight,
  durable activation, the compiler worker and monotonic catalog status.
- `zephium-blocker` parses bounded sources, prepares immutable platform policies,
  resolves cosmetic exceptions, and persists compiled artifacts.
- `zephium-app` owns profile preference revisions, site-control revisions,
  compile/install generations, private-session state and operation settlement.
- `zephium-engine` owns exact native registrations, view/frame lifetime,
  pause gates, fixed stylesheet delivery and host-initiated picker evaluation.
- Svelte projects the Rust state and sends typed intent. Admission does not
  imply durable or native success.

A source-cache hit avoids downloading; a compiled-policy hit avoids parsing;
a native artifact hit avoids WebKit compilation. Cache formats bind compiler
versions, limits, source identity and supported resource vocabulary. The actual
release-corpus restart regression proves the worker skips source loading on a
warm hit, including after releasing declarative JSON.

Worker retirement is ordered after accepted work and in-progress callback
completion. Exact generations independently reject stale application/native
callbacks. Shutdown shares one absolute deadline through service, updater,
compiler and native cleanup; missing termination proof is unclean. The new
local startup artifact does not weaken worker retirement barriers.

## Official list maintenance

Desktop builds use these fixed publisher endpoints:

- `https://easylist.to/easylist/easylist.txt`
- `https://easylist.to/easylist/easyprivacy.txt`

HTTPS authenticates the server/transport; these downloads do not have an
independent publisher signature. The app trusts the official publisher's site.
The signed application also embeds a licensed offline EasyList/EasyPrivacy pair.
Optional TUF support remains independently tested, but no TUF publication
infrastructure or signing keys are required by this release design.

The updater uses system TLS verification, bounded streaming, no redirects,
cookies or referrer, conditional ETag/Last-Modified requests, and one worker.
It persists a due time approximately 24–30 hours after a successful check,
respects longer server backoff, and performs no startup fetch while fresh.
Manual refresh coalesces and is rate-limited. Existing application maintenance
triggers due work; the blocker adds no idle polling timer.

Headers, titles, URLs, lengths, hashes, rule counts and suspicious shrinkage are
validated. Both sources form one candidate. The worker prepares the policy,
then macOS preflights the native network artifact before durable
activation. Static cosmetic syntax and budgets are validated by the compiler. Windows prepares its frozen matcher before publication. Failed
candidates leave the working policy installed. Current, previous and bundled
source material provide recovery; current bytes are verified with a bounded
streaming buffer. A 304 is accepted only for the exact verified retained bytes
and validators actually sent; otherwise one unconditional retry is allowed.
Unchanged source bytes avoid policy recompilation.

The current bundled pair is **202609300903**, 3,568,061 uncompressed bytes.
`assets/blocker-seed/v1/` contains exact compressed/raw identities, upstream
attribution, license, and deterministic quality report. Release tooling and SBOM
checks bind those artifacts to the shipping graph (`release-bundle` plus
`official-https`), rather than claiming downloaded bytes are release-signed.

## macOS

Network filtering uses digest-addressed `WKContentRuleList` artifacts installed
before protected navigation. Network exceptions remain in the same artifact as
their blocks. The previous blanket native cosmetic sidecar and child-frame
stylesheet adapters have been removed after the interactive performance review.

Cosmetics now use the main-document selective pipeline described below. Personal
hides remain independent from subscriptions. Pause removes subscription styles
live; reload is still needed for requests/scripts that already ran.

Native network compilation retains its bounded queue: one physical compile,
two distinct active/queued jobs, 64 MiB of encoded artifacts and a 60-second
watchdog. Timeout never reuses a physical slot before the callback returns.
The hostname-boundary optimization remains unchanged: the September corpus
measured 2.187 seconds in the isolated native compile fixture. The old additional
cosmetic compilation is no longer part of startup or update preflight.

## Windows

WebView2 uses the shared immutable Rust matcher on `WebResourceRequested`.
A native per-view pause/provisional gate bypasses matching before URL/method
conversion. No per-tab regex compilation is performed. The callback does no
filesystem, network, actor, UI, deferral or blocking-channel work; it never waits
for a matcher lock. Values and evaluation work are bounded. Matcher/budget or
response-construction errors fail open and increment aggregate health counters.
Blocked responses are empty `403 Blocked`, with `Cache-Control: no-store`.

Document-sourced stylesheet, image, media, font, script, XHR, fetch and beacon
contexts are represented. Top/subdocument navigation, WebSockets, object/other
contexts, service workers and already-running shared-worker requests are not
intercepted by the current adapter. Registering environment worker requests on
every tab would multiply synchronous work; they need a future environment owner.

WebView2 does not supply an exact initiating frame URL in this callback. The
matcher therefore applies source-independent decisions and conservatively
allows requests where an unknown-attribution exception could matter. It never
substitutes the mutable top-level URL or invents third-party classification.
The report records this coverage loss separately from resource-type losses.

Document-created fixed scripts and constructed sheets provide main-document
cosmetics and personal hides. Live updates use `ExecuteScript`, with exact
document token, URL, navigation identity and policy generation. There are no
adblock FrameCreated/Frame2/Frame7 handlers, CDP, WebMessage or host objects.
Physical Windows qualification must still establish CSP behavior and rendering.

## Cosmetics, site controls and picker

Static selectors are parsed/validated, with procedural syntax rejected. The
compiler uses adblock-rust's URL-specific resources, generic class/ID lookup,
selector exceptions and generic-hide controls. For the current seed the initial
CSS is about 25.8 KB. A shared 487 KB serialized class/ID lookup table travels to
the main document once per policy/document; it does not become a blanket sheet.
Unchanged same-document navigation and personal edits reuse the installed
subscription identity instead of resending this payload. Removing the page copy
altogether remains performance work, not a completed optimization.
A bounded weak reference also reuses an identical live public cosmetic policy
across later profile restores without retaining it after the last owner exits.

A bounded worker handles document lookup, hashing and script serialization.
Completion returns through the existing main-thread dispatcher before native
script evaluation. Heavy Rust preparation never runs on the UI thread.

The document installs generic CSS only for observed classes/IDs. Mutation work
is limited to added elements and id/class changes, with at most 256 queued roots,
200 elements / 2 ms per idle batch, and 2,048 discovered selectors. Attribute
changes inspect only that element. Quiet pages schedule no timer, hidden pages
and pagehide disconnect observation, and there are no child-frame deliveries.
Initial document discovery and visibility/BFCache restoration are bounded walks.
Network filtering still applies to supported frame requests; main-page rules
can hide frame containers. These budgets deliberately prefer partial cosmetics
to excessive rendering work on pathological documents.

Cache reads validate and compare network JSON one rule at a time rather than
reconstructing/re-encoding the full object graph. Cold canonical serialization
also avoids a second all-rules object vector. Personal rules never enter the
subscription cache.

Site scope is the exact canonical HTTP(S) host across schemes and ports;
subdomains do not inherit a pause automatically. Durable pauses and personal
hides have bounded storage and compare-and-swap revisions. Private-profile
changes stay in memory and disappear when that private session closes. Source
updates never erase or recompile personal edits into public cache artifacts.

The picker only tracks pointer movement while open, uses animation frames for
highlighting, prevents selected clicks from reaching page handlers, and expires
after two minutes. Escape, navigation, tab/profile teardown and Cancel clean it
up. Preview precedes Save. Save rereads the selection through native evaluation,
validates bounded selector/label/count data in Rust, and binds admission to the
current profile/site/native document. Closed shadows, canvas and frame interiors
use container selection. Layout-dependent selectors are labelled as such.

Personal/preview styles include a bounded fallback for author `!important`
display rules. It preserves original inline values and restores only values the
browser still owns. This explicit-edit/initial-DOM path is capped at 1,000
matched elements and does not turn subscription CSS into a page scan.

## Blocking statistics

Profiles retain seven local calendar-day buckets containing counts only. macOS
uses the optional `_webView:contentRuleListWithIdentifier:performedAction:forURL:`
SPI and `blockedLoad`, advertised only while an owned subscription counter is
installed. No request URL is copied. The SPI's [declared availability](https://github.com/WebKit/WebKit/blob/main/Source/WebKit/UIProcess/API/Cocoa/WKNavigationDelegatePrivate.h)
is macOS 10.15; the runtime getter is also checked. Windows increments its profile
atomic only after a blocking response is successfully installed.

The existing maintenance cycle collects dirty counters and writes at most once
per 60 seconds, plus a final shutdown flush. Batches belong to their collection
day, including across midnight, clock corrections and sleep. Private counts stay
in memory; browsing-data clearing and profile deletion erase the totals.
The Quit menu and Cmd+Q enter the coordinated shutdown path so the final flush
runs before the store closes. Native QA verified saved totals after quitting and
reopening; Dock Quit and OS termination have not been separately qualified.

`blocker_stats(profile)` returns `today`, `last7Days`, and seven `days` values,
oldest first. Only the focused profile is queryable. New Tab queries on show,
tab changes, calendar-day changes and visibility restoration; statistics add no polling timer or event
stream. Unavailable statistics produce an error, never an invented zero.

## Budgets and evidence

| Resource | Ceiling |
|---|---:|
| One source / all sources | 16 MiB / 32 MiB |
| Source count / candidate rules | 32 / 250,000 |
| Physical line / lines | 64 KiB / 500,000 |
| WebKit rules / JSON | 140,000 / 32 MiB |
| Runtime regexes / raw pattern bytes | 1,152 / 2 MiB |
| Filter evaluations per request | 256 |
| Request/source URL | 32 KiB each |
| Cosmetic rules / portable policy | 50,000 / 16 MiB |
| One selector / document stylesheet | 4 KiB / 1 MiB |
| Paused sites / personal hides | 256 / 256 |

The September report records 134,081 network candidates, 106,244 accepted input
rules, 27,518,677 bytes of WebKit network JSON, 1,024 prepared runtime regexes,
and 24,066 accepted static cosmetic rules. Network report rejections include
cosmetic syntax intentionally processed by the separate cosmetic compiler;
do not add those counts or describe them as total unsupported rules.

macOS whole-browser QA verified actual request blocking; generic-hide exceptions;
main-document cosmetic hiding; picker preview/undo/save; hide reload persistence;
live Show/Remove; site pause/reload/resume; and the real Privacy settings control.
Independent native fixtures cover strict CSP, nested/cross-origin frames, live
style clearing, stale document/generation rejection and click suppression.
Child-frame cosmetic delivery is no longer enabled. Native QA also caught and
fixed stale site controls after navigation: site identity/readiness now participate
in status deduplication and focused tab updates publish that status.
No test result claims Windows runtime, installer, battery or broad-web coverage.

## Rules WebKit never needed

WebKit turns a content rule list into automata and runs every one of them on
every resource load. Rules without `.*` share a few large automata; each rule
with `.*` gets one of its own, so the list's ~860 wildcard block rules carry
most of its per-load cost. `crates/zephium-blocker/src/webkit_pruned.txt` lists
the wildcard block rules a recorded page corpus never matched, and the WebKit
compiler leaves them out. The compile report counts them as pruned, not as
unsupported.

- Only block rules with `.*` are pruned; exceptions never are. Each entry is the
  SHA-256 of one rule exactly as emitted for WebKit, one per line, sorted, with
  no rule text. A new upstream rule, or an edit that changes the emitted rule,
  is kept until the next refresh. An edit that only changes letter case emits
  the same rule, since url-filters are case-insensitive, and stays pruned.
- The Windows runtime matcher has no such per-rule cost and compiles every rule.
- The corpus covers the benchmark sites, YouTube, ad-heavy news and betting
  sites, the top sites, pages where exceptions apply
  (`scripts/blocker-corpus/pages/`), and the hosts the wildcard rules name.
  Adult sites are left out of every list.

Refreshing the list is a release step, not CI: it loads hundreds of real pages
in WebKit with the network on (about five hours), then replays every recorded
load against the bundled lists through a local listener that answers nothing.
After `cargo xtask update-blocker-seed`, on an otherwise idle Mac:

```bash
scripts/blocker-corpus/run.sh target/blocker-corpus
```

Review the diff of `webkit_pruned.txt` (its header records the corpus and the
replay), then run `cargo xtask update-blocker-seed` again with the same lists
so `compile-report.json` records the pruned count, and
`cargo xtask check-blocker-seed`.

Use `cargo xtask check-blocker-seed` for exact bundled artifacts; blocker
property/fuzz/fork-contract tests for compiler boundaries; and
`synthetic_blocker_lab` for bounded matcher measurements. The native child-frame probe was removed with that delivery path. The `adblock-qa` desktop feature builds a separate,
debug-only `app.zephium.protection-qa` application and data directory. This
feature rejects opt-level 0; use `CARGO_PROFILE_DEV_OPT_LEVEL=2` for QA builds.

Before each release: run the Windows protection QA (`scripts/qualification/windows-protection.ps1`),
complete packaged endurance/process-family CPU/RAM/battery measurements, test
representative daily-use pages and user QA, and resolve any release-blocking
findings. These are evidence gates, not claims inferred from cross-compilation.
