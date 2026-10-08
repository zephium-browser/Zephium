# Contributing to Zephium

Zephium is a solo-maintained project with a strong product direction. Contributions are welcome, but **alignment matters more than volume**.

This document is about *whether* and *how* to contribute in a way that's likely to get merged, so neither of us wastes time. It's intentionally about principles, not internals - for how the code is actually laid out, read [docs/architecture.md](docs/architecture.md).

## How this project is run

- Zephium has one active maintainer ([@crynta](https://github.com/crynta)).
- Review bandwidth is limited.
- Not every contribution can be accepted, even if it's technically correct. Alignment with project direction matters as much as code quality.
- For scope and direction, read [docs/product-system.md](docs/product-system.md) before opening anything non-trivial.

This is normal for a solo maintained project. A "no" on a PR is not personal.

## What Zephium is

A lightweight, FOSS, zero-telemetry browser on the OS-native webview, Rust-heavy. Supported platforms are macOS (Apple Silicon) and Windows; Linux is not supported yet.

The priorities, in order:

1. **Security** - a browser is a hostile environment; security is designed in, not bolted on.
2. **Performance & resource use** - RAM, CPU, GPU, battery. We measure overhead, not vibes.
3. **UX** - best-in-class, plus our own ideas.
4. **Code & architecture quality** - production-grade everywhere.

If a change trades any of these away for convenience, it probably won't land. When you're unsure whether something fits, that's a question for discussion, not a surprise PR.

## Getting set up

### Prerequisites

Supported development platforms are macOS (Apple Silicon) and Windows 10/11. Linux development is not supported yet; the app refuses to start there.

- **Rust**: install [rustup](https://rustup.rs). The pinned toolchain in `rust-toolchain.toml` (with `rustfmt` and `clippy`) is installed automatically on first use.
- **Node.js**: the version in `.node-version` (`engines` in `package.json` allows `>=24.18.0 <25`).
- **pnpm**: enable it through Corepack (`corepack enable`); the exact version comes from `packageManager` in `package.json`.
- **Tauri platform requirements**: on macOS, the Xcode Command Line Tools (`xcode-select --install`). On Windows, the Microsoft C++ Build Tools (the "Desktop development with C++" workload) and the WebView2 runtime, which ships with current Windows 10 and 11. See the [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/) for details.

An outdated OS or WebView2 runtime starts with an update notice; only an unsupported one stops startup, with an explanation and exit status 78.

### Build and run

```sh
pnpm install --frozen-lockfile
pnpm dev
```

### Checks

Before opening a PR, run the checks CI runs for the area you touched:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test -p <crate> --locked        # the crates you changed
pnpm --dir frame check                # frontend changes
```

`cargo xtask ci` runs the complete gate; it is slow and memory hungry, so prefer the targeted commands while iterating.

For frontend work, start with [the frame guide](frame/README.md) and
[the frontend contract](docs/frontend.md). They define ownership, import boundaries,
colocated tests, native presentation rules, and the active migration gates.
The [documentation index](docs/README.md) lists the rest.

## Where to start

1. **Open an issue** with the bug or feature template. For design questions, scope questions, "should I work on X?" and quick feedback, start a [Discussion](https://github.com/zephium-browser/Zephium/discussions) instead.
2. **For a feature or any larger change, wait until the issue is labelled `accepted`** before writing code. Bug fixes and small, obvious fixes don't need to wait.
3. **Open the PR and link the issue** in its description (`Closes #123`).

## What makes a good contribution

These get merged fast:

- **Bug fixes** with clear reproduction steps.
- **Docs / typos / small UX fixes** - open a PR directly.
- **Pre-discussed features** - alignment in an issue or discussion first.
- **Small, focused changes** - easy to review, low risk.

If your change is small and obvious, open a PR directly. No issue required.

## Keep changes focused

**Only change what's needed to accomplish your stated goal.** Don't also reformat other files, clean up unrelated code, fix lint in files you didn't need to touch, or bundle several unrelated fixes together. Even when those are real improvements, they make review harder and slow everything down. Open a separate PR for cleanup after discussion.

**One PR = one logical change.** Multi-concern PRs will be asked to split.

## Discuss first (required for larger changes)

For anything beyond a small fix, **discussion is required before opening a PR**. This includes new features, UI/UX changes or changes to default behavior, refactors and "cleanup", performance rewrites, architectural changes, anything touching many systems, and anything that ships to everyone by default (default search engines, default filter lists, default settings).

Pull requests with significant unsolicited changes will be closed without detailed review. This isn't to discourage you - it ensures alignment before significant work goes in. A 10-minute conversation saves a 500-line PR that doesn't fit the roadmap.

## Respect the architecture

Zephium's boundaries are enforced by CI, not by review vigilance - a PR that violates them won't pass. The ones to know:

- The core stays pure: no I/O, no platform coupling, no `unsafe`. Side effects live at the edges, behind narrow interfaces.
- Network access is centralized and tightly limited. This is how the **zero-telemetry guarantee is enforced structurally**, not by policy - nothing outside that one path may reach the network.
- `unsafe` / FFI is confined to a single, clearly marked place, and every `unsafe` block is justified.
- Generated code (such as the typed frontend↔backend contract) is generated, never hand-edited; CI fails on drift.
- Abstractions are added when they earn their keep, not preemptively.

If your change needs to bend one of these, that's a design discussion before it's a PR.

## Quality bar

Zephium positions itself as **lightweight, fast, secure, production-grade**. Every PR is reviewed against:

- Type-checks, lints, and formatters are clean.
- All tests pass.
- Dependency policy is clean: FOSS licenses only, no banned crates, no known advisories.
- **No performance regressions in hot paths** - startup time, idle memory footprint, the render/compositing path, the frontend↔core message path, content blocking. Being light is the whole point; a regression here is a real bug, not a nitpick.
- No new heavy dependencies without justification (rough guide: >50KB gzipped in the frontend bundle, >5MB compiled on the Rust side).
- Platform parity preserved: macOS and Windows both still build and work.
- Security review for anything touching a sensitive surface (see below).

If you're not sure how to measure perf or what counts as a hot path, ask first. Better to confirm than get bounced.

## Load-bearing changes need a test

A test - not a second reviewer - is what catches the most dangerous class of bug: a **local fix with global blast radius**. The diff solves one reported case, reads fine, passes every check, and silently breaks the same subsystem in every other case. Review alone does not catch these.

So if your change alters behavior in a load-bearing path, the PR must add or extend a test that locks the invariant you're relying on. Load-bearing means:

- **A security or trust boundary** - what web content or code is allowed to do, what a profile may access, what the frontend or an automation may invoke. Test the deny side, not just the happy path.
- **Data isolation and privacy guarantees** - profile separation, and ephemeral/private state never touching disk.
- **Persistence and migrations** - no data loss on partial failure, forward-only, and the identity of saved records.
- **Pure logic with wide reach** - the transforms and state that many features depend on.

The bar is real coverage of the contract - the edge, the deny path, the malformed input - not a placeholder. If you can't see how to test it, ask before opening the PR. That conversation is shorter than the revert.

Pure UI, themes, and anything the type-checker already guarantees don't need tests.

## What Zephium is not

- Not aiming for pixel-perfect rendering parity with Chrome/Firefox. We use the system webview **by design**; rendering fidelity is a known, accepted tradeoff.
- **No telemetry, no analytics, no account requirement, no data collection - ever.** Any change that phones home will be closed.
- Not Electron. We don't bundle an engine. Weight and resource use are features, not afterthoughts.
- Not a kitchen-sink browser. It's opinionated and focused.
- Mechanical refactors, broad style changes, and drive-by rewrites are not helpful.
- AI-assisted contributions are okay (not vibe-coded), but the PR must reflect understanding of the existing patterns. Low-effort AI-generated code that the author clearly didn't read will be closed.

## Branches

Branch off `main`. Use these kebab-case prefixes: `feat/`, `fix/`, `chore/`, `docs/`, `perf/`, `security/`. Examples: `feat/vertical-tabs`, `fix/omnibox-focus`, `security/scheme-gate`. Don't open PRs from your fork's `main` - work on a feature branch.

## Commits & PRs

The **PR title becomes the squash commit** for most PRs. Multi-commit PRs with well-crafted atomic commits may be merged with a merge commit at the maintainer's discretion. Titles follow [Conventional Commits](https://www.conventionalcommits.org/):

```
feat(tabs): add vertical tab sidebar
fix(omnibox): keep focus when suggestions open
security(navigation): tighten top-level scheme gate
```

Types: `feat`, `fix`, `chore`, `docs`, `perf`, `refactor`, `test`, `build`, `ci`, `security`. Scope is just the area you're touching - keep it accurate, don't invent broad ones.

**Fill out the [PR template](.github/PULL_REQUEST_TEMPLATE.md):** what changed, why, and how you tested. "Tested manually by ..." is the bare minimum; add screenshots/GIFs for UI changes. Open a **draft PR early** if you want feedback mid-flight.

Every PR gets an automated review from CodeRabbit focused on correctness, performance and security, followed by a maintainer review. Address its findings or reply with why they don't apply.

### What gets merged faster
Clear problem statement · small, focused diff · follows existing patterns (read 2-3 nearby files first) · all checks pass · real testing notes.

### What gets bounced back
Mixed-concern PRs · large architectural PRs without prior discussion · new dependencies without justification · breaking changes without migration notes · incidental reformatting · AI code that obviously wasn't read.

## Code style

- Follow existing patterns. Read 2-3 adjacent files before adding new ones.
- TypeScript: no `any` unless you really mean it. Strict mode is on.
- Rust: `fmt` and `clippy` clean.
- Comments explain *why*, not *what*. Code should explain itself. No multi-paragraph docstrings.
- No emojis in code or commit messages.
- American English in user-facing strings.

## FAQ

**Should I ask before fixing a typo or obvious bug?** No, open a PR directly.

**I have an idea for a new feature.** Open an issue or start a Discussion. Don't open a PR without prior discussion.

**My PR was closed without detailed feedback.** Usually it didn't align with direction, or scope was too large to review responsibly. Normal for a solo project. Reopen with a smaller scope is welcome.

**Can I work on an open issue?** Comment first to confirm it's still relevant and nobody else is on it. For non-trivial work, discuss the approach first.

**I noticed cleaner code I could write while fixing my bug.** Focus on your stated goal. Submit cleanup as a separate PR after discussion.

**Can I change a default?** Defaults ship to everyone, so they're a product decision - discuss first. What users opt into themselves is a different matter.

**My PR conflicts after main moved.** If it's still relevant and reasonably small, rebase. Large stale PRs may be closed with an offer to reopen after rebase. Rotting velocity is real, not personal.

## Security issues

Don't file them as public issues. See [SECURITY.md](SECURITY.md) or use [private vulnerability reporting](https://github.com/zephium-browser/Zephium/security/advisories/new).

## License

By contributing you agree your work is licensed under [MPL-2.0](LICENSE). No CLA required.