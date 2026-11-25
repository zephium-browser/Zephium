# Bug: Concurrent starts show false recovery prompts and lose the launcher shortcut

Recovery: Run `open -na` with the default profile, then start a second instance before the first finishes loading. A recovery prompt can appear while the first instance is still starting. Expected: overlapping starts do not count as failed starts.

Shortcut: On macOS or Windows, let instance A register the global launcher shortcut, open instance B, then close A. The shortcut stops working while B remains open. Expected: B takes ownership.

The intended capacity is up to 16 instances using the default profile and data directory.
