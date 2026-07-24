# Implementation plan — PROJ-1487

Co-authored live in harness during `loop plan`. The `implement` and `debug`
stages receive this file's contents as `$PLAN`; keep it concrete and ordered.

1. **Schema first.** Add `churn_score double` to `gold.retention` via a
   schema-registry migration, and register that migration in the staging deploy
   manifest (not just author it — staging reads the manifest).
2. **Compute in the job.** In the `retention` Spark job, derive `churn_score`
   from the existing engagement features and write it to the new column. Do not
   re-derive the features.
3. **Expose on the API.** Add `churn_score` to the `AccountResponse` DTO and the
   `GET /accounts/:id` serializer, typed as a number.
4. **Backfill.** Run the standard backfill job for the trailing 30 days,
   inclusive of the start boundary (watch the off-by-one on the window).

## Risks / watch-items

- The migration-vs-manifest split has bitten us before; step 1 is the usual
  cause of a "column not found in gold schema" failure in staging QA.
- The backfill window boundary is inclusive — `>= start_date`, not `>`.
