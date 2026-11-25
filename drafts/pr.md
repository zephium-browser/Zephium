# Feat: Run multiple Zephium instances

## What changes for the user

`open -na` launches independent Zephium processes. Concurrent starts no longer trigger false recovery prompts. On macOS and Windows, a surviving instance takes over the global launcher shortcut when its owner exits. Windows cleanup reclaims stale WebView2 data after recorded browser processes exit, terminating orphans when possible.

## How I checked it

- `cargo test -p zephium-desktop --lib`: 439 passed, 2 ignored, including recovery and lease tests.
- macOS development build with 16 `open -na` processes observed running concurrently against the default profile and data directory.
- `cargo check -p zephium-core --tests --target x86_64-pc-windows-msvc` passed. Windows runtime behavior still needs a native Windows run.
