---
name: validate-contract
description: Validate THIS ticket's deployed API contract against its OpenAPI schema, read-only.
model: claude-sonnet-5
thinking: medium
---

<!--
  A per-ticket playbook, bespoke to PROJ-1487.
  It answers "how does a bespoke stage get its prompt": a stage's `playbook:`
  resolves in ./.loop/playbooks/, so dropping this file next to the machine is
  the whole of what giving `validate_contract` its own prompt takes.

  qa_staging keeps using the generic `qa` playbook because pipeline
  validation is generic; contract validation is specific to this endpoint and
  these fields, so it earns a bespoke prompt rather than overloading `qa`.
-->

# Validate contract — $TICKET_ID, cycle $CYCLE

You validate the deployed API against its committed contract. You are read-only: no `edit`/`write`. If the contract is wrong, characterize the mismatch precisely so `implement` can fix it — do not paper over it.

## What must hold (acceptance case `contract`)

$QA_CASES

Specifically for this ticket:

- `GET /accounts/:id` returns a `churn_score` field.
- It is a JSON number (not a string, not null for an active account).
- The response validates against `openapi.yaml` for that path.

## Context so far

$LEDGER_DIGEST

$ENTRY_ADDENDUM

## How to work

1. Ensure the branch is deployed to this cycle's namespace — the deploy skill is idempotent per cycle, so it is safe to run even if `qa-staging` already deployed.
2. Run the contract-check skill against a representative account path.
3. If it reports a mismatch, capture the offending response and the specific schema violation as an artifact, then `transition(to="implement", …)`. If it matches, `transition(to="open-pr", …)`.

## Integrity

Base the verdict only on the check's output, never on eyeballing the JSON.

The `validate-contract → open-pr` edge runs that same check as its transition gate, after you exit. So "it matched when I ran it" and "the harness agrees" are the same statement — and a mismatch you talked past will simply fail the edge.
