;; examples/local/machine.fnl  →  ./.loop/machine.fnl
;;
;; The per-ticket machine for PROJ-1487, in the v1 plain-table Fennel schema
;; (docs/09; the schema reference is crates/loop-fennel/src/convert.rs).
;;
;; The ticket's unique files are just this machine + its prose (task.md,
;; plan.md) + any bespoke local playbook (playbooks/validate-contract.md) + the
;; ledger it produces. Everything else it references — generic playbooks,
;; tools — lives in the portable toolbox at ~/.config/loop/ and is reused
;; untouched.
;;
;;   loop validate   → lints the graph + resolves every reference
;;   loop run        → drives it to a terminal
;;
;; See docs/06-example-walkthrough.md for the run this produces.

{:ticket "PROJ-1487"

 ;; Prose lives in markdown, referenced by path relative to this file. The
 ;; harness reads them into $TASK / $PLAN for every stage that templates them.
 ;; An inline string is still allowed for a throwaway ticket.
 :task "task.md"
 :plan "plan.md"

 ;; What "done and correct" means. Rendered into prompts as $QA_CASES.
 :qa-cases [{:id "pipeline"
             :desc "Retention job populates churn_score for all active accounts, 30d backfilled."}
            {:id "contract"
             :desc "GET /accounts/:id returns churn_score as a number matching the OpenAPI schema."}]

 ;; Sits under every state and over ~/.config/loop/config.fnl. A state's own
 ;; :model/:thinking wins; so does its playbook's frontmatter.
 :defaults {:provider "anthropic" :model "claude-sonnet-5" :thinking "medium"
            :tools ["read" "bash"]}

 ;; Hard stops the harness enforces. May only tighten the global budgets.
 :budgets {:usd 8 :wallclock-s 5400 :max-transitions 40}

 ;; The cheap agents that guard and reroute.
 :judge {:model "claude-haiku-4-5" :thinking "low"}
 :navigator {:model "claude-haiku-4-5" :thinking "low" :max-invocations 5}

 :entry "implement"
 :terminals ["done" "blocked"]
 :escalation-state "blocked"
 :transition-mode "constrained"

 ;; ── States ────────────────────────────────────────────────────────────────
 ;; Every stage's prompt IS its :playbook — a markdown file resolved
 ;; local-first (./.loop/playbooks/) then toolbox (~/.config/loop/playbooks/).
 ;; A generic stage names a toolbox playbook; a ticket-specific stage drops a
 ;; local .md of its own.
 :states
 {:implement {:playbook "implement"        ; toolbox
              :thinking "high"
              :tools ["edit" "write" "spark_build"]
              :description "Implement the plan; keep the build green."}

  :review {:playbook "review"              ; this playbook == the run-review skill
           :thinking "high"
           :tools ["agent" "select_review_model"]
           :description "Adversarial review of the diff; find real defects."}

  ;; No edit/write: a validation stage must not be able to quietly fix what
  ;; it is judging (docs/07 #1).
  :qa-staging {:playbook "qa"
               :thinking "high"
               :tools ["staging_deploy" "spark_run" "fetch_job_output"]
               :exclude-tools ["edit" "write"]
               :description "Deploy to staging, run the pipeline, grade it."}

  :debug {:playbook "debug-spark"
          :thinking "high"
          :tools ["edit" "spark_build" "use_playbook"]   ; may consult debug-transient
          :description "Diagnose a real pipeline failure and fix it."}

  :validate-contract {:playbook "validate-contract"      ; LOCAL: ./.loop/playbooks/
                      :thinking "medium"
                      :tools ["staging_deploy" "contract_check"]
                      :description "Confirm the API contract matches the OpenAPI schema."}

  :open-pr {:playbook "open-pr"
            :thinking "low"
            :tools ["open_pr"]
            :description "Open or update the pull request for this branch."}}

 ;; ── Transitions ───────────────────────────────────────────────────────────
 ;; Only these edges exist (the structural guard). Then :criteria, judged by an
 ;; independent cheap agent that sees the stage's output and artifacts but never
 ;; the worker's own claim that it succeeded.
 ;; :on-fail is "retry" | "abort" | {:route "state"}.
 :transitions
 [{:from "implement" :to "review"
   :criteria "The plan's four items are all addressed in the diff, the build is green, and no TODO/FIXME markers remain in changed files."
   :on-fail "retry"}

  {:from "review" :to "implement"
   :criteria "The review identified defects that require code changes."}
  {:from "review" :to "qa-staging"
   :criteria "The review found no defect requiring a code change."}

  ;; The three-way fail routing: a transient flake retries in place with
  ;; backoff and touches no code, a real failure spawns the debugger, a pass
  ;; moves on (docs/07 #4). What separates the first two is *where* the failure
  ;; came from, which is why each edge states it as a criterion.
  {:from "qa-staging" :to "qa-staging"
   :criteria "The pipeline run failed for infrastructure reasons — a lost executor, preemption, a shuffle/fetch failure, throttling, or a timeout — and not because of a defect in the code under test."
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

 ;; ── Loops ─────────────────────────────────────────────────────────────────
 ;; states[0] is the loop head — the state whose re-entry counts a cycle.
 :loops
 [{:name "qa" :states ["qa-staging" "debug"] :max-cycles 4 :on-exhausted "escalate"}
  {:name "qa-transient" :states ["qa-staging"] :max-cycles 3 :on-exhausted "escalate"}]}
