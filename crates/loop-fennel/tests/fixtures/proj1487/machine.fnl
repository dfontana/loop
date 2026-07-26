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

 :defaults {:provider "anthropic" :model "claude-sonnet-5" :thinking "medium"}
 :budgets {:usd 8 :wallclock-s 5400 :max-transitions 40}
 :judge {:model "claude-haiku-4-5" :thinking "low"}
 :navigator {:model "claude-haiku-4-5" :thinking "low" :max-invocations 5}

 :entry "implement"
 :terminals ["done" "blocked"]
 :escalation-state "blocked"
 :transition-mode "constrained"

 :states
 {:implement {:playbook "implement" :thinking "high"
              :skills ["spark-build"]
              :description "Implement the plan; keep the build green."}
  :review {:playbook "review" :thinking "high"
           :description "Get an independent review of the diff."}
  :qa-staging {:playbook "qa" :thinking "high"
               :skills ["staging-deploy" "spark-run"]
               :description "Run QA against staging."}
  :debug {:playbook "debug-spark" :thinking "high"
          :skills ["spark-build" "debug-transient"]
          :description "Diagnose and fix a QA failure."}
  :validate-contract {:playbook "validate-contract" :thinking "medium"
                       :skills ["staging-deploy" "contract-check"]
                       :description "Confirm the API contract matches the OpenAPI schema."}
  :open-pr {:playbook "open-pr" :thinking "low"
            :skills ["open-pr"]
            :description "Open the pull request."}}

 :transitions
 [{:from "implement" :to "review"
   :criteria "The plan's four items are all addressed in the diff, the build is green, and no TODO/FIXME markers remain in changed files."
   :on-fail "retry"}

  {:from "review" :to "implement"
   :criteria "The review identified defects that require code changes."}

  {:from "review" :to "qa-staging"
   :criteria "The review found no defect requiring a code change."}

  {:from "qa-staging" :to "qa-staging"
   :criteria "The pipeline run failed for infrastructure reasons rather than a defect in the code under test."
   :backoff-s 30
   :on-fail "abort"}

  {:from "qa-staging" :to "debug"
   :criteria "The pipeline run failed deterministically: a schema, contract, assertion, or logic error in the code under test."}

  {:from "qa-staging" :to "validate-contract"
   :criteria "The pipeline run completed successfully and its output sample satisfies the QA cases."}

  {:from "debug" :to "qa-staging"
   :criteria "A concrete fix was applied and the build is green."
   :on-fail "retry"}

  {:from "validate-contract" :to "implement"
   :criteria "The staging response does not match the committed OpenAPI schema."}

  {:from "validate-contract" :to "open-pr"
   :criteria "The staging response matches the committed OpenAPI schema."}

  {:from "open-pr" :to "done"
   :criteria "A pull request exists for this branch with a populated description."}]

 :loops
 [{:name "qa" :states ["qa-staging" "debug"] :max-cycles 4 :on-exhausted "escalate"}
  {:name "qa-transient" :states ["qa-staging"] :max-cycles 3 :on-exhausted "escalate"}]}
