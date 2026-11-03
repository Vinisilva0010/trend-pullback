#!/usr/bin/env bash
# Hourly breakout alerts; called by the Windows Task Scheduler through WSL.
set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p data/live
{
  echo "=== $(date -Is) ==="
  ./target/release/tp alerts --notify "$@"
} >> data/live/alerts.log 2>&1
