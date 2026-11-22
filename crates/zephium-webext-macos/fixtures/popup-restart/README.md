# Popup worker restart probe

Run from the repository root on macOS with the pinned toolchain:

```sh
node scripts/qualification/macos-popup-restart.mjs
```

The script builds `webext-lab` with `--locked` and launches it three times
against one newly generated WebKit profile identifier. The first process
closes the popup and unloads the extension before exiting. The second exits
with them still live. Both exits are followed by a five-second pause.

Each process invokes the native toolbar action and checks that its popup
receives a response from a newly started worker. A fixture-only storage
marker checks that the restarts use the same persistent profile. The probe
fails if a process times out, no popup/result arrives, the worker response
is missing, the worker boot identifier repeats, or the storage marker is
wrong. The cold startup has a 2.5-second settling period so this probe does
not also measure startup latency.

The command exits nonzero on a failed phase and preserves scenarios, logs
and `results.json` under Cargo's target directory in `popup-restart/<uuid>`.
WebKit manages the disposable profile's platform storage separately. This
is a manual investigation probe, not a default CI gate or a product fix.
It needs no marketplace download, account, native messaging host, personal
profile or network page. It does not establish Windows acceptance.
