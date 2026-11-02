#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
probe="${root}/scripts/ci/probe_macos_blocker_seed.swift"
workflow="${root}/.github/workflows/ci.yml"
engine_policy="${root}/crates/zephium-engine/src/host/content_rules.rs"

test -f "${probe}"
test ! -L "${probe}"

for invariant in \
  'private let maximumArtifactBytes: Int64 = 32 * 1024 * 1024' \
  'private let nativeDeadlineSeconds: TimeInterval = 170' \
  'private let productionColdCompileBudgetSeconds: TimeInterval = 15' \
  'O_RDONLY | O_CLOEXEC | O_NOFOLLOW | O_NONBLOCK' \
  '(before.st_mode & S_IFMT) == S_IFREG' \
  'before.st_nlink == 1' \
  'blocker artifact changed while it was being admitted' \
  'private let identifierPrefix = "app.zephium.rules.v1."' \
  'private let artifactDigestDomain = Data("zephium-webkit-content-rules".utf8)' \
  'private let artifactFormatVersion: UInt32 = 4' \
  '"93f6d4c6233b2e72fcf71cb554ed625fdaf25ac48e464f28c7b69a76f36b6ed5"' \
  'guard digest == expectedReleaseArtifactDigest else' \
  'let store = WKContentRuleListStore(url: cacheURL)' \
  'store.compileContentRuleList(' \
  'store.lookUpContentRuleList(forIdentifier: expectedIdentifier)' \
  'coldCompileSeconds <= productionColdCompileBudgetSeconds' \
  'native WKContentRuleListStore timings: cold_compile_seconds=' \
  'WebKit delivered a duplicate or out-of-order blocker callback' \
  'template.withUnsafeMutableBufferPointer' \
  'mkdtemp(baseAddress)' \
  'chmod(path, S_IRWXU)'
do
  grep -Fq "${invariant}" "${probe}"
done

if grep -Eq 'https?://|URLSession|NSURLConnection' "${probe}"; then
  echo "macOS blocker seed probe must remain offline" >&2
  exit 1
fi

step="$(
  awk '
    /^      - name: Prove the exact bundled blocker seed compiles in native WKContentRuleListStore$/ {
      capture = 1
    }
    capture && seen && /^      - name:/ {
      exit
    }
    capture {
      print
      seen = 1
    }
  ' "${workflow}"
)"
test -n "${step}"
grep -Fq 'cargo xtask materialize-blocker-seed-webkit --output "${artifact}"' <<<"${step}"
grep -Fq 'scripts/ci/probe_macos_blocker_seed.swift' <<<"${step}"
grep -Fq '"${probe}" --self-test' <<<"${step}"
grep -Fq '"${probe}" "${artifact}"' <<<"${step}"
grep -Fq -- '-warnings-as-errors' <<<"${step}"

test "$(
  grep -Fc 'cargo xtask materialize-blocker-seed-webkit --output "${artifact}"' "${workflow}"
)" -eq 1
test "$(grep -Fc 'scripts/ci/probe_macos_blocker_seed.swift' "${workflow}")" -eq 1
test "$(
  grep -Fc \
    'const DECLARATIVE_CONTENT_POLICY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);' \
    "${engine_policy}"
)" -eq 1

echo "macOS native blocker seed probe policy verified"
