#!/usr/bin/env bash
# Reads changed paths, one per line, and prints "true" when the Rust jobs must
# run. Only frame, docs and Markdown changes can skip them, and a frame file
# that Rust compiles in or reads in a test (frame_sources.rs, the generated
# IPC bindings) still counts as a Rust change.
set -euo pipefail
cd "$(dirname "$0")/../.."

while IFS= read -r path; do
  [[ -z "${path}" ]] && continue
  case "${path}" in
    docs/* | *.md) continue ;;
    frame/*)
      if grep -rqF --include='*.rs' "${path}" crates desktop xtask; then
        echo true
        exit 0
      fi
      ;;
    *)
      echo true
      exit 0
      ;;
  esac
done
echo false
