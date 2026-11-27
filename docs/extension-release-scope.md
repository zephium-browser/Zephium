# Extension scope

Extension installation and management are part of ordinary Zephium builds on
Windows and macOS. Linux is not supported. Installing an extension does not
establish that every feature works on both native engines.

The browser accepts a Chrome Web Store listing as an installation request.
Rust authenticates the package, evaluates compatibility and requests permission
review before committing profile-local installation. Listing content and catalog
recommendations are not permission grants. The installation review reports
unsupported capabilities; accepting a package must not imply that every declared
API is implemented. Package integrity and native authority checks remain separate.

Startup restores enabled installations. Store-installed extensions receive
background updates from the Store; local-file/folder installations do not.
Additional required access needs review. Native activation, disable, removal
and update settlement remain separate from acquisition and durable state;
unfinished cleanup must not be treated as released capacity.

See [architecture section 10](architecture.md#10-userscripts-and-extensions) for
implementation ownership and the distinction between extensions and userscript
probes, and the [user guide](extensions-user-guide.md) for management behavior.
Native changes require separate Windows and macOS validation; the
[Windows QA guide](windows-extension-qa.md) and
[Windows lab](../crates/zephium-webext-windows/README.md) describe specialized checks.
