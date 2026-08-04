---
name: staging-deploy
description: Deploy the current branch to this ticket's isolated staging namespace. Use before any stage that exercises the deployed service.
---

# Deploy to staging

```
bash "$(dirname "$0")/deploy.sh" <branch> <dev|staging>
```

Idempotent: the namespace is keyed on `$TICKET_ID` and `$CYCLE`, so re-running for the same cycle updates the existing deployment. That is what makes a crash-resumed stage safe to re-enter.

The script refuses any environment other than `dev` or `staging`, and reads the deploy token from `pass` rather than taking it as an argument — do not try to route around either. If you need an environment the script rejects, that is a machine-authoring decision, not a stage-time one: transition with `blocked=true` and say so.
