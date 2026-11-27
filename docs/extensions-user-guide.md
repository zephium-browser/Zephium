# Using extensions in Zephium

Extensions are available on Windows and macOS. Compatibility depends on the
extension and the platform; successful installation does not guarantee that all
of its features work.

## Install and review access

Open the extension's Chrome Web Store listing and choose **Add to Zephium**.
Zephium acquires the original package from Google. Review the requested access
and compatibility limitations before accepting installation.

Review any reported limitations: declared APIs and features may be unavailable
even when the package can be installed. Package validation and installation
do not turn those unavailable capabilities into supported ones. A recommendation
or catalog entry is not a permission grant or a permanent compatibility guarantee.

## Manage and update

Use the extension manager to enable, disable or remove an extension and change
its site access. Zephium checks Store-installed extensions for updates in the
background. Updates that need additional access wait for review. Extensions
installed from local files or folders are not updated automatically.

An entry marked **Couldn't start** reports its activation problem. Choose
**Retry** to attempt activation again. A capacity problem may require
disabling another extension; a restart requirement cannot be repaired by
repeatedly retrying in the same session. Pending activation and cleanup can keep
resources reserved, and limits differ between platforms.

## Native applications and limitations

On macOS, connecting an extension to a native application requires its
reviewed access and the application's own browser-trust setup. Native messaging
is unavailable on Windows. A separately named development or QA browser has a
distinct identity; trusting it does not
necessarily trust another build. On macOS, unsigned local development builds
do not provide the same identity as a properly signed application.

On macOS, external native-app access requires authenticated package provenance
and reviewed native-messaging authority. Installing an extension or offering a
compatibility transport does not grant that authority. Use the extension's
reported limitations rather than assuming every native API is available.

For an issue, include the extension version, operating system, browser version,
operation attempted and visible error. Report security issues privately through
[SECURITY.md](../SECURITY.md). Engineering ownership and qualification are in
[the extension scope](extension-release-scope.md).
