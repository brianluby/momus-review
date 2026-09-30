#!/usr/bin/env bash
# Install one selected release, or explicitly build this action's source.
set -euo pipefail
: "${RUNNER_TEMP:?}" "${RUNNER_OS:?}" "${RUNNER_ARCH:?}" "${ACTION_PATH:?}" "${GITHUB_PATH:?}"
VERSION=${VERSION:-latest}
VERIFY_ATTESTATIONS=${VERIFY_ATTESTATIONS:-required}
RELEASE_REPO=${RELEASE_REPO:-brianluby/momus-review}
fail() { echo "::error::$*" >&2; exit 1; }
case "$VERIFY_ATTESTATIONS" in required|legacy) ;; *) fail "invalid attestation policy: $VERIFY_ATTESTATIONS" ;; esac
[[ "$RELEASE_REPO" =~ ^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$ ]] || fail "invalid release repository"
root="$RUNNER_TEMP/momus-bin"
mkdir -p "$root/bin"
if [ "$VERSION" = source ]; then
  (cd "$ACTION_PATH" && rustup toolchain install --profile minimal &&
    cargo install --locked --path . --bin momus --root "$root")
else
  case "$RUNNER_OS-$RUNNER_ARCH" in
    Linux-X64) target=x86_64-unknown-linux-gnu ;;
    Linux-ARM64) target=aarch64-unknown-linux-gnu ;;
    macOS-ARM64) target=aarch64-apple-darwin ;;
    *) fail "no prebuilt release for $RUNNER_OS-$RUNNER_ARCH; select version: source explicitly" ;;
  esac
  if [ "$VERSION" = latest ]; then
    release=$(gh api "repos/$RELEASE_REPO/releases/latest")
  else
    [[ "$VERSION" =~ ^[A-Za-z0-9][A-Za-z0-9._-]*$ ]] || fail "invalid release tag"
    release=$(gh api "repos/$RELEASE_REPO/releases/tags/$VERSION")
  fi
  tag=$(jq -er '.tag_name | select(type == "string" and length > 0)' <<<"$release")
  [[ "$tag" =~ ^[A-Za-z0-9][A-Za-z0-9._-]*$ ]] || fail "invalid resolved release tag"
  [ "$VERSION" = latest ] || [ "$tag" = "$VERSION" ] || fail "release tag mismatch"
  jq -e '.draft == false' <<<"$release" >/dev/null || fail "release is a draft"
  dl="$RUNNER_TEMP/momus-dl"
  rm -rf "$dl" && mkdir -p "$dl"
  patterns=(--pattern "momus-$target.tar.gz" --pattern "momus-$target.tar.gz.sha256")
  if [ "$VERIFY_ATTESTATIONS" = required ]; then
    jq -e '.immutable == true' <<<"$release" >/dev/null || fail "required verification needs an immutable release"
    patterns+=(--pattern "momus-$target.cdx.json" --pattern "momus-$target.provenance.bundle.json"
      --pattern "momus-$target.sbom-attestation.bundle.json" --pattern release-manifest.json
      --pattern release-manifest.json.sha256)
    ref=$(gh api "repos/$RELEASE_REPO/git/ref/tags/$tag")
    type=$(jq -er '.object.type' <<<"$ref")
    sha=$(jq -er '.object.sha' <<<"$ref")
    # Peel annotated tags; reject anything that does not resolve to a commit.
    for _ in 1 2 3 4 5; do
      [ "$type" = tag ] || break
      ref=$(gh api "repos/$RELEASE_REPO/git/tags/$sha")
      type=$(jq -er '.object.type' <<<"$ref")
      sha=$(jq -er '.object.sha' <<<"$ref")
    done
    [[ "$type" = commit && "$sha" =~ ^[0-9a-f]{40}$ ]] || fail "release tag does not resolve to a commit"
  fi
  # Any download/verification failure aborts. No implicit source fallback.
  gh release download "$tag" --repo "$RELEASE_REPO" "${patterns[@]}" --dir "$dl"
  archive="momus-$target.tar.gz"
  expected=$(awk 'NR == 1 {print $1}' "$dl/$archive.sha256")
  actual=$(shasum -a 256 "$dl/$archive" | cut -d' ' -f1)
  [[ "$expected" =~ ^[0-9a-f]{64}$ && "$actual" = "$expected" ]] || fail "archive checksum mismatch"
  if [ "$VERIFY_ATTESTATIONS" = required ]; then
    "$ACTION_PATH/scripts/verify-release.sh" --dir "$dl" --repo "$RELEASE_REPO" \
      --signer-workflow "$RELEASE_REPO/.github/workflows/attest.yml" \
      --source-sha "$sha" --source-ref "refs/tags/$tag" --targets "$target"
  else
    echo "::notice::legacy policy: checksum-only verification for $tag"
  fi
  tar -xzf "$dl/$archive" -C "$dl"
  install -m 0755 "$dl/momus-$target/momus" "$root/bin/momus"
fi
echo "$root/bin" >> "$GITHUB_PATH"
"$root/bin/momus" --version
