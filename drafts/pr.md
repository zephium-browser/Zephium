# Feat: Run multiple Zephium instances

## What changes for the user

`open -na` can keep up to 16 Zephium processes running against the default profile and data directory. Concurrent starts no longer trigger false recovery prompts. On macOS and Windows, a surviving instance takes over the global launcher shortcut when its owner exits. Windows cleanup reclaims stale WebView2 data after recorded browser processes exit, terminating orphans when possible.

## How I checked it

- `cargo test -p zephium-desktop --lib`: 439 passed, 2 ignored, including recovery and lease tests.
- Mac development build and 16 `open -na` processes running concurrently.
- Windows runtime tests compile but still need a native Windows run.
