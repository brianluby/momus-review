#!/usr/bin/env bash
# Roll momus out to many repositories: set the TYPESAFE_API_KEY secret and
# open a pull request that adds .github/workflows/momus.yml (the caller in
# examples/momus.yml) plus, when the repo has none, a Dependabot config for
# GitHub Actions. That pull request is also each repo's first review.
#
# Usage:
#   scripts/rollout.sh [--apply] [--update] [--float] [--rotate-secret] REPO...
#   scripts/rollout.sh [options] --file repos.txt
#
# REPO is owner/name, or a bare name for your own account. Without --apply
# nothing changes: each repo is read and the planned actions are printed.
# The caller is pinned to the latest momus release's commit SHA (Dependabot
# proposes upgrades); --float keeps the moving @v0 tag instead. --update
# refreshes the files on an existing rollout branch (an open PR). The key
# comes from $TYPESAFE_API_KEY or a prompt, only when a secret must be set,
# and reaches gh on stdin, never a command line. --rotate-secret overwrites
# an existing secret.
set -euo pipefail

MOMUS_REPO=brianluby/momus-review
BRANCH=momus/enable
WORKFLOW=.github/workflows/momus.yml
DEPENDABOT=.github/dependabot.yml
EXAMPLES="$(cd "$(dirname "$0")/.." && pwd)/examples"

apply=false
rotate=false
update=false
float=false
repos=()
while [ $# -gt 0 ]; do
  case "$1" in
    --apply) apply=true ;;
    --rotate-secret) rotate=true ;;
    --update) update=true ;;
    --float) float=true ;;
    --file)
      [ $# -ge 2 ] || { echo "--file needs a path" >&2; exit 2; }
      [ -f "$2" ] || { echo "no such file: $2" >&2; exit 2; }
      while IFS= read -r line; do
        line="${line%%#*}"
        line="${line//[[:space:]]/}"
        if [ -n "$line" ]; then repos+=("$line"); fi
      done < "$2"
      shift ;;
    -h|--help) sed -n '2,18p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    -*) echo "unknown option: $1" >&2; exit 2 ;;
    *) repos+=("$1") ;;
  esac
  shift
done
[ ${#repos[@]} -gt 0 ] || { echo "no repositories given (see --help)" >&2; exit 2; }
for f in momus.yml dependabot.yml; do
  [ -f "$EXAMPLES/$f" ] || { echo "missing $EXAMPLES/$f" >&2; exit 1; }
done

me=$(gh api user -q .login)
# Creating a workflow file needs a token with the `workflow` scope.
if ! gh auth status 2>&1 | grep -q "'workflow'"; then
  echo "warning: your gh token lacks the 'workflow' scope; run: gh auth refresh -s workflow" >&2
  if $apply; then exit 1; fi
fi

# The caller, pinned to the latest release's commit unless --float.
caller=$(cat "$EXAMPLES/momus.yml")
if $float; then
  echo "caller: review.yml@v0 (floating)"
else
  tag=$(gh api "repos/$MOMUS_REPO/releases/latest" -q .tag_name)
  pin=$(gh api "repos/$MOMUS_REPO/commits/$tag" -q .sha)
  [[ "$pin" =~ ^[0-9a-f]{40}$ ]] || { echo "could not resolve $tag to a commit" >&2; exit 1; }
  uses="uses: $MOMUS_REPO/.github/workflows/review.yml"
  pinned=""
  while IFS= read -r line; do
    if [[ "$line" == *"$uses@v0" ]]; then line="${line%@v0}@$pin # $tag"; fi
    pinned+="$line"$'\n'
  done <<< "$caller"
  caller="$pinned"
  [[ "$caller" == *"@$pin # $tag"* ]] || { echo "examples/momus.yml has no $uses@v0 line" >&2; exit 1; }
  echo "caller: review.yml@${pin:0:12} ($tag)"
fi
caller_b64=$(printf '%s' "$caller" | base64 | tr -d '\n')
dependabot_b64=$(base64 < "$EXAMPLES/dependabot.yml" | tr -d '\n')

key=""
key_for_secret() {
  if [ -z "$key" ]; then
    key="${TYPESAFE_API_KEY:-}"
    if [ -z "$key" ]; then
      read -rsp "TYPESAFE_API_KEY: " key < /dev/tty
      echo >&2
    fi
  fi
  [ -n "$key" ]
}

# put_file REPO BRANCH PATH BASE64 MESSAGE: create or replace one file.
put_file() {
  local sha args=(-X PUT "repos/$1/contents/$3" -f branch="$2" -f message="$5" -f content="$4" --silent)
  if sha=$(gh api "repos/$1/contents/$3?ref=$2" -q .sha 2>/dev/null) && [ -n "$sha" ]; then
    args+=(-f sha="$sha")
  fi
  gh api "${args[@]}"
}

# exists REPO REF PATH
exists() { gh api "repos/$1/contents/$3?ref=$2" --silent 2>/dev/null; }

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
  if exists "$repo" "$default" "$WORKFLOW"; then has_workflow=true; fi
  has_branch=false
  if gh api "repos/$repo/git/ref/heads/$BRANCH" --silent 2>/dev/null; then has_branch=true; fi
  # Dependabot is added only where there is no config; an existing one is
  # the repo's own and is only reported.
  own_dependabot=""
  for f in .github/dependabot.yml .github/dependabot.yaml; do
    if exists "$repo" "$default" "$f"; then own_dependabot=$f; fi
  done

  # Secret.
  if $has_secret && ! $rotate; then
    echo "   secret: present"
  elif $apply; then
    # Each step checks its own result: one repo's failure is recorded and
    # the batch moves on (set -e would otherwise end the whole rollout).
    if key_for_secret && printf '%s' "$key" | gh secret set TYPESAFE_API_KEY --repo "$repo"; then
      echo "   secret: set"
    else
      echo "   secret: FAILED, skipping this repo"; status=1; continue
    fi
  else
    echo "   secret: would set"
  fi

  if [ -n "$own_dependabot" ]; then
    echo "   dependabot: $own_dependabot exists; add a github-actions entry for momus upgrade PRs"
  fi

  # Workflow (and Dependabot), via a pull request.
  if $has_workflow; then
    echo "   workflow: $WORKFLOW already on $default"
  elif $has_branch; then
    if ! $update; then
      echo "   workflow: branch $BRANCH already exists (open PR?), skipped; --update refreshes it"
    elif ! $apply; then
      echo "   workflow: would update $WORKFLOW on $BRANCH"
    elif put_file "$repo" "$BRANCH" "$WORKFLOW" "$caller_b64" "Update momus pull request review" &&
         { [ -n "$own_dependabot" ] ||
           put_file "$repo" "$BRANCH" "$DEPENDABOT" "$dependabot_b64" "Add Dependabot for GitHub Actions"; }; then
      echo "   workflow: updated $BRANCH"
    else
      echo "   workflow: FAILED to update $BRANCH"; status=1
    fi
  elif $apply; then
    if ! { sha=$(gh api "repos/$repo/git/ref/heads/$default" -q .object.sha) &&
           gh api -X POST "repos/$repo/git/refs" -f ref="refs/heads/$BRANCH" -f sha="$sha" --silent; }; then
      echo "   workflow: FAILED to create branch $BRANCH; rerun to retry"; status=1
    elif put_file "$repo" "$BRANCH" "$WORKFLOW" "$caller_b64" "Add momus pull request review" &&
         { [ -n "$own_dependabot" ] ||
           put_file "$repo" "$BRANCH" "$DEPENDABOT" "$dependabot_b64" "Add Dependabot for GitHub Actions"; } &&
         url=$(gh pr create --repo "$repo" --base "$default" --head "$BRANCH" \
           --title "Add momus pull request review" \
           --body "Adds \`$WORKFLOW\`: [momus](https://github.com/$MOMUS_REPO) reviews each pull request and posts inline comments plus one summary comment. It uses the \`TYPESAFE_API_KEY\` repository secret; fork PRs are skipped. The reusable workflow is pinned to a release commit, and Dependabot (github-actions) proposes upgrades. This pull request is its first run."); then
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
    extra=""
    if [ -z "$own_dependabot" ]; then extra=" and $DEPENDABOT"; fi
    echo "   workflow: would open a PR adding $WORKFLOW$extra on $default"
  fi
done
exit $status
