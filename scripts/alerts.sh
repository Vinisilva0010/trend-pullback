#!/usr/bin/env bash
# Hourly alerts; called by the Windows Task Scheduler through WSL.
# A run is cut at 15 minutes so a stuck network request cannot pile up runs;
# the end line records the exit code (124 means it was cut).
set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p data/live
{
  echo "=== $(date -Is) ==="
  code=0
  timeout 15m ./target/release/tp alerts --notify "$@" || code=$?
  echo "=== end $(date -Is), exit code $code ==="
} >> data/live/alerts.log 2>&1
