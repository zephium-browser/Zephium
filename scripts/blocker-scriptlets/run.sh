#!/bin/zsh
# Builds and runs the built-in scriptlet extension check (macOS 15.4+; skips below).
set -euo pipefail
here=${0:A:h}
out=$(mktemp -d)
xcrun swiftc -parse-as-library -O -module-cache-path "$out/cache" -framework WebKit "$here/spike.swift" -o "$out/spike"
"$out/spike" "$here/extension"
