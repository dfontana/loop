#!/usr/bin/env bash
# Latest CI conclusion for a branch. Exit 0 iff it succeeded.
set -euo pipefail

branch="${1:?usage: status.sh <branch>}"
git rev-parse --verify "$branch" >/dev/null 2>&1 || {
  echo "no such branch: $branch" >&2; exit 2
}

conclusion="$(gh run list --branch "$branch" --limit 1 \
  --json conclusion,status,workflowName \
  | jq -r '.[0].conclusion // "pending"')"

echo "$conclusion"
[ "$conclusion" = "success" ]
