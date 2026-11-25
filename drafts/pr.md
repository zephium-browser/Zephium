# Feat: Choose a custom user data directory

## What changes for the user

`zephium --user-data-dir <path>` stores settings, databases, bookmarks, sessions, logs, cache, and browser data under the selected private root across launches. Separate paths keep their data isolated. Without the flag, Zephium uses its existing default directory.

## How I checked it

- `cargo test -p zephium-desktop --lib data_root::tests` (6 passed).
- `cargo test -p zephium-private-fs` (125 passed).
- Windows private-fs tests compile for Windows. Native Windows execution is still needed.
