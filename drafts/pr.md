# Feat: Choose a custom user data directory

## What changes for the user

`zephium --user-data-dir <path>` stores all app data under the selected private directory and reuses that root across launches. Separate paths keep their app data isolated. Without the flag, Zephium uses its existing default directory.

## How I checked it

- Custom-root persistence, isolation, invalid paths, Unix ancestor checks, and root permissions.
- Windows private-fs tests compile for Windows but still need a native Windows run.
