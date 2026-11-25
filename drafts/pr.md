# Feat: Run multiple Zephium instances

## What changes for the user

`open -na` launches independent Zephium processes. Starting another instance during startup no longer shows a false recovery prompt. If the instance that registered the global launcher shortcut closes, a remaining instance takes it over so the configured shortcut still opens the launcher. Windows cleanup reclaims stale WebView2 data after recorded browser processes exit, terminating orphans when possible.

## How I checked it

- `cargo test -p zephium-desktop --lib`: 439 passed, 2 ignored, including recovery and lease tests.
- Manual macOS check: development build with 16 `open -na` processes observed running concurrently against the default profile and data directory.
- `cargo check -p zephium-core --tests --target x86_64-pc-windows-msvc` passed. Windows runtime behavior still needs a native Windows run.
