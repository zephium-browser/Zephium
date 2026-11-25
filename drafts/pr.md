# Feat: Run multiple Zephium instances

## What changes for the user

`open -na` can launch up to 16 Zephium processes. Startup recovery is coordinated across overlapping launches, global shortcut ownership transfers to a surviving instance, and Windows runtime cleanup reclaims stale per-process generations.

## How I checked it

- Startup recovery tests, 5 shortcut ownership tests, and the desktop suite (439 passed, 2 ignored).
- Mac development build and 16 distinct `open -na` process launches.
- Windows runtime tests compile but still need a native Windows run.
