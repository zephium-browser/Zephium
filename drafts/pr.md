# Feat: Run multiple Zephium instances

## What changes for the user

With the companion `--user-data-dir` option, instances on distinct roots share one global launcher shortcut. If its owner exits, another running instance claims it. On Windows, startup removes stale WebView2 runtime data while preserving data used by live instances.

## How it was checked

- `cargo test -p zephium-desktop --lib instance_lease::tests::distinct_data_roots_share_shortcut_ownership_and_promote_after_release --locked` (1 passed).
- `cargo test -p zephium-desktop --lib --locked`: 441 passed, 2 ignored.
- `cargo test -p zephium-core --lib --locked`: 515 passed.
- `cargo check -p zephium-core --tests --target x86_64-pc-windows-msvc --locked` passed.
