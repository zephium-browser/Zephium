# Feat: No command-line option to store app data outside the default directory

People who need Zephium's data on a separate volume cannot choose its location from the command line. Add `--user-data-dir <path>` so all persistent app data, including settings, databases, bookmarks, sessions, logs, cache, and WebView profiles, lives under `<path>` in app-managed subfolders. Without the flag, Zephium should keep using its existing default.
