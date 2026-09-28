#!/usr/bin/env bash
# Tests for scripts/rollout.sh against a stub `gh`: nothing touches GitHub.
# The stub answers the calls rollout.sh makes; repo names choose its
# behavior (see below). Run: scripts/test-rollout.sh
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
rollout="$here/rollout.sh"
stub="$(mktemp -d)"
trap 'rm -rf "$stub"' EXIT

# Repo names pick the stub's behavior:
#   has-secret    TYPESAFE_API_KEY already set
#   has-workflow  .github/workflows/momus.yml already on main
#   has-branch    momus/enable already exists
#   secretfail    `gh secret set` fails (403)
#   prfail        creating the workflow file fails (422)
#   private       a private repository
#   archived      an archived repository
# NOSCOPE=1 makes the token lack the `workflow` scope.
cat > "$stub/gh" <<'EOF'
#!/bin/bash
args="$*"
case "$args" in
  "api user -q .login") echo me ;;
  "auth status"*)
    if [ -n "${NOSCOPE:-}" ]; then echo "  - Token scopes: 'repo'"
    else echo "  - Token scopes: 'repo', 'workflow'"; fi ;;
  "secret list --repo me/has-secret"*) echo TYPESAFE_API_KEY ;;
  "secret list"*) ;;
  "secret set"*"--repo me/secretfail"*) cat > /dev/null; echo "HTTP 403" >&2; exit 1 ;;
  "secret set"*)
    key=$(cat)
    [ "$key" = s3cret ] || { echo "key not on stdin" >&2; exit 1; }
    [[ "$args" != *s3cret* ]] || { echo "key on the command line" >&2; exit 1; } ;;
  "api repos/me/has-workflow/contents/"*) ;;
  "api repos/me/"*"/contents/"*) exit 1 ;;
  "api repos/me/has-branch/git/ref/heads/momus/enable"*) ;;
  "api repos/me/"*"/git/ref/heads/momus/enable"*) exit 1 ;;
  "api repos/me/private -q"*) printf 'false\tprivate\tmain\n' ;;
  "api repos/me/archived -q"*) printf 'true\tpublic\tmain\n' ;;
  "api repos/me/"*" -q"*"archived"*) printf 'false\tpublic\tmain\n' ;;
  "api repos/me/"*"/git/ref/heads/main"*) echo abc123 ;;
  "api -X POST"*) ;;
  "api -X PUT repos/me/prfail/"*) echo "HTTP 422" >&2; exit 1 ;;
  "api -X PUT"*) ;;
  "pr create"*) r=${args#*--repo }; echo "https://github.com/${r%% *}/pull/1" ;;
  *) echo "unexpected: gh $args" >&2; exit 97 ;;
esac
EOF
chmod +x "$stub/gh"

failures=0
run() { PATH="$stub:$PATH" TYPESAFE_API_KEY=s3cret "$rollout" "$@" 2>&1; }

# expect NAME WANT_EXIT OUTPUT_PATTERN... -- ARGS...
expect() {
  local name=$1 want=$2; shift 2
  local patterns=()
  while [ "$1" != -- ]; do patterns+=("$1"); shift; done
  shift
  local out code=0
  out=$(run "$@") || code=$?
  local ok=true
  [ "$code" = "$want" ] || ok=false
  for p in "${patterns[@]}"; do grep -qF -- "$p" <<< "$out" || ok=false; done
  if grep -q "unexpected: gh" <<< "$out"; then ok=false; fi
  if $ok; then
    echo "ok   $name"
  else
    echo "FAIL $name (exit $code, want $want)"
    while IFS= read -r line; do echo "     $line"; done <<< "$out"
    failures=$((failures + 1))
  fi
}

expect "dry run changes nothing" 0 \
  "secret: would set" "workflow: would open a PR" \
  -- repo

expect "dry run reads existing state" 0 \
  "secret: present" "already on main" "branch momus/enable already exists" \
  "note: private repo" "archived, skipped" \
  -- has-secret has-workflow has-branch private archived

expect "apply sets the secret on stdin and opens a PR" 0 \
  "secret: set" "PR opened https://github.com/me/repo/pull/1" \
  -- --apply repo

expect "one repo failing does not stop the batch" 1 \
  "== me/secretfail" "secret: FAILED, skipping this repo" \
  "== me/prfail" "workflow: FAILED (delete branch momus/enable to retry)" \
  "PR opened https://github.com/me/good/pull/1" \
  -- --apply secretfail prfail good

expect "rotate replaces an existing secret" 0 \
  "secret: set" \
  -- --apply --rotate-secret has-secret

list="$stub/repos.txt"
printf '# comment\n repo \n\nhas-secret # trailing\n' > "$list"
expect "--file reads names, skipping comments and blanks" 0 \
  "== me/repo" "== me/has-secret" \
  -- --file "$list"

expect "--file with a missing file is a clear error" 2 \
  "no such file: /nonexistent/repos.txt" \
  -- --file /nonexistent/repos.txt

expect "an invalid name is skipped and fails the run" 1 \
  "not owner/name, skipped" \
  -- 'bad name!'

expect "no repositories is a usage error" 2 \
  "no repositories given" \
  --

NOSCOPE=1 expect "apply refuses without the workflow scope" 1 \
  "lacks the 'workflow' scope" \
  -- --apply repo

if [ "$failures" -gt 0 ]; then echo "$failures failed"; exit 1; fi
echo "all passed"
