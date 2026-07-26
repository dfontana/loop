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
 {:implement {:playbook "implement"
              :thinking "high"
              :description "Implement the plan; keep the build green."}

  :review {:playbook "review"
           :thinking "high"
           :description "Adversarial review of the diff; find real defects."}

  ;; The gate on the way out of this stage is a `:check` the harness runs
  ;; itself, so it does not matter what this stage can reach — a stage cannot
  ;; edit its way past a command it does not run.
  :test {:playbook "qa"
         :thinking "high"
         :description "Run the test suite and report a grounded pass/fail."}

  :open-pr {:playbook "open-pr"
            :thinking "low"
            :description "Open or update the pull request for this branch."}}

 :transitions
 [{:from "implement" :to "review"
   :criteria "Every item in the plan is addressed in the diff, the build is green, and no TODO/FIXME markers remain in the changed files."
   :on-fail "retry"}

  ;; `review.result` comes from a tool the review playbook calls, not from the
  ;; worker's prose — see docs/03 on trusted vars.
  {:from "review" :to "implement"
   :when (fn [v] (= v.review.result "changes_requested"))}
  {:from "review" :to "test"
   :when (fn [v] (= v.review.result "clean"))}

  {:from "test" :to "implement"
   :when (fn [v] (= v.test.result "fail"))}
  {:from "test" :to "open-pr"
   :when (fn [v] (= v.test.result "pass"))}

  {:from "open-pr" :to "done"
   :criteria "A pull request exists for this branch with a populated description."}]

 :loops
 [{:name "fix" :states ["implement" "review" "test"] :max-cycles 4
   :on-exhausted "escalate"}]}
