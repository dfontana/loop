---
name: debug-transient
description: Tell an infrastructure flake apart from a real bug before deciding whether to spend a code fix on a failure. Use when a pipeline or test failure could plausibly be either.
---

<!--
  A SKILL, not a stage playbook: situational know-how a stage consults when it
  hits the situation, rather than a task with its own `transition`. The `debug`
  state loads it via `:skills`, which becomes `pi --skill <this dir>`. The
  caller stays in control of its own stage. See docs/03-customizing.md.
-->

# Situational guidance: is this transient, or real?

You were handed a failure and want to know whether to spend a code fix on it or
just let it retry. Misclassifying is expensive both ways: debugging a flake wastes
a cycle; retrying a real bug wastes the whole `qa` loop budget. Work this
checklist against the job log before deciding.

## Signals it's TRANSIENT (retry, don't fix)
- Executor / worker lost mid-run: `ExecutorLostFailure`, `lost executor`, node
  preempted, `Container killed by YARN/k8s`.
- Timeouts against a healthy job: shuffle fetch timeout, broadcast timeout, a
  stage that had been progressing.
- Cluster-side hiccups: driver OOM on a run that normally fits, throttled object
  store, `503`/`connection reset` talking to staging infra.
- **Tell:** the failure is about *where/when it ran*, and the same code/inputs
  would plausibly pass on a clean re-run.

## Signals it's REAL (fix, don't retry)
- Deterministic assertion / schema errors: `column ... not found`, type mismatch,
  `AnalysisException`, contract validation failure.
- Wrong output on a job that *completed*: nulls where values are required, wrong
  row counts, off-by-one on a window/backfill boundary.
- Fails identically across retries at the same point.
- **Tell:** the failure is about *what the code does*, and re-running changes
  nothing.

## If you genuinely can't tell (`unknown`)
Prefer one bounded re-run over a speculative code change. If it fails the same way
twice at the same point, treat it as real and debug it. Never mark a criterion
passed to escape the ambiguity — say `unknown` honestly and let the harness's
bounded-retry policy decide.
