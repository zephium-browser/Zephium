# Feat: Choose a custom user data directory

## What changes for the user

`zephium --user-data-dir <path>` routes settings, databases, bookmarks, sessions, logs, cache, and WebView profiles under one selected root. Relative paths resolve from the current directory. Without the flag, Zephium uses its existing default. New roots are created privately. Existing roots must pass ownership and privacy checks or startup fails.

## How I checked it

- `cargo test -p zephium-desktop --lib data_root::tests`: 6 root-resolution tests passed.
- `cargo test -p zephium-private-fs`: 125 tests passed.
- `cargo check -p zephium-private-fs --tests --target x86_64-pc-windows-msvc` passed. Native Windows execution is still needed.
