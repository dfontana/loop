#!/usr/bin/env bash
# ~/.config/loop/tools/bin/classify-spark.sh (staged to $PI_AGENT_DIR/bin/)
#
# Reads a sparkctl job-output JSON document on stdin and prints exactly one
# authoritative `LOOP_VARS` line carrying qa.result + qa.error_class, then the
# raw sample for the worker to read. Invoked by spark.yaml's fetch_job_output.
#
# The taxonomy lives HERE, in a versioned + testable script, rather than in a
# prompt — so "executor-lost/timeout → transient, schema/assertion → real" is one
# reviewable place and the model can never fake a pass. The harness scrapes the
# LOOP_VARS line into a `vars_set` ledger event; the machine's `when` guards gate
# on it (see docs/03-ledger.md, docs/04-toolbox.md).
#
# Exit code mirrors the verdict so the harness has a second, non-textual signal:
#   0 = pass, 1 = real failure, 2 = transient failure, 3 = unknown.
set -euo pipefail

payload="$(cat)"

# jq helpers — tolerate missing fields.
status="$(printf '%s' "$payload" | jq -r '.status      // "unknown"')"
reason="$(printf '%s' "$payload" | jq -r '.exit_reason // .error // ""')"

emit() {  # $1=result $2=error_class $3=exit_code
  if [ -n "${2:-}" ]; then
    printf 'LOOP_VARS {"qa":{"result":"%s","error_class":"%s","detail":%s}}\n' \
      "$1" "$2" "$(printf '%s' "$reason" | jq -R .)"
  else
    printf 'LOOP_VARS {"qa":{"result":"%s"}}\n' "$1"
  fi
  printf '%s\n' "$payload"          # hand the raw sample to the worker as context
  exit "$3"
}

# 1) Clean success.
if [ "$status" = "succeeded" ]; then
  emit pass "" 0
fi

# 2) Transient: it's about WHERE/WHEN it ran, not what the code does.
transient_re='ExecutorLostFailure|lost executor|Container killed|preempted|FetchFailed|ShuffleFetch|broadcast.*timeout|connection reset|throttl|503|driver.*OOM'
if printf '%s' "$reason" | grep -qEi "$transient_re"; then
  emit fail transient 2
fi

# 3) Real: deterministic — about WHAT the code does.
real_re='not found|AnalysisException|schema|type mismatch|assertion|contract|NullPointer|expected .* got|row count'
if printf '%s' "$reason" | grep -qEi "$real_re"; then
  emit fail real 1
fi

# 4) A job that completed but we have no clean status → inspect output, default real.
if [ "$status" = "completed" ] || [ "$status" = "failed" ]; then
  emit fail real 1
fi

# 5) Genuinely can't tell. The harness bounded-retries, then treats as real.
emit fail unknown 3
