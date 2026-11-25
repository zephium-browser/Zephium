# Feat: Concurrent launches can trigger false recovery prompts

With the default profile and data directory, overlapping launches can be counted as repeated failed starts and show recovery prompts while another instance is still starting. The global shortcut also stops working when the instance that registered it closes while other instances remain open. Support up to 16 concurrent instances with coordinated recovery and shortcut handoff.
