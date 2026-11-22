# macOS extension worker recovery

## Failure and change

After an exit with an extension context still live, WebKit can restore its
service-worker registration without starting the worker. Its native background
completion reports success, but the popup's runtime message receives no response.
Two fresh-profile baseline runs at `2e95353e3346d674a907c7ec07e59498048da2bf`
produced pass/pass/fail for first launch, clean restart and unclean restart.
An immediate restart could mask the failure; both controls wait five seconds.

The runtime now waits for the prepared worker's existing `worker.started`
signal. After five seconds without it, a guarded `_backgroundWebView` getter
accesses that context's native background page. From that page, recovery
unregisters only the exact expected worker and registers the same URL, scope
and classic/module type. It waits for activation and has a further five-second
deadline. Ordinary extension pages cannot perform this recovery on the tested
WebKit (`InvalidStateError: Client is unknown`).

The operation checks origin, script URL, context and view identity. Concurrent
callers share one attempt, capped at 64 waiters. Unload cancels pending callers;
late callbacks cannot settle a replacement context. The ready view is weakly
held so it does not keep an idle worker alive. Background pages and unadapted
fixtures retain their existing behavior. Permission policy is unchanged.
No website registrations, extension storage or native messaging grants are reset.

## Native qualification

Host: Apple Silicon M5 Max, macOS 26.5.2 (25F84). Run:

```sh
node scripts/qualification/macos-popup-restart.mjs
```

| Case | Classic worker | Module worker |
| --- | --- | --- |
| First launch: popup answers with a boot ID | Pass | Pass |
| Clean restart: new worker answers | Pass | Pass |
| Unclean restart: new worker answers | Pass | Pass |
| 65 concurrent requests: 64 succeed, one refused | Pass | Pass |
| Eight cancelled requests: eight errors, no late duplicate | Pass | Pass |
| Reload after cancellation: popup answers | Pass | Pass |

Markers in chrome.storage.local, localStorage and IndexedDB survived every
restart and the reload after cancellation. Raw fixture evidence is under Cargo's
`target/popup-restart/1d9ce3c0-2699-49b9-9a24-6552c17fb3fc` (classic) and
`267197bc-83ea-4483-9d66-718d0eabdca5` (module).

An existing disposable, unsigned-in 1Password 8.12.46.29 profile that previously
had no worker-start trace now starts its module worker and responds to its native
action by requesting its welcome page. External native messaging was denied in
this lab session. This is worker/action evidence only: no account sign-in,
vault unlock, autofill or desktop-app integration was tested. The installed
user-profile symptom was observed earlier; its causal equivalence to this
restart failure is not conclusively established.

## Checks and remaining evidence

- `node --test scripts/qualification/macos-popup-recovery.test.mjs`: six passing
  tests, including wrong-origin/registration refusal and activation waiting.
- `cargo clippy -p zephium-webext-macos --all-targets --features lab --locked -- -D warnings`: passed.
- `cargo test -p zephium-webext-macos --lib --locked`: 30 passed.
- Workspace/macOS CI results are recorded in the pull request after final checks.
- Windows native execution and signed-in 1Password acceptance remain untested.
  This change is confined to the macOS adapter; Windows compilation is not
  a substitute for native acceptance.
