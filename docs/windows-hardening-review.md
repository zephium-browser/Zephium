# Windows hardening review — October 7, 2026

Branch: `windows/hardening`, based on `origin/main` at `e25c8a86`, including
the public-release trust/privacy merge `3259a03e` (#54). This is an engineering
review in progress, not a production-readiness certification. Everyday browsing,
visual acceptance, video fullscreen, and real workload resource measurements
remain with the owner. No personal browser profile is used by these checks.

Review commits:

| Commit | Change |
| --- | --- |
| `5b61919d` | Preserve runtime ownership across independent data roots. |
| `95f90547` | Bound privileged renderer recovery and Windows initialization; verify native visibility. |
| `3ec48edb` | Validate settled fullscreen exit and use the fallback on script failure. |
| `345a8133` | Bound shared media imports while reading a single file handle. |
| `8c7b12cb` | Correct Windows test fixtures and diagnose native extension delivery stalls. |
| `58c7bba9` | Resolve four development dependency advisories with published fixes. |

## First changes

### October 10: reported download, icon and video failures

Local changes on `windows/hardening`, awaiting owner visual acceptance:

- Reproduced `FILE_FAILED` with a loopback HTTP attachment and the production
  private staging directory. WebView2 accepted the verbatim destination spelling
  but failed the transfer. Verified ordinary Win32 spelling completed the same
  transfer, publication and Internet-zone/file-identity checks. Canonical ancestor
  leases and staging ACLs remain authoritative. Windows now honors automatic save
  to Downloads and the optional ask-first setting; a custom folder still requires
  its native picker identity. Multiple automatic downloads remain denied.
- Fixed favicon delivery ownership when switching profiles. Returning to a profile
  resends unchanged rasters; a late background-profile icon cannot replace the
  foreground webview's cache. Empty profiles explicitly reset the surface owner.
  All 18 favicon tests pass, including both regressions.
- Native generated-video qualification opened and closed video PiP successfully.
  The installed runtime exposes its menu command as `pictureInPciture`; the host
  filter had discarded it. Preserve that command and the observed native media
  controls, retaining foreground/document admission. Document PiP is unavailable
  in this runtime; no emulation or media URL extraction was introduced.
- Element fullscreen now conceals intermediate native geometry with a black owned
  popup, then fades out after geometry settles. Windows animation preferences are
  respected; the cover allocates only the current client area and releases its
  HWND/timer within 600 ms. Native tests cover expiry and replacement. Visual
  smoothness, mixed-DPI monitors and real-site acceptance remain owner checks.

Validation: 446 application tests and 264 engine tests passed (3 opt-in engine
tests ignored by the ordinary suite). The native download/PiP qualifier was run
separately and passed, including dispatch of the actual native PiP menu command
and profile cleanup after browser-process exit. Strict Clippy passed for both
crates and all their targets. Frontend check passed (736 tests in 131 files), and
the frontend build passed its bundle budgets. Existing build warnings about motion
initialization and CSS highlight selectors remain outside these fixes.
These are debug correctness checks, not release performance measurements.

The Review QA identity is also admitted to the closed Windows Work-storage identity
list so the requested isolated app can start. The separate namespace-validation
fixture still fails with `Unsafe` while preparing its original Product fixture,
before reaching the added Review QA assertion; no security predicate was relaxed.

Owner checks for the rebuilt candidate: start fresh downloads from the original
sites; toggle ask-first and choose a folder; verify filename collisions and Open/
Reveal; enter/exit YouTube fullscreen from normal and maximized windows; open PiP
from the native video menu; switch private/regular profiles and revisit favicons.

### October 7: initial hardening

- Runtime-generation maintenance no longer deletes shared volatile registry
  markers just because their generation is absent from the current data root.
  QA, production, and other app identities share the registry namespace. The
  previous prune could erase another identity's same-boot proof and let a later
  launch reclaim its runtime files without process-exit proof. Exact cleanup
  tickets still remove their own markers; otherwise they expire on reboot.
  A regression covers two independent roots and a simulated parent restart for
  both private and privileged runtime kinds. Filesystem formats are unchanged.
- Privileged Windows chrome previously had no `ProcessFailed` observer. The main
  document and launcher now each have two recovery attempts per application run,
  with a 60-second readiness deadline. Recovery reloads the fixed trusted
  document, keeps its existing native security handlers, and waits for readiness.
  Browser-process exit, exhausted attempts, or failed recovery produces a native
  restart explanation and uses orderly terminal shutdown. GPU/utility failures
  and mere unresponsiveness do not trigger destructive reloads. A renderer crash
  can still lose edits that had not reached durable storage.
- Windows startup and onboarding handover allow 60 seconds instead of 15.
  Other platforms keep their existing timeout. This changes failure tolerance,
  not measured startup speed.
- Windows chrome visibility now reports the actual native result. Previously a
  successfully dispatched callback could return success despite a failed
  `SetIsVisible`, leaving onboarding with hidden controls.
- Fullscreen exit now waits for the JavaScript promise and validates the CDP
  result. An HRESULT success carrying `exceptionDetails`, a rejected promise,
  or a false result triggers the existing script fallback. Invalid execution
  context IDs are rejected. Video and multi-monitor acceptance remain separate.
- Local media import opens the selected file once and bounds the read itself.
  A file that grows after the metadata check cannot cause an unlimited read.
  This shared fix applies to Windows and macOS.
- Work fault fixtures wait for asynchronous worker reaping before releasing
  their test serialization lock. Production runtime exclusivity and failure
  outcomes are preserved. Command-policy tests now exercise POSIX and Windows
  explicitly instead of expecting the POSIX allowlist on Windows.
- The native strict-CSP fixture now matches the current discard policy: it
  excludes transient activity/edit grace bits from its durable-state assertions,
  uses native text input to protect an edited closed-shadow form, and exercises
  the current 50,000-element scan limit. No production discard rule was weakened.

Recovery follows [Microsoft's process-failure guidance](https://learn.microsoft.com/en-us/microsoft-edge/webview2/concepts/process-related-events):
a renderer can be navigated/reloaded, while an exited browser process leaves its
controllers closed. The fullscreen result check follows the
[Runtime.evaluate contract](https://chromedevtools.github.io/devtools-protocol/tot/Runtime/#method-evaluate).

## Initial platform inventory

These are code findings, not claims that every native mechanism or feature has
passed live testing. The remaining work column is deliberately explicit.

| Area | Windows implementation / macOS comparison | Remaining work |
| --- | --- | --- |
| Native chrome | Custom native caption, `HTMAXBUTTON` Snap hit testing, DPI-change handling, and high-contrast menu fallback exist. | Physical DPI, Snap, focus/IME, Narrator and touch acceptance; broader shortcut trace. |
| Lifecycle | Content process-failure handling exists. Hidden tabs receive low-memory hints; sleeping tabs use `TrySuspend`/`Resume`. Native low/high-memory notifications drive the shared policy. Privileged recovery passed two real renderer crashes. | Longer sleep/wake and discarded-tab stress; launcher recovery acceptance. |
| Certificates / runtime | Content certificate errors explicitly cancel. Actual WebView2 version, private mode and UDF identity are checked. Shipping arguments retain SmartScreen. Wry enables custom crash reporting to suppress automatic uploads. | Native failure-path checks and final security review. The owner-controlled vendor release blocker is unchanged. |
| Permissions | Windows camera/microphone use WebView2's prompt; macOS has a browser-owned deferred broker. Other page permissions and client certificates are denied; HTTP sign-in on human tabs uses Zephium's own card, and is denied elsewhere. Privileged UI denies ambient capabilities. | Permission UX parity and supported capability decisions; no silent granting of currently denied capabilities. |
| IPC / navigation | Chrome has a navigation lock and caller-scoped commands; content uses separate native views. Privileged subframes and external URI launches have native guards. External-app requests pass through browser approval before `ShellExecuteW`. Windows userscripts remain disabled pending CDP isolation proof. | Complete origin/source and navigation-race review across all bridges; userscript support is a deliberate parity gap. |
| Downloads | Windows uses private staging, handle/identity checks, Attachment Services, and verified Internet-zone `Zone.Identifier` metadata. | Real interrupted/resumed downloads, scanner rejection, filesystem and file-lock edge cases. |
| Private data | Runtime generations and browser-process exit ownership govern UDF cleanup; deleting live locked WebView2 data is avoided. | Crash/relaunch residue checks and broader cookie-isolation qualification. |
| Storage | Root composition still uses `app_data_dir` (Roaming on Windows). Native paths verify canonical identity before projecting Win32/short spelling. | Local AppData migration design; redirected/long-path and SQLite locking stress. |
| Ad blocking | Document-source HTTP(S) filters cover selected resource contexts; reusable bounded request buffers and one blocked response reduce per-request allocation. | End-to-end release A/B numbers. These optimizations do not establish zero overhead on clean pages. |
| Fullscreen / painting | Windows has native fullscreen observation, isolated-world exit, stage placement and paint covers; macOS uses WKWebView-specific transitions. | Owner video/F11/Snap/monitor checks; native fallback result fix covered separately. |
| Updates / distribution | Per-user NSIS, signature re-verification, retained interrupted update, and install after a clean requested relaunch exist. Release workflow stages the NSIS installer/signature, separately retains PDBs, verifies updater signatures, and emits both Windows updater target keys. Automatic install-on-quit is macOS-only. | Interrupted installer/rollback qualification; Windows install-on-quit decision. Signing remains owner-provided SignPath work. |
| Default browser / links | Windows registers per-user HTTP/HTTPS capabilities on request and opens Default Apps; startup link handoffs are bounded and queued until the shell exists. Uninstall removes those registration keys. | `.html`/`.pdf` file associations are absent from this registration; early-startup handoff stress. |
| Logging / OS services | Windows stderr has bounded rotation and retention; Show Logs opens Explorer. `media.rs` manages imported files, not media-key integration. Focus timer OS notifications are macOS-only; the non-macOS branch silently returns. | Log-content privacy review (rotation does not redact URLs/paths), media controls and native notification integration. Page notifications currently remain denied. |
| Work | Shared policy includes approval for typing on sites the person did not name; Windows has native input, screenshot, network and rendering adapters. Frame capture bounds native jobs to eight, image workers to two, and PNG/dimension/output budgets; image decoding is off the STA. | End-to-end Windows enforcement and capture-cost verification beyond shared unit tests. |
| Extensions | Current main includes native action-host crash recovery and popup/worker lifecycle handling, superseding several old handoff findings. All six old cases were traced; one complete native run covered recovery, restored-page closing, bounded worker snapshots, cap/removal, popup cleanup and process restart. | Repeated native runs exposed intermittent missing extension notifications after the forced host crash; see reproducible open issue below. Real-extension compatibility remains separate. |

## Extension follow-up still open

`host::webext_windows::qualification::native_windows_runtime_recovery` is not a
stable passing gate. Three of four observed runs timed out on the injected
action-change notification after host-renderer recovery. The complete passing
run is in `target/windows-extension-native.log`; the final failure, with added
diagnostics, is in `target/windows-extension-native-repeat.log`.

The failed run reaches 20 tab switches with two bounded worker snapshots, then
the extension page's `chrome.runtime.sendMessage` remains pending and the host
observes neither the notification nor a new action snapshot. That is evidence
of a delivery failure in this native fixture; it does not yet establish whether
the cause is runtime recovery, suspended-view behavior, or fixture sequencing.
The added diagnostics preserve the failure instead of silently swallowing it.
Do not treat the single passing run as closure of this issue.

Reproduce on Windows using the workspace build so feature selection matches:

```powershell
$env:CARGO_PROFILE_DEV_DEBUG='0'
$env:CARGO_PROFILE_TEST_DEBUG='0'
$env:CARGO_INCREMENTAL='0'
cargo test --workspace --locked -j 2 --no-run
# Use the zephium_engine test executable printed above; run alone:
& <engine-test-executable> native_windows_runtime_recovery --ignored --nocapture --test-threads=1
```

The test creates its own temporary profile and packages; it does not need an
installed extension or the owner's browsing profile. Its passing branches also
verify sleeping-tab style refresh, popup crash closure, one failed native popup
close, disabled-extension removal at capacity, and restoration in a fresh
process. Stable queue order and retry on a freed slot are implemented in
`crates/zephium-app/src/shell/webext.rs`; native admission/removal ownership is in
`crates/zephium-engine/src/host/webext_windows.rs`.

## Validation record

The approved npm dependency audit found four moderate development-tool
advisories, resolved by pinning `fast-uri` 3.1.8, `brace-expansion` 5.0.12,
`smol-toml` 1.9.0 and `postcss-selector-parser` 7.1.6. The repeated registry
audit reports zero moderate/critical findings and one high finding for
`braces` 3.0.3. That package has no published upstream fix; its existing local
depth-bound patch remains in place. The repository's exact-byte verification
and all four audit regression tests pass, including hostile patterns and ASTs.
This is the existing narrowly verified development-tool exception, not a clean
raw audit or an additional waiver. The original local installation lacked the
patched bytes despite its patch identifier; a forced reinstall repaired it
without changing the verifier or its expected hashes. Rust advisory scanning
was not performed (`cargo-audit` and `cargo-deny` are not installed).

Advisories: [fast-uri](https://github.com/advisories/GHSA-hrr3-gc8f-f4qj),
[brace-expansion](https://github.com/advisories/GHSA-q2hr-2g5m-vwhr),
[smol-toml](https://github.com/advisories/GHSA-r4xh-jqrq-34v2),
[selector parser](https://github.com/advisories/GHSA-rj75-hqrm-r3gf), and
[braces](https://github.com/advisories/GHSA-vfj7-8cjw-p6xm).

Machine: Windows 11 Pro, build 26200; Intel i3-1115G4, two cores/four logical
processors; 12,377,400 KiB visible RAM. Installed WebView2 folders include
154.0.4258.53 and 154.0.4258.62. The isolated chrome recovery test reports the
actually selected runtime as 154.0.4258.62.

Toolchain: Rust 1.95.0; repository-local Node 24.18.0 and pnpm 11.17.0.
Use one `target` directory. Tests use `CARGO_PROFILE_DEV_DEBUG=0`,
`CARGO_PROFILE_TEST_DEBUG=0`, `CARGO_INCREMENTAL=0`, and `-j 2` to limit disk/RAM.
These are debug correctness checks, not release performance measurements.

- Frontend `pnpm -C frame run check`: passed; 736 tests in 131 files.
- Frontend `pnpm -C frame run build`: passed, including bundle budgets. Existing
  build warnings concern initial Svelte motion state, CSS highlight parsing, and
  the onboarding wordmark reference.
- Frontend check/build were repeated successfully after the dependency fixes;
  logs are `target/windows-frame-check-dependencies.log` and
  `target/windows-frame-build-dependencies.log`. The approved raw registry
  audit reports are `target/windows-dependency-audit.json` and
  `target/windows-dependency-audit-after.json`.
- Isolated native chrome recovery: passed, including two `Page.crash` events,
  navigation/readiness recovery and exhausted-budget refusal. Log:
  `target/windows-renderer-native.log`.
- Rust workspace: 3,559 passed, 12 opt-in tests ignored; no failures. These
  include the eight Windows runtime-generation ownership/cleanup tests.
- Strict workspace clippy: passed with `--all-targets -- -D warnings`.
- Formatting and diff whitespace: passed.
- Native network protection: passed (100 useful script requests reached the
  fixture, 100 ad requests were blocked, zero matcher/fail-open errors).
- Native strict-CSP protection: passed, including edited closed-shadow forms,
  unload handlers, bounded scans, cosmetic styles, undo and picker lifecycle.
- Extension action host/observer JavaScript tests: 11 passed. Native extension
  qualification remains intermittent as described above.
- Logs: `target/windows-hardening-tests.log`,
  `target/windows-hardening-clippy.log`, `target/windows-runtime-isolation.log`,
  `target/windows-protection-network.log`, `target/windows-protection-csp.log`,
  `target/windows-extension-js.log`.
- No new startup, RAM, CPU, GPU, battery, or ad-blocker speed numbers are claimed.

The native chrome test is opt-in under
`platform::windows::renderer::tests::privileged_renderer_recovers_twice_through_native_process_failed`.
The engine's `native_protection_qualification` test uses
`ZEPHIUM_PROTECTION_SITE=fixture` or `csp`, `ZEPHIUM_PROTECTION_MODE=on`,
and `--ignored --nocapture --test-threads=1`. All reported native checks here
use debug binaries for correctness. Their incidental timing output must not
be presented as release performance evidence.

## October 18 download handoff follow-up

The owner confirmed ordinary download completion, fullscreen and manual video
PiP, then reported Open/Show in Folder failures and fullscreen reveals before
slow page layouts settled. The two completed QA download receipts still name
existing files, but neither file retains a `Zone.Identifier` stream. This
explains the protection error; the cause of the later stream removal is not
established.

Show in Folder now verifies the recorded file identity without requiring
execution protection. Open still requires protection: for an unchanged original
it reruns Attachment Services when protection is missing, restores the receipt's
last-write timestamp after the ADS-only update, and verifies protection again.
Modified or substituted files remain refused. Explorer and shell verbs receive
the strictly validated ordinary spelling of the canonical local path.

The Windows download suite passes 22 tests (one native opt-in test excluded),
including missing-stream recovery, rejection of edited payloads, and shell PIDL
parsing of Unicode names with spaces. No downloaded executable was run during
qualification. The native loopback download also retains its provenance after
the original WebView2 process exits; that fixture did not reproduce later stream
removal.

The earlier 600 ms fullscreen cover is superseded by readiness sampling. The
cover now waits for native window geometry, a matching page viewport/fullscreen
state, and 200 ms of stable video rectangles, then fades over 180 ms (or reveals
without animation for reduced motion). Enter requires the page to fill the
window and the window to cover its monitor; exit requires restoration of the
saved window rectangle. A four-second failure deadline prevents a hung renderer
from leaving the window black. Competing page slides are suppressed while the
cover is active. Engine tests pass 267 cases with three opt-in tests excluded.
Automatic PiP remains a separate implementation on the integrated branch.

## October 18 integrated branch

The Windows work was committed and pushed independently at `f937e2db` before
merging into `feat/work-production` (`ce746743`). Merge `f1328d0a` preserves the
integrated preview cleanup and clipping alongside fullscreen slide suppression,
and retains both profile handling and Windows chrome-renderer recovery. The
split drop bar and recorded bundle budgets were not replaced or re-recorded.

Automatic video PiP now follows ordinary tab selection through a native engine
port. Windows evaluates the fixed presentation program in a dedicated CDP world;
macOS uses a named public WKContentWorld. Neither exposes a page-callable chrome
command or copies media into another browser window. Only a substantial playing
video with sound is eligible, and site PiP opt-outs remain effective. Returning
closes only presentation opened by this feature. Manual dismissal is respected,
rapid switches share pending native entry, visible split panes are excluded,
and profile/private-context changes cancel automatic presentation. The switch
is in Settings > Tabs; disabling it closes existing automatic presentation.
Existing discard protection now treats native PiP as active media even when
paused or muted.

The Windows native fixture opens PiP after hiding its source WebView2 and closes
it on return. JavaScript regression cases also exercise pending-entry races,
manual ownership, dismissal, opt-out, rejection and WebKit presentation dispatch.
This does not qualify native macOS behavior. The bounded video scan includes
the main document and accessible same-origin frames; cross-origin embeds and
sites that disable native PiP are not claimed as supported.

The integration exposed Chromium model-menu focus loss. Focus is now assigned
in the popover's opening lifecycle rather than an earlier render tick, and
closing from the trigger preserves focus restored to the composer. Existing
interaction assertions are retained. Windows Work qualification also needed
macOS-only profile seeding separated from shared loopback requests. Windows
refuses that optional seeding mode explicitly; it does not fake cookie-store
inspection results.

Correctness logs for this integration live under `target/review-qa/integrated-*`,
with native PiP evidence in `automatic-pip-native.log`. These are debug-build
checks, not release performance measurements. Final Windows appearance and
native macOS PiP still require owner acceptance.

### Integrated Windows verification and blockers

This integration is not release-qualified. The machine is Windows 11 Pro
10.0.26200 (build 26200), Intel Core i3-1115G4, four logical processors,
approximately 12 GiB installed RAM. WebView2 155.0.4283.45 and 155.0.4283.62
are installed; installation inventory alone does not identify which runtime
each completed fixture used. The final running Review QA process uses
155.0.4283.62, verified from its actual child executable. All native evidence
below uses debug builds.

Verified changes are `50e6a8c1` (automatic PiP), `5e727e94` (model-picker
keyboard focus), `9e4cc00a` (calendar edits commit only once on Enter/blur),
`e9079b0b` (build/test portability), `56a7f4d8` (Windows Work probe), and
`92c1bdf2` (Chromium editor fixture line breaks).
The earlier Windows branch remains available independently at `f937e2db`.

| Check | Observed result |
| --- | --- |
| Workspace/all-target check | Passed on the integrated tree; subsequent formatting and test portability changes are recorded separately |
| Engine with `agentic-browser` | 532 passed, 3 opt-in tests ignored |
| Composition with `durable-runtime` | 135 passed |
| Controller with `provider-transport,probe-harness` | 127 passed |
| Agentic with `provider-transport` | 1,110 passed after retaining SystemRoot in the isolated Windows proxy subprocess |
| App with `work-runtime,work-execution` | Initial 859 passed/41 failed/14 ignored; using safe temporary fixtures: 893 passed/7 failed/14 ignored |
| Frontend unit tests | Initial 907 passed and two load-related timeouts; both failing files passed a serial rerun (10 tests) |
| Scoped Chromium components | Initial 427 passed/5 failed/6 skipped; four affected files passed all 12 tests after the calendar fix and test portability/readiness corrections |
| Final frontend typecheck, scoped lint, build | Passed, zero Svelte diagnostics; existing bundle budgets retained |
| Native automatic PiP | Entry after hiding the page and exit on return passed |
| Final-tree native download and media fixture | Passed, including download provenance after browser exit and automatic PiP round trip |
| Rust formatting, compact semantic runtime, PiP JavaScript | Passed; all seven PiP policy/race tests passed |
| Native Work lifecycle, first preview, action guard | Passed |
| Native Work `webmail-inbox`, `writes`, `human` | Initial runs failed; fresh narrow reruns and limits are detailed below |
| Workspace source-contract tests (`xtask`) | 159 passed, 14 failed; the first workspace run stopped here |
| Remaining workspace targets (`--exclude xtask --no-fail-fast`) | Completed; only `zephium-app --lib` failed (the default AppData temporary-directory run described above) |
| Strict workspace Clippy | Failed: oversized variants, boolean-expression lints, argument count and an `unreachable!` in the integrated Work code |
| Frame emitted-style check | Failed on the integrated Work input glow (`0 -10px 40px -20px var(--glow)`); the checker still requires at most a 3px contact-shadow blur |

Application tests must place disposable fixtures outside AppData. Work
intentionally rejects every AppData descendant, including the default Windows
temporary directory. Set `TMP` and `TEMP` for the test process to a dedicated
directory such as `target/review-qa/work-test-temp`. This does not change the
product policy. The seven remaining failures are one git-index test attempting
to overwrite a handle-pinned index (Windows sharing violation), two CLI tests
expecting execution despite `ensure_available()` refusing that capability, and
four trash-retention tests requiring a private namespace unavailable on Windows.
Do not enable those capabilities or loosen their guards to satisfy the tests.

The native mail fixture initially lost exactly three newlines: Chromium's
Shift+Enter inserts BR elements inside a paragraph, whereas the fixture read
`textContent`, which omits them. The controlled fixture now serializes those
breaks and excludes the trailing caret placeholder. `node --test
scripts/test-work-editor.mjs` verifies native paragraph insertion, Shift+Enter,
trailing breaks and rejection of unadmitted raw DOM replacement. The exact
native send assertion passes after rebuilding the probe: one approved send,
unchanged expected multiline content, no reply, clean native teardown.
The final run still reports failure to read/preserve the result after sending;
it explicitly refuses automatic repetition. Thus the fixture's exact-send
success is not evidence of complete end-to-end result extraction. Logs:
`work-webmail-diagnostic.log` and `work-webmail-final.log`.

The native `writes` and `human` fixtures encounter a selected-profile browser
generation mismatch after a temporary cookie-presence controller closes. The
new controller reports another browser PID while the retained process handle
still reports its original process running and its exit observer remains
pending. Admission correctly refuses and retains the uncertain owners; later
cases in the same fixture then fail because the profile is quarantined. This
does not establish a vendor defect. Fresh narrow `slackwrite` and `human-signin`
runs subsequently pass with clean exact teardown; the generation failure is
intermittent and remains unresolved. The sign-in fixture preserves the native
page through its hold, resumes with the original limits, and refuses a retired
check. This is synthetic handover evidence, not manual move/resize acceptance.
Reproduce in a fresh disposable session
with `windows-work-probe --loopback-site slackwrite` or `human-signin`; never
accept a different PID without exact retirement proof for the old generation.

The 14 source-contract failures are in `agent_controller_boundary`,
`agentic_probe_boundary`, and `work_resource_boundary`. Several assertions
search for implementation literals that moved in the integrated Work branch.
They still need a security-boundary review; no assertion was rewritten just
to obtain a passing gate. Full details are retained in
`target/review-qa/integrated-workspace-tests.log` and
`target/review-qa/integrated-clippy-4.log`.

The final QA executable is `target/review-qa/Zephium Review QA.exe`, built with
`--debug --no-bundle --config ../target/review-qa/tauri.review-qa.conf.json`.
Its identifier is `app.zephium.review-qa`; its security configuration matches
the integrated product configuration. Direct launch from the packaged development
host redirects newly created Roaming descendants into that host's MSIX cache,
which correctly fails the runtime canonical-parent check. Launching through
`explorer.exe` opens the app normally outside that inherited package identity;
the window is responding. No ownership check or product storage policy changed.

## Remaining acceptance and engineering work

The extension delivery issue above is an engineering follow-up. The inventory
also leaves installer interruption, locked-file storage behavior, private-data
residue, complete log-content privacy, and full Work bridge enforcement open;
code inspection and the passing workspace suite do not close those native cases.
Existing denied/gated capabilities must not be advertised as Windows parity.

Owner acceptance should concentrate on video fullscreen versus F11 (including
Snap/maximize restoration and monitor changes), keyboard focus/IME across the
address field and pages, and minimize/lock/sleep/wake with active media. Use a
separate QA identity for a desktop build. A debug Review QA executable is open;
no release package or installer was produced by this pass. Check download
Open/Show in Folder, slow video fullscreen entry/exit, and automatic PiP when
switching tabs and returning. Native macOS automatic PiP still needs its own
build and acceptance run.

For resource measurements, use a release build with the same tabs and settings,
record WebView2 version and hardware, and include all child WebView2 processes.
Measure protection on/off over repeated matched loads, plus focused/minimized
idle and tab counts. This remains owner-run work; the code changes above make
no claim that blocking has zero cost on every page.

## Decisions for a later reviewed change

- Local AppData: migrate only with exclusive profile ownership, verify the copied
  databases and profile tree before switching, retain a recoverable old location,
  and make an interrupted migration resumable. Do not move a live WebView2 UDF.
- Windows install-on-quit: prepare/reverify the installer before terminal teardown,
  launch only after clean shutdown, suppress relaunch for ordinary quit, and keep
  installer execution outside the hard-exit watchdog. Keep this a separate change.
- Uninstall: recommend retaining browsing data by default, with an explicit
  owner-approved removal option if desired.

No migration, installer-scope change, signing implementation, or release-blocker
change is part of this pass. This change is suitable for draft review of the
verified fixes, not release approval. Merging remains a separate owner decision;
the existing pending-vendor-fix publication block remains active.

## Windows chrome polish (2026-10-19)

The acrylic wash was overridden by the more specific transparent-material
reset. Its ground now lives in the material axis, retaining the existing 78%
theme tint instead of exposing bare system acrylic. Vibrancy and the opaque
fallback keep their previous grounds.

Windows now reveals a fading native caption across the top of the page.
It leaves sidebar navigation accessible; internal pages reveal corner controls.
The page retains its 8 DIP gutter, with no reserved title band and no page
resize on hover. The existing native overlay handles pointer input above
WebView2, dragging, resize edges, minimize/restore/close and keyboard focus.
Maximize nonclient messages target the resizable owner for Snap integration.
The reveal target also works when maximized and retires while the caption
is visible so it cannot intercept the buttons. Fullscreen suppression is
per window. Reduced motion and high contrast skip the fade.
The retired DOM caption no longer receives JavaScript state updates on
every resize and focus event. A reveal originating above the sidebar keeps
checking for departure until the pointer leaves that otherwise hidden target.

The Windows rail beside Downloads and other tool panels puts Browse/Work
in its top row and search below it. Settings keeps the same ordering;
macOS keeps its traffic-light and search layout.

Owner acceptance for this pass:

- Compare dark and light material over the same wallpaper and over another
  window; check the sidebar and outer gutter for a visible seam.
- Hover the top edge, then leave it. The caption must fade over a stationary
  page. Repeat maximized and after moving between monitors with different DPI.
- Use caption drag, double-click, minimize, restore, close and maximize-hover
  Snap. Check Alt+Space and Alt+F10 keyboard access, and return focus to the page.
- Enter and exit video fullscreen and F11; a second browser window must keep
  its controls. Check Browse/Work and search in Downloads, Bookmarks and Settings.

These are native visual/input acceptance checks, not claims established by
the component tests. The broader engineering limitations above remain open.

Validation for this UI pass: all 10 scoped shell/sidebar component tests pass,
including the computed acrylic ground and Windows rail ordering. The eight
core layout tests and the native caption resize-hit test pass on Windows.
Frontend type checking, lint, stylelint, formatting, unused-code checks and
both UI builds pass; bundle budgets are unchanged. The unit run passed 908
of 909 tests; its locale test imported the entire generated message catalog
and timed out even on a focused rerun. Importing only the message that test
exercises preserves its assertions and makes all five locale tests pass in
1.61 seconds (909 unit tests passed across the run and focused recheck).
The emitted-style gate still rejects the existing Work glow's 40px shadow;
that previously recorded failure is unchanged.

Implementation: `03bab7f2` (Windows chrome) and `d4507b04` (locale test).
The final debug Review QA build succeeded and was launched through Explorer;
the isolated executable has a responding window. Its SHA-256 is
`b4c2726a4e4c1cb65b382f19a4bf7df816d361b3885f60dae153243d632a9305`.
No installer or release-performance measurement was produced.

## Caption refinement after owner feedback (2026-10-19)

This supersedes the preceding caption appearance and acceptance notes. The
window's theme wash is now 68% rather than 78%. The caption exposes the same
acrylic as the sidebar: its background is color-keyed instead of painting a
separate opaque grey band. High contrast retains the system's opaque colors.

Revealing the caption translates the native page containers downward by 40 DIP
over 400 ms. The WebView2 controller keeps its dimensions, and the stage clips
the displaced page to its original bounds. This uses the split preview's native
window positioning mechanism, not a screenshot or a compositor-only transform.
One animation clock drives the controls and the page. Reduced motion skips the
animation. Fullscreen suppression and native move/size transitions restore the
page's original position; translation remains local to each window.

The input surface covers each complete 46-by-32 DIP button rectangle, including
transparent pixels. A press captures input and activates only after release in
the same button. Motion pauses during a press so a moving target cannot escape
the pointer. Maximize clicks now issue an explicit owner system command rather
than forwarding nonclient button tracking between different HWNDs. Maximize
hover still forwards to the resizable owner for Snap. Caption dragging waits
for the system drag threshold, preserving the second click of a double-click.

Focused owner acceptance:

- Reveal and dismiss the caption over a web page and a split. Check for smooth
  travel in both directions, unchanged page layout, and matching acrylic.
- Click maximize near each corner and in the center of its button; repeat to
  restore. Press, move outside, and release: it must not activate. Try a press
  during reveal, caption double-click/drag, and the maximize-hover Snap menu.
- Repeat at fractional DPI, after maximize/minimize, and after video fullscreen
  or F11. Check a second window, internal pages, and keyboard caption access.

Native visual/input acceptance remains owner-run: desktop automation did not
expose the existing Review QA window in this session. This pass does not replace
the broader release qualification and open failures recorded above.

Validation: `frame check` passes, including all 909 unit tests; both focused
Windows chrome component tests pass. Both frontend builds pass without budget
changes. Eight Windows stage tests and two native caption geometry tests pass,
including unchanged controller dimensions during translation, fractional DPI,
negative monitor coordinates, and isolation after owner-handle reuse. Formatting
and the isolated debug application build pass. The emitted-style gate still
rejects the existing Work glow (`0 -10px 40px -20px var(--glow)`) against its 3px
contact-shadow limit; that unrelated visual treatment was left unchanged.

## Alpha caption rendering and lighter material (2026-10-20)

This supersedes the preceding color-keyed caption implementation. A small,
cached, premultiplied-alpha bitmap now draws the three controls with antialiased
glyphs, rounded hover surfaces and focus rings. Native BUTTONs retain input and
accessibility on a separate input host; their default theme painting cannot
appear in the visible surface. Both owned windows hide before close or a window
state command can trigger focus repaint. A vetoed close restores the reveal
target. High contrast uses the system's opaque colors.

The window wash is 28%, down from 68%; raised surfaces use 56%, while launcher
contrast is retained. Windows 10's additional acrylic tint is also reduced.
This remains Windows native acrylic, not an implementation of refractive glass.

Caption travel is 32 DIP over 220 ms, with 44-by-26 DIP button targets. Page
containers still move without resizing the WebView2 viewport. A press or native
move loop holds the current position until release; dragging no longer retires
the caption. Hover works on an inactive window without deliberately raising or
activating it. Clicking activates the owner. Keyboard access exposes the full
target before focusing it, and accessibility invocation does not depend on the
cursor position. Stationary hover uses leave tracking instead of periodic
dismissal polling. Geometry and bitmap updates are skipped when unchanged.

The alpha buffer occupies 17,664 bytes at 100% scaling and 70,656 bytes at 200%.
These figures exclude the input host, Windows and compositor allocations.
On this Windows 11 build 26200 / Core i3-1115G4 machine, an optimized isolated
benchmark of the exact `raster` function measured:

| Scale | Mean redraw | Median | p95 |
| --- | --- | --- | --- |
| 100% | 223.18 us | 215.60 us | 242.50 us |
| 200% | 863.45 us | 815.90 us | 969.70 us |

Method: compile the constants, Appearance, dimensions and pure raster helpers
from `windows_caption_paint.rs` with `rustc -O --edition=2021`; allocate the
buffer once, vary the hovered button and maximized glyph each iteration, and
pass input/output through `std::hint::black_box`. Discard 100 warmups and record
2,000 timed redraws per DPI. Local harness and output are in
`target/review-qa/caption_raster_bench.rs` and `caption-raster-bench.log`.
Animation reuses the rasterized pixels until appearance changes. This measures
neither layered-window upload nor page movement, full-app CPU/GPU, or battery
use. It is not release-performance qualification.

The implementation follows Microsoft's advice to keep an
[UpdateLayeredWindow surface small](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-updatelayeredwindow).
The visible surface never calls SetLayeredWindowAttributes, which is incompatible
with subsequent UpdateLayeredWindow calls until the layered style is reset.
The engine still positions HWND containers; a compositor-only page animation
would require a separate WebView2 hosting change. Existing page-container corner
regions are unchanged; the new alpha edges belong to the caption controls.

Focused visual acceptance: compare material over the same wallpaper; reveal the
caption in Browse, Work and Settings; hover and press each button, drag while
holding the caption, repeat with an inactive window, then close. Check for white
flashes, stable hit targets and smooth rounded edges at fractional DPI. Repeat
maximize/restore, Snap, Alt+F10, reduced motion and high contrast. Automated pixel
and native-surface tests do not substitute for this owner acceptance.

Validation: all five focused native caption tests pass, including real hidden
layered-window presentation, cropping and DPI changes on this machine. Rust
formatting and strict desktop Clippy (`--lib --tests --no-deps -- -D warnings`)
pass. `frame check` passes all 909 unit tests and its type/lint/style/format
checks; both focused Windows chrome component tests and both UI builds pass.
Bundle budgets are unchanged. Dependency-inclusive Clippy still stops on the
seven existing agent-controller diagnostics (large variant, boolean expressions,
argument count and `unreachable!`). The emitted-style gate still rejects the
existing Work glow's 40px shadow. Neither gate was weakened.

The isolated debug Review QA build succeeded and opened with a responding
window. Its SHA-256 is
`425b34603c4e3b39a466de7edf9f3d0bcdceb145ccb0dc8748788707457c68da`.
After testing it, the owner reported it was mostly okay and requested committing
and pushing the completed work. This feedback does not establish every optional
visual check above or broader release qualification.
