---
name: ci-status
description: Read or wait on CI for a branch. Use in a review or pre-PR stage that must not proceed while CI is red or still running.
---

# CI status

Read the latest run's conclusion:

```
bash "$(dirname "$0")/status.sh" <branch>
```

Or block until an in-flight run finishes (exits non-zero if it fails):

```
bash "$(dirname "$0")/wait.sh" <branch>
```

Not bound by PROJ-1487's machine — it is here because a real toolbox carries
more skills than any one ticket loads, and a stage picks the subset it needs.
