# Keep the generic cosmetic index out of the page

## Problem

Before this change, every main document received the whole generic cosmetic index:
`CosmeticPolicy::generic_index` (about 487 KB of JSON) is embedded in the
script `document_style_script` evaluates, then `content_style.js` parses it
into a `Map` of token → selectors. Each tab pays for parsing that script and
keeps its own copy of the index in its renderer, although Rust already holds
one shared `Arc<str>`.

On Windows, protection-on pages load as fast or faster than protection-off
pages but never use less peak memory (Yahoo 349 → 365 MiB, GitHub 418 →
451 MiB; see [historical Windows protection report](https://github.com/zephium-browser/Zephium/blob/86730cee0aabfd16c0d4d971742c59aaf8172fc9/docs/windows-protection-performance.md)).
macOS has the same per-document cost. The product goal is that protection makes
pages lighter, not just faster.

## Goal

The page holds no index. It reports the class and id tokens it sees; the host
answers with the few selectors those tokens match. Brave keeps its index in
the browser process the same way. Expected result: renderer memory and
per-navigation parse time with protection on fall below protection off on
ad-heavy pages, and stay within noise on clean pages.

## Constraints

- Shared code for macOS and Windows. No new page-to-host bridge (no
  WebMessage, script message handler or host object): the host already
  evaluates scripts in the page and reads results, so it pulls tokens through
  that same channel. Keep the closed, protected page script and its token checks.
- No steady polling. A visible view owns at most one host-initiated asynchronous
  pull. DOM discovery resolves it only when new tokens exist; navigation, policy
  and visibility changes cancel or re-arm it. Hidden tabs send no token batches.
  This also covers ads inserted long after loading, unlike a finite series of pulls.
- Keep the current bounds: 2,048 generic selectors per document, sliced DOM
  discovery, `generichide`, per-selector exceptions, personal hides and the
  picker unchanged. Same-document navigations must not resend work.
- Matching on the host happens on the style worker, never the UI thread, and
  shares the one immutable index across all tabs and profiles.
- Measure first: confirm with an index-off build how much of the on/off memory
  gap the index explains before rewriting. If it is not the cause, report what is.

## Evidence

On both platforms, protection off vs on, matched builds, three or more
alternating runs: Yahoo, Bloomberg, a news article, GitHub, and one clean page.
Report load time, renderer/process-family memory (peak and settled),
CPU, and blocked counts; plus 10 tabs and a five-minute idle with 10 tabs.
Show that late-inserted ads (infinite scroll, delayed slots) are still hidden.
Keep raw runs under `target/`; commit a short results table in this file.

## Implemented transport

macOS awaits one `callAsyncJavaScript` result in the existing page world;
Windows awaits a fixed `Runtime.evaluate` call through the existing WebView2
controller. Neither exposes a native message handler or a general CDP bridge
to the page. The style worker validates document token, navigation epoch, URL
and policy fingerprint, then matches against the existing immutable policy.
Stale replies are discarded before installation. Linux retains the prior path.

Each batch has at most 256 class/id tokens and 64 KiB of token text. The page
holds a bounded 4,096-token/128 KiB memo, one outstanding batch and one pending
batch, rather than the 487,107-byte index. The existing 2,048-selector ceiling,
sliced DOM traversal, domain exceptions, personal rules and picker remain.
Incremental deliveries share the initial stylesheet's 64 MiB retained-script
budget. At rest, the pending promise does not schedule timers. Visibility and
page teardown disconnect discovery and cancel the waiter; showing the page
catches up on changes made while hidden.

## Qualification — 2026-10-04

macOS 26.6.2, optimized release shell, isolated `app.zephium.cosmetics-qa`
identity launched with `open -n`; the user's normal profile was not used.
Raw measurements, app hashes, source snapshots and the temporary native load
logger are under `target/cosmetic-index-evidence/` in the qualification checkout.
The load logger is absent from committed source.

### Controlled index cost

Medians of three alternating pairs in fresh native WebKit documents using the
actual bundled policy and page script (not a same-size synthetic index):

| Installation | Renderer footprint | Evaluate round trip | Delivered script |
| --- | ---: | ---: | ---: |
| Original, index omitted | 24.42 MiB | 3.66 ms | 28,323 B |
| Original, full index | 29.69 MiB | 4.66 ms | 568,150 B |
| Candidate, generic matching disabled | 24.34 MiB | 3.52 ms | 28,324 B |
| Candidate, host matching enabled | 24.55 MiB | 3.32 ms | 28,323 B |

This isolates approximately 5.27 MiB of original per-document footprint. The
candidate installation overhead is approximately 0.20 MiB. The setup fixture
is not a measurement of the complete asynchronous matching pipeline.

Ten resident clean tabs in the real release app, sleeping disabled, three
matched 10-second samples per version, all with 18 processes/13 WebContent:

| Version | Median peak footprint | Median terminal footprint | CPU over 10 s | Idle wakeups |
| --- | ---: | ---: | ---: | ---: |
| Original `23b8de9e` | 511.61 MiB | 510.53 MiB | 8.95 ms | 22 |
| Candidate | 460.56 MiB | 459.47 MiB | 13.25 ms | 17 |

The terminal reduction is **51.06 MiB (10.0%) across ten tabs**. These are warm
profile measurements; the first original sample was already running and later
samples restarted the app. CPU differences are too small to claim improvement.
Five-minute idle runs were also recorded, but ended with different process
inventories (17 original, 18 candidate), so their footprint and wakeup differences
are not treated as a causal comparison. Native fixtures separately verify an
unresolved idle pull does not deliver periodic results.

The clean fixtures use `[::1]`: EasyList exempts `127.0.0.1` from generic hiding.
Earlier IPv4 runs, the input-timeout run and runs with unmatched process counts
are retained as raw diagnostics but excluded from the index-saving result.

### Real-site method and limits

Three alternating on/off 20-second process-family samples per site use the
candidate release app, the same warm isolated profile and the built-in per-site
pause switch (which reloads the page). They include native UI observation work;
"off" does not unload the shared policy or remove the native protection engine.
Each preceding site was closed before opening the next. Pages were visibly
rendered; optional cookie choices remained identical within each pair.
Native `Started` → `Finished` timings are available only where the temporary
logger captured matching navigation IDs; missing values are not estimated.

Network-block counts come from the existing New Tab counter before/after each
UI-driven trial, not from cosmetic hides. Collection occurs outside the exact
20-second sampler window, so delayed callbacks from the preceding protected
load can appear in a paused trial. Bloomberg's third pair was interrupted before
counter collection and its block counts are excluded. These counts demonstrate
operation, not a precisely synchronized network benchmark.

The on/off runs evaluate total protection behavior. Only the controlled
original-versus-candidate measurements above isolate this patch's savings.
They do not explain the remaining Windows 16–33 MiB gap or establish battery
improvement. A physical Windows run is still required; cross-typechecking the
exact native adapter with real Wry/WebView2 bindings is not runtime qualification.

Medians (MiB physical footprint; CPU is summed process CPU over 20 seconds):

| Site | Protection | Peak | Terminal | CPU (ms) | Load (ms) | Observed blocks per run |
| --- | --- | ---: | ---: | ---: | --- | --- |
| yahoo | on | 1001.8 | 688.7 | 4968 | 205 (n=3) | 18, 19, 19 |
| yahoo | off | 1018.0 | 711.4 | 5781 | 300 (n=3) | 0, 0, 0 |
| bloomberg | on | 1651.3 | 1286.2 | 7735 | 748 (n=3) | 42, 54, excluded |
| bloomberg | off | 1690.6 | 1379.0 | 10084 | 1973 (n=2) | 0, 0, excluded |
| guardian | on | 702.6 | 441.6 | 1089 | Unavailable | 31, 32, 32 |
| guardian | off | 702.9 | 587.0 | 3074 | Unavailable | 2, 2, 1 |
| github | on | 821.7 | 454.9 | 1431 | Unavailable | 7, 4, 5 |
| github | off | 582.9 | 427.2 | 982 | Unavailable | 0, 0, 1 |
| clean | on | 364.0 | 261.8 | 220 | Unavailable | 0, 0, 0 |
| clean | off | 381.7 | 261.4 | 327 | Unavailable | 0, 0, 0 |

Bloomberg memory grew across successive reloads; its third pair also followed
an interruption and its process inventory changed from 10 to 11. These samples
are not a stable estimate of this patch's effect. GitHub still costs more with
protection enabled; clean-page settled footprint is effectively unchanged.
The claim supported here is removal of duplicated index memory, not universal
on-versus-off superiority. Further attribution of the remaining protection
cost belongs in Windows qualification with equivalent native measurements.

### Functional and build gates

- Native WebKit fixture: initial selectors, delayed insertion, no idle result
  traffic, hidden quiescence, visible catch-up and bounded overflow all pass.
- Real release app: initial and late generic targets hide; pausing shows them
  and re-enabling hides them again. Useful page content remains visible.
- Eight WebKit component cases cover lookup, reuse, bounded discovery, pending
  pull cancellation and a node removed while discovery is under backpressure.
- All 91 blocker and 220 engine library tests pass, including domain exceptions,
  generichide, untrusted batch bounds, visibility revisions and stale documents.
  Strict affected-crate Clippy, repository formatting, `pnpm check` (677 unit
  tests), the production frontend build and frame style checks pass.
- Windows adapter cross-typecheck passes with real Wry and WebView2 types.
  Full Windows build, runtime lifecycle and on/off measurements remain open.
