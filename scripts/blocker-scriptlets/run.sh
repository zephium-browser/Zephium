#!/bin/zsh
# Runs the built-in scriptlet extension checks in real WebKit (macOS 15.4+; both skip below).
set -euo pipefail
here=${0:A:h}
root=${here:h:h}
out=$(mktemp -d)
trap 'rm -rf "$out"' EXIT
xcrun swiftc -parse-as-library -O -warnings-as-errors -module-cache-path "$out/cache" -framework WebKit "$here/spike.swift" -o "$out/spike"
"$out/spike" "$here/extension"
# From the repository, so cargo uses its pinned toolchain.
(cd "$root" && cargo build --quiet -p zephium-webext-macos --features lab --bin page-script-lab)
"${CARGO_TARGET_DIR:-$root/target}/debug/page-script-lab" "$out/lab"
