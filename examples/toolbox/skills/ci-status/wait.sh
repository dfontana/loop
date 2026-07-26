#!/usr/bin/env bash
# Block until the branch's in-flight CI run finishes. Exit mirrors the result.
set -euo pipefail

branch="${1:?usage: wait.sh <branch>}"
run_id="$(gh run list --branch "$branch" --limit 1 --json databaseId -q '.[0].databaseId')"
[ -n "$run_id" ] || { echo "no CI run for $branch" >&2; exit 2; }

gh run watch "$run_id" --exit-status
