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

## Scriptlets and YouTube (design, not built)

YouTube serves its ads from the hosts that serve the video, so network rules
cannot remove them: with protection on, every watch page still shows a
pre-roll. uBlock Origin and Brave remove them by editing what the page's own
scripts read and send. This section is the plan for doing the same the way
Brave does; nothing here is implemented yet. The bar is Brave on the same run
and the same videos, on macOS and Windows.

**Matching is adblock-rust's.** The engine we already use parses
`example.com##+js(name, arg, ...)`, matches hostnames and entities
(`youtube.*`), applies `#@#+js()` exceptions, and gates rules by permission:
`ParseOptions.permissions` is set per list, each scriptlet resource carries the
permission it needs, and `url_cosmetic_resources(url).injected_script` returns
the finished script for a page (the cosmetic path already calls it). Our
library is loaded into the engine as adblock resources: one `fn/javascript`
resource per scriptlet, named and aliased as the lists name them, with shared
helpers as declared dependencies, so the engine emits each function and its
helpers once and calls it with the rule's arguments. There is no second parser
or matcher.

**Lists.** uBlock Origin's "uBlock filters" and "uBlock filters – Quick fixes"
are where the YouTube rules live, so they join the catalog first; their network
and cosmetic rules come with them. Both are GPLv3: they ship as separate seed
data with attribution, are fetched from pinned HTTPS origins with the existing
shrink guards, and each gets its own WebKit pruning pass. Quick fixes changes
daily and is checked on its own short interval (about 6 hours); the others keep
the 24–30 hour cadence. `!#if` blocks are evaluated as uBO does on Chromium
(`env_chromium`, no `cap_html_filtering`, no `env_firefox`), on both platforms:
WebKit has no response filtering either, and these are the conditions the
YouTube rules are written for. Rules that need Firefox's response filtering
(`$replace`, `##^`) are counted as unsupported.

**Trust.** Bit 0 of the permission mask is "trusted": only the two uBO lists
are parsed with it, and every `trusted-*` scriptlet requires it. User filters
and every non-uBO list are parsed without it, so a `trusted-*` rule from them
never injects.

**The library is ours, written clean-room.** It ships in the app, reviewed MPL
code, never fetched. Its author works from uBO's documented behavior (the
Resources Library and JSONPath wiki pages) and our fixtures, and does not read
uBO's `scriptlets.js`. Brave's own resources are not used: Brave assembles its
scriptlets from uBO's GPL source at build time, and of Brave's MPL files only
`brave-yt-sabr-fix.js` concerns YouTube playback, and Brave ships it disabled.
The first release covers every name the current YouTube rules use
(`youtube.com`, `m.`, `music.`, `tv.`, `youtubekids.com`,
`youtube-nocookie.com`), with aliases:

- Values and timers: `set-constant` (`set`), `adjust-setTimeout`
  (`nano-stb`), `prevent-setTimeout` (`nostif`), `remove-attr` (`ra`).
- JSON: `json-prune`, `json-prune-fetch-response`, `json-prune-xhr-response`,
  `json-edit-fetch-response`.
- Requests: `no-xhr-if`, `trusted-replace-fetch-response` (`trusted-rpfr`),
  `trusted-replace-xhr-response`, `trusted-json-edit-fetch-request`.
- Outbound edits: `trusted-edit-inbound-object`,
  `trusted-replace-outbound-text`, `trusted-replace-argument`.
- Page and DOM: `trusted-prevent-dom-bypass`, `replace-node-text` (`rpnt`),
  `remove-node-text` (`rmnt`), `href-sanitizer`.

Rules that use the outbound edits and the JSONPath edits are the current
anti-adblock and throttling countermeasures (they mark player requests as
inline or ad-unit playback so no ad is served), so they ship in the first
release, not later. Names YouTube used recently and may return to are in the
set. The rest of the 51 names the two lists use come with phase 2.

**Unknown names are counted.** The compiler records every `##+js` rule whose
name the library lacks, per list and per name, in the compile report, and the
diagnostics page shows the count for YouTube's hosts. The weekly YouTube check
fails when that count is not zero, so a move to a new scriptlet shows before
users see ads. A new name needs an app release; new rules for known names
arrive with list updates.

**Where it runs: one model on both platforms.** At document start, in the page
world, in all frames, one script per profile: a prelude that looks up the
frame's hostname in a table, then runs the matching entry. The table is built
at compile time from adblock-rust's answers: one key per hostname or entity the
lists name, whose entry is `injected_script` for that key's own host (entities
use a host under a public suffix no rule names for that label). The prelude
walks the hostname's suffixes, then its entity forms, and takes the first,
most specific key, so a subdomain inherits its parent's answer and exceptions
stay adblock-rust's. Pages whose host has no key run nothing else. The library
sits inside a function body the prelude calls only on a match, so other pages
pay for a lookup, not for compiling scriptlets. macOS uses a `WKUserScript`
in the page content world for all frames, Windows the document-created script.
WebKit's scoped injection (include match patterns) is SPI, so it is not used
unless Speedometer shows the in-script check costs something; then it gets a
per-version check with this model as the fallback. A site pause or protection
off removes the script with the policy. YouTube embeds on other sites match by
the frame's own host.

**Script size budget.** With the full uBO lists the table covers about 2,800
hosts and 2,600 rules (about 170 KB of rule text), and it is parsed in every
frame. Entries are stored as indices into deduplicated invocations, the
prelude stays a hostname lookup, and the budget is set from the phase 2 counts
before the lists ship: Speedometer and the sites mode with the full lists
loaded must show no change against protection on without scriptlets.

**Editing responses without breaking playback.** `fetch` and `XMLHttpRequest`
are wrapped once per page, so their `toString` and identity look native. A
request is examined only when it matches a rule (for YouTube: `/youtubei/v1/`
player, next, browse, reel and get_watch requests); everything else, including
`googlevideo.com` media and streaming bodies, passes untouched. A matched fetch
body is read as text, edited, and returned as a new `Response` with the
original status, headers and URL; a body that is not JSON, or an edit that
throws, returns the original. XHR edits apply at load through that request's
`response`/`responseText` only, keeping `readyState` and event order. Pruning
never removes `streamingData`, `playabilityStatus` or `videoDetails`.

**Coverage.** Pre-roll and mid-roll (player responses and initial data),
in-feed and home-feed ads (browse and next responses, plus EasyList and uBO
cosmetics), Shorts ads (reel responses), the masthead, YouTube Music, embeds on
other sites, the enforcement wall, and the fake-buffering throttle (the
outbound request edits). Server-stitched ads, where the ad is part of the
video stream itself, have been in testing since mid-2024; neither uBO nor
Brave removes a stitched segment, and their countermeasure is the same request
edit, so the ad is not served. The check records any ad time that still
appears, so we see stitching if it reaches our test accounts.

**Tests.** Fixtures are minimal synthetic JSON with the real shapes (player,
next, browse, reel responses with and without ads), with no identifiers,
tokens or visitor data. A headless WKWebView, and WebView2 for the Windows
script, serves them from a local scheme to a page that calls them the way
YouTube does, runs the scriptlets, and checks: ad fields gone, playback fields
byte-identical, outbound edits applied only to matching requests, wrappers
native-looking, untouched requests unchanged.

**Gates.** The bench's YouTube mode runs Zephium and Brave on the same videos
in one run, on macOS and on Windows. Pass: no ad time, `adMs` 0, no player ad
data, no wall, and video start and seek latency no worse than Brave's or than
protection off. On other pages nothing changes: Speedometer, and the sites
mode's request counts and console errors, against protection on without
scriptlets. The weekly scheduled YouTube check runs the same mode and the
unknown-name count.

**Order.** One branch, reviewable commits, with the YouTube effect visible from
the first push: the uBO lists and the permission mask, then the resource
loading and the injection on both platforms with `set` and the `json-prune`
family, then the request and outbound edits, then the remaining names, the
unknown-name count and the weekly check. The rest of phase 2 runs beside it.

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

Use `cargo xtask check-blocker-seed` for exact bundled artifacts; blocker
property/fuzz/fork-contract tests for compiler boundaries; and
`synthetic_blocker_lab` for bounded matcher measurements. The native child-frame probe was removed with that delivery path. The `adblock-qa` desktop feature builds a separate,
debug-only `app.zephium.protection-qa` application and data directory. This
feature rejects opt-level 0; use `CARGO_PROFILE_DEV_OPT_LEVEL=2` for QA builds.

Before each release: run the Windows protection QA (`scripts/qualification/windows-protection.ps1`),
complete packaged endurance/process-family CPU/RAM/battery measurements, test
representative daily-use pages and user QA, and resolve any release-blocking
findings. These are evidence gates, not claims inferred from cross-compilation.
