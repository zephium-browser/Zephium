# Windows protection qualification

## Checkout and scope

Work from `main` (the ad blocker and Windows extensions are merged there) on a
new branch, for example `feat/windows-protection`. Record `git rev-parse HEAD`
in the report. Do not merge or push to `main`; the user's QA is the merge gate.

Goal: Windows reaches the macOS experience, and where WebView2 allows, beats it.
With protection on, ad-heavy pages must load faster and use less memory and CPU
than with it off; clean pages must stay within noise. The interface is shared
and already finished on macOS: the Utilities menu's protection section (site
name, "N blocked today", per-site switch that reloads the page, Hide elements,
Hidden on this site), the sidebar hiding bar (Undo, Done), the rail's native
More menu (Block Ads and Trackers, Hide Elements…) and the new tab counter.
Do not fork the frame; fix Windows behavior underneath it.

Protection defaults to on for every profile. Until this qualification passes,
the user decides whether Windows ships with it on; report the evidence that
decision needs.

Implementations are in `platform/windows/content_filter.rs`,
shared `host/content_styles.rs`, `host/style_worker.rs`,
`host/content_style.js`, and `host/element_picker.rs`. `zephium-app` owns durable
site controls and private-session routing. Read [adblock.md](adblock.md) and
[the design](adblock-release-design.md) before changing native boundaries.

Cross-compilation on macOS passes. It does not qualify runtime APIs, CSP,
frame event ordering, installer behavior or resource use. The macOS browser
passes the interaction cases listed below. Keep Linux and YouTube/scriptlets
outside this release task.

## Build and baseline

Record OS build, CPU/RAM, WebView2 runtime version, power mode, exact SHA and
build mode. Start with a clean **QA data directory**, not the user's ordinary
profile. From the repository root:

```text
pnpm -C frame check
pnpm -C frame test:component src/features/settings/tests/ProtectionControls.component.test.ts
pnpm -C frame build
cargo xtask check-frame-styles
cargo test -p zephium-core -p zephium-app -p zephium-engine -p zephium-store --lib
cargo test -p zephium-desktop --lib
cargo xtask check-blocker-seed
```

From `desktop`, build the isolated QA identity with:

```powershell
$env:CARGO_PROFILE_DEV_OPT_LEVEL = "2"
pnpm exec tauri build --debug --features adblock-qa --config tauri.protection-qa.conf.json
```

It must identify as **Zephium Protection QA**, `app.zephium.protection-qa`.
The QA feature requires an optimized debug build and is refused in release builds; use the
normal release graph in a separately isolated measurement environment for
performance qualification. Do not disable that guard to manufacture a QA release.
Protection defaults to on for new profiles. Verify the existing-profile migration
also enables it; use Utilities or Privacy settings for the disabled control run.

Three existing Windows engine Clippy findings were observed during cross-check:
`download_files_windows.rs` redundant boolean conversion and test-module ordering,
and `downloads/platform_windows.rs` large error result. Verify on the exact SHA;
they do not originate in the adblock style changes. Do not suppress new warnings
under those existing failures.

## Required behavior

Use a local fixture plus representative daily-use pages. Keep raw results and
screenshots locally under ignored `target/` directories; commit only concise
results and reproducible qualification tooling. EasyList deliberately has generic-hide exceptions
for localhost and 127.0.0.1. Use IPv6 loopback or an explicitly controlled test
hostname when checking generic cosmetics. Verify selectors against the shipped
list. A script path such as `/ads/cbr.js` is present in the current network list.

1. Fresh enabled profile: first page browses while protection prepares; no native
   empty-list compilation, no blank-tab hold, no endless starting status. Restart
   with fresh lists: no HTTP fetch; warm compiled cache skips source parsing.
2. Verify a blocked request never reaches the fixture server, while useful
   resources do. Test pause/resume, redirects, failed navigation, history, popups,
   tab reuse and profile switching. Pause bypasses matcher work before URL copying.
3. Static hiding: generic/domain rules, exclusions, per-selector exceptions,
   `generichide`, dynamically added matching elements, CSP `script-src 'none'` /
   `style-src 'none'`, author important styles, and ordinary content remaining visible.
4. Frames: verify network filtering and main-document container hiding. Automatic
   child-frame cosmetic delivery has been removed. Verify that inactive tabs do
   not run generic discovery and that DOM mutation bursts stay bounded. Record
   first-paint flash and input/frame timing on realistic pages.
5. Hiding elements (Utilities menu → Hide elements): the page shows a tinted
   outline with the element's name and size and a hint bar; ↑/↓ widen or
   narrow the pick. One click hides the element at once and saves it (the
   sidebar bar counts hides; Undo/⌘Z removes the newest; Done/Esc ends).
   Page click handlers never run for picked clicks; several picks in a row
   all persist; no flash between the instant hide and the saved rule. Ending
   by navigation, reload, tab close or switching tabs stops the picker. Late
   results must not save into another document/profile. Check author inline
   important display, strict CSP (the picker uses a constructed sheet and no
   innerHTML), closed shadow/canvas/frame container picks, and restart
   persistence. "Hidden on this site" shows each hide again individually or all.
   The per-site switch pauses/resumes and reloads the tab.
6. Private windows: pause and hide work during the session, survive ordinary
   tab operations as intended, and leave no durable rules after session closure.
7. Updates: both official sources, conditional 304, unchanged bytes, offline,
   truncation, oversized/malformed data, shrinkage, rate limits, cancellation,
   corrupt current/previous/candidate caches, restart during activation. Keep
   the last working policy throughout. No fresh-startup fetch or idle poll loop.
8. Repeat updates and tab/frame churn, then close the app. Verify exact handler
   teardown, no retained frame cycles, no late policy replacement and clean
   bounded shutdown. Do not enable CDP, WebMessage or host objects as a shortcut.

## Performance evidence

Measure blocked and allowed requests separately. Capture p50/p95/p99/max for
**the complete WebResourceRequested callback**, including COM extraction and
blocked-response construction, separately from matcher-only time. Record
budget exhaustion, unavailable attribution and other fail-open counters.

Use 1, 10 and 30 resident tabs, plus paused views. Confirm one shared immutable
matcher, no per-tab regex work, and no multiplied worker-request interception.
Measure process-family peak/retained memory, CPU time, idle wakeups, cold/warm
startup, page completion, repeated updates and frame churn. Use matched builds,
fixtures and run counts for on/off comparison; retain medians and variance.
Battery claims require a controlled workload and power measurement.

Mac reference only: 10,000 synthetic rules / 100,000 requests, release mode,
source-independent matcher p50 750 ns, p95 875 ns, p99 1,000 ns, max 29,792 ns,
zero errors/budget exhaustion. Exact-attribution p99 was 2,458 ns. This is neither
the full EasyList workload nor Windows callback evidence.

Report unsupported network contexts honestly: document/subdocument navigation,
WebSockets, object/other, service workers and already-running shared-worker
requests are not currently intercepted. Unknown initiator attribution is
conservative. Do not infer full filter-list parity from an ad-test score.

Finish with exact SHA, commands/results, fixture and real-page observations,
resource tables, concrete remaining limitations and any follow-up commits. Keep
source changes focused and leave merge to the user's QA decision.

## Blocking statistics follow-up

The new-tab query is `blocker_stats(profile)`: `today`, `last7Days`, and seven
oldest-to-newest daily counts. The Windows adapter increments the owning profile
counter only after `WebResourceRequested` successfully installs a blocking
response. Verify real blocked requests, allow/failed-response paths, site pause,
profile isolation, private teardown, clear-history reset, and restart persistence.
Repeat the optimized Yahoo counting-on/off and ten-static-tab five-minute idle
comparison on Windows. macOS measurements and their limitations are recorded in
`docs/qa/blocker-stats-2026-09-30.json`; they are not Windows runtime proof.
