---
name: spark-build
description: Build and unit-check the Spark pipeline for the current working tree. Use after any substantive code change, and before proposing a transition out of an implementation or debug stage.
---

# Build the pipeline

Run `./build.sh` from this skill's directory. It compiles the pipeline and runs
the fast unit checks, printing the tail of `build.log` on failure.

```
bash "$(dirname "$0")/build.sh"
```

A non-zero exit means the tree is red. Fix it before you transition — the
harness runs its own build check on the way out of this stage, so a red tree
will fail the edge regardless of what you report.
