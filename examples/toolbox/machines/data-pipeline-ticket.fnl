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
              ;; EDIT — swap spark_build for this pipeline's build tool.
              :tools ["edit" "write" "spark_build"]
              :description "Implement the plan; keep the build green."}

  :review {:playbook "review"
           :thinking "high"
           :tools ["agent" "select_review_model"]
           :description "Adversarial review of the diff; find real defects."}

  ;; Read-only by construction: a QA stage that can edit is a QA stage that
  ;; will "fix" what it is supposed to be grading (docs/07 #1).
  :qa-staging {:playbook "qa"
               :thinking "high"
               ;; EDIT — the deploy/run/fetch trio for this pipeline. Whatever
               ;; you use, it must emit LOOP_VARS with `result` and
               ;; `error_class`, or the guards below have nothing to gate on.
               :tools ["staging_deploy" "spark_run" "fetch_job_output"]
               :exclude-tools ["edit" "write"]
               :description "Deploy to staging, run the pipeline, grade it."}

  :debug {:playbook "debug-spark"
          :thinking "high"
          :tools ["edit" "spark_build" "use_playbook"]
          :description "Diagnose a real pipeline failure and fix it."}

  :open-pr {:playbook "open-pr"
            :thinking "low"
            :tools ["open_pr"]
            :description "Open or update the pull request for this branch."}}

 :transitions
 [{:from "implement" :to "review"
   :criteria "Every item in the plan is addressed in the diff, the build is green, and no TODO/FIXME markers remain in the changed files."
   :on-fail "retry"}

  {:from "review" :to "implement"
   :when (fn [v] (= v.review.result "changes_requested"))}
  {:from "review" :to "qa-staging"
   :when (fn [v] (= v.review.result "clean"))}

  ;; The heart of this template: transient failures retry in place with
  ;; backoff and touch no code; real ones spawn the debugger. Burning debug
  ;; cycles on a flaky cluster — or worse, "fixing" code to match a broken
  ;; environment — is the failure this routing exists to prevent (docs/07 #4).
  {:from "qa-staging" :to "qa-staging"
   :when (fn [v] (and (= v.qa.result "fail") (= v.qa.error_class "transient")))
   :backoff-s 30
   :on-fail "abort"}
  {:from "qa-staging" :to "debug"
   :when (fn [v] (and (= v.qa.result "fail") (not= v.qa.error_class "transient")))}
  {:from "qa-staging" :to "open-pr"
   :when (fn [v] (= v.qa.result "pass"))}

  {:from "debug" :to "qa-staging"
   :criteria "A concrete fix was applied and the build is green."
   :on-fail "retry"}

  {:from "open-pr" :to "done"
   :criteria "A pull request exists for this branch with a populated description."}]

 :loops
 [{:name "qa" :states ["qa-staging" "debug"] :max-cycles 4 :on-exhausted "escalate"}
  {:name "qa-transient" :states ["qa-staging"] :max-cycles 3 :on-exhausted "escalate"}]}
