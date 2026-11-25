# Feat: No command-line option to store app data outside the default directory

People with limited space on their system drive or a portable setup need Zephium's data elsewhere. There is no command-line option to choose a root. Add `--user-data-dir <path>` so all app data and WebView profiles live under that path, including settings, databases, bookmarks, saved app sessions, logs, and cache. Without the flag, keep the existing default.
