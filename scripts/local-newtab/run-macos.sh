#!/bin/sh
# Standalone native experiment; never opens an installed browser or personal profile.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
mkdir -p "$root/target"
probe_root=$(mktemp -d "$root/target/local-newtab-probe.XXXXXX")
trap 'rm -rf "$probe_root"' EXIT HUP INT TERM
xcrun swiftc -parse-as-library -O -warnings-as-errors \
  -module-cache-path "$probe_root/module-cache" \
  -framework WebKit -framework AppKit \
  "$root/scripts/local-newtab/macos.swift" -o "$probe_root/probe"
cd "$root"
"$probe_root/probe"
