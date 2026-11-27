# Popup worker restart qualification

Run from the repository root on macOS with the pinned toolchain:

```sh
node scripts/qualification/macos-popup-restart.mjs
node --test scripts/qualification/macos-popup-recovery.test.mjs
```

The native script builds `webext-lab` with `--locked` and exercises both classic
and module workers in separate disposable WebKit profiles. Each variant first
closes its popup and unloads the extension, then restarts and exits with them
still live, then restarts again. Five-second pauses prevent an immediate
relaunch from masking the failure.

Each process invokes the native toolbar action and checks that its popup
receives a response from a newly started worker. Markers in `chrome.storage.local`,
localStorage and IndexedDB must survive every restart. The module variant
imports its worker from another module, checking recovery preserves module type.
The cold startup has a 2.5-second settling period; this is not a latency benchmark.

A second profile uses the raw fixture without the compatibility startup signal,
while explicitly enabling verification. This forces the pending state even when
WebKit happens to restart promptly. It checks concurrent recovery (64 accepted
waiters and one capacity refusal), unload cancellation (eight callers each settle
once), and successful reload after cancellation. The probe observes callbacks
beyond both readiness deadlines to catch duplicate settlement.

The command exits nonzero on a failed phase and preserves scenarios, logs
and `results.json` under Cargo's target directory in `popup-restart/<uuid>`.
WebKit manages the disposable profile's platform storage separately. The Node
tests exercise the exact recovery script's origin, registration, unregister
and activation refusal paths; they do not replace native qualification.

These probes need no marketplace download, account, native messaging host,
personal profile or network page. They do not establish signed-in password
manager behavior or Windows acceptance.
