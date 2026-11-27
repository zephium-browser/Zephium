# Windows Extensions QA

Use ordinary non-administrator PowerShell and the isolated QA profile.

```powershell
# Close QA before rebuilding. Rust and Node/pnpm are required.
powershell -NoProfile -ExecutionPolicy Bypass -File desktop\run-webext-qa.ps1 -Build -Release
# Later launches use the existing executable:
powershell -NoProfile -ExecutionPolicy Bypass -File desktop\run-webext-qa.ps1
```

The launcher verifies **Zephium Extensions QA** and refuses a second instance.
The binary, `revision.txt` and `build-profile.txt` live in `target\webext-qa`.
Data stays in `%APPDATA%\app.zephium.webext-qa`; normal profiles are not imported.
Execution policy changes only for the invoked PowerShell process.

For repeatable checks without changing the usual QA session, append
`-Session memory-review-20260930`. This QA-only option uses
`qa-sessions\session-memory-review-20260930` beneath that same isolated root.
Close QA before changing sessions; omit `-Session` to return to the usual data.

If a packaged development tool reports that a WebView2 generation escaped its
runtime root, use ordinary PowerShell or open the verified binary in File Explorer.
Packaged-parent AppData redirection can cause that mismatch. Keep the startup
gate intact; do not copy, reset or delete profile data.

## Daily-use acceptance

Record each result individually, including build revision, extension version,
WebView2 version and the exact failing step. Do not share credentials or complete
authentication callback URLs. Use disposable accounts for credential tests.

1. Install Bitwarden or Dark Reader using the sidebar **Add to Zephium** control.
   The store's Add to Chrome control should be hidden. Review permissions and
   check installation settles without an error.
2. On an ordinary webpage, open Utilities and the action. Check title/icon/badge,
   anchoring and close/reopen. Rapidly switch tabs, then open it again: state and
   target must belong to the current tab. Internal pages intentionally omit tiles.
   Check the borderless popup with Bitwarden, 1Password, Dark Reader and Grammarly
   at 100% and 150% display scaling: short/tall content, no initial white flash,
   placement near the screen edge, and keyboard focus. Dismiss with Esc, outside
   click, the same action button, tab switch/navigation, and browser move/resize/
   minimize. A second-monitor check remains separate from single-monitor scaling.
3. Check account workflows separately: Grammarly login and persistence after
   restart; 1Password setup, sign-in, fill, repeat click and extension-tab restore;
   Bitwarden sign-in, fill, and its right-click submenu on a login field.
   A visible menu or welcome popup alone does not qualify filling.
4. Disable, reload and check new content-script injection stops. Re-enable and
   reload; it should resume. Existing script effects can remain until reload.
5. Change All sites to Specific sites with one allowed host. Reload that host
   and another; only the allowed host should receive content scripts. Switch back
   and verify extension settings/sign-in persist.
6. Open Options in a normal tab. Disable the extension and verify reload is
   refused. A profile without that enabled install must also refuse its document.
7. Repeat enablement/storage checks in a second profile. Remove from only one,
   reload and restart: it should stay removed there and work in the other.
   Private profiles do not support extensions.
8. Try the invalid-CSP fixture: native load should show **Couldn't start / Retry**,
   affecting only that install. Existing tabs/extensions must remain usable.
   A package rejected during preparation can instead stop at review.

The keyed fixture at `crates/zephium-webext-windows/fixtures/storage` can be
installed via **Install from file > Unpacked folder**. Save its value, switch
All -> Specific -> All, restart and recheck without account data. Remove test
fixtures when finished.

The `fixtures/popup` folder provides short/tall content and `window.close()`
buttons without account data. It shares the storage fixture's public test key:
use only one of those fixtures in a session. It must not be enabled in the
three-extension memory comparison.

## Optional permission prompt

1. Build and run the QA browser with `-Build -Release -Session permission-279`.
   Install Bitwarden in this session. Use a disposable account and open
   **Settings > Autofill > Make Bitwarden your default password manager**.
2. Turn the option on and choose **Deny** in Zephium's access prompt. Verify
   the toggle stays off and the main window responds. Repeat with **Allow**.
   Verify the toggle turns on and survives a QA restart. Install the extension
   separately in a second profile and verify its toggle stays off. In the
   first profile, open an ordinary tab after Allow and check that the browser
   stops offering to save new passwords or fill general form data.
3. To force Chromium's prompt, use a throwaway checkout and prefix
   `permissions.request = function` with `if (false)` in
   `crates/zephium-webext/src/windows/permissions.js`. Rebuild the QA browser
   and use a fresh `-Session permission-279-native`. Install Bitwarden there.
   Open Bitwarden in a browser tab using its pop-out control and dismiss any
   toolbar popup before turning on the same toggle. If this WebView2 version
   reproduces the hidden native modal, verify the main window recovers without
   a restart and the log contains one `recovered hidden Windows WebView2 modal`
   line.
4. On the same build, leave a normal webpage's visible file picker open for
   at least two seconds. Verify it stays open until dismissed and produces
   no recovery line.

## Performance checks

Use a release build and the same resident tabs, package versions, permissions
and active page. Separate initial loading, steady use and idle. Restarting can
leave tabs unloaded and reduce memory without a code improvement.

For each condition, restart QA, load and reload every fixed tab in the same
order, and leave the same page active. Allow at least two minutes to settle
before recording. Keep the laptop plugged in, awake and unlocked with its lid
open; temporarily extend display/sleep timeouts past the run and restore them
afterward. Do not compile, run native labs or use QA during the capture.
Close the popup, leave QA untouched, and use the PID printed by the launcher:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File desktop\measure-webext-qa.ps1 -QaProcessId 1234 -Seconds 600 -Label fixed-tabs-idle
```

Repeat with extensions disabled and the same tabs resident. Results under
`target\webext-qa-resources` cover the QA process tree without a debugger:
private memory, summed working sets, process count and CPU as percent of one core.
Divide CPU by logical processor count for whole-machine percent. Working sets
double-count shared pages; short-lived processes can undercount sampled CPU.
Interrupted/user-active samples are observations, not controlled idle qualification.
Check for at least 21 samples over ten minutes and no gap longer than 45 seconds. Use
sampler `6cc490ed` or newer: earlier versions rounded fractional CPU deltas in
Windows PowerShell or could include unrelated processes after parent PID reuse.
Its regression check runs without querying real processes:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File desktop\measure-webext-qa.test.ps1
```

Record cold/warm startup and fixed simple/complex public-page loads, then repeat
install/remove and popup open/close cycles. Allow caches and workers to settle
before comparing memory. Battery claims need a controlled power test.

## Management view tradeoff

Each installed extension keeps a persistent hidden management view so that
asynchronous action notifications (badge, title, icon) keep arriving. A shared
view parked on `about:blank` was measured as an alternative:

| Extensions | Persistent manager per extension | Shared manager parked on about:blank | Processes |
| --- | ---: | ---: | ---: |
| 1 | 227.0 MiB | 233.2 MiB | 9 / 9 |
| 3 | 352.8 MiB | 341.3 MiB | 11 / 11 |
| 5 | 405.5 MiB | 361.7 MiB | 13 / 12 |

Means cover the final 30 seconds of 120-second minimal-lab trials, not the full
browser (Intel i3-1115G4, Windows 11, WebView2 154). Sharing saved about 44 MiB at
five extensions but loses asynchronous action notifications while parked, so
persistent observers stay until a replacement preserves correctness. The
extension admission budget is a QA limit, not a WebView2 limit.

## Limits

Eight enabled extensions process-wide remains
provisional. Dark Reader's per-site popup controls are not qualified. On-click
access, native messaging/desktop companions and MV2 are
unavailable or unqualified. Work remains separate. Fixture passes do not
establish compatibility with every store extension.
