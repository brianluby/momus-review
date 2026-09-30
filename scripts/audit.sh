#!/usr/bin/env bash
# RustSec advisory gate for momus releases and CI.
#
# Runs `cargo audit` against Cargo.lock with a documented, time-limited
# exception list. Policy (docs/security.md "Dependency auditing"):
#
# - An advisory that is not excepted fails the run.
# - An exception is one line in scripts/audit-exceptions.txt:
#       RUSTSEC-0000-0000|2026-12-31|why we accept it for now
#   Expired exceptions fail the run: renew (with a fresh reason and date)
#   or fix the dependency. Exceptions are per advisory id, not per run.
# - Advisory-database outage (RustSec/advisory-db unreachable): the release
#   gate fails — no release ships without a fresh audit. In PR CI
#   (--advisory-db-outage warn) it degrades to a warning so an upstream
#   outage cannot block unrelated pull requests; the release path still
#   fails.
#
# Usage: scripts/audit.sh [--advisory-db-outage {fail,warn}]
set -euo pipefail

outage_policy=fail
while [ $# -gt 0 ]; do
  case "$1" in
    --advisory-db-outage)
      [ $# -ge 2 ] || { echo "audit.sh: $1 needs an argument" >&2; exit 2; }
      outage_policy=$2
      shift 2
      ;;
    *) echo "audit.sh: unknown argument: $1" >&2; exit 2 ;;
  esac
done
case "$outage_policy" in fail|warn) ;; *) echo "audit.sh: bad policy: $outage_policy" >&2; exit 2 ;; esac

repo=$(cd "$(dirname "$0")/.." && pwd)
exceptions_file="$repo/scripts/audit-exceptions.txt"
today=$(date -u +%Y-%m-%d)

if [ ! -f "$exceptions_file" ]; then
  echo "audit.sh: missing $exceptions_file" >&2
  exit 2
fi

ignore_args=()
outdated=0
while IFS='|' read -r advisory expires reason; do
  case "$advisory" in ''|'#'*) continue ;; esac
  if ! [[ "$advisory" =~ ^RUSTSEC-[0-9]{4}-[0-9]{4}$ ]]; then
    echo "audit.sh: bad exception id: $advisory" >&2
    exit 2
  fi
  if ! [[ "$expires" =~ ^[0-9]{4}-[0-9]{2}-[0-9]{2}$ ]]; then
    echo "audit.sh: bad exception expiry for $advisory: $expires (want YYYY-MM-DD)" >&2
    exit 2
  fi
  if [ -z "${reason// /}" ]; then
    echo "audit.sh: exception $advisory needs a reason" >&2
    exit 2
  fi
  # Hard expiry: the run fails ON the expiry date, not after it.
  if [[ ! "$expires" > "$today" ]]; then
    echo "audit.sh: exception $advisory expired on $expires: $reason" >&2
    echo "  renew it in scripts/audit-exceptions.txt or fix the dependency." >&2
    outdated=1
  else
    echo "audit: accepting $advisory until $expires: $reason"
    ignore_args+=(--ignore "$advisory")
  fi
done < "$exceptions_file"
[ "$outdated" -eq 0 ] || exit 1

# cargo audit fetches the advisory database on every run; transient network
# failures get a short retry window before the outage policy applies.
db_outage=0
for attempt in 1 2 3; do
  set +e
  audit_output=$(cd "$repo" && cargo audit --file Cargo.lock \
      "${ignore_args[@]+"${ignore_args[@]}"}" 2>&1)
  status=$?
  set -e
  if [ "$status" -eq 0 ]; then
    echo "$audit_output"
    echo "audit: no unexcepted RustSec advisories"
    exit 0
  fi
  # An outage is an *error* about fetching/reaching the advisory database.
  # Ordinary runs always print "Fetching advisory database from ..." as
  # progress, and real findings mention crates and connections in their
  # titles — matching those would let vulnerabilities through in warn mode.
  if echo "$audit_output" | grep -i '^error' \
      | grep -qiE 'fetch|download|network|advisory|timeout|refused|reset|resolve|os error [0-9]+'; then
    echo "audit: advisory-database fetch failed (attempt $attempt):"
    echo "$audit_output" | tail -n 5 >&2
    sleep $((attempt * 10))
    db_outage=1
    continue
  fi
  # A real finding or a genuine tool error: report and stop.
  echo "$audit_output"
  exit "$status"
done

if [ "$db_outage" -eq 1 ]; then
  if [ "$outage_policy" = warn ]; then
    echo "::warning::cargo audit could not fetch the RustSec advisory database; skipping the audit gate for this CI run. The release gate still fails on outages."
    exit 0
  fi
  echo "::error::cargo audit could not fetch the RustSec advisory database; failing per policy (release gate)."
  exit 1
fi
