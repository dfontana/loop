#!/usr/bin/env bash
# Classify this cycle's pipeline run as pass / transient failure / real failure.
#
# The taxonomy lives here, in a versioned and testable script, rather than in a
# prompt — "executor-lost/timeout -> transient, schema/assertion -> real" is one
# reviewable place.
#
# Two callers, deliberately identical:
#   classify.sh                  → prints the verdict, for an agent to read
#   classify.sh --expect real    → exits 0 iff the verdict matches, for a
#                                  transition `:check`
#
# The job is found from $TICKET_ID/$CYCLE rather than passed in, so the harness
# can run this with no knowledge of what the agent did. That is what makes the
# gate and the agent's own reading the same reading.
set -euo pipefail

expect=""
[ "${1:-}" = "--expect" ] && expect="${2:?--expect needs pass|transient|real}"

ns="loop-${TICKET_ID:?}-${CYCLE:?}"
token="$(pass show spark/run-token)"

payload="$(sparkctl fetch --namespace "$ns" --latest --token "$token" --sample 50 --json)"
status="$(printf '%s' "$payload" | jq -r '.status      // "unknown"')"
reason="$(printf '%s' "$payload" | jq -r '.exit_reason // .error // ""')"

classify() {
  [ "$status" = "succeeded" ] && { echo pass; return; }

  # Transient: about WHERE/WHEN it ran, not what the code does.
  local transient='ExecutorLostFailure|lost executor|Container killed|preempted|FetchFailed|ShuffleFetch|broadcast.*timeout|connection reset|throttl|503|driver.*OOM'
  printf '%s' "$reason" | grep -qEi "$transient" && { echo transient; return; }

  # Real: deterministic, about WHAT the code does.
  local real='not found|AnalysisException|schema|type mismatch|assertion|contract|NullPointer|expected .* got|row count'
  printf '%s' "$reason" | grep -qEi "$real" && { echo real; return; }

  # A job that finished but told us nothing useful: treat as real. Retrying a
  # real failure burns the whole qa loop budget, so this is the safe default.
  case "$status" in
    completed|failed) echo real; return ;;
  esac
  echo unknown
}

verdict="$(classify)"

if [ -n "$expect" ]; then
  # As a transition check: quiet agreement, loud disagreement.
  if [ "$verdict" = "$expect" ]; then
    echo "run classified $verdict${reason:+ — $reason}"
    exit 0
  fi
  echo "expected $expect, but this run classifies as $verdict${reason:+ — $reason}" >&2
  exit 1
fi

echo "$verdict${reason:+ — $reason}"
printf '%s\n' "$payload"
