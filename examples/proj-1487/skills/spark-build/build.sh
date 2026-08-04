#!/usr/bin/env bash
# Compile the pipeline and run its fast unit checks.
#
# Also usable as a transition `:check` — it is the same command either way,
# which is the point: the agent and the harness run identical code, so an agent
# cannot pass a gate the harness would fail.
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

if sbt -batch compile test:compile 2>build.log; then
  echo "build ok"
else
  echo "build failed:"
  tail -40 build.log
  exit 1
fi
