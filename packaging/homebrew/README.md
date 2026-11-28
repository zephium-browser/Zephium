# Homebrew cask for Zephium

`Casks/zephium.rb` is the source of truth for the Zephium Homebrew cask. It
mirrors the file of the same name in the project's tap repository,
`zephium-browser/homebrew-tap`, so the two must always be updated together.

## Install

The published releases ship an Apple Silicon (arm64) DMG only, so the cask
refuses Intel machines:

The project tap repository (`zephium-browser/homebrew-tap`) is not public
yet, so the tap command below is pending publication. Until it exists, install
the cask directly from a checkout of this repository:

```sh
brew install --cask ./packaging/homebrew/Casks/zephium.rb
```

Once the tap is public, the published form will be:

```sh
brew tap zephium-browser/tap https://github.com/zephium-browser/homebrew-tap
brew install --cask zephium-browser/tap/zephium
```

`brew upgrade --cask zephium-browser/tap/zephium` picks up a new version once
the tap is updated. `brew uninstall --cask zephium-browser/tap/zephium` removes
the app only. To also remove Zephium's support, container, cache, preference,
saved-state and WebKit directories through the `zap` stanza, use
`brew uninstall --zap --cask zephium-browser/tap/zephium` (or the local cask
path while the tap is pending) instead.

## Keeping version and sha256 in sync

`version` is the release tag without the leading `v`, and `sha256` is the
digest of that release's `Zephium-macOS-arm64.dmg`. The release workflow
publishes both in `SHA256SUMS` next to the DMG. After a release, update the
cask in one commit:

```sh
version=1.0.5
curl -fsSL \
  "https://github.com/zephium-browser/Zephium/releases/download/v${version}/SHA256SUMS" \
  | awk '$2 == "Zephium-macOS-arm64.dmg" { print $1 }'
```

Copy that value into `sha256` and set `version` to the same release version.
The `url` interpolates `version`, so no other edit is needed. Never bump one
without the other: Homebrew refuses to install when the downloaded bytes do
not match the declared checksum.

Before sending a tap update, check the cask's syntax with `ruby -c
Casks/zephium.rb` and confirm the checksum against `SHA256SUMS` from the
release. The tap repository's CI performs the same check.
