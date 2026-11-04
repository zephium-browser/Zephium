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

Procedural cosmetics, resource replacement, response-CSP mutation and
URL-parameter rewriting are out of scope. On macOS 15.4+ and Windows,
`##+js(...)` rules run scriptlets from a library built into the app, never
fetched; lists only choose the scriptlet and its arguments, and only uBlock
Origin's two lists may call `trusted-*` ones.
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

Desktop builds use these fixed publisher endpoints, in catalog order:

| List | Endpoint | License |
| --- | --- | --- |
| EasyList | `https://easylist.to/easylist/easylist.txt` | CC-BY-SA-3.0 |
| EasyPrivacy | `https://easylist.to/easylist/easyprivacy.txt` | CC-BY-SA-3.0 |
| uBlock filters | `https://ublockorigin.github.io/uAssets/filters/filters.txt` | GPL-3.0-only |
| uBlock filters – Quick fixes | `https://ublockorigin.github.io/uAssets/filters/quick-fixes.txt` | GPL-3.0-only |

HTTPS authenticates the server/transport; these downloads do not have an
independent publisher signature. The app trusts the official publishers' sites.
The signed application also embeds the same four lists as a licensed offline
seed. EasyList lists carry a numeric `Version` and upstream `Commit`; the uBlock
Origin lists have neither and are versioned by the UTC minute of their
`Last modified` header; the seed also records that header in full, so a copy
changed within the same minute is accepted only when it was modified later.
Their `!#include` lines are not followed.
Optional TUF support remains independently tested, but no TUF publication
infrastructure or signing keys are required by this release design.

The updater uses system TLS verification, bounded streaming, no redirects,
cookies or referrer, conditional ETag/Last-Modified requests, and one worker.
It persists a due time approximately 24–30 hours after a successful check;
quick fixes follow their own `Expires` header (8 hours today) within 4–24
hours. It respects longer server backoff, and performs no startup fetch while fresh.
Manual refresh coalesces and is rate-limited. Existing application maintenance
triggers due work; the blocker adds no idle polling timer.

Headers, titles, URLs, lengths, hashes, rule counts and suspicious shrinkage are
validated. The four lists form one candidate. The worker prepares the policy,
then macOS preflights the native network artifact before durable
activation. Static cosmetic syntax and budgets are validated by the compiler. Windows prepares its frozen matcher before publication. Failed
candidates leave the working policy installed. Current, previous and bundled
source material provide recovery; current bytes are verified with a bounded
streaming buffer. A 304 is accepted only for the exact verified retained bytes
and validators actually sent; otherwise one unconditional retry is allowed.
Unchanged source bytes avoid policy recompilation.

A rejected candidate is remembered. A list whose download is a copy that
candidate held keeps its current copy, so the other lists' updates go through;
that holds for an older copy too, so a revert to rejected bytes is never staged
again. The rejection is forgotten only after a refresh that checked every list.
Known limit: the candidate is rejected as a whole, so every list that changed
in it is withheld, including those that compiled fine. Each recovers when its
publisher next changes it (quick fixes within about 8 hours, the others daily).

The current bundled seed is **202611021408**, 4,073,004 uncompressed bytes.
`assets/blocker-seed/v1/` contains exact compressed/raw identities, upstream
attribution, both license texts (`LICENSE-CC-BY-SA-3.0.txt`,
`LICENSE-GPL-3.0.txt`), and a deterministic quality report. The seed is dated by
its oldest list, which the release gate allows to be at most 30 days old.
Release tooling and SBOM checks bind those artifacts to the shipping graph
(`release-bundle` plus `official-https`), rather than claiming downloaded bytes
are release-signed. Refresh the seed with:

```sh
cargo xtask update-blocker-seed --easylist PATH --easyprivacy PATH \
  --ublock-filters PATH --ublock-quick-fixes PATH \
  --license assets/blocker-seed/v1/LICENSE-CC-BY-SA-3.0.txt \
  --gpl-license assets/blocker-seed/v1/LICENSE-GPL-3.0.txt
```

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

## Zephium's own rules

`crates/zephium-blocker/src/zephium_rules.txt` is a short filter list of our
own, reviewed and shipped with the app, compiled after every catalog's lists
on macOS; its digest is part of the WebKit artifact's cache key. Windows leaves
it out until its requests carry their page: the runtime matcher sees them
without one, and a site-scoped exception would then apply on every site. It
holds exceptions only, never blocks (a test enforces this), for breakage the
downloaded lists do not fix yet. Each rule has a comment naming its site, the
evidence and the date, and is reported upstream when it applies there.

- Ad recovery: some sites (allrecipes, simplyrecipes, people, bild, mirror,
  tomsguide) probe AdSense's loader and, when it is blocked, re-serve the same
  ads through rotating proxy hosts: 40 to 710 requests to those hosts in 45
  seconds of one article, against 2 or 3 with protection off. Blocking the
  recovery instead replaces the page with an "allow ads" wall. Allowing that
  one file on those sites and their subdomains keeps the recovery idle while
  the ads stay blocked. In Zephium on macOS with the fix (3 runs each), protection on
  used 4.4 s of CPU and 123 requests on allrecipes against 16.2 s and 301 off,
  and 2.4 s and 89 requests on simplyrecipes against 8.6 s and 299.

## Budgets and evidence

| Resource | Ceiling |
|---|---:|
| One source / all sources | 16 MiB / 32 MiB |
| Source count / candidate rules | 32 / 250,000 |
| Physical line / lines | 64 KiB / 500,000 |
| WebKit rules / JSON | 140,000 / 32 MiB |
| Runtime regexes / raw pattern bytes | 1,536 / 2 MiB |
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

## YouTube

uBlock Origin's `set` rules remove the pre-roll ad data from the watch page, so
no pre-roll plays, but the video still starts later than in Brave. Measured on
2026-11-04 with QA builds, 5 videos × 3 runs per browser, mean ± sd:

| | Zephium | Brave |
| --- | --- | --- |
| macOS start ms | 8871 ± 6078 | 3831 ± 5339 |
| macOS stall ms | 7200 ± 5805 | 2083 ± 5245 |
| Windows start ms | 11308 ± 5403 | 4036 ± 3764 |
| Windows stall ms | 7650 ± 5438 | 1567 ± 3521 |

The stall is the player waiting for its first content media. Both browsers are
served the ad data in the page and SABR streaming. On macOS, YouTube's client
under WebKit requests neither of its ad probes
(`static.doubleclick.net/instream/ad_status.js`,
`googleads.g.doubleclick.net/pagead/id`) nor a second `/youtubei/v1/player`,
with protection on or off, and the hold is timed by the server: with the ad
left in, muted and played at 16×, content still arrived about the ad's length
after it began. uBlock Origin's rule that rewrites YouTube's page script did not
shorten the stall on either platform.

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
- A rule is listed only when the replay could test it and nothing matched. A
  load WebKit did not check as its recorded type keeps every rule of that type
  whose url-filter matches it. Each visit is recorded as ok, failed, timeout or
  late; a rule that applies only on sites whose page did not load (an
  `if-domain` rule, or a first-party rule for the host it names) is kept. A
  replay batch that never reports back, or a recorded load with no replay row,
  fails the pass.

Refreshing the list is a release step, not CI: it loads hundreds of real pages
in WebKit with the network on (about five hours), then replays every recorded
load against the bundled lists through a local listener that answers nothing.
After `cargo xtask update-blocker-seed`, on an otherwise idle Mac:

```bash
scripts/blocker-corpus/run.sh target/blocker-corpus
```

Review the diff of `webkit_pruned.txt` (its header records the corpus, the
replay, the visits that loaded and the rules kept for untested loads), then run `cargo xtask update-blocker-seed` again with the same lists
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
