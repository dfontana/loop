---
name: contract-check
description: Validate a deployed staging endpoint against the committed OpenAPI schema. Use in a stage that must confirm the API contract before opening a PR.
---

# Check the API contract

```
bash "$(dirname "$0")/check.sh" /accounts/42
```

Hits the path on this ticket's staging deployment and validates the response
against `./openapi.yaml`. Exit 0 means the response matches the schema.

The same script is what the `validate-contract → open-pr` edge runs as its
transition check, so running it here tells you exactly what the gate will
decide. There is no version of "it matched when I ran it" that the harness
disagrees with.
