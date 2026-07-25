# PROJ-1487 — Expose `churn_score` on the account API

Add a `churn_score` column to the retention Spark pipeline and expose it on
`GET /accounts/:id`. Backfill the last 30 days.

## Why

Success and Support want a single at-a-glance retention-risk number per account
without joining against the modeling warehouse. The score already exists
implicitly in the engagement features; this ticket surfaces it as a first-class
pipeline column and API field.

## Scope

- In: the `retention` Spark job, the `gold.retention` schema, the account read
  API, a 30-day backfill.
- Out: any change to how the score is *computed* (a separate modeling ticket
  owns the formula), alerting, or the dashboard.

## Acceptance

The structured acceptance cases live in `machine.fnl` (`qa-cases`) so stages can
reference them by id; this file is the human framing.
