---
name: open-pr
description: Open or update the pull request for the current branch. Use in the final stage of a ticket, once the work has passed review and QA.
---

# Open the pull request

Write the PR body to a file first — assembled from the ledger digest you were given, not from memory — then:

```
bash "$(dirname "$0")/open.sh" "<title>" <body-file>
```

Idempotent: if a PR already exists for this branch it is edited rather than duplicated, so a crash-resumed stage does the right thing on re-entry.
