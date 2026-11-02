#!/usr/bin/env bash
# checkout(ref: tag) must agree with the workflow context used by provenance.
set -euo pipefail
: "${TAG:?Release tag is required}"
if [[ "${GITHUB_REF:-}" != "refs/tags/${TAG}" ]]; then
  echo '::error::Run Release from the requested tag ref: gh workflow run release.yml --ref <tag> -f tag=<tag>.'
  exit 1
fi
if [[ "$(git rev-parse HEAD)" != "${GITHUB_SHA:-}" ]]; then
  echo '::error::The checked-out tag differs from the workflow commit. Do not rebuild a moved tag.'
  exit 1
fi
