#!/usr/bin/env bash
# Roll momus out to many repositories: set the TYPESAFE_API_KEY secret and
# open a pull request that adds .github/workflows/momus.yml (the caller in
# examples/momus.yml). That pull request is also each repo's first review.
#
# Usage:
#   scripts/rollout.sh [--apply] [--rotate-secret] REPO...
#   scripts/rollout.sh [--apply] [--rotate-secret] --file repos.txt
#
# REPO is owner/name, or a bare name for your own account. Without --apply
# nothing changes: each repo is read and the planned actions are printed.
# With --apply the key comes from $TYPESAFE_API_KEY or a prompt, and is
# passed to gh on stdin, never on a command line. --rotate-secret also
# overwrites an existing secret (to rotate the key).
set -euo pipefail

BRANCH=momus/enable
WORKFLOW=.github/workflows/momus.yml
TEMPLATE="$(cd "$(dirname "$0")/.." && pwd)/examples/momus.yml"

apply=false
rotate=false
repos=()
while [ $# -gt 0 ]; do
  case "$1" in
    --apply) apply=true ;;
    --rotate-secret) rotate=true ;;
    --file)
      [ $# -ge 2 ] || { echo "--file needs a path" >&2; exit 2; }
      [ -f "$2" ] || { echo "no such file: $2" >&2; exit 2; }
      while IFS= read -r line; do
        line="${line%%#*}"
        line="${line//[[:space:]]/}"
        if [ -n "$line" ]; then repos+=("$line"); fi
      done < "$2"
      shift ;;
    -h|--help) sed -n '2,14p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    -*) echo "unknown option: $1" >&2; exit 2 ;;
    *) repos+=("$1") ;;
  esac
  shift
done
[ ${#repos[@]} -gt 0 ] || { echo "no repositories given (see --help)" >&2; exit 2; }
[ -f "$TEMPLATE" ] || { echo "missing $TEMPLATE" >&2; exit 1; }

me=$(gh api user -q .login)
# Creating a workflow file needs a token with the `workflow` scope.
if ! gh auth status 2>&1 | grep -q "'workflow'"; then
  echo "warning: your gh token lacks the 'workflow' scope; run: gh auth refresh -s workflow" >&2
  if $apply; then exit 1; fi
fi

key=""
if $apply; then
  key="${TYPESAFE_API_KEY:-}"
  if [ -z "$key" ]; then
    read -rsp "TYPESAFE_API_KEY: " key < /dev/tty
    echo >&2
  fi
  [ -n "$key" ] || { echo "empty key" >&2; exit 1; }
fi

content=$(base64 < "$TEMPLATE" | tr -d '\n')
status=0

for repo in "${repos[@]}"; do
  case "$repo" in */*) ;; *) repo="$me/$repo" ;; esac
  if ! [[ "$repo" =~ ^[A-Za-z0-9._-]+/[A-Za-z0-9._-]+$ ]]; then
    echo "== $repo: not owner/name, skipped"; status=1; continue
  fi
  echo "== $repo"
  if ! info=$(gh api "repos/$repo" -q '[.archived, .visibility, .default_branch] | @tsv' 2>/dev/null); then
    echo "   not found or no access, skipped"; status=1; continue
  fi
  IFS=$'\t' read -r archived visibility default <<< "$info"
  if [ "$archived" = true ]; then echo "   archived, skipped"; continue; fi
  if [ "$visibility" != public ]; then
    echo "   note: $visibility repo, runs use Actions minutes"
  fi

  has_secret=false
  if gh secret list --repo "$repo" --json name -q '.[].name' | grep -qx TYPESAFE_API_KEY; then
    has_secret=true
  fi
  has_workflow=false
  if gh api "repos/$repo/contents/$WORKFLOW?ref=$default" --silent 2>/dev/null; then
    has_workflow=true
  fi
  has_branch=false
  if gh api "repos/$repo/git/ref/heads/$BRANCH" --silent 2>/dev/null; then
    has_branch=true
  fi

  # Secret.
  if $has_secret && ! $rotate; then
    echo "   secret: present"
  elif $apply; then
    # Each step checks its own result: one repo's failure is recorded and
    # the batch moves on (set -e would otherwise end the whole rollout).
    if printf '%s' "$key" | gh secret set TYPESAFE_API_KEY --repo "$repo"; then
      echo "   secret: set"
    else
      echo "   secret: FAILED, skipping this repo"; status=1; continue
    fi
  else
    echo "   secret: would set"
  fi

  # Workflow, via a pull request.
  if $has_workflow; then
    echo "   workflow: $WORKFLOW already on $default"
  elif $has_branch; then
    echo "   workflow: branch $BRANCH already exists (open PR?), skipped"
  elif $apply; then
    if ! { sha=$(gh api "repos/$repo/git/ref/heads/$default" -q .object.sha) &&
           gh api -X POST "repos/$repo/git/refs" -f ref="refs/heads/$BRANCH" -f sha="$sha" --silent; }; then
      echo "   workflow: FAILED to create branch $BRANCH; rerun to retry"; status=1
    elif gh api -X PUT "repos/$repo/contents/$WORKFLOW" -f branch="$BRANCH" \
           -f message="Add momus pull request review" -f content="$content" --silent &&
         url=$(gh pr create --repo "$repo" --base "$default" --head "$BRANCH" \
           --title "Add momus pull request review" \
           --body "Adds \`$WORKFLOW\`: [momus](https://github.com/brianluby/momus-review) reviews each pull request and posts inline comments plus one summary comment. It uses the \`TYPESAFE_API_KEY\` repository secret; fork PRs are skipped. This pull request is its first run."); then
      echo "   workflow: PR opened $url"
    else
      # Roll back the branch this run created, so a plain rerun retries from
      # scratch instead of skipping the repo as "branch already exists".
      if gh api -X DELETE "repos/$repo/git/refs/heads/$BRANCH" --silent; then
        echo "   workflow: FAILED; removed branch $BRANCH, rerun to retry"
      else
        echo "   workflow: FAILED; could not remove branch $BRANCH, delete it to retry"
      fi
      status=1
    fi
  else
    echo "   workflow: would open a PR adding $WORKFLOW on $default"
  fi
done
exit $status
