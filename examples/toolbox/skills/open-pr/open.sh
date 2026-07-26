#!/usr/bin/env bash
# Open (or update) the PR for the current branch. Safe to re-run.
set -euo pipefail

title="${1:?usage: open.sh <title> <body-file>}"
body_file="${2:?usage: open.sh <title> <body-file>}"
test -f "$body_file" || { echo "no such body file: $body_file" >&2; exit 2; }

if existing="$(gh pr view --json url -q .url 2>/dev/null)"; then
  echo "PR already exists: $existing"
  gh pr edit --title "$title" --body-file "$body_file"
else
  gh pr create --title "$title" --body-file "$body_file" --fill
fi
