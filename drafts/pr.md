# Feat: Choose a custom user data directory

## What changes for the user

`zephium --user-data-dir <path>` routes app persistence and WebView profiles beneath one selected root, including app settings, databases, bookmarks, saved app sessions, logs, and cache. Cache and logs use subdirectories under the root. Relative paths resolve from the current directory. Without the flag, Zephium uses its existing default. New roots are created privately. Existing roots must pass Unix owner/mode/ancestor checks or Windows ACL/path checks.

## How I checked it

- `cargo test -p zephium-desktop --lib data_root::tests`: 6 unit tests passed for parsing and root resolution.
- `cargo test -p zephium-private-fs`: 125 tests passed.
- `cargo check -p zephium-private-fs --tests --target x86_64-pc-windows-msvc` passed. Native Windows execution is still needed.
