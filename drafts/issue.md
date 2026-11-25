# Bug: Concurrent launches can trigger false recovery prompts

Launching two or more instances with `open -na` and the default profile can show recovery prompts while another instance is still starting. If the instance that registered the global launcher shortcut closes, the shortcut stops working while other instances remain open. Support up to 16 concurrent instances with coordinated startup recovery and shortcut handoff.
