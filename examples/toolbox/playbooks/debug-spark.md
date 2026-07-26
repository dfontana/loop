---
name: debug-spark
description: Diagnose a real (non-transient) Spark/pipeline QA failure, apply a concrete fix, and leave the build green.
model: claude-sonnet-5
thinking: high
---

# Debug (Spark) — ticket $TICKET_ID, cycle $CYCLE

QA classified a failure as **real** — the change itself is wrong, not the infrastructure. Your job is to find the root cause and apply a concrete fix, not to re-run and hope. Every stage starts fresh, so use the ledger digest and referenced artifacts to understand what implement already did.

## What was being validated

$TASK

## Context so far — read the failure carefully

$LEDGER_DIGEST

$ENTRY_ADDENDUM

The most recent `qa` failure detail (error_class `real`) is in the digest, with a job-log artifact. Start there, not from the top.

## How to work

1. **Reproduce the diagnosis, don't guess.** Read the QA job-log artifact and the relevant code/config. Name the root cause in one sentence before you touch anything.
2. **Consult situational know-how when the class fits.** The `debug-transient` skill is loaded into this stage. If the symptom looks like a flake that slipped past classification, work its checklist before assuming a code bug. Reach for it when the signal is ambiguous — don't burn a code fix on an executor that simply died.
3. **Fix the actual cause.** Common real failures on this pipeline:
   - `column X not found in gold schema` → the migration was authored but never registered in the staging deploy manifest (staging reads the manifest, not the migration file). Register it.
   - nulls written for the new column → the job isn't computing/writing it, or is writing to the wrong table version.
   - schema/type mismatch on read → DTO or serializer disagrees with the column type.
4. **Verify green.** Run the build. Do not finish on a red tree — the harness re-runs the same build itself as this edge's check, so a red tree fails the transition regardless of what you report.
5. Finish with `transition(to="qa_staging", rationale="<root cause → the fix>", artifacts=[{name:"diff", path:"<the fix diff>"}])`. If the root cause is outside what you can reach (needs a plan change, missing access), call `transition(blocked=true, rationale=…)` rather than papering over it.

## Notes

- One concrete fix per cycle. If you're changing three unrelated things, you haven't found the root cause yet.
- The next stage is read-only QA against staging — leave the tree deployable and the migration registered so QA can actually observe your fix.
