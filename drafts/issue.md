# Feat: Multiple Zephium instances interfere with startup and shortcuts

With the default profile and data directory, launching multiple Zephium instances can trigger conflicting recovery prompts. The global shortcut also stops working when the instance that registered it closes, even if other instances remain open. Support up to 16 concurrent instances with coordinated recovery and shortcut handoff.
