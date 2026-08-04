#!/usr/bin/env bash
# Deploy a branch to this ticket+cycle's isolated staging namespace.
#
# The guardrails that used to be `validationCmd` entries in scoped-tools live
# here now. Moving them into the script did not weaken them: a stage with bash
# could always have called `stagectl` directly, so the yaml was never the
# boundary. What the script buys is that the *intended* path is checked, in one
# reviewable and testable place.
set -euo pipefail

branch="${1:?usage: deploy.sh <branch> <dev|staging>}"
env="${2:?usage: deploy.sh <branch> <dev|staging>}"

git rev-parse --verify "$branch" >/dev/null 2>&1 || {
  echo "no such branch: $branch" >&2
  exit 2
}
case "$env" in
  dev|staging) ;;
  *) echo "refusing to deploy to '$env' — dev or staging only" >&2; exit 2 ;;
esac

ns="loop-${TICKET_ID:?}-${CYCLE:?}"
token="$(pass show staging/deploy-token)"

stagectl deploy --branch "$branch" --env "$env" --namespace "$ns" \
  --token "$token" --wait --json
