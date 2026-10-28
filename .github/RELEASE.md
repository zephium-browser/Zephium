# Releasing Zephium

Pushing a `v*` tag runs [`release.yml`](workflows/release.yml). It builds and
verifies macOS (Apple Silicon) and Windows (x64), then creates a **draft**
GitHub release. Nothing is public until a maintainer publishes the draft.

## One-time setup

### Secrets

Add these under **Settings → Secrets and variables → Actions** as repository
secrets. The release gate fails with the names of any that are missing.

| Secret | Value |
| --- | --- |
| `APPLE_CERTIFICATE` | Base64 of the Developer ID Application `.p12` (`base64 -i cert.p12`) |
| `APPLE_CERTIFICATE_PASSWORD` | The `.p12` export password |
| `APPLE_SIGNING_IDENTITY` | `Developer ID Application: <Name> (<TEAMID>)` |
| `APPLE_TEAM_ID` | The 10-character team ID; the workflow checks the app is signed by it |
| `APPLE_API_ISSUER` | App Store Connect API issuer ID (Users and Access → Integrations → Keys) |
| `APPLE_API_KEY` | That API key's key ID |
| `APPLE_API_KEY_PATH` | The **contents** of `AuthKey_<KEYID>.p8`; the workflow writes it to disk |
| `TAURI_SIGNING_PRIVATE_KEY` | The updater private key (file contents) |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | Its password |

The API key needs the Developer role to notarize.

### Updater key

Generate the minisign key pair once and keep the private key and password in a
password manager:

```sh
pnpm --dir desktop tauri signer generate -w ~/.tauri/zephium-updater.key
```

The public key (the `.pub` file contents) is embedded in
`desktop/tauri.conf.json` at `plugins.updater.pubkey`. Installed copies trust
only that key: losing the private key or changing the public key strands every
existing install on its current version.

### Repository

- The repository must be public: build provenance attestations and the
  `releases/latest/download/…` links depend on it.
- Protect `v*` tags with a ruleset so only maintainers can create them.

## Cutting a release

1. **Refresh the blocker seed.** EasyList asks to be refreshed every four days,
   and the release gate rejects a seed past that point, so do this right
   before tagging:

   ```sh
   curl -fsSL -o /tmp/easylist.txt https://easylist.to/easylist/easylist.txt
   curl -fsSL -o /tmp/easyprivacy.txt https://easylist.to/easylist/easyprivacy.txt
   cargo xtask update-blocker-seed --easylist /tmp/easylist.txt \
     --easyprivacy /tmp/easyprivacy.txt \
     --license assets/blocker-seed/v1/LICENSE-CC-BY-SA-3.0.txt
   ```

   Then copy the new WebKit digest into the native seed probe and its policy
   test, which pin it (`scripts/ci/probe_macos_blocker_seed.swift` and
   `scripts/ci/test_macos_blocker_seed_probe.sh`):

   ```sh
   jq -r '.compilers[] | select(.target == "webkit") | .native_artifact_sha256' \
     assets/blocker-seed/v1/compile-report.json
   ```

2. **Bump the version** in `desktop/Cargo.toml` (`[package] version`, e.g.
   `1.0.0-beta.1`). It is the only version source: `tauri.conf.json` has none.
   Run `cargo check -p zephium-desktop` so `Cargo.lock` follows.
3. **Run the release gate locally.** It is the same check the workflow runs
   first: strict engine-floor and advisory reviews, seed freshness, and
   outstanding vendor fixes.

   ```sh
   cargo xtask check-release-engine-security
   ```

4. Merge to `main` with CI green.
5. **Tag and push** the merged commit:

   ```sh
   git tag -a v1.0.0-beta.1 -m "Zephium 1.0.0-beta.1"
   git push origin v1.0.0-beta.1
   ```

6. **Review the draft** once the workflow finishes. For a stable version,
   review the final assets below and use **Publish stable draft**. For a prerelease,
   install the DMG and Windows installer and review the notes before publication. Keep "Set as the
   latest release" checked and leave "Set as a pre-release" unchecked, even for
   betas: `releases/latest` skips pre-releases, and both the website and the
   updater resolve through it.

To rebuild a tag (for example after a flaky runner), select the same tag in
both **Actions → Release → Run workflow → Use workflow from** and the `tag`
input. The gate binds that tag to the workflow commit; CI and both build jobs
then check out the immutable `github.sha`, even if the tag moves later. With the
CLI, pass the tag ref explicitly:

```sh
gh workflow run release.yml --ref v1.0.0-beta.1 -f tag=v1.0.0-beta.1
```

The [CLI defaults to the default branch](https://cli.github.com/manual/gh_workflow_run)
without `--ref`. Release refuses a different workflow ref or checkout commit so
its workflow `head_sha` and provenance describe the candidate's tag commit.
An existing draft for that tag is replaced; a published release is never
touched.

## Draft review and stable publication

Draft creation requires main CI for the exact release commit, both build/sign
jobs, the release security gate and every WebKit component shard. Components use
one retry (`--retry=1`) to tolerate an isolated runner failure.

For an urgent release, dispatch **Release** from the candidate tag with the same
`tag` input and set the boolean **`emergency`** input to `true`. This skips only
components; main CI, security, build and signing checks still gate the draft.
The generated release notes visibly state **EMERGENCY: Component tests were
skipped for this release.** Review that disclosure before publishing.

Install the draft's final DMG and Windows installer and review native behavior,
platform signing, updater signatures and provenance on the shipping platforms.
Hosted checks provide source/API coverage; retain useful device results with the
candidate commit and artifact hashes so a rebuild cannot be mistaken for the
bytes reviewed.

For a stable `vMAJOR.MINOR.PATCH`, dispatch **Publish stable draft** with the
existing annotated tag. It requires successful main-push CI and a successful
Release run from that tag for the same commit. The Release run has already gated
components, or recorded the explicit emergency bypass. The publisher downloads
the existing draft's assets and verifies the DMG, macOS updater archive and
Windows installer attestations against that commit and `release.yml` before
publishing. It never builds or replaces assets. Release and publication share a
same-tag workflow lock, and publication jobs serialize their latest-release check
and edit across tags. A stable draft becomes latest only when its canonical
semantic version is newer than the current latest stable release; equal or older
versions explicitly use `--latest=false`. An absent latest release (HTTP 404)
allows the first stable promotion; other lookup errors or malformed latest tags
stop publication. Maintainers can also publish
reviewed drafts through GitHub's release UI.

## Published assets

The website links `releases/latest/download/<name>`, so these names are a
contract:

| Asset | Purpose |
| --- | --- |
| `Zephium-macOS-arm64.dmg` | macOS download |
| `Zephium-macOS-arm64.app.tar.gz` + `.sig` | macOS updater payload |
| `Zephium-Windows-x64-setup.exe` + `.sig` | Windows download and updater payload |
| `latest.json` | Updater manifest (`darwin-aarch64[-app]`, `windows-x86_64[-nsis]`) |
| `SHA256SUMS` | Digests of every other asset |

Renaming does not invalidate an updater signature. Windows signatures bind the
release version and `windows-x86_64-nsis` target as well as the final bytes;
the updater rejects an unbound or mismatched signature.

## What the workflow verifies

- The tag is `v` + the `zephium-desktop` version, valid semver, and
  `tauri.conf.json` carries no competing version.
- `cargo xtask check-release-engine-security` passes.
- macOS: `codesign --verify --deep --strict`, the signing team is
  `APPLE_TEAM_ID`, Gatekeeper (`spctl`) accepts the app, the app and the DMG
  are notarized and stapled, the main executable's entitlements are exactly
  camera and microphone, and no other Mach-O carries any.
- Both updater signatures verify against the public key in
  `desktop/tauri.conf.json`, and the asset set is exactly the one above.
- Build provenance is attested for the DMG, the updater archive and the
  installer (`gh attestation verify <file> --repo zephium-browser/Zephium`).

The macOS dSYM and Windows PDB are kept as workflow artifacts for 90 days.
Download them for any release you need to symbolicate later.

## Windows signing

The SignPath integration is awaiting test signing and production certificate
issuance. Do not enable this release workflow on the default branch until the
qualification checklist in [Windows signing setup](../signpath/README.md) is
complete. Existing public releases remain unsigned.

The Windows job signs the packaged browser, generated uninstaller, and final
installer in that order. Each production request requires approval in SignPath.
Missing credentials, a missing certificate pin, or failed signature checks stop
the job; there is no unsigned fallback. Updater signing runs **after** all
Authenticode changes. Exactly one unconditional `cargo xtask sign-windows-updater`
step signs the final bytes and binds the version and `windows-x86_64-nsis` target.
No later step may replace this signature.

The Windows job allows 75 minutes for compilation, 85 minutes for each of the
three signing stages, and 30 minutes for setup, verification and uploads (360
minutes total). Each stage waits up to 60 minutes for approval, five minutes
for service availability and three minutes for download. Each stage also allows
ten minutes for the packaging hook to become ready. Its step cap includes
verification and other overhead; timeouts stop the release.

SmartScreen reputation can still affect newly signed downloads.
After publishing, submit `Zephium-Windows-x64-setup.exe` at
<https://www.microsoft.com/wdsi/filesubmission> as a software developer
("Incorrectly detected as malware/malicious" when it is flagged). Keep the
asset name and download URL unchanged between releases, and keep the bundle's
`publisher` at `Zephium`: Windows keys the per-user install by it.

## Recovery

- **Before publishing**, anything can be redone: delete the draft, fix the
  cause, and rerun. If the tagged source was wrong, delete and recreate the
  tag, or simply bump the version.
- **After publishing**, never replace assets, edit binaries, or delete the
  release or tag. Users and the updater may already hold those bytes. Fix
  forward with a higher version (`1.0.0-beta.2`).
- If the updater private key leaks, ship a release whose embedded key is new,
  signed with the old key, and treat it as a security incident.
