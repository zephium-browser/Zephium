Fixes #567

<!-- Title: feat(macos): open multiple regular browser windows -->

## What

On macOS, File → New Window (`⌘N`) opens another regular browser window in the current profile. Each window keeps its own active tab, layout, and native content stage.

## Why

Issue #567 requests a regular New Window command, which Zephium didn’t have.

## How tested

**How I checked it:**

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --locked -- -D warnings`
- `cargo test -p zephium-core --lib --locked` (517 passed)
- `cargo test -p zephium-desktop --lib --locked` (435 passed, 2 ignored)
- `cargo test -p zephium-app --lib new_browser_window_gets_its_own_focus_and_closes_without_closing_the_shell` (1 passed)
- `pnpm --dir frame check` (1,430 tests passed)
- macOS Computer Use: pressed `⌘N` and used File → New Window; cycled between windows with `⌘`` and confirmed each retained its own active tab; closed one window and confirmed Zephium remained open

The broader app suite previously had one onboarding timeout; the test passed on isolated rerun. The new window lifecycle test passed both in the targeted run and the earlier suite run.

## Checklist

- [x] `cargo fmt --all --check` and `cargo clippy --workspace --all-targets --locked -- -D warnings` are clean
- [x] Relevant tests pass and new behavior is covered, including the deny path for anything security-sensitive
- [x] `pnpm --dir frame check` is clean (frontend changes)
- [ ] Screenshot or recording attached (menu item change)
- [x] Docs updated where behavior or architecture changed
- [x] No new telemetry, network path or heavy dependency without prior discussion
