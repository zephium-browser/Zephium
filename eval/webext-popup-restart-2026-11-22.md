# macOS popup worker restart investigation

## Scope

A report that the installed 1Password toolbar button does nothing led to
this native runtime investigation. The installed failure was observed, but
its causal link to the fixture failure below is not established. A separate
disposable, unsigned-in 1Password session also produced an empty popup.
This change supplies a reproduction, not a verified 1Password fix.

The committed fixture contains no third-party extension code or user data.
No installed application or personal profile is used by the probe.

## Native reproduction

Host: Apple Silicon, macOS 26.5.2. Base: `2e95353e3346d674a907c7ec07e59498048da2bf`.

```sh
node scripts/qualification/macos-popup-restart.mjs
```

| Phase | Popup response | Persistent fixture marker | Result |
| --- | --- | --- | --- |
| First launch | `ready`, new worker boot ID | Absent initially | Pass |
| Reopen after closing popup and unloading context | `ready`, different boot ID | Preserved | Pass |
| Reopen after process exit with popup/context live | `missing-worker-response` | Preserved | Fail |

Two fresh-profile runs reproduced this pass/pass/fail sequence. The command
returns exit code 1. The native background-loading callback
reports success in the failing phase, but the worker-start trace is absent
and the popup receives no response object. The failure is below the frame's
button handling: this probe uses `Runtime::perform_action` directly.

Timing matters: an immediate relaunch passed all three phases in an earlier
control. The committed probe waits five seconds after both shutdown modes.
This is evidence of a restart-sensitive runtime failure, not proof of a
particular WebKit cache or persistence mechanism.

## Limits and checks

- JavaScript syntax checks and `cargo fmt --all -- --check` passed.
- `cargo clippy -p zephium-webext-macos --all-targets --features lab --locked -- -D warnings` passed on macOS.
- No production behavior, generated contracts or permission policy changes.
- Windows native execution, the full workspace CI matrix and signed-in
  1Password acceptance have not been performed.
- Earlier popup-retention and URL-scheme experiments did not establish a
  fix; their diagnostic changes were removed.
