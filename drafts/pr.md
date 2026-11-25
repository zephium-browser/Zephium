# Feat: Share bookmarks across profiles in one data directory

Hi, thanks for building Zephium! I appreciate the work on it. This PR shares bookmarks across profiles in one data directory while keeping bookmark actions scoped to the authorized profile.

## What changes for the user

Bookmarks appear in every profile using the same data directory. Bookmark actions still require authorization for the active profile. History, cookies, and other website data remain profile-specific.

## How I checked it

- Store bookmark tests: 17 passed, covering one-time migration, cross-profile visibility, and isolated history.
- BookmarksPanel WebKit component suite: 16 passed.
