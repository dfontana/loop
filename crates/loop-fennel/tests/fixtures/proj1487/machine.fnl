;; Ported from examples/local/machine.yaml (PROJ-1487) into the plain-table
;; Fennel schema documented in src/convert.rs. Used by tests/machine.rs and
;; tests/guards.rs as the "complete, realistic machine" fixture.

{:ticket "PROJ-1487"
 :task "task.md"
 :plan "plan.md"
 :qa-cases [{:id "pipeline"
             :desc "Retention job populates churn_score for all active accounts, 30d backfilled."}
            {:id "contract"
             :desc "GET /accounts/:id returns churn_score as a number matching the OpenAPI schema."}]

 :defaults {:provider "anthropic" :model "claude-sonnet-5" :thinking "medium" :tools ["read" "bash"]}
 :budgets {:usd 8 :wallclock-s 5400 :max-transitions 40}
 :judge {:model "claude-haiku-4-5" :thinking "low"}
 :navigator {:model "claude-haiku-4-5" :thinking "low" :max-invocations 5}

 :entry "implement"
 :terminals ["done" "blocked"]
 :escalation-state "blocked"
 :transition-mode "constrained"

 :states
 {:implement {:playbook "implement" :thinking "high"
              :tools ["edit" "write" "bash" "spark_build"]
              :description "Implement the plan; keep the build green."}
  :review {:playbook "review" :thinking "high"
           :tools ["read" "bash" "agent" "select_review_model"]
           :description "Get an independent review of the diff."}
  :qa-staging {:playbook "qa" :thinking "high"
               :tools ["staging_deploy" "spark_run" "fetch_job_output"]
               :exclude-tools ["edit" "write"]
               :description "Run QA against staging."}
  :debug {:playbook "debug-spark" :thinking "high"
          :tools ["edit" "bash" "spark_build" "use_playbook"]
          :description "Diagnose and fix a QA failure."}
  :validate-contract {:playbook "validate-contract" :thinking "medium"
                       :tools ["staging_deploy" "contract_check"]
                       :description "Confirm the API contract matches the OpenAPI schema."}
  :open-pr {:playbook "open-pr" :thinking "low"
            :tools ["open_pr"]
            :description "Open the pull request."}}

 :transitions
 [{:from "implement" :to "review"
   :criteria "The plan's four items are all addressed in the diff, the build is green, and no TODO/FIXME markers remain in changed files."
   :on-fail "retry"}

  {:from "review" :to "implement"
   :when (fn [v] (= v.review.result "changes_requested"))}

  {:from "review" :to "qa-staging"
   :when (fn [v] (= v.review.result "clean"))}

  {:from "qa-staging" :to "qa-staging"
   :when (fn [v] (and (= v.qa.result "fail") (= v.qa.error_class "transient")))
   :backoff-s 30
   :on-fail "abort"}

  {:from "qa-staging" :to "debug"
   :when (fn [v] (and (= v.qa.result "fail") (not= v.qa.error_class "transient")))}

  {:from "qa-staging" :to "validate-contract"
   :when (fn [v] (= v.qa.result "pass"))}

  {:from "debug" :to "qa-staging"
   :criteria "A concrete fix was applied and the build is green."
   :on-fail "retry"}

  {:from "validate-contract" :to "implement"
   :when (fn [v] (= v.contract.result "mismatch"))}

  {:from "validate-contract" :to "open-pr"
   :when (fn [v] (= v.contract.result "match"))}

  {:from "open-pr" :to "done"
   :criteria "A pull request exists for this branch with a populated description."}]

 :loops
 [{:name "qa" :states ["qa-staging" "debug"] :max-cycles 4 :on-exhausted "escalate"}
  {:name "qa-transient" :states ["qa-staging"] :max-cycles 3 :on-exhausted "escalate"}]}
