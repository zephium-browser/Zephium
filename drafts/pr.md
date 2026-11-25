# Feat: Select a profile at launch

## What changes for the user

`--profile <name>` starts Zephium with that profile, creating it on first use. The override applies only to this launch and does not change the saved default. If the requested profile cannot be created or opened, startup reports the failure and exits instead of opening another profile.

## How I checked it

- Existing and first-use profile selection, invalid arguments, and unchanged default profile.
- Startup alert tests.
