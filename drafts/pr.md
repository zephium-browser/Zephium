# Feat: Share bookmarks across profiles

Hi, thanks for building Zephium. This PR shares bookmarks across profiles in the same data directory while keeping bookmark actions scoped to the authorized profile.

## What changes for the user

Bookmarks appear in every profile using the same data directory. History, cookies, and other website data remain profile-specific.

## How I checked it

- Store migration and cross-profile bookmark-sharing tests.
- Bookmarks panel tests (16 passed).
