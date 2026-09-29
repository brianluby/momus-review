#!/usr/bin/env bash
# Tests for scripts/rollout.sh against a stub `gh`: nothing touches GitHub.
# The stub answers the calls rollout.sh makes and logs them; repo names
# choose its behavior (see below). Run: scripts/test-rollout.sh
# Expected PR text holds literal Markdown backticks in single quotes.
# shellcheck disable=SC2016
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
rollout="$here/rollout.sh"
stub="$(mktemp -d)"
trap 'rm -rf "$stub"' EXIT
PIN=0123456789abcdef0123456789abcdef01234567

# Repo names pick the stub's behavior:
#   has-secret      TYPESAFE_API_KEY already set
#   has-workflow    .github/workflows/momus.yml already on main
#   has-branch      momus/enable already exists (its momus.yml too)
#   has-dependabot  .github/dependabot.yml already on main
#   rolled          secret set and momus/enable open: a rolled-out repo
#   secretfail      `gh secret set` fails (403)
#   prfail          creating the workflow file fails (422)
#   branchfail      creating the momus/enable branch fails
#   stuck           the file fails and so does deleting the branch
#   private         a private repository
#   archived        an archived repository
# NOSCOPE=1 makes the token lack the `workflow` scope.
cat > "$stub/gh" <<EOF
#!/bin/bash
args="\$*"
echo "\$args" >> "$stub/calls"
case "\$args" in
  "api user -q .login") echo me ;;
  "auth status"*)
    if [ -n "\${NOSCOPE:-}" ]; then echo "  - Token scopes: 'repo'"
    else echo "  - Token scopes: 'repo', 'workflow'"; fi ;;
  "api repos/brianluby/momus-review/releases/latest -q .tag_name") echo v0.1.2 ;;
  "api repos/brianluby/momus-review/commits/v0.1.2 -q .sha") echo $PIN ;;
  "secret list --repo me/has-secret"*|"secret list --repo me/rolled"*) echo TYPESAFE_API_KEY ;;
  "secret list"*) ;;
  "secret set"*"--repo me/secretfail"*) cat > /dev/null; echo "HTTP 403" >&2; exit 1 ;;
  "secret set"*)
    key=\$(cat)
    [ "\$key" = s3cret ] || { echo "key not on stdin" >&2; exit 1; }
    [[ "\$args" != *s3cret* ]] || { echo "key on the command line" >&2; exit 1; } ;;
  "api repos/me/has-workflow/contents/.github/workflows/momus.yml?ref=main"*) ;;
  "api repos/me/has-dependabot/contents/.github/dependabot.yml?ref=main"*) ;;
  "api repos/me/has-branch/contents/.github/workflows/momus.yml?ref=momus/enable"*|\\
  "api repos/me/rolled/contents/.github/workflows/momus.yml?ref=momus/enable"*) echo filesha ;;
  "api repos/me/"*"/contents/"*) exit 1 ;;
  "api repos/me/has-branch/git/ref/heads/momus/enable"*|"api repos/me/rolled/git/ref/heads/momus/enable"*) ;;
  "api repos/me/"*"/git/ref/heads/momus/enable"*) exit 1 ;;
  "api repos/me/private -q"*) printf 'false\tprivate\tmain\n' ;;
  "api repos/me/archived -q"*) printf 'true\tpublic\tmain\n' ;;
  "api repos/me/"*" -q"*"archived"*) printf 'false\tpublic\tmain\n' ;;
  "api repos/me/"*"/git/ref/heads/main"*) echo abc123 ;;
  "api -X POST repos/me/branchfail/"*) echo "HTTP 403" >&2; exit 1 ;;
  "api -X POST"*) ;;
  "api -X PUT repos/me/prfail/"*|"api -X PUT repos/me/stuck/"*) echo "HTTP 422" >&2; exit 1 ;;
  "api -X PUT"*) ;;
  "api -X DELETE repos/me/stuck/"*) echo "HTTP 500" >&2; exit 1 ;;
  "api -X DELETE"*) ;;
  "pr create"*) r=\${args#*--repo }; echo "https://github.com/\${r%% *}/pull/1" ;;
  *) echo "unexpected: gh \$args" >&2; exit 97 ;;
esac
EOF
chmod +x "$stub/gh"

failures=0
# KEY= (empty) runs without a key in the environment.
run() { PATH="$stub:$PATH" TYPESAFE_API_KEY="${KEY-s3cret}" "$rollout" "$@" < /dev/null 2>&1; }

pass() { echo "ok   $1"; }
fail() { echo "FAIL $1"; shift; for l in "$@"; do echo "     $l"; done; failures=$((failures + 1)); }

# expect NAME WANT_EXIT OUTPUT_PATTERN... -- ARGS...
expect() {
  local name=$1 want=$2; shift 2
  local patterns=()
  while [ "$1" != -- ]; do patterns+=("$1"); shift; done
  shift
  : > "$stub/calls"
  local out code=0
  out=$(run "$@") || code=$?
  local ok=true missing=()
  [ "$code" = "$want" ] || ok=false
  for p in "${patterns[@]}"; do grep -qF -- "$p" <<< "$out" || { ok=false; missing+=("missing: $p"); }; done
  if grep -q "unexpected: gh" <<< "$out"; then ok=false; fi
  if $ok; then pass "$name"; else
    local lines=()
    while IFS= read -r line; do lines+=("$line"); done <<< "$out"
    fail "$name (exit $code, want $want)" "${missing[@]}" "${lines[@]}"
  fi
}

# The decoded content of the last PUT of PATH, from the call log.
uploaded() {
  grep -F "api -X PUT repos/me/$1/contents/$2 " "$stub/calls" | tail -n 1 |
    sed 's/.*-f content=\([^ ]*\).*/\1/' | base64 -d 2>/dev/null
}
calls_matching() { grep -cF -- "$1" "$stub/calls" || true; }
# The body of the PR the last run opened in REPO.
pr_body() { grep -F "pr create --repo me/$1 " "$stub/calls" | tail -n 1 | sed 's/.*--body //'; }
# body_has NAME REPO PATTERN [!PATTERN]: the PR body contains PATTERN (and not the ! one).
body_has() {
  local body; body=$(pr_body "$2")
  if grep -qF -- "$3" <<< "$body" && { [ -z "${4:-}" ] || ! grep -qF -- "$4" <<< "$body"; }; then
    pass "$1"
  else
    fail "$1" "$body"
  fi
}
# no_calls NAME PATTERN: passes when the last run made no call matching PATTERN.
no_calls() {
  if [ "$(calls_matching "$2")" = 0 ]; then pass "$1"; else fail "$1" "$(grep -F -- "$2" "$stub/calls")"; fi
}

expect "dry run changes nothing" 0 \
  "caller: review.yml@0123456789ab (v0.1.2)" "secret: would set" \
  "would open a PR adding .github/workflows/momus.yml and .github/dependabot.yml" \
  -- repo
no_calls "dry run makes no writes" "api -X"

expect "dry run reads existing state" 0 \
  "secret: present" "already on main" "branch momus/enable already exists" \
  "--update refreshes it" "note: private repo" "archived, skipped" \
  "dependabot: .github/dependabot.yml exists" \
  -- has-secret has-workflow has-branch private archived has-dependabot

expect "apply sets the secret on stdin and opens a PR" 0 \
  "secret: set" "PR opened https://github.com/me/repo/pull/1" \
  -- --apply repo
caller=$(uploaded repo .github/workflows/momus.yml)
if grep -qF "review.yml@$PIN # v0.1.2" <<< "$caller" &&
   grep -qF "if: github.event.pull_request.head.repo.full_name == github.repository" <<< "$caller" &&
   ! grep -q "review.yml@v0" <<< "$caller"; then
  pass "the caller is pinned to the release commit and skips forks"
else
  fail "the caller is pinned to the release commit and skips forks" "$caller"
fi
if uploaded repo .github/dependabot.yml | grep -q "package-ecosystem: github-actions"; then
  pass "a repo without Dependabot gets a github-actions config"
else
  fail "a repo without Dependabot gets a github-actions config"
fi
body_has "the PR body describes the pin and the added Dependabot" repo \
  'pinned to the v0.1.2 release commit. Dependabot (`.github/dependabot.yml`, github-actions)' "moving"

expect "an existing Dependabot config is left alone" 0 \
  "dependabot: .github/dependabot.yml exists" "PR opened" \
  -- --apply has-dependabot
no_calls "no Dependabot upload over an existing config" "contents/.github/dependabot.yml -f"
body_has "the PR body says to extend the existing Dependabot config" has-dependabot \
  'add a `github-actions` entry to `.github/dependabot.yml`' "proposes each upgrade"

expect "--float keeps the moving tag" 0 "caller: review.yml@v0 (floating)" "PR opened" \
  -- --apply --float repo
if uploaded repo .github/workflows/momus.yml | grep -qF "review.yml@v0"; then
  pass "--float uploads review.yml@v0"
else
  fail "--float uploads review.yml@v0"
fi
no_calls "--float skips the release lookup" "releases/latest"
no_calls "--float adds no Dependabot config" "contents/.github/dependabot.yml -f"
body_has "the --float PR body describes the moving tag" repo \
  'follows the moving `@v0` tag' "pinned"

expect "--float dry run plans no Dependabot file" 0 \
  "would open a PR adding .github/workflows/momus.yml on main" \
  -- --float repo

expect "--update without --apply only reports" 0 \
  "workflow: would update .github/workflows/momus.yml on momus/enable" \
  -- --update rolled

KEY='' expect "--update refreshes an open rollout PR without needing a key" 0 \
  "secret: present" "workflow: updated momus/enable" \
  -- --apply --update rolled
if grep -F "api -X PUT repos/me/rolled/contents/.github/workflows/momus.yml " "$stub/calls" | grep -qF -- "-f sha=filesha" &&
   uploaded rolled .github/workflows/momus.yml | grep -qF "review.yml@$PIN # v0.1.2"; then
  pass "--update replaces the file in place with the pinned caller"
else
  fail "--update replaces the file in place with the pinned caller" "$(cat "$stub/calls")"
fi

expect "one repo failing does not stop the batch" 1 \
  "== me/secretfail" "secret: FAILED, skipping this repo" \
  "== me/prfail" "workflow: FAILED; removed branch momus/enable, rerun to retry" \
  "PR opened https://github.com/me/good/pull/1" \
  -- --apply secretfail prfail good

expect "a failed step after the branch removes it" 1 \
  "workflow: FAILED; removed branch momus/enable, rerun to retry" \
  "workflow: FAILED to create branch momus/enable; rerun to retry" \
  "workflow: FAILED; could not remove branch momus/enable, delete it to retry" \
  -- --apply prfail branchfail stuck
deletes=$(grep "^api -X DELETE" "$stub/calls" | sed 's|.*repos/me/\([^/]*\)/.*|\1|' | sort | tr '\n' ' ')
if [ "$deletes" = "prfail stuck " ]; then
  pass "rollback deletes only branches this run created"
else
  fail "rollback deletes only branches this run created" "deleted: $deletes"
fi

expect "rotate replaces an existing secret" 0 "secret: set" \
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
