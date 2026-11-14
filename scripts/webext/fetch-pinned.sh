#!/bin/sh
# Fetch pinned open-source CRXs into the XDG cache for local smoke tests. Never commit packages.
# Usage: fetch-pinned.sh [--extract] [--print-path <id>]
set -eu
here=$(cd "$(dirname "$0")" && pwd)
cache="${XDG_CACHE_HOME:-$HOME/.cache}/zephium/webext-pinned"
extract=0 only=
while [ $# -gt 0 ]; do
  case $1 in
    --extract) extract=1 ;;
    --print-path) shift; only=${1:?id required} ;;
    *) echo "unknown arg: $1" >&2; exit 2 ;;
  esac
  shift
done

py() { python3 -I -c '
import hashlib, io, json, os, sys, tomllib, zipfile
def payload(b):
    assert b[:4] == b"Cr24", "not a CRX"
    v = int.from_bytes(b[4:8], "little")
    if v == 3: return b[12 + int.from_bytes(b[8:12], "little"):]
    assert v == 2, "unsupported CRX version"
    return b[16 + int.from_bytes(b[8:12], "little") + int.from_bytes(b[12:16], "little"):]
cmd, a = sys.argv[1], sys.argv[2:]
if cmd == "list":
    for e in tomllib.load(open(a[0], "rb"))["extension"]:
        print(e["id"], e["version"], e["sha256"])
elif cmd == "sha":
    print(hashlib.sha256(open(a[0], "rb").read()).hexdigest())
elif cmd == "version":
    z = zipfile.ZipFile(io.BytesIO(payload(open(a[0], "rb").read())))
    print(json.loads(z.read("manifest.json"))["version"])
elif cmd == "extract":
    z = zipfile.ZipFile(io.BytesIO(payload(open(a[0], "rb").read())))
    for n in z.namelist():
        assert not n.startswith(("/", "\\")) and ".." not in n.replace("\\", "/").split("/"), "unsafe entry " + n
    z.extractall(a[1])
' "$@"; }

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$cache"
py list "$here/pinned-crx.toml" | while read -r id ver sha; do
  [ -z "$only" ] || [ "$only" = "$id" ] || continue
  crx="$cache/$id-$ver.crx"
  if [ ! -f "$crx" ] || [ "$(py sha "$crx")" != "$sha" ]; then
    curl --fail --silent --show-error --location --max-time 120 --proto =https -o "$tmp/$id.crx" \
      "https://clients2.google.com/service/update2/crx?response=redirect&prodversion=152.0.0.0&acceptformat=crx2,crx3&x=id%3D$id%26uc"
    [ "$(py sha "$tmp/$id.crx")" = "$sha" ] || { echo "sha256 mismatch for $id: $(py sha "$tmp/$id.crx")" >&2; exit 1; }
    [ "$(py version "$tmp/$id.crx")" = "$ver" ] || { echo "version mismatch for $id" >&2; exit 1; }
    mv "$tmp/$id.crx" "$crx"
  fi
  if [ "$extract" = 1 ] && [ ! -f "$cache/$id-$ver/manifest.json" ]; then
    rm -rf "$tmp/x" "$cache/$id-$ver"
    py extract "$crx" "$tmp/x"
    mv "$tmp/x" "$cache/$id-$ver"
  fi
  echo "$crx"
  [ "$extract" != 1 ] || echo "$cache/$id-$ver"
done
