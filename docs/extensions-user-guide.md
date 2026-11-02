# Using extensions in Zephium

Open an extension's Chrome Web Store page and choose **Add to Zephium** in the
sidebar. Zephium downloads the original package directly from Google. Review
its access and any compatibility limitations before installing. Installation
does not guarantee that every feature of an extension works on every platform.

Unsupported optional APIs or optional site access can be withheld instead of
blocking installation. The review lists them as unavailable; they cannot be
enabled through permission management. External messaging may also be restricted
to the extension itself, with websites and other extensions unable to connect.
Unsupported required APIs still prevent installation.

Use the extension manager to disable or remove an extension, change optional
permissions, or choose **Check for updates**. Zephium also checks automatically in the background.
An update requesting new required access needs your approval; it cannot silently expand permissions.

If an installed extension is **Not active**, its entry shows the last known
activation problem. Use **Retry activation** for a retryable failure. A capacity
message means another extension must be disabled first; a restart message means
retrying in the same browser session will not help. Activation always rechecks
the current package and permissions.

## Connecting 1Password

To share accounts and unlocking with the 1Password desktop app:

1. Install the signed Zephium application and the 1Password desktop app.
2. Open and unlock 1Password.
3. Open **Settings → Browser**, enable **Connect with 1Password in the browser**,
   and choose **Add Browser** under additional browsers.
4. Select the Zephium application you actually use. A separately named QA or lab
   build has its own application identity; approving one does not necessarily
   approve another.
5. Return to Zephium and reopen the extension. If it still shows a loading
   indicator, fully quit and reopen Zephium after checking the selected app.

See [1Password's complete additional-browser instructions](https://support.1password.com/additional-browsers/)
for macOS, Windows and Linux. On macOS use a properly signed application;
unsigned local development builds cannot establish the same trust.

The modern 1Password browser extension can operate independently of the desktop
app in supported browsers. Zephium testing has confirmed successful use after
desktop setup and continued use after closing the desktop app. Fresh setup in
Zephium without the desktop app remains unqualified. Do not interpret those
different scenarios as equivalent.

Zephium currently discloses limitations such as unavailable HTTP authentication
autofill, idle detection and extension-managed downloads. An active extension
can still have these feature limitations. Browser passkeys also depend on the
platform and signed application's capabilities.

## Native app access

External native applications on macOS require a publisher-verified CRX and the
original `nativeMessaging` permission accepted during install review. The browser's
compatibility transport does not grant that access. Unsigned development packages,
legacy records without reviewed provenance, and optional native-app permissions
without a separate reviewed grant are denied; review a signed install to grant
required access. Replacing or unloading its native context retires the authority.

## Compatibility and resource limits

Dark Reader, Vimium, 1Password and JSON Formatter have real workflow evidence on macOS; other
extensions can use the same supported capabilities, but have not all been tested.
Windows qualification is separate. The browser currently admits up to twelve
extension runtime owners across up to three active extension profiles. Pending
activation and unfinished teardown retain their slots until safely settled.

The native-app broker caps outstanding input at 32 frames and 64 MiB per host
(128 MiB across the browser), and pending replies at eight events / 2 MiB per
host. A full reply inbox pauses its pipe reader until delivery drains the inbox,
so the host is backpressured without being disconnected. Exceeding the outbound
input budget closes the port. Closing a port terminates and reaps its
exact host independently of pipe I/O; physical host slots remain held until reap.
The limit is eight physical hosts per profile/extension and 64 browser-wide.
