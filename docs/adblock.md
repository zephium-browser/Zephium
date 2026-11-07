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
Origin's own lists may call `trusted-*` ones. The two that replace response
text, `trusted-replace-fetch-response` and `trusted-replace-xhr-response`, and
`trusted-edit-inbound-object`, which edits what YouTube's player sends, run
only on YouTube's hosts; their rules for any other site are dropped. The edit
reads the query language of uBlock Origin's wiki ("uBO flavored JSONPath", v2
without `call()`) and the older form today's YouTube rules use; a query
outside it is counted as unsupported and never runs. Zephium's own scriptlets
need a permission only its built-in rules hold, and those rules never hold the
one `trusted-*` scriptlets need.
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
| uBlock filters | `https://ublockorigin.github.io/uAssets/filters/filters.min.txt` | GPL-3.0-only |
| uBlock filters – Quick fixes | `https://ublockorigin.github.io/uAssets/filters/quick-fixes.txt` | GPL-3.0-only |
| uBlock filters – Link shorteners | `https://ublockorigin.github.io/uAssets/filters/ubo-link-shorteners.txt` | GPL-3.0-only |
| uBlock filters – Privacy | `https://ublockorigin.github.io/uAssets/filters/privacy.min.txt` | GPL-3.0-only |
| uBlock filters – Resource abuse | `https://ublockorigin.github.io/uAssets/filters/resource-abuse.txt` | GPL-3.0-only |
| uBlock filters – Unbreak | `https://ublockorigin.github.io/uAssets/filters/unbreak.min.txt` | GPL-3.0-only |
| uBlock filters – Badware risks | `https://ublockorigin.github.io/uAssets/filters/badware.min.txt` | GPL-3.0-only |

HTTPS authenticates the server/transport; these downloads do not have an
independent publisher signature. The app trusts the official publishers' sites.
The signed application also embeds the same nine lists as a licensed offline
seed. EasyList lists carry a numeric `Version` and upstream `Commit`; the uBlock
Origin lists have neither and are versioned by the UTC minute of their
`Last modified` header; the seed also records that header in full, so a copy
changed within the same minute is accepted only when it was modified later.
`filters.min.txt` is uBlock Origin's own build of `filters.txt` with its
includes (the general filters and one file per year); the `.min` lists carry no
`Homepage` header. No other `!#include` line is followed, and the mobile
filters are not used.
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
validated. The nine lists form one candidate. The worker prepares the policy,
then macOS preflights the native network artifact before durable
activation. Static cosmetic syntax and budgets are validated by the compiler. Windows prepares its frozen matcher before publication. Failed
candidates leave the working policy installed. A runtime candidate must still
block known EasyList and EasyPrivacy requests and allow a harmless one, judged
on a neutral page; a build without exact attribution judges them with no page
known, where uBlock Origin's site-scoped wildcard exceptions
(`@@*$script,1p,domain=…`) let everything through. Current, previous and bundled
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

An app that last updated the four lists of earlier releases has no package
that holds all nine, so after upgrading it uses the bundled seed until its
first refresh commits, even where its own copies were newer. Every list is due
at once, from six hours after the new seed was built, so the first maintenance
pass after the upgrade refreshes. It reuses the EasyList, EasyPrivacy and
quick-fixes copies it kept while their bytes verify. The five new lists and
uBlock filters, whose `filters.txt` copy lacked its includes, have no copy and
must all download in one refresh; until they do, as when offline, the seed
stays in use and the refresh backs off from 15 minutes to a day.

The current bundled seed is **202611041052**, 6,211,955 uncompressed bytes.
`assets/blocker-seed/v1/` contains exact compressed/raw identities, upstream
attribution, both license texts (`LICENSE-CC-BY-SA-3.0.txt`,
`LICENSE-GPL-3.0.txt`), and a deterministic quality report. The seed is dated by
its oldest list, which the release gate allows to be at most 30 days old;
resource abuse, which changes every few months, does not date it.
Release tooling and SBOM checks bind those artifacts to the shipping graph
(`release-bundle` plus `official-https`), rather than claiming downloaded bytes
are release-signed. Refresh the seed with:

```sh
cargo xtask update-blocker-seed --sources DIR \
  --license assets/blocker-seed/v1/LICENSE-CC-BY-SA-3.0.txt \
  --gpl-license assets/blocker-seed/v1/LICENSE-GPL-3.0.txt
```

`DIR` holds each list under its seed name: `easylist.txt`, `easyprivacy.txt`,
`ublock-filters.txt`, `ublock-quick-fixes.txt`, `ublock-link-shorteners.txt`,
`ublock-privacy.txt`, `ublock-resource-abuse.txt`, `ublock-unbreak.txt` and
`ublock-badware.txt`.

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

WebView2 does not supply the initiating frame URL in this callback. Each view
instead keeps the HTTP(S) main document it last committed, and every request it
filters is matched with that document as its source: `$domain` options and
first/third-party classification are judged against the main document, not the
frame. A frame's requests are therefore third party to the page that embeds it
whenever their host's site differs from the page's.

The source changes at commit (WebView2 `ContentLoading`, which runs before the
new document's scripts), not when a navigation starts. The old document keeps
its own source until then, so its requests during the navigation are not judged
against the incoming site, and a redirect, failed navigation or download changes
nothing. Requests it sends while unloading, such as beacons, can arrive after
the commit and are judged against the new document. A request the new document
makes before its commit, such as an Early Hints preload, is judged against the
previous document. Before a view's first HTTP(S) commit, and after a commit to
any other scheme or to a URL over 32 KiB, matching is source-independent:
domain- and party-scoped rules are skipped, blocks and exceptions alike, so an
exception that needs the page, such as `@@*$script,1p,domain=…`, never lifts a
block there. Service- and shared-worker requests are not
intercepted, so they have no main-document source either. The report still
counts attribution-sensitive rules separately from resource-type losses.

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

## Pages the lists refuse

Every page and frame is judged before it loads, and nothing is requested for
one the lists refuse:

- uBlock Origin's badware list (scam and malware sites) is compiled on its own
  for the runtime matcher, on both platforms, and kept in the compiled cache
  with the lists so a restart reads no source. WebKit never gets its wildcard
  rules.
- On Windows the full runtime policy then judges the document against the
  lists' `$document` rules, which WebView2's request filter never sees. On
  macOS WebKit's content rules apply them, and their refusal is reported the
  same way.
- A refused page covers the tab with Zephium's blocked page, naming the site
  and the reason: a known scam or malware site on the badware list, or a known
  ad or tracking redirect. Go back returns to the page that was showing, or
  back in history; Continue anyway opens it and allows its host in that profile
  until the app quits. A refused frame is only left empty.
- A tab opened straight at a refused address is judged before its view is
  built: the view starts on a blank page under the blocked page, so no
  navigation is cancelled during construction. The address bar shows the
  refused address and the tab is titled with its host.
- A site with protection paused is not judged. Site choices that have not
  loaded yet pause nothing.

Protection QA builds (`adblock-qa`) add `blocked-badware.test` and
`blocked-ads.test`, one host per reason, so the page can be shown without any
real site; no release build carries them.

## Zephium's own rules

`crates/zephium-blocker/src/zephium_rules.txt` is a short filter list of our
own, reviewed and shipped with the app, compiled after every catalog's lists;
its digest is part of the artifact's cache key. On Windows its site-scoped
exceptions are judged against the view's main document, like every rule there
(see above); before a view's first HTTP(S) commit they do not apply, as no
site-scoped exception does there. It never blocks (a test enforces this):
it holds exceptions for breakage the downloaded lists do not fix yet, and
calls to Zephium's own scriptlets. Each rule has a comment naming its site, the evidence and the date, and is
reported upstream when it applies there.

- Ad recovery: some sites (allrecipes, simplyrecipes, people, bild, mirror,
  tomsguide) probe AdSense's loader and, when it is blocked, re-serve the same
  ads through rotating proxy hosts: 40 to 710 requests to those hosts in 45
  seconds of one article, against 2 or 3 with protection off. Blocking the
  recovery instead replaces the page with an "allow ads" wall. Allowing that
  one file on those sites and their subdomains keeps the recovery idle while
  the ads stay blocked. In Zephium on macOS with the fix (3 runs each), protection on
  used 4.4 s of CPU and 123 requests on allrecipes against 16.2 s and 301 off,
  and 2.4 s and 89 requests on simplyrecipes against 8.6 s and 299.
- Ad recovery, second probe: mirror and tomsguide also fetch criteo's prebid
  tag beside the loader. EasyPrivacy blocks it as third party, which starts the
  same recovery: 36 to 58 requests to the proxy hosts in 16 seconds of the home
  page in Zephium on Windows, against 2 or 3 with the exception. The exception
  allows only that fetch on those two sites; the tag never loads as a script.
- YouTube start: with its ads blocked, YouTube holds a video with no buffer
  before it starts, unless the player's client carries one of the markers
  uBlock Origin's quick fixes key their player-request edits on (macOS,
  2026-11-06, 5 videos x 5 runs: Brave marked 18/18 not held and unmarked 7/7
  held; Zephium 25/25 held, stall median 8.5 s). `zephium-player-marker` sets
  that marker and moves to the next on a hold; the edits themselves come from
  the quick fixes. Markers, hold signature and Premium check are arguments,
  and a marker no admitted quick-fix rule both edits on and strips from
  comment requests is dropped when compiling, so a renamed marker is never
  sent and the worst case is the hold. A hold moves to the next marker only
  when the held data came from a request the quick fixes edited, which their
  `#reloadxhr` tag in the response's tracking URL shows; data inlined in the
  page or fetched ahead of a click is reloaded with the same marker, at most
  twice per video, and never moves it. While a reload it asked for is still
  unanswered, the player's wait is not counted as a hold. The player's own
  `onSnackbarMessage` event, a scriptlet argument, ends the wait at once,
  once per video, when the player's stats show the hold; the poll stays for
  holds the listener misses. In a probe (macOS, 2026-11-06, 9 loads, every one
  held) the event came at every hold the listener was attached for, 7 of 9,
  about 1.5 s before the poll acted, and on none of the 8 loads that played;
  the other 2 were raised before the listener attached. No global function is
  wrapped: a wrapped `Map.prototype.has` would show in its `toString`.
  With it (macOS, 2026-11-06, 5 videos x 5 runs and two in-page clicks per
  run): no in-page video was held 1 s or more (0 of 10, against 10 of 10
  before), and the event ended 10 of 25 direct-load holds, with a stall of
  0 to 500 ms. YouTube often raises it within 2 ms of the player's API
  appearing, before any listener can be attached, so the other 15 waited for
  the poll (stall 1000 to 1750 ms) and the direct-load stall median stayed
  1250 ms. Observing the player's container instead attached later: the
  container appears after the player.
  A navigation that never finishes stops guarding the player after 10 s, or
  when playback is asked for again.
  An unstarted player whose video was asked to play counts as held too: a
  consent prompt answered on the watch page leaves the player that way:
  without it a new visitor's first video was held 9.25 to 9.5 s (2 of 2), and
  with it played 1.8 to 2.0 s after the answer (4 of 4), never reloaded while
  the prompt showed. A
  video in a playlist the player holds is reloaded through the playlist,
  which keeps the player's list and place; a mix has no list in the player
  and is reloaded as a video. In both, the next item played in order through
  every kind of reload (2 runs, a mix and a playlist, 16 of 16), and loop and
  shuffle were kept. In a later run, YouTube's mix once went on to a video
  outside its panel with no request from Zephium.

## Budgets and evidence

| Resource | Ceiling |
|---|---:|
| One source / all sources | 16 MiB / 32 MiB |
| Source count / candidate rules | 32 / 250,000 |
| Physical line / lines | 64 KiB / 500,000 |
| WebKit rules / JSON | 140,000 / 32 MiB |
| Runtime regexes / raw pattern bytes | 4,096 / 2 MiB |
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
no pre-roll plays. Without a marker (see `zephium-player-marker` above) YouTube
then holds the video before it starts. Measured on macOS on 2026-11-06 with QA
builds, 5 videos × 5 runs, medians:

| Zephium | before the marker | with the marker |
| --- | --- | --- |
| watch page start ms | 10380 | 3339 |
| watch page stall ms | 8500 | 1250 |

- Watch pages: the first marker held 24 of 25 times and played; seeks that did
  not play again within 15 s, 0 of 50; no blocker warning.
- Related-video clicks without a page load: 10 of 10 held 1.25 s, then played
  with the page's marker, which never changed.
- The next video on a page behind another tab, after one played in view: 5 of
  5 kept the marker and played, with no reload.
- Every remaining hold is the marker's wait: 1 s of hold, then up to one 250 ms
  sample before it reloads.
- Known case: one watch page held on every marker in turn, played after 6.0 s
  unmarked, and showed YouTube's "Experiencing interruptions?" notice.
- The notice also showed on every related-video click in both browsers,
  always after the player had requested the clicked video's data again.
- Cost on a watch page, 30 s of playback, mean: CPU 12.6 s against Brave's
  22.5 s; footprint 1024 MB against 879 MB.

Brave 1.99.55 in the same runs, as context for these runs and not a claim about
Brave in general: 15 of 25 watch pages carried a marker and were not held
(start median 1847 ms), 10 were unmarked and held (stall median 13.75 s), and
40 of 50 seeks did not play again within 15 s. Its marked/unmarked split
changed from run to run with the same filter components. Brave's home-feed
numbers are not comparable: in earlier runs its feed load never finished, and
why is not yet recorded.

The hold is timed by the server. On macOS, YouTube's client under WebKit
requests neither of its ad probes
(`static.doubleclick.net/instream/ad_status.js`,
`googleads.g.doubleclick.net/pagead/id`) nor a second `/youtubei/v1/player`,
with protection on or off; with the ad left in, muted and played at 16×,
content still arrived about the ad's length after it began. uBlock Origin's
rule that rewrites YouTube's page script did not shorten the stall. Windows was
not measured with the marker.

## Rules WebKit never needed

WebKit turns a content rule list into automata and runs every one of them on
every resource load. Rules without `.*` share a few large automata; each rule
with `.*` gets one of its own, so the lists' ~1,600 wildcard block rules carry
most of its per-load cost. `crates/zephium-blocker/src/webkit_pruned.txt` lists
the wildcard block rules a recorded page corpus never matched, and the WebKit
compiler leaves them out. The compile report counts them as pruned, not as
unsupported.

- A rule whose only `.*` comes from a trailing `^`, `x([^A-Za-z0-9_.%-].*)?$`,
  is emitted as two rules without one, `x$` and `x[^A-Za-z0-9_.%-]`, which match
  the same URLs, so it gets no automaton of its own and is never pruned.
- Only block rules with `.*` are pruned; exceptions never are. Each entry is the
  SHA-256 of one rule exactly as emitted for WebKit, one per line, sorted, with
  no rule text. A new upstream rule, or an edit that changes the emitted rule,
  is kept until the next refresh. An edit that only changes letter case emits
  the same rule, since url-filters are case-insensitive, and stays pruned.
- The Windows runtime matcher has no such per-rule cost and compiles every rule.
- uBlock Origin's badware list names scam and malware sites, which no corpus
  visits, so its wildcard rules are never pruned. Those for page and frame loads
  alone are left out of the WebKit list (counted as `navigation_checked`, 310 in
  the seed) and judged when a page or frame starts loading. Every rule for any
  other type stays in WebKit, since that check never sees subresources. Keeping
  the 151 such wildcard rules adds 151 WebKit rules (114,818 to 114,969) and no
  measurable compile time: a cold `WKContentRuleListStore` compile of the seed
  took 2.66–2.71 s before and 2.69–2.70 s after (macOS 27.2, Mac mini, three
  runs each). Its rules without `.*` cost WebKit almost nothing and stay.
- The corpus covers the benchmark sites, YouTube, ad-heavy news sites, the top
  sites and pages where exceptions apply (`scripts/blocker-corpus/pages/`), and
  those hosts the wildcard rules name that are on the reviewed list. Only hosts
  listed exactly in `scripts/blocker-corpus/hosts-allowed.txt`, which holds
  well-known sites alone, are loaded: `cargo xtask blocker-prune admit`
  withholds every other page of every list and prints it for review, and the
  recorder cancels every navigation and redirect, in the page or in a frame, to
  any other host and logs it. Never added: piracy or unlicensed streaming;
  cracks, warez or free downloads; link shorteners; adult sites; gambling and
  betting, including odds sites; scams. Licensed services such as Netflix,
  Spotify, Twitch, Vimeo, Crunchyroll and YouTube are fine.
- A rule is listed only when the replay could test it and nothing matched. A
  load WebKit did not check as its recorded type keeps every rule of that type
  whose url-filter matches it. Each visit is recorded as ok, failed, timeout or
  late; a rule that applies only on sites whose page did not load (an
  `if-domain` rule, or a first-party rule for the host it names) is kept, and
  so is a rule for any page whose host no recorded load or visit reached: a site
  that is not recorded keeps its blocking. A replay batch that never reports
  back, or a recorded load with no replay row, fails the pass.

Speedometer 3.1 in the QA build, warm profiles, AC power, five round-robin
rounds (mean ± sd), with main's lists (d31275c5f) and the full set (47ae4ceba).
The full-set cost is paired by round.

| Mac | macOS | Off | Main lists | Full set | Full set vs main |
| --- | --- | ---: | ---: | ---: | ---: |
| Mac mini | 26.6.2 (25G83) | 48.32 ± 0.24 | 47.46 ± 0.13 | 46.98 ± 0.13 | −0.48 ± 0.21 (−1.0%) |
| M5 MacBook | 27.2 (26B123) | 61.98 ± 0.38 | 61.68 ± 0.26 | 61.74 ± 0.17 | −0.06 ± 0.39 (−0.1%) |

On 27.2 the full set costs nothing measurable; on the Mac mini, with 26.6.2's
older WebKit, it costs 1%. macOS ships the full set without a trim.

Refreshing the list is a release step, not CI, and runs only on a Mac whose
macOS and Safari meet the engine security floor the app pins:
`scripts/blocker-corpus/run.sh` starts with `cargo xtask blocker-prune
host-floor`, which refuses below it. It loads hundreds of real pages
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
