#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."
node --test eval/webext-conformance/*.test.mjs
timeout -k 10 420 node eval/webext-conformance/run.mjs --check "$@"
