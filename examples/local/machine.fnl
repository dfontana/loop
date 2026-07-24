(import-macros {: machine : stage : to : retry-loop} :loop)
(local {: transient? : real? : mins : secs} (require :loop))

(local coder [read edit write bash spark_build])

(machine PROJ-1487
  (task-file "task.md")
  (plan-file "plan.md")
  (qa-cases
    (pipeline "Retention job populates churn_score for all active accounts, 30d backfilled.")
    (contract "GET /accounts/:id returns churn_score as a number matching the OpenAPI schema."))

  (defaults :model claude-sonnet-5 :thinking medium :session fresh :tools [read bash])
  (budgets  :usd 8 :wallclock (mins 90) :transitions 40)
  (cheap-agents claude-haiku-4-5)

  (stage implement :thinking high :session (continue impl) :tools coder
    (to review :judge "Plan's four items addressed; build green; no TODO/FIXME in the diff."
               :on-fail retry))

  (stage review :thinking high :tools [read bash agent select_review_model]
    (to implement  :when (= review.result :changes_requested))
    (to qa-staging :when (= review.result :clean)))

  (stage qa-staging :playbook qa :thinking high
                    :tools [read bash staging_deploy spark_run fetch_job_output]
    (to validate-contract :when (= qa.result :pass))
    (to qa-staging        :when (and (= qa.result :fail) (transient? qa)) :backoff (secs 30))
    (to debug             :when (and (= qa.result :fail) (real? qa))))

  (stage debug :playbook debug-spark :thinking high :session (continue impl)
                :tools [read edit bash spark_build use_playbook]
    (to qa-staging :judge "A concrete fix was applied and the build is green." :on-fail retry))

  (stage validate-contract :thinking medium :tools [read bash staging_deploy contract_check]
    (to implement :when (= contract.result :mismatch))
    (to open-pr   :when (= contract.result :match)))

  (stage open-pr :thinking low :tools [read bash open_pr]
    (to done :judge "A PR exists for this branch with a populated description."))

  (retry-loop qa           [qa-staging debug] :max 4 :on-exhausted escalate)
  (retry-loop qa-transient [qa-staging]       :max 3 :on-exhausted escalate))
