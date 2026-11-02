# Windows file-workflow candidate qualification

The Windows adapter is implemented; run these checks on a Windows machine before relying on it. A macOS
cross-check is compilation evidence only. Do not label this candidate Windows
production-ready until the checks below pass on the supported Windows/WebView2
floor and a current stable WebView2 runtime.

## Build an isolated candidate

From a checkout of this repository, with the Rust/MSVC, Node, pnpm and
WebView2 prerequisites installed. Install locked frontend/desktop dependencies
using the normal repository setup, then run in PowerShell:

```powershell
./scripts/qualification/build_file_workflows_windows.ps1
```

The script runs native library/desktop tests, Chromium component tests, frontend
checks/build/CSS gate, then builds a debug executable without an installer. It
uses `app.zephium.files-integration-qa` and the explicit Windows QA configuration.
The default output is `target/debug/zephium-desktop.exe`; respect
`CARGO_TARGET_DIR` if configured. The script does not launch it. The QA feature
cannot be used in release builds or combined with another QA identity. 

Start the generated-only fixture in a separate terminal:

```powershell
python scripts/qualification/file_workflows.py --files "$env:TEMP/zephium-files-qa/uploads"
```

Keep that terminal running. Use the newly printed token URL, never one copied
from an older test report. Select a new disposable download directory under
`$env:TEMP/zephium-files-qa/`. Do not select personal upload files.

## Native acceptance matrix

Record the exact Git commit, `git diff --stat`, executable SHA-256, Windows build,
WebView2 version, filesystem type, test result and any failure for each row.

| Workflow | Required result |
| --- | --- |
| First use, empty Downloads, settings, all dropdowns | Usable after click, selection, Escape, focus changes; no CSP violation or pointer lock |
| Single/multiple/directory uploads and Unicode names | Fixture reports exact bytes; paths remain in native picker/engine |
| Cross-origin frame upload, accept hints, cancelled picker, navigation/close during picker | Correct frame gets selected files; cancelled/stale selection grants nothing |
| Drag/drop and cloud-provider file selection | Correct bytes or visible failure; no hang; test each claimed provider separately |
| Attachment, redirect, cookie-authenticated, POST, blob, large data URL | Exact expected bytes; native session semantics; Completed only after publication |
| Empty file / unknown Content-Length | Completes with exact zero/actual bytes; no false 100% during transfer |
| Truncated response and disconnect | Failed/interrupted; no published partial advertised as Completed |
| Existing name, repeated download, maximum-length Unicode name | Existing file untouched; bounded distinct suffix; correct extension |
| Save dialog cancel, receiving cancel, cancel at completion | Settles once; no published partial; temporary ownership eventually cleared |
| Close source tab during slow transfer | Transfer finishes or truthfully fails; closed page cannot regain authority |
| Quit during native transfer/finalization; reopen | No hang; interrupted receipts recover without opening Downloads |
| Clear history, then restart / failed cleanup retry | Clearing history never loses cleanup ownership; retry does not delete unrelated files |
| Delete persistent/private profile during transfer | Native operations drain; profile deletion does not bypass pending cleanup |
| Private download, close private profile, quit | No private paths/history in SQLite; saved file remains; orderly shutdown removes temporary payloads |
| Change/remove saved file; Open / Reveal | Exact identity checked; changed/missing files give useful errors |
| Full/read-only/unplugged target; locked file; junction/reparse substitution | Explicit failure; no overwrite, recursive deletion or write outside selection |
| Privileged chrome and agent-originated downloads | Still denied |
| Repeated downloads and idle observation | Eight-transfer admission bound; no accumulating controllers, timers, COM handlers or worker threads |

For a completed fixture file, inspect Windows provenance:

```powershell
Get-FileHash -Algorithm SHA256 -LiteralPath 'C:\path\zephium-download.txt'
Get-Content -LiteralPath 'C:\path\zephium-download.txt' -Stream Zone.Identifier
```

The attachment hash is
`af816d943b8b840356cccd91cd2540b50d3c07d2f5b05c7fa128b77e55b4d559`.
The stream must have a single `[ZoneTransfer]` section and Internet/Restricted
`ZoneId` (3 or 4). The implementation also invokes Windows Attachment Services;
plain-text fixture success is not proof of malware-detection coverage. Verify
signed packaged-app execution and enterprise protection policy separately.
Never weaken security policy or disable Defender to make a check pass.

## Explicit limits

- Foreground downloads save to Downloads by default, or to a directory with a
  picker-issued identity. Ask-first remains optional. The event has no originating-
  frame activation/navigation ID, so this does not claim initiating-frame consent;
  WebView2's multiple-automatic-download permission remains denied.
- Upload selection is WebView2-owned. The pinned stable API has no equivalent
  to Zephium's macOS initiating-frame upload broker; do not claim parity for that
  interception boundary.
- Automatic resume/replay is not implemented. Interrupted POST/blob downloads
  must be retried from their website; replaying a saved URL would lose semantics.
  A person can pause a receiving download (`DownloadOperation.Pause`) and resume
  it from the same retained operation; a user pause is held for 30 minutes.
- A finished download whose type is on the dangerous list (`is_dangerous_file`)
  stays in its hidden staging directory in the `held` state until the person
  keeps or deletes it, for up to an hour; only Keep publishes it, with the same
  Attachment Services and zone-stream treatment as any other download.
- Protected publication requires a filesystem supporting the enforced identity,
  ACL and zone-stream guarantees. Unsupported destinations fail closed. FAT/exFAT
  and particular network/cloud-provider paths are not promised.
- Private cleanup receipts remain in memory. Abrupt process termination can leave
  an incomplete hidden staging directory in the selected destination; orderly
  closure drains it. Private crash cleanup without persisting browsing/path
  history is still a release-design gap.
- Linux and local-document navigation remain out of this delivery's scope.

## Implementation notes

Ownership, recovery and platform behavior of the shared download/upload
lifecycle, as implemented.

### Shared ownership and recovery

- Migration 21 (`0x3483179692c933a6`) adds a bounded `download_cleanup` journal,
  independent of history. Migrations 1–20 keep their existing fingerprints.
  Forget/history eviction cannot discard cleanup ownership. Only an exact
  filesystem receipt acknowledgement can remove an obligation.
- Startup enumerates registered and deletion-pending durable profiles. It cleans
  in 50-record pages with one recovery worker, without opening Downloads. Late
  terminal persistence requests a rescan; the coordinator stops its idle timer.
- Profile deletion retains internal cleanup/terminal-save access after the normal
  registry grant is revoked. A failed drain does not mark the profile retired and
  thereby allow a subsequent deletion attempt to bypass proof.
- Private cleanup has a separate, bounded, memory-only receipt map. Completion
  and removal from visible history do not discard that receipt. Orderly private
  profile/global shutdown drains it without persisting private paths.
- Explicit Cancelling state; late progress samples cannot change terminal Store
  acknowledgement revisions. Cancellation wins a race with native completion
  before publication, while finalization ignores late native failure callbacks.
- Destination decisions cancel on drop. Detached cleanup/persistence work also
  counts against admission, preventing repeated fast transfers from creating an
  unbounded backlog of slow filesystem workers. Existing bounds remain eight
  active transfers, four UI calls, 64 recent/private receipts, 10,000 durable
  history/cleanup records; new admission pauses at 32 outstanding work items.
- Portable filename handling covers reserved Windows device names, alternate
  streams, separators and bidi controls. Collision suffixes remain within 240
  UTF-8 bytes, preserving normal extensions and complete Unicode characters.

### Windows

- Human WebViews opt into a narrow Wry DownloadStarting hook. Explicit denial
  wins for privileged/agent contexts. WebView2 retains transport, cookies, POST
  bodies and blob/data ownership. No URL replay or second HTTP client is used.
- Native deferrals remain cancelled until a selected destination and durable
  staging receipt are acknowledged. Native COM file/folder dialogs, interruption
  mapping, cancellation, progress, Open and Reveal are implemented.
- Windows honors the automatic-save preference and retains the foreground-document
  admission checks. Native confirmation is used when requested or when a legacy
  custom directory lacks its picker-issued identity. Canonical staging paths are
  projected to verified ordinary Win32 spelling for WebView2's download writer.
- Canonical ancestor handles, protected user/SYSTEM staging ACLs, reparse rejection,
  complete 128-bit file IDs, exclusive same-volume publication, Attachment Services
  and verified Internet-zone metadata protect the file boundary. A protection or
  unsupported-filesystem failure cannot be shown as Completed.
- Recovery refuses an open writer and retains uncertain native process ownership
  by PID plus creation time. Deletion addresses only the fixed payload and exact
  empty staging directory, never a recursive user-directory walk.
- Closed source tabs revoke page authority, hide/disable scripts and park at
  about:blank while a bounded retained controller owns the transfer. Shutdown
  joins download drain with existing verified process-group teardown.
- Native stopped/cancelled navigation is distinguished from controller failure,
  preserving an uncommitted download destination context without inventing a
  document commit or presentation permission.
- Origin-less data URLs expose only their page context in history, explicitly
  labelled as such; large data URL payloads are not copied into UI metadata.

### UI and qualification support

- Downloads shows Cancelling and actionable cleanup failures/retry while keeping
  history usable. A late history response cannot erase a newer cleanup failure.
- The status indicator takes one bounded native snapshot on subscribe, including
  after profile changes, then follows events without history or idle polling.
- Extended generated-only loopback fixtures cover empty, unknown-length and
  deliberately truncated responses plus a large data URL.
- Added an isolated Windows debug QA configuration and
  `scripts/qualification/build_file_workflows_windows.ps1`. The QA feature still
  rejects release builds and mixed QA identities. Windows instructions and the
  native acceptance matrix are in [file-workflows-windows-qa.md](file-workflows-windows-qa.md).
