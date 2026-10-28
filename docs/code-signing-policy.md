# Code signing policy

Windows code signing is being configured and tested. The production certificate
is pending; this policy does not imply that previously published Windows builds
are signed. Self-signed test builds are for validation only and are not releases.

Free code signing provided by [SignPath.io](https://signpath.io), certificate by
[SignPath Foundation](https://signpath.org).

## Responsibilities

- Committer and reviewer: [Hayk Injighulyan (@crynta)](https://github.com/crynta),
  the maintainer identified in [CONTRIBUTING.md](../CONTRIBUTING.md) and
  [CODEOWNERS](../CODEOWNERS).
- Signing approver: Hayk Injighulyan. Production requests require manual approval
  in SignPath; the CI submitter does not approve its own requests.

All team members must use multi-factor authentication for GitHub and SignPath.
Changes to the release workflow and signing configuration require maintainer
review. Public releases must be built from the official repository by GitHub
Actions, pass release checks, and satisfy SignPath's origin restrictions.

## Signing scope

The Windows workflow signs the Zephium browser executable, generated NSIS
uninstaller, and final installer. Third-party binaries retain their upstream
signatures or unsigned status; they are not signed as Zephium-owned code.
The installer receives its updater signature only after Authenticode signing is
complete. Published release assets must not be replaced; corrections ship in a
new version.

A valid signature establishes publisher identity and file integrity. It does
not guarantee the absence of vulnerabilities or prevent all SmartScreen prompts.

## Privacy and reporting

The [Zephium privacy policy](https://zephium.app/privacy) covers the browser,
updates, online features, and relevant third-party services. Websites and model
providers selected by the user have their own privacy policies.

Report security concerns through [SECURITY.md](../SECURITY.md).
