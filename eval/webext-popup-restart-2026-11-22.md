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
restart and the reload after cancellation. Capacity and cancellation now use a
separate raw fixture with startup verification enabled but no startup signal,
forcing the pending state. An earlier capacity probe assumed every unclean
restart would stall; a post-merge run correctly started promptly and settled all
65 requests, exposing that test assumption rather than a capacity-limit failure. Raw fixture evidence is under Cargo's
`target/popup-restart/eb529596-f874-43f8-b5e0-32f4c659a3cd` (classic) and
`fdab5858-36b6-49c8-9404-f7c13cdb4bb9` (module).

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
- `cargo fmt --all -- --check`, workspace Clippy with both `--all-targets` and
  `--lib`, and `-D warnings`: passed after merging main `5cf563fec`.
- `cargo nextest run --workspace --locked --profile ci --retries 2`: 6,472 passed,
  50 skipped. One existing trash-retention test was slow; the xtask test
  `agent_controller_boundary::tests::mutation_cannot_bypass_action_authority_or_drop_pending_ownership`
  passed with a nextest leaked-child report. Its focused rerun passed without
  that report; the original result was not suppressed.
- `cargo test -p tauri-plugin-updater --lib --locked`: 17 passed. Generated IPC
  bindings, work constants and theme presets show no drift.
- Engine floors, advisory exceptions, agentic probe boundaries, fork locks and
  `cargo deny --locked check`: passed. Engine-floor checks emitted the existing
  November 17 review-deadline warnings. Fork fetch/check commands ran at the
  same commit in a temporary worktree outside the parent workspace, because the
  nested worktree confuses Cargo's independent vendored workspace discovery.
- Native bundled seed compile/reload and admission self-test, blocker scriptlet
  runtime checks and off-list corpus refusal checks: passed using the CI recipes.
- Hosted CI awaits GitHub workflow approval; no hosted result is claimed here.
- Windows native execution and signed-in 1Password acceptance remain untested.
  This change is confined to the macOS adapter; Windows compilation is not
  a substitute for native acceptance.
