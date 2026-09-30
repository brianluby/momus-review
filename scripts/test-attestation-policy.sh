#!/usr/bin/env bash
# Real cryptographic rejection checks, using the same signed rehearsal bytes.
set -euo pipefail
: "${GITHUB_REPOSITORY:?}" "${GITHUB_SHA:?}" "${GITHUB_REF:?}"
dir=${1:?asset directory}
signer=${2:?signer digest}
archive="$dir/momus-x86_64-unknown-linux-gnu.tar.gz"
bundle="$dir/momus-x86_64-unknown-linux-gnu.provenance.bundle.json"
common=("$archive" --bundle "$bundle" --repo "$GITHUB_REPOSITORY"
  --source-digest "$GITHUB_SHA" --source-ref "$GITHUB_REF"
  --signer-workflow "$GITHUB_REPOSITORY/.github/workflows/attest.yml"
  --signer-digest "$signer" --deny-self-hosted-runners)
# Positive control prevents missing/broken input from satisfying negative cases.
gh attestation verify "${common[@]}" >/dev/null
reject() {
  if gh attestation verify "${common[@]}" "$@" >/dev/null 2>&1; then
    echo "::error::attestation accepted invalid policy: $*" >&2
    exit 1
  fi
  echo "ok rejected $1"
}
reject --source-digest 0000000000000000000000000000000000000000
reject --source-ref refs/heads/not-the-release-source
reject --signer-workflow "$GITHUB_REPOSITORY/.github/workflows/not-the-signer.yml"
reject --signer-digest 0000000000000000000000000000000000000000
reject --repo octocat/not-the-release-repository
reject --predicate-type https://example.invalid/not-the-predicate
