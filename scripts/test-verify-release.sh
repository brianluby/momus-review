#!/usr/bin/env bash
# Offline failure-mode tests for scripts/verify-release.sh.
#
# `gh` is replaced by a stub that records its arguments and returns a fixed
# verification result, so these tests exercise OUR enforcement surface:
# checksum comparison, subject/bundle wiring, SBOM predicate matching,
# manifest digest matching, missing-asset detection, and the exact policy
# flags handed to gh (repository, source digest, signer workflow and
# digest, predicate type, hosted runners). The cryptographic verification
# behind those flags is gh + Sigstore's; it is exercised for real by the
# release workflow's verify job and rehearsal.
#
# Covered failure modes (each must block): tampered archive, altered SBOM,
# missing bundle, missing target, manifest digest mismatch, and passing
# wrong source/signer expectations onward. Signing failure and conflicting
# rerun are enforced by the workflow graph (needs: sign-macos, and the
# publish job's release-exists check).
set -uo pipefail

here=$(cd "$(dirname "$0")" && pwd)
verify="$here/verify-release.sh"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
bin="$work/bin"
mkdir -p "$bin"

cat > "$bin/gh" <<'STUB'
#!/usr/bin/env bash
if [ "$1" = "--version" ]; then
  echo "gh version 2.101.0 (test stub)"
  exit 0
fi
printf '%s\n' "$*" >> "$GH_ARGS_LOG"
# Real gh rejects bundles unless the filename ends in .json or .jsonl.
# Enforce the same file contract instead of accepting every fixture path.
while [ "$#" -gt 0 ]; do
  case "$1" in
    --source-digest) [ "$2" = 0000000000000000000000000000000000000001 ] || exit 1 ;;
    --source-ref) [ "$2" = refs/heads/main ] || exit 1 ;;
    --signer-workflow) [ "$2" = brianluby/momus-review/.github/workflows/attest.yml ] || exit 1 ;;
    --signer-digest) [ "$2" = 0000000000000000000000000000000000000001 ] || exit 1 ;;
    --repo) [ "$2" = brianluby/momus-review ] || exit 1 ;;
    --predicate-type)
      case "$2" in https://slsa.dev/provenance/v1|https://cyclonedx.org/bom) ;; *) exit 1 ;; esac ;;
  esac
  if [ "$1" = --bundle ]; then
    case "$2" in
      *.json|*.jsonl) ;;
      *) echo "bundle file extension not supported, must be json or jsonl" >&2; exit 1 ;;
    esac
    shift
  fi
  shift
done
cat "$GH_STUB_OUT"
exit 0
STUB
chmod +x "$bin/gh"
export PATH="$bin:$PATH"
GH_ARGS_LOG="$work/gh-args.log"; export GH_ARGS_LOG
GH_STUB_OUT="$work/gh-out.json"; export GH_STUB_OUT
: > "$GH_ARGS_LOG"

TARGET=x86_64-unknown-linux-gnu
SHA=0000000000000000000000000000000000000001
SIGNER="brianluby/momus-review/.github/workflows/attest.yml"

make_fixture() {
  local dir=$1
  rm -rf "$dir"
  mkdir -p "$dir/pkg"
  echo '#!/bin/sh' > "$dir/pkg/momus"
  tar -C "$dir" -czf "$dir/momus-$TARGET.tar.gz" pkg
  (cd "$dir" && if command -v sha256sum >/dev/null 2>&1; then
      sha256sum "momus-$TARGET.tar.gz" > "momus-$TARGET.tar.gz.sha256"
    else
      shasum -a 256 "momus-$TARGET.tar.gz" > "momus-$TARGET.tar.gz.sha256"
    fi)
  rm -rf "$dir/pkg"
  cat > "$dir/momus-$TARGET.cdx.json" <<'EOF'
{"bomFormat":"CycloneDX","specVersion":"1.5","version":1,"components":[{"type":"library","name":"serde","version":"1.0.219"}]}
EOF
  echo '{"mediaType":"application/vnd.dsse.envelope.v1+json"}' > "$dir/momus-$TARGET.provenance.bundle.json"
  echo '{"mediaType":"application/vnd.dsse.envelope.v1+json"}' > "$dir/momus-$TARGET.sbom-attestation.bundle.json"
  python3 - "$dir" "$TARGET" <<'PY'
import hashlib, json, sys
root, target = sys.argv[1], sys.argv[2]
def digest(name):
    return hashlib.sha256(open(f"{root}/{name}", "rb").read()).hexdigest()
manifest = {"version": "9.9.9", "targets": {target: {
    "archive": {"name": f"momus-{target}.tar.gz", "sha256": digest(f"momus-{target}.tar.gz")},
    "sbom": {"name": f"momus-{target}.cdx.json", "sha256": digest(f"momus-{target}.cdx.json")},
}}}
open(f"{root}/release-manifest.json", "w").write(json.dumps(manifest, indent=2))
PY
  # The stub's verification result embeds the SBOM as its predicate, the
  # same canonicalization verify-release.sh applies to the published file.
  jq -Sjc '[{verificationResult:{statement:{predicate:.}}}]' "$dir/momus-$TARGET.cdx.json" > "$GH_STUB_OUT"
  (cd "$dir" && if command -v sha256sum >/dev/null 2>&1; then
      sha256sum release-manifest.json > release-manifest.json.sha256
    else
      shasum -a 256 release-manifest.json > release-manifest.json.sha256
    fi)
}

run_verify() { # dir [extra args...]
  local dir=$1; shift
  "$verify" --dir "$dir" --source-sha "$SHA" --signer-workflow "$SIGNER" \
    --targets "$TARGET" "$@" >"$work/out.txt" 2>&1
}

expect_pass() {
  local name=$1 dir=$2
  if run_verify "$dir"; then
    echo "ok   $name"
  else
    echo "FAIL $name: expected success, got failure"; sed 's/^/     /' "$work/out.txt"; FAILED=1
  fi
}

expect_fail() {
  local name=$1 dir=$2 want=$3
  if run_verify "$dir"; then
    echo "FAIL $name: expected failure, got success"; FAILED=1
  elif grep -q "$want" "$work/out.txt"; then
    echo "ok   $name (blocked: $want)"
  else
    echo "FAIL $name: failed without the expected message '$want'"; sed 's/^/     /' "$work/out.txt"; FAILED=1
  fi
}

FAILED=0

# 1. Good path: everything verifies.
make_fixture "$work/good"
expect_pass "good fixtures" "$work/good"

# 2. The policy flags we must hand to gh on every attestation check.
gh_flags_ok=1
grep -q -- "--repo brianluby/momus-review" "$GH_ARGS_LOG" || gh_flags_ok=0
grep -q -- "--source-digest $SHA" "$GH_ARGS_LOG" || gh_flags_ok=0
grep -q -- "--signer-workflow $SIGNER" "$GH_ARGS_LOG" || gh_flags_ok=0
if grep -q -- "--signer-digest" "$GH_ARGS_LOG"; then
  echo "FAIL signer digest passed to gh without an explicit pin"; FAILED=1
else
  echo "ok   signer digest omitted unless pinned"
fi
grep -q -- "--predicate-type https://slsa.dev/provenance/v1" "$GH_ARGS_LOG" || gh_flags_ok=0
grep -q -- "--predicate-type https://cyclonedx.org/bom" "$GH_ARGS_LOG" || gh_flags_ok=0
grep -q -- "--deny-self-hosted-runners" "$GH_ARGS_LOG" || gh_flags_ok=0
grep -q -- "attestation verify momus-$TARGET.cdx.json" "$GH_ARGS_LOG" || gh_flags_ok=0
grep -q -- "--bundle momus-$TARGET.provenance.bundle.json" "$GH_ARGS_LOG" || gh_flags_ok=0
grep -q -- "--bundle momus-$TARGET.sbom-attestation.bundle.json" "$GH_ARGS_LOG" || gh_flags_ok=0
if [ "$gh_flags_ok" -eq 1 ]; then
  echo "ok   policy flags (repo, source, signer, predicates, hosted runners, SBOM subject)"
else
  echo "FAIL policy flags handed to gh are incomplete:"; sed 's/^/     /' "$GH_ARGS_LOG"; FAILED=1
fi
: > "$GH_ARGS_LOG"

# 3. Tampered archive: appended bytes vs the old checksum.
make_fixture "$work/tampered"
printf 'extra' >> "$work/tampered/momus-$TARGET.tar.gz"
expect_fail "tampered archive" "$work/tampered" "checksum mismatch"

# 4. Altered SBOM: published bytes differ from the attested predicate.
make_fixture "$work/altered-sbom"
python3 - "$work/altered-sbom/momus-$TARGET.cdx.json" <<'PY'
import json, sys
p = sys.argv[1]
d = json.load(open(p))
d["components"][0]["version"] = "9.9.9-eviltwin"
open(p, "w").write(json.dumps(d))
PY
expect_fail "altered SBOM" "$work/altered-sbom" "predicate does not match"

# 5. Missing attestation bundle for a target.
make_fixture "$work/no-bundle"
rm "$work/no-bundle/momus-$TARGET.provenance.bundle.json"
expect_fail "missing bundle" "$work/no-bundle" "provenance.bundle.json: missing"

# 6. Missing target entirely (asked for it, nothing there).
mkdir -p "$work/no-target"
expect_fail "missing target" "$work/no-target" "missing"

# 7. Manifest records a digest that does not match the shipped file.
make_fixture "$work/bad-manifest"
python3 - "$work/bad-manifest" <<'PY'
import json, sys
p = f"{sys.argv[1]}/release-manifest.json"
d = json.load(open(p))
for t in d["targets"].values():
    t["archive"]["sha256"] = "0" * 64
open(p, "w").write(json.dumps(d))
PY
# Re-checksum so the failure isolates the digest-mismatch path (a stale
# checksum would fail earlier with "checksum mismatch" — covered by
# "tampered archive" style checks on the manifest's own chain).
python3 - "$work/bad-manifest" <<'PY'
import hashlib, sys
p = f"{sys.argv[1]}/release-manifest.json"
d = hashlib.sha256(open(p, "rb").read()).hexdigest()
open(f"{sys.argv[1]}/release-manifest.json.sha256", "w").write(f"{d}  release-manifest.json\n")
PY
expect_fail "manifest digest mismatch" "$work/bad-manifest" "manifest says"

# 8. Policy rejection must fail the verifier, not merely forward arguments.
make_fixture "$work/expectations"
for flag in source-sha source-ref signer-workflow signer-digest repo; do
  if run_verify "$work/expectations" "--$flag" wrong; then
    echo "FAIL wrong $flag accepted"; FAILED=1
  elif grep -q "provenance verification failed" "$work/out.txt"; then
    echo "ok   wrong $flag rejected"
  else
    echo "FAIL wrong $flag failed unexpectedly"; cat "$work/out.txt"; FAILED=1
  fi
done
if run_verify "$work/expectations" --source-ref refs/heads/main --signer-digest "$SHA"; then
  echo "ok   valid source ref and signer pin"
else
  echo "FAIL valid source ref and signer pin"; cat "$work/out.txt"; FAILED=1
fi

# 9. Multi-target manifest checking: the manifest walk must receive one
# argument per target. A single space-joined argument would make every
# target "not recorded in the release manifest" and block all releases.
make_fixture "$work/multi"
python3 - "$work/multi" "$TARGET" <<'PY'
import json, sys
root = sys.argv[1]
m = json.load(open(f"{root}/release-manifest.json"))
other = "aarch64-apple-darwin"
m["targets"][other] = {
    "archive": {"name": f"momus-{other}.tar.gz", "sha256": "0" * 64},
    "sbom": {"name": f"momus-{other}.cdx.json", "sha256": "0" * 64},
}
import hashlib
blob = json.dumps(m).encode()
open(f"{root}/release-manifest.json", "wb").write(blob)
open(f"{root}/release-manifest.json.sha256", "w").write(
    f"{hashlib.sha256(blob).hexdigest()}  release-manifest.json\n")
PY
if run_verify "$work/multi" && grep -q "recorded digests match" "$work/out.txt"; then
  echo "ok   multi-target manifest lookup"
else
  echo "FAIL multi-target manifest lookup"; sed 's/^/     /' "$work/out.txt"; FAILED=1
fi

# 10. A requested target with no files blocks.
if "$verify" --dir "$work/good" --source-sha "$SHA" --signer-workflow "$SIGNER" \
     --targets "$TARGET,aarch64-apple-darwin" >"$work/out.txt" 2>&1; then
  echo "FAIL second target missing: expected failure, got success"; FAILED=1
elif grep -q "aarch64-apple-darwin.provenance.bundle.json: missing" "$work/out.txt"; then
  echo "ok   second target missing (blocked)"
else
  echo "FAIL second target missing: unexpected failure output"; sed 's/^/     /' "$work/out.txt"; FAILED=1
fi

if [ "$FAILED" -eq 0 ]; then
  echo "test-verify-release.sh: all tests passed"
else
  echo "test-verify-release.sh: FAILED" >&2
  exit 1
fi
