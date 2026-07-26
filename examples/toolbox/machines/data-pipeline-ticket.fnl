;; ~/.config/loop/machines/data-pipeline-ticket.fnl
;;
;; A machine TEMPLATE for a data-pipeline change validated against staging: it
;; adds a deploy + real-vs-transient QA loop and an optional contract check on
;; top of the standard spine. PROJ-1487's ../../local/machine.fnl is this
;; template, filled in (the churn_score task/plan and a bespoke
;; validate-contract stage). Copy it with:
;;
;;   loop init PROJ-1487 --template data-pipeline-ticket
;;
;; then hack it. The parts you almost always edit are marked EDIT.

{:ticket "$TICKET"
 :task "task.md"
 :plan "plan.md"

 ;; EDIT — what "done and correct" means for this pipeline.
 :qa-cases [{:id "pipeline" :desc "The job produces the expected rows for the expected window."}
            {:id "contract" :desc "The downstream API/schema contract still holds."}]

 :defaults {:thinking "medium"}
 :budgets {:usd 8 :wallclock-s 5400 :max-transitions 40}

 :entry "implement"
 :terminals ["done" "blocked"]
 :escalation-state "blocked"

 :states
 {:implement {:playbook "implement"
              :thinking "high"
              ;; EDIT — swap in this pipeline's build skill.
              :skills ["spark-build"]
              :description "Implement the plan; keep the build green."}

  :review {:playbook "review"
           :thinking "high"
           :description "Adversarial review of the diff; find real defects."}

  ;; What stops a QA stage from "fixing" what it is grading is not what it
  ;; can reach — it is that the edges out of it are gated on commands the
  ;; harness runs itself, plus a Judge that never sees this stage's own claims
  ;; (docs/07 #1).
  :qa-staging {:playbook "qa"
               :thinking "high"
               ;; EDIT — the deploy/run skills for this pipeline.
               :skills ["staging-deploy" "spark-run"]
               :description "Deploy to staging, run the pipeline, grade it."}

  :debug {:playbook "debug-spark"
          :thinking "high"
          :skills ["spark-build" "debug-transient"]
          :description "Diagnose a real pipeline failure and fix it."}

  :open-pr {:playbook "open-pr"
            :thinking "low"
            :skills ["open-pr"]
            :description "Open or update the pull request for this branch."}}

 :transitions
 [{:from "implement" :to "review"
   :check "bash ~/.config/loop/skills/spark-build/build.sh"
   :criteria "Every item in the plan is addressed in the diff, and no TODO/FIXME markers remain in the changed files."
   :on-fail "retry"}

  {:from "review" :to "implement"
   :criteria "The review identified defects that require code changes."}
  {:from "review" :to "qa-staging"
   :criteria "The review found no defect requiring a code change."}

  ;; The heart of this template: transient failures retry in place with
  ;; backoff and touch no code; real ones spawn the debugger. Burning debug
  ;; cycles on a flaky cluster — or worse, "fixing" code to match a broken
  ;; environment — is the failure this routing exists to prevent (docs/07 #4).
  ;;
  ;; Each edge asserts one branch of `classify.sh`'s taxonomy as a `:check`, so
  ;; the split is decided by a versioned regex set and an exit code rather than
  ;; by an agent that would rather retry than debug. EDIT the script to match
  ;; your pipeline's failure vocabulary.
  {:from "qa-staging" :to "qa-staging"
   :check "bash ~/.config/loop/skills/spark-run/classify.sh --expect transient"
   :backoff-s 30
   :on-fail "abort"}
  {:from "qa-staging" :to "debug"
   :check "bash ~/.config/loop/skills/spark-run/classify.sh --expect real"}
  {:from "qa-staging" :to "open-pr"
   :check "bash ~/.config/loop/skills/spark-run/classify.sh --expect pass"
   :criteria "The output sample satisfies every QA case, not just the job's exit status."}

  {:from "debug" :to "qa-staging"
   :check "bash ~/.config/loop/skills/spark-build/build.sh"
   :criteria "A concrete fix to the diagnosed failure was applied — not a retry, a widened assertion, or a disabled check."
   :on-fail "retry"}

  {:from "open-pr" :to "done"
   :criteria "A pull request exists for this branch with a populated description."}]

 :loops
 [{:name "qa" :states ["qa-staging" "debug"] :max-cycles 4 :on-exhausted "escalate"}
  {:name "qa-transient" :states ["qa-staging"] :max-cycles 3 :on-exhausted "escalate"}]}
