#!/usr/bin/env bash
# Validate one staging endpoint against the committed OpenAPI schema.
# Exit 0 = the response matches. Doubles as a transition `:check`.
set -euo pipefail

path="${1:?usage: check.sh /some/api/path}"
printf '%s' "$path" | grep -qE '^/[A-Za-z0-9/_:-]+$' || {
  echo "not a valid API path: $path" >&2
  exit 2
}

base="https://loop-${TICKET_ID:?}-${CYCLE:?}.staging.internal"

curl -sf "$base$path" | openapi-validate --spec ./openapi.yaml --path "$path" --json
