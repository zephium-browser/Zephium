# Bug: Concurrent instances trigger false recovery prompts and lose the launcher shortcut

Launching two instances with `open -na` and the default profile can show a recovery prompt while the first instance is still starting. If the instance that registered the global launcher shortcut closes, it stops working while other instances remain open. Expected: no false prompt and shortcut handoff. Support up to 16 concurrent instances.
