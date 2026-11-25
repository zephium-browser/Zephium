# Feat: Select a profile at launch

## What changes for the user

Pass `--profile <name>` to open an existing profile or create it on first use without changing the saved default. Without the flag, Zephium restores the active profile or falls back to the saved default. Unlike [Orion, which keeps each profile in a separate browser with its own Dock icon](https://orionbrowser.com/platforms/macos), Zephium keeps profiles in one app. If it can't create or open the requested profile, Zephium reports the error and exits.

## How it was checked

- `cargo test -p zephium-desktop profile_argument_tests --locked`: 2 tests passed for `--profile NAME`, `--profile=NAME`, and invalid or duplicate arguments.
- `cargo test -p zephium-desktop startup_alert::tests --locked`: 10 startup alert tests passed, including the unavailable-profile message.
