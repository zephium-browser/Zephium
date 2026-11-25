# Bug: Concurrent starts show false recovery prompts and lose the launcher shortcut

Recovery: On macOS, run `open -na Zephium` twice with the default profile. Start B before A shows its first browser window. B can show a recovery prompt while A is still starting. Expected: A opens normally and B opens without a recovery prompt.

Shortcut: On macOS or Windows, confirm A opens the launcher with the default `CmdOrCtrl+Shift+Space` shortcut, open B, then close A. Press the shortcut while B remains open. It no longer opens the launcher. Expected: the shortcut opens B's launcher.

On macOS, the intended target is 16 concurrent instances using the default profile and data directory.
