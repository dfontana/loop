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
              :tools ["edit" "write"]
              :description "Implement the plan; keep the build green."}

  :review {:playbook "review"
           :thinking "high"
           :tools ["agent" "select_review_model"]
           :description "Adversarial review of the diff; find real defects."}

  ;; No edit/write: a stage that validates must not be able to quietly fix
  ;; what it is judging.
  :test {:playbook "qa"
         :thinking "high"
         :exclude-tools ["edit" "write"]
         :description "Run the test suite and report a grounded pass/fail."}

  :open-pr {:playbook "open-pr"
            :thinking "low"
            :description "Open or update the pull request for this branch."}}

 ;; Every gate here is a `:criteria` judged by the independent Judge agent,
 ;; because that is the only kind of gate that works on a fresh install with an
 ;; empty ~/.config/loop/tools/.
 ;;
 ;; UPGRADE THIS. A `:when` guard over a var a real tool emitted is strictly
 ;; better than a Judge's opinion (docs/07 #2): the tool asserts the fact from
 ;; an exit code, and no amount of model optimism can fake it. As soon as you
 ;; have a scoped-tool that runs your suite and prints
 ;;
 ;;   LOOP_VARS {"test":{"result":"pass"}}
 ;;
 ;; add it to the `test` state's :tools and replace that stage's criteria with
 ;;
 ;;   {:from "test" :to "open-pr" :when (fn [v] (= v.test.result "pass"))}
 ;;   {:from "test" :to "implement" :when (fn [v] (= v.test.result "fail"))}
 ;;
 ;; `loop validate` warns when a `:when` gates on a scope nothing looks able to
 ;; emit, which is the other half of this trade.
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
   :criteria "The test suite was actually run in this stage (the output is present, not asserted), and it passed. A suite that was not run is a fail."
   :on-fail {:route "implement"}}

  {:from "open-pr" :to "done"
   :criteria "A pull request exists for this branch with a populated description."}]

 :loops
 [{:name "fix" :states ["implement" "review" "test"] :max-cycles 4
   :on-exhausted "escalate"}]}
