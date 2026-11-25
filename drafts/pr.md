# Feat: Run multiple Zephium instances

## What changes for the user

`open -na` can launch up to 16 Zephium processes. Overlapping starts no longer trigger false recovery prompts. If the instance holding the global launcher shortcut exits, another open instance can take ownership. Windows cleanup reclaims stale per-process WebView2 data after an unclean exit when its browser processes can be proven gone.

## How I checked it

- Startup recovery tests and 5 shortcut ownership tests. The desktop suite passed 439 tests with 2 ignored.
- Mac development build and 16 distinct `open -na` process launches.
- Windows runtime tests compile but still need a native Windows run.
