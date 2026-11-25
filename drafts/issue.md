# Bug: Zephium can't run separate data directories at once

Once `--user-data-dir` is available, could separate Zephium data directories run at the same time?

**Observed:** Both instances start with distinct `--user-data-dir` paths, but the remaining one does not reliably reclaim the global launcher shortcut after its owner exits.

**Expected:** The remaining instance claims the shortcut after its owner exits.
