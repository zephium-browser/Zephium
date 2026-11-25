# Bug: Concurrent starts show false recovery prompts and lose the launcher shortcut

Recovery: On macOS, run `open -na Zephium` to start A with the default profile. When A's process appears in Activity Monitor but its first browser window has not opened, run the command again to start B. B can show a recovery prompt while A is still starting. Expected: A opens normally and B opens without a recovery prompt.

Shortcut: On macOS, leave the default `Cmd+Shift+Space` shortcut registered in A. On Windows, use `Ctrl+Shift+Space`. Open B, close A, then press the shortcut while B remains open. It no longer opens the launcher. Expected: the shortcut opens B's launcher.

On macOS, launch 16 instances using the same default profile and data directory. All should open without false recovery prompts, and a remaining instance should keep the global launcher shortcut available when its owner closes.
