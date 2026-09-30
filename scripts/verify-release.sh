#!/usr/bin/env bash
# Verify a momus release download end-to-end: checksums, signed SLSA
# provenance for every archive and its SBOM, the SBOM attestation with its
# predicate matched byte-for-byte (canonical JSON) against the published
# SBOM, and — on macOS — Developer ID signing, hardened runtime, secure
# timestamp and Apple notarization of the apple-target binary.
#
# A checksum alone authenticates nothing the publisher cannot forge; the
# attestations are signed by GitHub's OIDC-issued short-lived certificate,
# so this script also pins:
#   - the repository the artifact is linked to      (--repo)
#   - the exact source commit it was built from     (--source-sha)
#   - the signer workflow path                       (--signer-workflow)
#   - the signer workflow's commit                   (--signer-digest)
#   - GitHub-hosted runners only                     (--deny-self-hosted-runners)
#
# --source-sha must come from OUTSIDE the release being verified (the tag
# the consumer selected, resolved independently, or GITHUB_SHA in CI), never
# from release-manifest.json, which is release content itself.
#
# Requires: gh >= 2.66 (attestation policy flags), jq, python3, shasum.
# Needs network access to GitHub and Sigstore's public-good instance.
#
# Usage:
#   scripts/verify-release.sh --dir DIST --source-sha SHA [options]
#     --dir DIR              directory holding the downloaded assets
#     --source-sha SHA       required expected source commit (plain hex)
#     --repo OWNER/REPO      default brianluby/momus-review
#     --signer-workflow P    default <repo>/.github/workflows/builder.yml
#     --signer-digest SHA    optional. The builder workflow's pinned commit
#                            (recorded in the release manifest). Omitted by
#                            consumers who cannot know it independently;
#                            release-time verification always passes it.
#     --apple-team-id ID     default DVH6X33J83
#     --targets A,B,C        subset to verify (default: all three)
#     --skip-codesign        skip the macOS codesign/notarization checks
#                            (they only run on darwin hosts anyway); CI
#                            jobs that skip them must say why
set -euo pipefail

ALL_TARGETS="x86_64-unknown-linux-gnu,aarch64-unknown-linux-gnu,aarch64-apple-darwin"
REPO="brianluby/momus-review"
SIGNER_WORKFLOW="brianluby/momus-review/.github/workflows/builder.yml"
APPLE_TEAM_ID="DVH6X33J83"
SOURCE_SHA=""
SIGNER_DIGEST=""
TARGETS="$ALL_TARGETS"
SKIP_CODESIGN=0
DIR=""

usage() { grep '^#' "$0" | sed 's/^# \{0,1\}//'; exit 2; }
need() { [ -n "$2" ] || { echo "verify-release.sh: missing --$1" >&2; usage; }; }

while [ $# -gt 0 ]; do
  case "$1" in
    --dir) DIR=$2; shift 2 ;;
    --repo) REPO=$2; shift 2 ;;
    --source-sha) SOURCE_SHA=$2; shift 2 ;;
    --signer-workflow) SIGNER_WORKFLOW=$2; shift 2 ;;
    --signer-digest) SIGNER_DIGEST=$2; shift 2 ;;
    --apple-team-id) APPLE_TEAM_ID=$2; shift 2 ;;
    --targets) TARGETS=$2; shift 2 ;;
    --skip-codesign) SKIP_CODESIGN=1; shift ;;
    -h|--help) usage ;;
    *) echo "verify-release.sh: unknown argument: $1" >&2; usage ;;
  esac
done
need dir "$DIR"
need source-sha "$SOURCE_SHA"
# An array so the digest flag is simply absent when not pinned.
digest_args=()
[ -n "$SIGNER_DIGEST" ] && digest_args=(--signer-digest "$SIGNER_DIGEST")
for cmd in gh jq python3; do
  command -v "$cmd" >/dev/null 2>&1 || { echo "verify-release.sh: needs $cmd on PATH" >&2; exit 2; }
done
gh_version=$(gh --version | head -n1 | sed 's/gh version \([0-9]*\)\..*/\1/')
[ "$gh_version" -ge 2 ] || { echo "verify-release.sh: gh too old for attestation policy flags (need >= 2.66)" >&2; exit 2; }
# shellcheck disable=SC2034  # POSIX shasum vs GNU sha256sum
sha256_file() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | cut -d' ' -f1
  else shasum -a 256 "$1" | cut -d' ' -f1; fi
}

fail() { echo "FAIL $1" >&2; FAILED=1; }
FAILED=0

# ---- Checksums first: every archive and the release manifest -------------
cd "$DIR"
for target in ${TARGETS//,/ }; do
  archive="momus-$target.tar.gz"
  checksum="$archive.sha256"
  sbom="momus-$target.cdx.json"
  provenance="momus-$target.provenance.bundle"
  sbom_att="momus-$target.sbom-attestation.bundle"
  for f in "$archive" "$checksum" "$sbom" "$provenance" "$sbom_att"; do
    [ -f "$f" ] || { fail "$f: missing (target $target)"; continue; }
  done
  [ -f "$archive" ] || continue
  expected=$(awk '{print $1}' "$checksum" 2>/dev/null) || expected=""
  if [ -z "$expected" ]; then
    fail "$checksum: unreadable or empty"
  else
    actual=$(sha256_file "$archive")
    [ "$actual" = "$expected" ] || fail "$archive: checksum mismatch (want $expected, got $actual)"
  fi

  # ---- Provenance: the archive AND its SBOM are subjects ----------------
  for subject in "$archive" "$sbom"; do
    if ! gh attestation verify "$subject" \
        --bundle "$provenance" \
        --repo "$REPO" \
        --predicate-type "https://slsa.dev/provenance/v1" \
        --signer-workflow "$SIGNER_WORKFLOW" \
        "${digest_args[@]}" \
        --source-digest "$SOURCE_SHA" \
        --deny-self-hosted-runners >/dev/null; then
      fail "$subject: SLSA provenance verification failed ($provenance)"
      continue
    fi
    echo "ok $subject: provenance verified"
  done

  # ---- SBOM attestation: predicate must equal the published SBOM bytes --
  if gh_json=$(gh attestation verify "$archive" \
      --bundle "$sbom_att" \
      --repo "$REPO" \
      --predicate-type "https://cyclonedx.org/bom" \
      --signer-workflow "$SIGNER_WORKFLOW" \
      "${digest_args[@]}" \
      --source-digest "$SOURCE_SHA" \
      --deny-self-hosted-runners --format=json); then
    if ! diff <(printf '%s' "$gh_json" | jq -Sjc '.[0].verificationResult.statement.predicate') \
              <(jq -Sjc . "$sbom"); then
      fail "$sbom: SBOM attestation predicate does not match the published SBOM"
    else
      echo "ok $sbom: attestation predicate matches published bytes"
    fi
  else
    fail "$archive: SBOM attestation verification failed ($sbom_att)"
  fi

  # ---- macOS: Developer ID, hardened runtime, timestamp, notarization ---
  if [ "$target" = aarch64-apple-darwin ] && [ "$SKIP_CODESIGN" -ne 1 ]; then
    if [ "$(uname -s)" != Darwin ]; then
      echo "notice: skipping codesign checks for $archive (not on macOS); provenance and checksums still verified"
    else
      extract=$(mktemp -d)
      tar -xzf "$archive" -C "$extract"
      exe="$extract/momus-$target/momus"
      display=$(codesign -dv --verbose=4 "$exe" 2>&1 || true)
      codesign --verify --strict "$exe" || fail "$exe: codesign strict verification failed"
      grep -q "TeamIdentifier=$APPLE_TEAM_ID" <<<"$display" \
        || fail "$exe: not signed by team $APPLE_TEAM_ID"
      grep -Eq 'flags=0x[0-9a-f]*\(.*runtime' <<<"$display" \
        || fail "$exe: hardened runtime not enabled"
      grep -q '^Timestamp=' <<<"$display" \
        || fail "$exe: no secure timestamp in signature"
      if codesign --verify --strict --check-notarization -R=notarized "$exe"; then
        echo "ok $exe: signed, hardened, timestamped, notarized"
      else
        fail "$exe: Apple notarization check failed"
      fi
      rm -rf "$extract"
    fi
  fi
done

# ---- Release manifest: its own checksum, then recorded digests vs bytes --
if [ -f release-manifest.json ]; then
  if [ -f release-manifest.json.sha256 ]; then
    expected_manifest=$(awk '{print $1}' release-manifest.json.sha256)
    actual_manifest=$(sha256_file release-manifest.json)
    [ "$actual_manifest" = "$expected_manifest" ] \
      || fail "release-manifest.json: checksum mismatch"
  else
    fail "release-manifest.json.sha256: missing"
  fi
  # One argument per target: a quoted "${TARGETS//,/ }" would pass a single
  # space-joined string and no manifest key would ever match it.
  IFS=, read -r -a wanted_targets <<< "$TARGETS"
  python3 - "$PWD" "${wanted_targets[@]}" <<'PYTHON' || fail "release-manifest.json: recorded digests do not match files"
import hashlib, json, sys
root, wanted = sys.argv[1], set(sys.argv[2:])
manifest = json.load(open(f"{root}/release-manifest.json"))
bad = []
for target in sorted(wanted):
    entry = manifest.get("targets", {}).get(target)
    if entry is None:
        bad.append(f"{target}: not recorded in the release manifest")
        continue
    for kind in ("archive", "sbom"):
        recorded = entry.get(kind, {}).get("sha256")
        name = entry.get(kind, {}).get("name")
        if not (recorded and name):
            bad.append(f"{target}.{kind}: missing digest or name")
            continue
        path = f"{root}/{name}"
        try:
            actual = hashlib.sha256(open(path, "rb").read()).hexdigest()
        except OSError:
            bad.append(f"{name}: not found")
            continue
        if actual != recorded:
            bad.append(f"{name}: manifest says {recorded}, file is {actual}")
if bad:
    print("\n".join(bad), file=sys.stderr)
    sys.exit(1)
print("ok release-manifest.json: recorded digests match")
PYTHON
else
  fail "release-manifest.json: missing"
fi

[ "$FAILED" -eq 0 ] || { echo "verify-release.sh: FAILED" >&2; exit 1; }
echo "verify-release.sh: all checks passed"
