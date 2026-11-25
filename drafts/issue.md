# Feat: Zephium can't use a custom data directory

The default data location can be impractical on portable setups or machines with limited disk space. Could Zephium support Chromium's [`--user-data-dir`](https://github.com/chromium/chromium/blob/main/docs/user_data_dir.md) flag on macOS and Windows?

It should keep all browser data, including settings, databases, bookmarks, and sessions, under the chosen path. Relative paths should resolve from the current directory, new roots should use private permissions, and invalid paths or repeated flags should fail. Without the flag, Zephium should keep its current default. Distinct roots should run concurrently, while launches using one root should share an instance.
