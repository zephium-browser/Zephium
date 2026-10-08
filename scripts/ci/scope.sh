#!/usr/bin/env bash
# Reads a pull request's changed paths, one per line, and prints which CI
# jobs they need as rust=, frontend= and workflows= lines. Docs and
# repository metadata need none. A frame file that Rust compiles in or reads
# in a test (frame_sources.rs, the generated IPC bindings) still needs the
# Rust jobs, and a change to CI itself runs everything.
set -euo pipefail
cd "$(dirname "$0")/../.."

rust=false
frontend=false
workflows=false
while IFS= read -r path; do
  [[ -z "${path}" ]] && continue
  case "${path}" in
    docs/* | *.md | .coderabbit.yaml | .github/ISSUE_TEMPLATE/* | CODEOWNERS | LICENSE*) ;;
    frame/*)
      frontend=true
      if grep -rqF --include='*.rs' "${path}" crates desktop xtask; then
        rust=true
      fi
      ;;
    package.json | pnpm-lock.yaml | pnpm-workspace.yaml | .node-version | patches/*)
      frontend=true
      ;;
    crates/* | xtask/* | vendor/* | desktop/src/* | desktop/windows-log-tests/* | \
      Cargo.toml | Cargo.lock | rust-toolchain.toml | deny.toml | .cargo/* | .config/*)
      rust=true
      ;;
    .github/* | scripts/ci/*)
      rust=true
      frontend=true
      workflows=true
      ;;
    *)
      rust=true
      frontend=true
      ;;
  esac
done
echo "rust=${rust}"
echo "frontend=${frontend}"
echo "workflows=${workflows}"
