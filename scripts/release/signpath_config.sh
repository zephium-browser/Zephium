#!/usr/bin/env bash
# Production releases require signing; absent or partial setup fails closed.
set -euo pipefail
if [[ -z "${SIGNPATH_API_TOKEN:-}" ]]; then
  echo '::error::Set the SignPath CI submitter token before releasing.'
  exit 1
fi
if [[ ! "${SIGNPATH_RELEASE_CERT_THUMBPRINT:-}" =~ ^[A-Fa-f0-9]{40}$ ]]; then
  echo '::error::Set the verified production certificate thumbprint before releasing. See signpath/README.md.'
  exit 1
fi
echo 'enabled=true' >> "${GITHUB_OUTPUT}"
