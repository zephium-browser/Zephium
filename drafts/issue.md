# Bug: Concurrent starts show false recovery prompts and lose the launcher shortcut

Recovery: On macOS, start A with `open -na Zephium` using the default profile and data directory. When A appears in Activity Monitor but its first browser window has not opened, run the same command for B. B can show a recovery prompt while A is still starting. Expected: both instances open without a false recovery prompt.

Shortcut: On macOS, confirm A opens the launcher with `Cmd+Shift+Space`. On Windows, use `Ctrl+Shift+Space`. Start B with the same profile, close A, then press the shortcut. It no longer opens the launcher. Expected: B opens its launcher.

Acceptance: On macOS, launch 16 instances using the same default profile and data directory. All should remain open without false recovery prompts. Closing the shortcut-owning instance should leave the shortcut working in a remaining instance.
