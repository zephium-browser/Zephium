# Contributing to Zephium

Zephium is a native Rust browser with a Tauri + Svelte interface. Windows and
macOS are supported with equal priority. Linux is not supported; Linux CI
runners handle platform-neutral checks, not browser builds.

Keep a contribution focused on one problem. Small fixes and documentation
changes can go straight to a PR. Discuss larger features, default changes,
new dependencies and architectural changes in an issue first. Read the
[architecture](docs/architecture.md) and the guide for the subsystem you touch.
Coding agents also follow [AGENTS.md](AGENTS.md).

## Setup

Use the versions recorded in [rust-toolchain.toml](rust-toolchain.toml),
[.node-version](.node-version) and the root
[packageManager field](package.json). Install Rust through rustup with the
pinned rustfmt and Clippy components, Node.js, and that exact pnpm version.
The native CI jobs also install cargo-nextest; policy checks use cargo-deny.
Their versions are pinned in [ci.yml](.github/workflows/ci.yml).

| Host | Native prerequisites |
| --- | --- |
| Windows | Windows 10/11, MSVC C++ Build Tools with the Desktop development with C++ workload and Windows SDK, and the Evergreen WebView2 runtime. Use a normal user session for native storage qualification. |
| macOS | Xcode Command Line Tools and a supported macOS/Safari combination. Published macOS releases currently target Apple Silicon. Startup checks both the OS and the installed system WebKit; see [macOS admission policy](crates/zephium-core/src/macos.rs). |

Keep supported systems and engines updated. The
[WebView2 admission policy](crates/zephium-core/src/webview2.rs) distinguishes
unsupported runtimes from update recommendations. Do not override these checks
to make a development build start.

Run commands below from the repository root, unless a directory is stated:

```sh
pnpm install --frozen-lockfile
```

## Build and run

The root [package scripts](package.json) delegate native builds to
[desktop/package.json](desktop/package.json):

```sh
pnpm dev
pnpm build
```

`dev` launches a development browser using `desktop/tauri.dev.conf.json`
(`app.zephium.dev`). `build` packages the browser; it is not a frontend check.
Signing and publication belong to the maintainer's
[release procedure](.github/RELEASE.md). The macOS Cargo runner can sign selected
native development executables; inspect [the runner](scripts/dev-sign-run.sh)
before using it in unattended work.

For interface assets without launching the native browser:

```sh
pnpm --dir frame run build
```

Do not test against an installed Zephium application or its personal profile.
Native tests must identify their own checkout, executable and isolated data.
Do not change system settings, sign into accounts or trigger password prompts
as part of routine automated checks.

## Checks matching CI

[ci.yml](.github/workflows/ci.yml) is the executable source for the commands,
hosts and exclusions below. PR jobs are selected by changed paths; main runs
the complete matrix. A skipped job is not evidence that its tests passed.
Use targeted tests while iterating, then cover the affected host jobs before
calling a behavior change complete. Never suppress lints to obtain a pass.

### Frontend

The frontend CI job runs these commands after dependency installation:

```sh
node --test scripts/ci/audit-dependencies.test.mjs
node scripts/ci/audit-dependencies.mjs
pnpm --dir frame run check
node scripts/test-semantic-cdp-action.mjs
pnpm --dir frame run build
pnpm --dir frame run check:styles
```

`check` runs i18n generation, Svelte/TypeScript checks, ESLint, Stylelint,
Prettier, Knip and unit tests. Build precedes emitted-style checks.
See [frontend ownership and testing](docs/frontend.md).

Browser-component tests are separate. On macOS, the
[component workflow](.github/workflows/components.yml) installs WebKit and runs
`pnpm --dir frame run test:component --retry=1 --shard=1/8`, repeated for shards
2 through 8. Its setup is:

```sh
pnpm --dir frame run i18n
pnpm --dir frame exec playwright install webkit
```

For a local Windows component run, install `chromium` instead of `webkit`, then
run `pnpm --dir frame run test:component`. The
[Vitest configuration](frame/vitest.config.ts) selects WebKit on macOS and
Chromium on Windows; `ZEPHIUM_COMPONENT_BROWSER` can override the choice.
Hosted component CI currently covers macOS WebKit only and runs for the
`full-ui-tests` label, manual invocation and releases. Neither browser engine
in this suite substitutes for native WKWebView/WebView2 qualification.

### Shared Rust policy

CI runs these platform-neutral checks on Ubuntu:

```sh
cargo fmt --all -- --check
node scripts/blocker-scriptlets/library.test.mjs
cargo xtask check-engine-floors
cargo xtask check-advisory-exceptions
cargo fetch --locked
cargo fetch --locked --manifest-path vendor/adblock/Cargo.toml
cargo fetch --locked --manifest-path vendor/tauri/Cargo.toml
cargo fetch --locked --manifest-path vendor/tauri-runtime-wry/Cargo.toml
cargo fetch --locked --manifest-path vendor/wry/Cargo.toml
cargo xtask check-security-fork-locks
cargo xtask check-agentic-probe-boundary
cargo deny --locked check
```

The fetches populate every target graph needed by the offline fork check.
Engine/advisory review deadlines warn in ordinary CI; the
[security-review workflow](.github/workflows/security-review.yml) uses strict
checks. Release gates are separate. Do not weaken policy or change evidence to
silence a deadline.

### Native Rust: both platforms

Run on Windows and on macOS:

```sh
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test -p tauri-plugin-updater --lib --locked
```

macOS additionally runs:

```sh
cargo clippy --workspace --lib --locked -- -D warnings
cargo nextest run --workspace --locked --profile ci --retries 2
git diff --exit-code frame/src/shared/ipc/bindings.ts
```

Its remaining CI steps compile and exercise the bundled seed through native
WebKit, run `scripts/blocker-scriptlets/run.sh`, and run
`scripts/blocker-corpus/test-record.sh`. Use the exact seed materialization and
Swift compilation recipe in `ci.yml`; a Rust-only pass cannot replace it.
These steps open native test views, so require an appropriate isolated test host.

Windows uses this test selection, followed by the standalone logging tests:

```powershell
cargo nextest run --workspace --exclude zephium-desktop --locked --profile ci --retries 2 --no-fail-fast -E 'not (test(/^platform::windows::native_storage::work::tests::/) or (package(zephium-store) and test(/^(hub::agent_work|actor::tests::work_journal_|hub::work_document::runtime_store::tests::approved_deadline_)/)))'
cargo test --locked --manifest-path desktop/windows-log-tests/Cargo.toml
```

The workflow excludes the desktop binary because its runner lacks the runtime
loader, and selected storage/journal tests because of native ownership and
runner constraints. Those cases require the Windows QA environment. Preserve
the documented exceptions; do not add exclusions to hide a failure. Native
behavior changes need checks on both hosts, including affected excluded paths.
Record any platform not exercised explicitly.

### Local aggregate and specialized gates

`cargo xtask ci` is a larger local aggregate implemented in
[xtask/src/main.rs](xtask/src/main.rs). It includes boundary checks, fork and
blocker checks, selected feature/release builds, adapter Clippy/tests, native
macOS probes and frontend checks. It uses workspace `cargo test`; it does not
reproduce hosted nextest selection, dependency audits or component CI.

Use the public xtask dispatcher for available gates. For example,
`check-agentic-probe-boundary` includes Work composition, persistence and
resource checks; those modules are not separate CLI subcommands.
For changes to workflow tooling, follow the Workflows job in `ci.yml`, including
its scope, action-pin, release-policy and actionlint checks. For generated assets
and vendor changes, follow [AGENTS.md](AGENTS.md#generated-files-and-forks) and
the owning runbook rather than copying a partial release procedure.

## Branches, commits and review

- Work on a task branch from current main, such as `docs/engineering-guide` or
  `fix/tab-focus`. Preserve other worktrees and unrelated changes. Never commit
  directly to main or use stash/reset to rearrange someone else's work.
- Use scoped conventional commits and PR titles: `docs(architecture): clarify
  native view ownership`. Keep commits focused. Add no authorship trailers,
  generated-by notices or tool attribution.
- Follow the [PR template](.github/PULL_REQUEST_TEMPLATE.md): problem, change,
  reason and actual validation. Link the agreed issue for a larger change;
  attach screenshots for visual changes. Add regression coverage for changed
  behavior, including failure/deny paths at trust boundaries.
- Draft PRs can collect early feedback. Ready PRs receive automated review and
  maintainer review. Resolve findings or explain why they do not apply. Most
  PRs are squash-merged; a maintainer may merge curated atomic commits.
- Merge updated main into your branch before opening the PR and recheck affected
  work. Do not rebase, amend pushed commits or force-push.
- Report commands, host and outcomes accurately. Separate executed checks from
  commands verified against CI. A known failure still needs reporting; complete
  independent checks and do not claim skipped/native tests passed.

Report vulnerabilities privately through [SECURITY.md](SECURITY.md).
Contributions use the project's [MPL-2.0 license](LICENSE).
