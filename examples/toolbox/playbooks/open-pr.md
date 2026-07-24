---
name: open-pr
description: Assemble a PR body from the ledger digest and open (or update) the pull request. Low-stakes, low-thinking terminal-adjacent stage.
model: claude-sonnet-5
thinking: low
---

# Open PR — ticket $TICKET_ID, cycle $CYCLE

Everything upstream passed: the change is implemented, reviewed clean, QA'd
against staging, and the contract matched. Your only job is to open a good pull
request. Keep it mechanical — no code changes, no re-litigating the work.

## The change, in brief
$TASK

## The full story to summarize
$LEDGER_DIGEST

## How to work
1. Write the PR body to a file, assembled from the digest — not invented:
   - **What & why** — one paragraph from the task/plan.
   - **How it was validated** — the QA cycles (including the transient retry and
     the real fix that were resolved) and the contract-match result. Cite the
     evidence artifacts by name.
   - **Notes for the reviewer** — anything the loop surfaced worth a human's eye.
2. Call `open_pr(title="<ticket>: <one-line summary>", body_file="<the file>")`.
   It is idempotent — if a PR already exists for this branch it updates it, so
   this is safe on crash-resume.
3. Finish with `transition(to="done", rationale="PR #<n> open with populated
   description", artifacts=[{name:"pr", path:"<url file>"}])`.

## Notes
- `done` is a terminal state; the transition into it is gated by a Judge checking
  a PR actually exists with a real description — so make the body substantive, not
  a stub.
- If `open_pr` fails (auth, no remote), `transition(blocked=true, rationale=…)`;
  don't fabricate a PR URL.
