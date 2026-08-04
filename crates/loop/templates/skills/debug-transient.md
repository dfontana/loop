---
name: debug-transient
description: Tell an infrastructure flake apart from a real bug before deciding whether to spend a code fix on a failure. Use when a test or pipeline failure could plausibly be either.
---

<!--
  A SKILL, not a stage prompt. The `test` state names it in `:skills`, which
  becomes `pi --skill <this file>` — offered by the description above, and
  loaded only when the stage decides it is in this situation. It is
  deliberately a checklist rather than a task with its own handoff: the
  calling stage stays in control of its own transition.

  No `model:`/`thinking:` frontmatter here. Those keys are read off a stage
  prompt, which is bound to a state and so has a model to influence; loop
  never opens a skill at all, so they would be silently inert.

  See docs/03-customizing.md.
-->

# Situational guidance: is this transient, or real?

You were handed a failure and want to know whether to spend a code fix on it or just let it retry. Misclassifying is expensive both ways: debugging a flake wastes a cycle; retrying a real bug wastes the whole `qa` loop budget. Work this checklist against the job log before deciding.

## Signals it's TRANSIENT (retry, don't fix)

- Executor / worker lost mid-run: `ExecutorLostFailure`, `lost executor`, node preempted, `Container killed by YARN/k8s`.
- Timeouts against a healthy job: shuffle fetch timeout, broadcast timeout, a stage that had been progressing.
- Cluster-side hiccups: driver OOM on a run that normally fits, throttled object store, `503`/`connection reset` talking to staging infra.
- **Tell:** the failure is about _where/when it ran_, and the same code/inputs would plausibly pass on a clean re-run.

## Signals it's REAL (fix, don't retry)

- Deterministic assertion / schema errors: `column ... not found`, type mismatch, `AnalysisException`, contract validation failure.
- Wrong output on a job that _completed_: nulls where values are required, wrong row counts, off-by-one on a window/backfill boundary.
- Fails identically across retries at the same point.
- **Tell:** the failure is about _what the code does_, and re-running changes nothing.

## If you genuinely can't tell (`unknown`)

Prefer one bounded re-run over a speculative code change. If it fails the same way twice at the same point, treat it as real and debug it. Never mark a criterion passed to escape the ambiguity — say `unknown` honestly and let the harness's bounded-retry policy decide.
