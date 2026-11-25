# Feat: Choose a custom user data directory

## What changes for the user

`--user-data-dir <path>` routes app data into a private custom directory and keeps separate roots isolated.

## How I checked it

- Custom-root persistence, isolation, invalid paths, and permissions.
- Windows ACL coverage still needs a Windows run.
