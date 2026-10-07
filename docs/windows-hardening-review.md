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

## First changes

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
| Permissions | Windows camera/microphone use WebView2's prompt; macOS has a browser-owned deferred broker. Other page permissions, HTTP basic auth and client certificates are denied. Privileged UI denies ambient capabilities. | Permission UX parity and supported capability decisions; no silent granting of currently denied capabilities. |
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

## Remaining acceptance and engineering work

The extension delivery issue above is an engineering follow-up. The inventory
also leaves installer interruption, locked-file storage behavior, private-data
residue, complete log-content privacy, and full Work bridge enforcement open;
code inspection and the passing workspace suite do not close those native cases.
Existing denied/gated capabilities must not be advertised as Windows parity.

Owner acceptance should concentrate on video fullscreen versus F11 (including
Snap/maximize restoration and monitor changes), keyboard focus/IME across the
address field and pages, and minimize/lock/sleep/wake with active media. Use a
separate QA identity for a desktop build. No packaged QA build or installer was
produced by this pass.

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
