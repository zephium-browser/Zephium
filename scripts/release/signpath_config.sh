#!/usr/bin/env bash
# All configuration absent means the pending-certificate integration is inactive.
set -euo pipefail
case "${SIGNPATH_RELEASE_MODE:-}" in
  '') ;;
  unsigned)
    if [[ -n "${SIGNPATH_ORGANIZATION_ID:-}" || -n "${SIGNPATH_SIGNER_SUBJECT:-}" || -n "${SIGNPATH_RELEASE_CERT_THUMBPRINT:-}" ]]; then
      echo '::error::Remove unsigned release mode before configuring production SignPath signing.'
      exit 1
    fi
    echo 'enabled=false' >> "${GITHUB_OUTPUT}"
    echo '::notice::Windows is explicitly unsigned while the production certificate is pending; the shared test token is inactive for releases.'
    exit 0
    ;;
  *)
    echo '::error::SIGNPATH_RELEASE_MODE must be unset or unsigned.'
    exit 1
    ;;
esac
configured=0
for name in SIGNPATH_API_TOKEN SIGNPATH_ORGANIZATION_ID SIGNPATH_SIGNER_SUBJECT; do
  [[ -z "${!name:-}" ]] || configured=$((configured + 1))
done
if (( configured != 3 )) && { (( configured != 0 )) || [[ -n "${SIGNPATH_RELEASE_CERT_THUMBPRINT:-}" ]]; }; then
  echo '::error::Complete the SignPath configuration in .github/RELEASE.md or leave all signing values unset.'
  exit 1
fi
if (( configured == 3 )); then
  echo 'enabled=true' >> "${GITHUB_OUTPUT}"
else
  echo 'enabled=false' >> "${GITHUB_OUTPUT}"
  echo '::notice::SignPath is inactive: certificate/setup pending. Windows drafts remain unsigned.'
fi
