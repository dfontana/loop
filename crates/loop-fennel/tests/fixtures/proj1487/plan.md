1. Add the `churn_score` column and backfill migration.
2. Populate it from the retention job.
3. Expose it on `GET /accounts/:id`.
4. Contract-check the response shape.
