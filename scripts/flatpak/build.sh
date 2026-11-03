#!/usr/bin/env bash
# Builds Zephium as a Flatpak on the GNOME runtime and installs it for the
# current user. Run `pnpm install` first; the frontend is built on the host.
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$root"

flatpak install --user -y --noninteractive flathub \
    org.gnome.Platform//51 org.gnome.Sdk//51 org.freedesktop.Sdk.Extension.rust-stable//26.08

pnpm frame:build

flatpak-builder --user --install --force-clean \
    --state-dir=target/flatpak/state \
    target/flatpak/build scripts/flatpak/app.zephium.Zephium.yml
