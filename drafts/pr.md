# Feat: Select a profile at launch

## What changes for the user

`--profile <name>` starts Zephium with that profile, creating it on first use. The override applies only to this launch and does not change the saved default. If the requested profile cannot be created or opened, startup reports the failure and exits instead of opening another profile.

## How I checked it

- Existing and first-use selection, invalid arguments, and saved-default preservation tests.
- `cargo test -p zephium-desktop startup_alert::tests --locked` (10 passed).
