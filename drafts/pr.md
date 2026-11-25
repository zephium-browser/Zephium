# Feat: Choose a custom user data directory

## What changes for the user

Pass `--user-data-dir <path>` to put Zephium's app data in a chosen directory. Chromium offers a similarly named [`--user-data-dir` flag](https://github.com/chromium/chromium/blob/main/docs/user_data_dir.md). Separate roots get their own instance identity. Launches using the same root hand off to the existing instance. Relative paths resolve from the current directory. Cache and logs stay in `cache/` and `logs/`. New roots get owner-only permissions on macOS and a Windows ACL for the current user, SYSTEM, and Administrators. Without the flag, Zephium keeps its current default. Invalid paths or repeated flags fail at startup.

## How it was checked

- macOS: `cargo test -p zephium-desktop --lib data_root::tests --locked`: 9 parsing, root-resolution, per-root identity, and logs failure tests passed.
- macOS: `cargo check -p zephium-desktop --lib --locked` passed.
- macOS: `cargo test -p zephium-private-fs --locked`: 125 tests passed.
- Windows compile check: `cargo check -p zephium-private-fs --tests --target x86_64-pc-windows-msvc` passed.
