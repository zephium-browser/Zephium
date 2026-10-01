# Windows protection and resource qualification

Worktree: `windows-protection`, based on `cffa2fe2`. This work does not
change the shared product interface or merge into main. The original extension
checkout is untouched.

Implementation commit: `3d31174b76fcf29ccd0efa9866ce24bbe61500ad`.
This is a measured, focused improvement, **not completed Windows release
qualification**. Whole-browser release startup, foreground rendering, wakeups,
battery energy, and the remaining coverage gaps still need qualification.

## Implemented changes

- Each Windows policy registration constructs one empty, stream-free `403`
  response with `Cache-Control: no-store` before installing its callback. It
  reuses that immutable response for blocked requests. Failure to prepare the
  response fails native installation; replacement remains transactional. The
  successful `SetResponse` remains the sole blocking-counter increment point.
- The full callback can be measured in an opt-in native test, separately from
  matcher time. All clocks, preallocated samples and diagnostic fixtures are
  excluded from shipping builds. Paused, allowed, blocked and native-failure
  outcomes are separate. Matcher capacity diagnostics remain URL-free.
- Windows resource visibility considers all windows. A layout for one window
  must not mark another window's selected tab hidden or make it eligible for
  suspension. An uncertain reentrant stage borrow conservatively keeps a view
  awake. Normal hidden-tab memory hints and suspension eligibility remain.
- Dormant-tab admission selects the next ID without allocating and sorting all
  remaining candidates on every slot or synchronous failure.
- At the existing 2,048-selector cosmetic limit, generic discovery disconnects
  its observer, cancels queued discovery, and releases its now-unused lookup
  index and exceptions. Policy replacement creates a fresh discovery state.
  Previously, mutations kept scheduling work that could never add another rule.

## Machine and method

Intel Core i3-1115G4, two cores/four logical processors, Windows 11, approximately
12 GB RAM, Windows build `26200`, Balanced power scheme. WebView2
`154.0.4258.48`. Raw runner metadata
records the OS build and actual memory for each sampled run.

The native qualifier uses the production registration, matcher worker, shipped
EasyList/EasyPrivacy seed, and counter with a fresh disposable profile. It uses
no CDP, page-to-host WebMessage bridge, credentials or ordinary browser profile.
Its release test binary is an optimized **native adapter harness**, not the
complete browser: it does not qualify browser startup, chrome, restoration,
power draw, or all cosmetic behavior through the product UI. Test panic handling
also differs from the product's abort-on-panic release executable.
The host window is hidden and its WebView2 controllers are created visible;
these are resident adapter views. They do not measure foreground paint, input
latency, or the product's tab-suspension policy. Public runs wait three seconds
without a new navigation after the last
completion; that wait is excluded from `host_load_ms`. Later redirects remain
possible. Consent/challenge pages are identified separately and never treated
as successful ad-heavy-page qualification.

The fixture contains 100 useful scripts and 100 ad scripts. Server-side counts
and page execution counts prove delivery versus blocking. This fixture measures
interception, not realistic advertising CPU or network cost. A second fixture
contains only useful scripts. Fresh profiles do not guarantee cold OS/network
caches. Quantiles use nearest rank. Samples are bounded at 200,000; overflow is
reported. The reported full callback includes test-only matcher timing overhead.

## Response reuse comparison

Five trials per variant, alternating in paired order. All 500 ad requests in
each variant were blocked before reaching the fixture server, and all 500 useful
scripts executed. Counters matched successful blocks. No dropped timing samples.

| Metric, median across trials | Original callback | Response reuse |
| --- | ---: | ---: |
| Blocked callback p50 | 10.3 µs | 6.2 µs |
| Blocked callback p95 | 33.4 µs | 24.0 µs |
| Allowed callback p50 | 7.4 µs | 7.5 µs |
| Fixture completion range | 378–556 ms | 363–540 ms |

Blocked-callback median fell about 40%. Completion ranges overlap; these trials
do not establish a general page-load speedup. They do not measure battery life.
The earlier unoptimized qualifier's millisecond matcher timings are excluded
from performance comparisons.

Baseline executable SHA-256:
`5AC2C76B590007275831E885F56AA1103A056C8A1F98115F917D0BAB17E76A5A`.
Raw local logs: `target/protection-evidence/baseline-comparison.log` and
`target/protection-evidence/response-reuse-comparison.log`.
The corresponding complete result records are retained under
`docs/qa/windows-protection/*-comparison.json`. Response-reuse executable hash:
`B4123F59062B89AD73CDB392558AC90D3C9808D91178B8A2E0F75A26B6CFDFA4`.

## Public pages and clean control

Three trials per arm, alternating order, one resident native view, fresh
disposable profiles. Both arms compile and retain the same policy before view
construction to isolate interception and page behavior; this does not measure
the full memory/startup cost of enabling protection in the product. CPU below
is the sampled process-family lifecycle total, including seed preparation,
the three-second public-page settle wait, five post-load seconds, and teardown.
It is not callback CPU or load-only CPU. Memory is sampled peak private bytes,
not proportional set size. Values are medians; load ranges retain the observed
variance rather than implying statistical significance from three trials.

| Page | Load off / on, ms (range) | Peak private off / on, MiB | CPU off / on, s | Successful blocks with protection |
| --- | --- | ---: | ---: | --- |
| Clean local scripts | 414 (387–628) / 560 (419–596) | 171.4 / 174.0 | 2.48 / 2.41 | 0 / 0 / 0 |
| Yahoo home | 1,264 (1,076–1,380) / 1,026 (994–1,065) | 348.7 / 364.9 | 7.83 / 7.48 | 14 / 14 / 14 |
| Bloomberg Europe | 6,167 (5,887–6,488) / 6,124 (5,839–8,162) | 973.2 / 976.1 | 16.31 / 17.11 | 20 / 17 / 16 |
| Guardian article | 862 (813–1,017) / 791 (746–801) | 232.5 / 234.8 | 3.84 / 3.86 | 11 / 11 / 11 |
| GitHub rust-lang/rust | 1,909 (1,859–2,354) / 2,365 (1,754–2,438) | 417.8 / 451.0 | 6.72 / 6.88 | 3 / 3 / 3 |

**The release performance gate remains open.** The Yahoo and article load
results are encouraging, but memory/CPU reductions are not consistent. Neither
the clean fixture nor GitHub establishes an overhead-within-noise guarantee.
Callback time excludes WebView2's cross-process interception/dispatch delay;
microsecond callback improvements cannot be assumed to remove that cost.
This needs full-product release measurement and investigation before claiming
macOS parity or shipping approval.

Google returned HTTP 429 in the first two pairs. Only the third pair reached
actual search results: 1,146 / 1,109 ms, 224.8 / 224.6 MiB, 3.58 / 3.83 CPU s,
seven successful blocks. One pair is insufficient to qualify search results.
No challenge was bypassed.

The Yahoo counting-off control reached consent instead of the home page in
its first trial, so that trial is excluded from the counting comparison. The
two remaining off-counting runs used 7.62 / 7.50 CPU s and peaked at 350.5 /
353.2 MiB; the corresponding counted runs used 7.27 / 7.88 CPU s and 351.5 /
364.9 MiB. This small, variable sample establishes no detectable counting
benefit or penalty. Counter removal does not disable filtering.

The preliminary `final-*` public runs sampled the first completion before a
settle interval and are excluded. `settled-*` records use the corrected
qualifier from `1751e285`. Query/fragment identifiers are removed from the
shareable records; raw stdout remains local under `target/protection-evidence`.

Full-callback timings on the public runs below use medians of each trial's
quantiles, with the maximum across all three trials. Units are microseconds.
They include native extraction and successful response/counter installation,
and report allowed and blocked requests independently. Matcher-only quantiles
are retained separately in each result record. No measured public run reported
native fail-open extraction, failed responses, unavailable/unprepared matchers,
evaluation errors, or budget exhaustion; the intentional source-independent
coverage restrictions remain in force.

| Page/outcome | p50 | p95 | p99 | max |
| --- | ---: | ---: | ---: | ---: |
| Clean / allowed | 7.9 | 53.9 | 81.3 | 180.2 |
| Yahoo / allowed | 49.8 | 106.6 | 150.8 | 245.1 |
| Yahoo / blocked | 67.2 | 125.4 | 125.4 | 166.1 |
| Bloomberg / allowed | 33.6 | 104.0 | 154.8 | 1,197.7 |
| Bloomberg / blocked | 35.2 | 90.3 | 109.6 | 122.8 |
| Article / allowed | 45.2 | 107.3 | 162.9 | 380.0 |
| Article / blocked | 34.4 | 110.6 | 110.6 | 116.4 |
| GitHub / allowed | 19.8 | 63.5 | 105.5 | 394.9 |
| GitHub / blocked | 66.5 | 79.0 | 79.0 | 115.8 |

The controlled scaling runs are single trials, so they are correctness and
capacity evidence rather than statistically established resource improvements:

| Resident views | Protection | Requests blocked / allowed | Peak private, MiB | Sampled lifecycle CPU, s |
| ---: | --- | --- | ---: | ---: |
| 10 | Off | 0 / 2,000 | 491.9 | 9.78 |
| 10 | On | 1,000 / 1,000 | 488.9 | 10.00 |
| 10 | Paused | 0 / 2,000 | 521.0 | 11.53 |
| 30 | Off | 0 / 6,000 | 1,214.4 | 24.97 |
| 30 | On | 3,000 / 3,000 | 1,177.2 | 26.30 |
| 30 | Paused | 0 / 6,000 | 1,232.2 | 29.61 |

All views share one compiled immutable policy. The 10-view paused callback
reported p50 0.1 / p95 0.2 / p99 0.3 / max 1.4 microseconds, with zero matcher
invocations. The fixture's ad scripts deliberately do little work; it should
not be used to predict savings from real advertising payloads.

## Five-minute idle controls

Ten clean native views per control, one run each, no builds or other benchmark
cases running concurrently. The fixture thread exits before idle starts. The
native message wait uses the remaining deadline rather than periodic polling;
the runner excludes intervals crossing teardown. Differences in admitted
sample duration come from the approximately one-second sampling boundary.

| Control | Retained private median, MiB | Sampled idle CPU, s | Admitted idle interval, s |
| --- | ---: | ---: | ---: |
| Protection on, counting on | 429.6 | 1.25 | 299.60 |
| Protection on, counting off | 426.1 | 1.53 | 297.88 |
| Protection off | 423.5 | 1.03 | 299.11 |

All three had low idle CPU. One run per control does not establish a counting
penalty or battery improvement. The qualifier excludes the application shell's
statistics persistence/query path; automated shell tests and the QA interaction
checks cover that path separately. Wakeups and energy were not measured.

The 51 complete native cases, per-run callback/matcher quantiles, machine and
binary metadata, navigation records, and process samples are under
`docs/qa/windows-protection/runs/`; `summary.json` contains the resource table.
The corrected public/idle qualifier SHA-256 is
`F42F1B4E4D111AD0947359F679A580A027F1C03F266F227DFFAA596A2D76C334`.
The recorded dirty flag includes uncommitted report artifacts; the later
Windows-only dev-dependency restriction does not change the Windows graph.

To export completed runs without transient URL query identifiers:

```powershell
./scripts/qualification/summarize-windows-protection.ps1 `
  -EvidenceRoot target/protection-evidence -OutputDirectory <new-directory>
```

## Native behavior and coverage

The strict-CSP fixture uses `script-src 'none'; style-src 'none'` plus Trusted
Types enforcement. It verifies that page inline script is denied, constructed
personal sheets hide elements, inline important display is overridden, multiple
hides apply, clearing restores the original inline value, useful content stays
visible, stale document tokens are rejected, and picker start/stop succeeds.

The actual optimized-debug **Zephium Protection QA** application also passed:

- Three consecutive real pointer clicks on the strict-CSP page hid three
  separate headings; the sidebar reported three saved hides.
- Undo restored only the newest heading. Done ended the picker while retaining
  the other two. A clean exit and restart retained those two hides, with the
  useful heading, body text, and undone heading visible.
- The site switch reloaded `/network`. The fixture server received
  `/ads/cbr.js` only during the paused navigation. Resuming blocked it again.
- The new-tab counter displayed exactly two blocks after the enabled and
  resumed navigations; the paused load did not increment it.
- A second clean restart restored the network page and blocked its ad again.
  The new-tab total was three: the prior two persisted plus that new block.

Screenshots and the reproducible loopback server are retained in
`docs/qa/windows-protection/` and `scripts/qualification/protection-ui-fixture.cjs`.
No flash was apparent in these observations, but frame-by-frame flash timing
was not measured. Private-session persistence/isolation is covered by automated
shell/store tests, not a completed native private-window interaction run.

The separate coverage observer owns one disposable environment and does not
install a blocking policy. Broad filters affect every handler on a view, so
coverage observation and production filtering are intentionally separate runs.
Observed native metadata on this runtime:

| Request | Context | Source kind | Available `Sec-Fetch-Dest` |
| --- | --- | --- | --- |
| Main document | Document | Document | absent |
| Child document | Document | Document | absent |
| Dedicated worker entry script | Other | Document | absent |
| Shared worker entry script | Other | Document | absent |
| Service worker entry script | Other | ServiceWorker | absent |
| Dedicated worker fetch | XHR | Document | absent |
| Shared worker fetch | XHR | SharedWorker | absent |
| Service worker fetch | XHR | ServiceWorker | absent |

The WebSocket attempt produced no `WebResourceRequested` event even with the
broad observer. A declared WebSocket enum is not runtime interception proof.
Do not misclassify Document or Other as an exact script/subdocument request.
There is no trustworthy initiating-frame URL for source-domain or party rules.

Shared/service-worker requests need one environment owner with policy lifetime,
profile statistics, and pause semantics independent of any particular tab.
Registering them on every view duplicates synchronous work; Microsoft explicitly
[documents this constraint](https://learn.microsoft.com/en-us/dotnet/api/microsoft.web.webview2.core.corewebview2.addwebresourcerequestedfilter?view=webview2-dotnet-1.0.3800.47).
Those coverage gaps remain unclosed; this change does not claim Brave/uBlock
filter-list parity.

## Reproduction

Build the qualifier with `cargo test --release -p zephium-engine --lib --no-run`.
Use the exact emitted `zephium_engine-*.exe` path (record its hash), then:

```powershell
./scripts/qualification/windows-protection.ps1 -Binary <test-executable> `
  -Site fixture -Mode on -Tabs 10 -IdleSeconds 300 `
  -OutputDirectory target/protection-evidence/<new-run-name>
```

Sites: `fixture`, `clean`, `csp`, `coverage`, `yahoo`, `bloomberg`, `news`,
`github`, `google`. Modes: `off`, `on`, `paused`. Tab counts: 1, 10, 30.
Coverage runs require one tab and are observations only. News uses the Guardian
article [Google defeats US justice department bid to force ad tech sale](https://www.theguardian.com/technology/2026/sep/02/google-defeats-justice-department-bid-ad-tech-sale).
`-Counting off` removes the counter from the same protection-on callback for
the statistics control run.

The runner samples descendants using PID plus creation-time validation and
preserves fractional CPU seconds. Private bytes are summed; working sets can
double-count shared pages. CPU is a lower bound when a process exits between
samples. Idle intervals start only after the fixture thread exits and the
qualifier stops periodic load polling. CPU does not measure wakeups or energy.
Do not run builds or unrelated workloads during resource comparisons.

The application uses the existing `tauri.protection-qa.conf.json` and `adblock-qa`
feature, with an optimized debug build. Its debug-only guard remains intact.
Full-browser release performance still requires the separately isolated normal
release graph described in [the qualification guide](adblock-windows-qualification.md).

The rebuilt local executable is `target/protection-qa/Zephium Protection QA.exe`,
SHA-256 `7EB4B2374F63ABDA1CCBB1E9B8A9349DCEAA49AF36C2FD1FEA50CD5CAC29E86F`.
The build used `--debug --no-bundle --features adblock-qa` with
`CARGO_PROFILE_DEV_OPT_LEVEL=2`; this is an executable QA rebuild, not an
installer qualification. The existing global shortcut was already registered
on this machine; that warning did not prevent browsing or the interaction tests.

## Validation

- `pnpm -C frame check`: type, lint, styles, formatting, dependency checks and
  298 unit tests passed. `pnpm -C frame build` passed.
- ProtectionControls: two component tests passed. SelectiveStyles: four passed,
  including the new saturation/no-more-idle-work and policy-replacement test.
- Native Rust suites: core 215, app 335, engine 210, desktop 111 passed.
  The storage suite exposed a pre-existing Unix-only absolute-path fixture;
  using the platform temporary directory fixed it. The final combined rerun
  passed all 208 storage tests: **1,079 Rust tests passed**, three explicitly
  ignored tests across the five crates, no failures.
- `cargo clippy -p zephium-engine --all-targets -- -D warnings` passed; existing
  warnings in vendored Wry were emitted by the dependency, not suppressed here.
- `cargo xtask check-frame-styles` passed.
- Blocker seed `202609300903`: 3,568,061 source bytes verified offline, Runtime
  and WebKit goldens exact. On Windows the running `xtask.exe` locks the output
  that its child compiler rebuilds. The unchanged driver was copied to
  `target/protection-evidence/seed-check-driver.exe` and invoked with
  `check-blocker-seed` to run the same gate successfully.
- Native CSP, broad coverage observation, and 1/10/30-view policy tests passed.
  At 30 views: 3,000 blocked plus 3,000 allowed requests, no dropped samples,
  response failures, matcher errors, or candidate-budget exhaustion. Paused
  views made zero matcher decisions.

## Remaining release gates

1. Run the normal, isolated **full browser release graph** in the foreground:
   cold/warm first usable paint, session restoration, page completion, main-frame
   cosmetics, and 1/10/30-tab resource behavior. The adapter harness above is
   not a substitute. Investigate native request dispatch and the mixed clean-
   page results before calling the clean-page budget satisfied.
2. Qualify native private sessions, profile switching, clear-history
   counters, rapid input and frame timings, popup/navigation churn, and update
   failure/recovery through the finished UI. Existing automated tests cover
   these state machines, but not every physical Windows integration case.
3. Design and qualify an environment-owned worker policy before admitting
   shared/service-worker sources. Keep native document/Other ambiguity and
   absent WebSocket interception explicit until a trustworthy alternative is
   implemented and measured.
4. Measure scheduler wakeups with ETW and energy with a controlled power
   workload. CPU seconds are not either measurement. Validate additional
   Windows runtimes/hardware and the installer separately.

No default-on shipping decision or merge is made by this branch.

## Windows efficiency follow-up (2026-10-01)

This pass audits the complete Windows product's startup, residency, background
maintenance and fixed page scripts. It retains the shared frontend and the
existing discard grace periods and safety vetoes.

Changes:

- Windows keeps the already-hardened utility panel on `about:blank` until the
  first actual panel request. The latest Rust-owned route and profile survive
  loading, and the existing `panel_ready` barrier still precedes native reveal.
  The native panel/environment is still created at startup; this defers its
  application document, not its entire process group.
- The discard observer holds weak references to shadow roots instead of keeping
  detached component trees alive. Collected slots are compacted only when
  admission needs capacity. Live closed shadow roots remain inspected; missing
  APIs, overflow, dirty forms, media/capture and uncertain state veto discard.
- Discard probes traverse at most 4,097 elements to accept a maximum 4,096-element
  snapshot. Oversized pages are refused immediately. They no longer allocate a
  whole-document `querySelectorAll('*')` result or repeatedly traverse each
  shadow tree for every fixed safety selector.
- Bare global `addEventListener('beforeunload', ...)` and corresponding removal
  are tracked correctly. The previous strict wrapper missed an undefined
  receiver even though the browser accepted the registration on Window.
- Cosmetic discovery coalesces queued descendants covered by a queued subtree.
  Mutations arriving during a partial walk remain independently queued, and
  the per-node token-matching closure is eliminated. Existing 2 ms/200-element
  slices, hidden-document pause and 2,048-selector cap remain in place.
- Windows suspension completion checks the native view generation. Closing and
  recreating an item cannot let the previous callback settle its replacement.
  Revealing a view retains its in-flight admission slot until completion, so
  quick show/hide transitions cannot issue overlapping suspend requests.
- One diagnostic records Rust application entry to initialized main-document
  reveal. It is not an OS cold-launch, first content paint or page-ready metric.
- `tauri.performance.windows.conf.json` provides an isolated identity for the
  normal release graph, with no QA features or runtime automation bridge.
  Build validation now applies Linux override identity rules only on a Linux
  build; repository Linux identity validation and QA release exclusions remain.

Audit observations: restored tabs are already lazy; resident views already use
soft/pressure/absolute limits of 12/24/32, dormant admission has a five-minute
idle threshold, and exact safety probes precede discard. The app maintenance
interval is 60 seconds. The style worker blocks on its bounded queue, the
blocker reuses application maintenance instead of adding an idle timer, and
Windows download progress polling stops once active work drains. This pass does
not shorten grace periods or change those policies without workload evidence.

Reproduction (from repository root, with the pinned Node/pnpm tools and installed
Playwright browser path available):

```powershell
pnpm -C desktop exec tauri build --no-bundle --config tauri.performance.windows.conf.json
node scripts/qualification/windows-dom-performance.cjs de3cb3c3 target/dom-before.json
$env:ZEPHIUM_EXPECT_OPTIMIZED = '1'
node scripts/qualification/windows-dom-performance.cjs working-tree target/dom-after.json
./scripts/qualification/windows-browser-resources.ps1 -RootProcessId <isolated-app-pid> -Seconds 60 -OutputDirectory target/unique-run
```

The renderer fixture uses an independent Playwright Chromium, not the product.
It checks actual production script bytes, detached-root collection, live closed
shadow forms, unbound and removed unload handlers, large-page refusal, nested
mutation coalescing and changes during partial scans. Explicit GC is confined
to this test browser; collection timing is never product policy. Native WebView2
also passed the discard checks and existing hiding/picker checks on the strict
CSP fixture, with no page-to-host bridge. Fixture step counts are deterministic
work evidence, not whole-browser speedup percentages.

### Measured results and release limits

Evidence is retained in [qa/windows-efficiency](qa/windows-efficiency/).
The baseline production sources are de3cb3c3; the final script hashes are in the
fixture JSON. Measurements ran on the i3-1115G4 (2 cores / 4 logical processors),
about 12 GB RAM, Windows 11 build 26200, Balanced power plan. No compiler was
running during the final samples.

| Targeted renderer fixture | Before | After |
| --- | ---: | ---: |
| Detached closed shadow roots retained after explicit test GC (20 removed) | 20 | 0 |
| 100,000-element safety probe, median of 15 calls | 16.6 ms | 0.4 ms |
| Whole-document selector results allocated across those calls | 1,500,060 | 0 |
| Bounded TreeWalker steps across those calls | 0 | 61,455 |
| Nested cosmetic mutation traversal steps | 12,352 | 160 |
| Bare beforeunload listener correctly vetoes discard | No | Yes |

These use standalone Chromium 153.0.8010.12 and exact production scripts, not
WebView2 app timings. Both versions refuse oversized documents. Live closed
shadow forms still veto discard after GC. All 11 behavior cases and hiding
updates during partial walks passed. Timing samples are one fixture run;
operation counts and retention are the stronger evidence.

| Full release process family, minimized new tab | Before | After |
| --- | ---: | ---: |
| Median private committed memory | 367.22 MiB | 351.64 MiB |
| Peak private committed memory | 391.78 MiB | 375.30 MiB |
| Sampled CPU time | 0.3125 s | 0.6875 s |
| Measured interval | 58.79 s | 59.25 s |
| Process count at end | 13 | 13 |

One matched pair used the same isolated profile, default sidebar, unused panel,
no network page, and a minimized window. The approximately 15.58 MiB reduction
is an observation, not a statistical bound or a general RAM claim. CPU did not
improve in this sample. There is no energy/wakeup result. Earlier unmatched
foreground/occluded runs and the initial sampler run with a process timestamp
precision bug are excluded. The sampler now compares process creation timestamps
at CIM's microsecond precision, retaining the PID-reuse check.

Normal release artifacts (no QA features):

- Before: target/windows-performance-baseline/Zephium Performance.exe,
  SHA256 1153B74DBBE003EF5D3F24A9CAEB5CDE1EA6E0E93CAF76A6EAF2E2BCFC934AAA.
  Includes only the build-validation change needed to give Windows its isolated
  measurement identity; runtime sources predate this efficiency pass.
- After: target/windows-performance-after/Zephium Performance.exe,
  SHA256 EEBF3861A7D1FF8D4633C5A013223BD254F923E20517B9F70E90D92460CFA33A.
- Rust-entry to initialized main-window show was 2,556 ms and 2,276 ms on two
  observed launches. No comparable baseline marker exists. This does not establish
  cold-start improvement, instant startup, or time to first paint.

Validation: 112 desktop and 211 engine unit tests passed (two ignored native
qualifications); the strict-CSP native WebView2 qualification passed separately
on runtime 154.0.4258.48, including the new discard checks and existing hiding /
picker checks. Four SelectiveStyles component tests passed. Engine all-target
Clippy with warnings denied passed apart from the existing dependency warnings.
The normal release build succeeded. Native UI checks covered first panel use,
reopening, Escape dismissal and the docked Notes surface. No shared frontend
product source was changed.

An initial baseline shutdown while test compilation was active failed the
existing two-second privileged-environment exit proof and correctly quarantined
its temporary generation. The next baseline shutdown and both updated-release
shutdowns (panel used and never used) completed without this warning. This is
not a demonstrated fix for shutdown under contention; stress qualification is
still needed. No exit-proof or quarantine safeguard was weakened.

Release sign-off remains separate: repeated foreground and ten-tab mixed-site
measurements, restored-session cold/warm startup, ETW wakeups/energy, low-memory
pressure, and suspend/resume stress still need qualification. The previously
reported network-coverage gaps and mixed public-site protection results remain;
this pass does not establish the original all-sites CPU/RAM/page-load targets.

The optimized-debug QA app was rebuilt with the adblock-qa feature and isolated
app.zephium.protection-qa identity. Artifact:
target/protection-qa/Zephium Protection QA.exe, SHA256
CFFACDEB4999A54DD8C8E592424A89A75D8AEBF7AB492AD4C99807C4C51C332F.
It is a separate QA build, not a distributable release artifact.
