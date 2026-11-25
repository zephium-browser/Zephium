# Bug: Concurrent starts show false recovery prompts and lose the launcher shortcut

Recovery: On macOS, run `open -na Zephium` twice with the default profile. Start B before A opens its first window. B can show a recovery prompt while A is still starting. Expected: B opens without a recovery prompt.

Shortcut: On macOS or Windows, leave the default `CmdOrCtrl+Shift+Space` shortcut registered in A, open B, then close A. Press the shortcut while B remains open. It no longer opens the launcher. Expected: B takes ownership.

The intended capacity is up to 16 concurrent instances using the default profile and data directory.
