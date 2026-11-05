#!/usr/bin/env bash
# Runs the daily signal; called by the Windows Task Scheduler through WSL.
# Extra arguments are passed through (for example --dry-run for a test).
set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p data/live
{
  echo "=== $(date -Is) ==="
  code=0
  ./target/release/tp daily --notify --capital 2200 "$@" || code=$?
  echo "=== end $(date -Is), exit code $code ==="
} >> data/live/run.log 2>&1
