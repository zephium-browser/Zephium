# Feat: Select a profile at launch

## What changes for the user

`--profile <name>` selects or creates the profile for that launch. Startup failures show an error and do not open a different profile.

## How I checked it

- Existing and first-use profile selection, invalid arguments, and unchanged default profile.
- Startup alert tests.
