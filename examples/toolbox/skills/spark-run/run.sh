#!/usr/bin/env bash
# Run one named job against this ticket+cycle's namespace and wait for it.
set -euo pipefail

job="${1:?usage: run.sh <retention|backfill|engagement>}"
case "$job" in
  retention|backfill|engagement) ;;
  *) echo "unknown job: $job" >&2; exit 2 ;;
esac

ns="loop-${TICKET_ID:?}-${CYCLE:?}"
token="$(pass show spark/run-token)"

sparkctl run --job "$job" --namespace "$ns" --token "$token" --wait --json
