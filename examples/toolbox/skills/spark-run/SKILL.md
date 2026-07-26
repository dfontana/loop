---
name: spark-run
description: Run a named Spark job against this ticket's staging namespace and classify its outcome. Use in a QA stage that must exercise the pipeline end to end.
---

# Run a pipeline job

```
bash "$(dirname "$0")/run.sh" <retention|backfill|engagement>
```

The namespace is derived from `$TICKET_ID` and `$CYCLE`, which the harness
exports — so re-running within a cycle updates the same deployment rather than
creating a second one.

`classify.sh` beside it turns this cycle's run into a one-line verdict:

```
bash "$(dirname "$0")/classify.sh"
```

It prints `pass`, `transient`, or `real`. Read its reasoning before deciding
where to transition — a transient failure is about *where* the job ran, a real
one about *what the code does*, and they route to different states.

The edges out of the QA stage run this same script with `--expect <verdict>` as
their transition check. So there is no gap between your reading and the gate's:
if you propose the transient edge on a run that classifies as real, the harness
will say so and route you to the debugger instead.
