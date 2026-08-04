;; standard-ticket — the plain code-only spine.
;;
;; implement → review → test → open-pr, with the two back-edges that make it a
;; loop rather than a pipeline. Copy it to .loop/machine.fnl and hack it for
;; the ticket; that is the whole workflow.
;;
;;      ┌───────────┐  findings
;;      │ implement │◀────────────┐
;;      └─────┬─────┘             │
;;            ▼                   │
;;      ┌───────────┐             │
;;      │  review   │─────────────┤
;;      └─────┬─────┘ clean       │
;;            ▼                   │
;;      ┌───────────┐  failures   │
;;      │   test    │─────────────┘
;;      └─────┬─────┘
;;            ▼ pass
;;      ┌───────────┐
;;      │  open-pr  │──▶ done
;;      └───────────┘

{:ticket "$TICKET"
 :task "task.md"
 :plan "plan.md"

 :qa-cases [{:id "behavior" :desc "The change does what the plan says, end to end."}
            {:id "regression" :desc "The existing test suite still passes."}]

 :defaults {:thinking "medium"}
 :budgets {:usd 8 :wallclock-s 5400 :max-transitions 40}

 :entry "implement"
 :terminals ["done" "blocked"]
 :escalation-state "blocked"

 :states
 {:implement {:stage-prompt "implement"
              :thinking "high"
              :description "Implement the plan; keep the build green."}

  :review {:stage-prompt "review"
           :thinking "high"
           :description "Adversarial review of the diff; find real defects."}

  ;; The gate on the way out of this stage is a `:check` the harness runs
  ;; itself, so it does not matter what this stage can reach — a stage cannot
  ;; edit its way past a command it does not run.
  ;; `:skills` is the other half of what a stage is told, and it works the
  ;; opposite way to `:stage-prompt`: the prompt above is always in the
  ;; system prompt, a skill is offered by name and description and loaded
  ;; only if the stage decides it is in that situation. Which is exactly
  ;; right for "is this failure a flake or a real bug" — most runs never
  ;; need to ask.
  :test {:stage-prompt "qa"
         :thinking "high"
         :skills ["debug-transient"]
         :description "Run the test suite and report a grounded pass/fail."}

  :open-pr {:stage-prompt "open-pr"
            :thinking "low"
            :description "Open or update the pull request for this branch."}}

 ;; Two kinds of gate, and the difference matters.
 ;;
 ;; A `:check` is a command the HARNESS runs, in its own subprocess, after the
 ;; stage exits. Exit 0 passes the edge. It is the only signal here a worker
 ;; cannot author — everything else on the ledger passed through the worker's
 ;; session first — so put a check on any edge where a mechanical fact settles
 ;; the question. `test -f`, `cargo test`, `curl | schema-validate`.
 ;;
 ;; A `:criteria` is judged by an independent cheap agent that sees the stage's
 ;; output, its artifacts, and the check's stdout — never the worker's own
 ;; claim that it succeeded. Use it for what no exit code can decide: "every
 ;; item in the plan is addressed", "this is a real fix, not a workaround".
 ;;
 ;; They compose. The check is a precondition; the criteria is the semantic
 ;; layer on top. A failed check is not appealable to the Judge.
 ;;
 ;; UPGRADE THIS. `:criteria "the suite passed"` is a stopgap — swap in the
 ;; command that actually runs your suite as a `:check` and let the criteria
 ;; cover only what the exit code can't.
 :transitions
 [{:from "implement" :to "review"
   :criteria "Every item in the plan is addressed in the diff, the build is green, and no TODO/FIXME markers remain in the changed files."
   :on-fail "retry"}

  ;; A failed review isn't a dead end — it routes back to implement with the
  ;; findings in the ledger digest.
  {:from "review" :to "test"
   :criteria "The review found no blocking defects: no correctness bugs, no missing error handling on the changed paths, and no unaddressed review findings from a previous cycle."
   :on-fail {:route "implement"}}

  {:from "test" :to "open-pr"
   ;; Replace with the command that runs your suite — then the gate stops
   ;; depending on the stage reporting honestly.
   ;; :check "cargo test --quiet"
   :criteria "The test suite was actually run in this stage (the output is present, not asserted), and it passed. A suite that was not run is a fail."
   :on-fail {:route "implement"}}

  {:from "open-pr" :to "done"
   :criteria "A pull request exists for this branch with a populated description."}]

 :loops
 [{:name "fix" :states ["implement" "review" "test"] :max-cycles 4
   :on-exhausted "escalate"}]}
