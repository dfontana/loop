;; examples/proj-1487/machine.fnl  →  ./.loop/machine.fnl
;;
;; The per-ticket machine for PROJ-1487, in the v1 plain-table Fennel schema
;; (docs/05-design-notes.md; the schema reference is crates/loop/src/fennel/convert.rs).
;;
;; Everything this machine references lives beside it: the prose it reads
;; (task.md, plan.md), the stage prompts its states name, the skills those stages
;; load, and the ledger the run appended. A ticket directory is self-contained,
;; so `loop validate` here resolves every reference without looking anywhere
;; else — copy the directory and you have copied the whole setup.
;;
;;   loop validate   → lints the graph + resolves every reference
;;   loop run        → drives it to a terminal
;;
;; See examples/ for the run this produces.

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

 ;; Sits under every state and over loop's built-in floor. A state's own
 ;; :model/:thinking wins; so does its stage prompt's frontmatter.
 :defaults {:provider "anthropic" :model "claude-sonnet-5" :thinking "medium"
            :skills [] :mcp []}

 ;; Hard stops the harness enforces. May only tighten the global budgets.
 :budgets {:usd 8 :wallclock-s 5400 :max-transitions 40}

 ;; The cheap agents that guard and reroute.
 :judge {:model "claude-haiku-4-5" :thinking "low"}
 :navigator {:model "claude-haiku-4-5" :thinking "low" :max-invocations 5}

 :entry "implement"
 :terminals ["done" "blocked"]
 :escalation-state "blocked"

 ;; ── States ────────────────────────────────────────────────────────────────
 ;; Every stage's prompt IS its :stage-prompt — a markdown file resolved
 ;; resolved in ./.loop/stage-prompts/, beside this file.
 ;; A reusable stage names a stage prompt copied in with the rest; a one-off drops a
 ;; local .md of its own.
 :states
 {:implement {:stage-prompt "implement"
              :thinking "high"
              :skills ["spark-build"]
              :description "Implement the plan; keep the build green."}

  :review {:stage-prompt "review"              ; this stage prompt == the run-review skill
           :thinking "high"
           :description "Adversarial review of the diff; find real defects."}

  ;; What keeps this stage from grading its own homework is not what it can
  ;; reach — it is that the edges out of it are gated on `:check` commands the
  ;; harness runs itself, and on a Judge that never sees this stage's own
  ;; claims (docs/05-design-notes.md).
  ;; :mcp names servers in YOUR ~/.pi/agent/mcp.json — loop neither reads nor
  ;; ships that file. Every server starts a session disconnected, so the entry
  ;; message asks this stage to `mcp({connect: "warehouse"})` before it works.
  ;; A stage that doesn't name a server can't reach one.
  :qa-staging {:stage-prompt "qa"
               :thinking "high"
               :skills ["staging-deploy" "spark-run"]
               :mcp ["warehouse"]
               :description "Deploy to staging, run the pipeline, grade it."}

  :debug {:stage-prompt "debug-spark"
          :thinking "high"
          :skills ["spark-build" "debug-transient"]
          :description "Diagnose a real pipeline failure and fix it."}

  :validate-contract {:stage-prompt "validate-contract"      ; LOCAL: ./.loop/stage-prompts/
                      :thinking "medium"
                      :skills ["staging-deploy" "contract-check"]
                      :description "Confirm the API contract matches the OpenAPI schema."}

  :open-pr {:stage-prompt "open-pr"
            :thinking "low"
            :skills ["open-pr"]
            :description "Open or update the pull request for this branch."}}

 ;; ── Transitions ───────────────────────────────────────────────────────────
 ;; Only these edges exist at all — a target that is not one of them goes to
 ;; the Navigator, not to a guard. Past that, two tiers, cheapest first.
 ;;
 ;;  1. :check     — a command the HARNESS runs, in its own subprocess, after
 ;;                  the stage exits. Exit 0 passes. The one signal here a
 ;;                  worker cannot author, because it never touches the
 ;;                  worker's session. A failed check is not appealable.
 ;;  2. :criteria  — an independent cheap Judge, which sees the stage's output,
 ;;                  its artifacts, and the check's stdout — but never the
 ;;                  worker's own claim that it succeeded.
 ;;
 ;; Note the same scripts appear here and in the states' :skills. That is the
 ;; point: the agent and the harness run identical code, so an agent cannot
 ;; pass a gate the harness would fail.
 ;;
 ;; :on-fail is "retry" | "abort" | {:route "state"}.
 :transitions
 [{:from "implement" :to "review"
   :check "bash .loop/skills/spark-build/build.sh"
   :criteria "The plan's four items are all addressed in the diff, and no TODO/FIXME markers remain in changed files."
   :on-fail "retry"}

  {:from "review" :to "implement"
   :criteria "The review identified defects that require code changes."}
  {:from "review" :to "qa-staging"
   :criteria "The review found no defect requiring a code change."}

  ;; The three-way fail routing: a transient flake retries in place with
  ;; backoff and touches no code, a real failure spawns the debugger, a pass
  ;; moves on (docs/05-design-notes.md). Each edge asserts its own branch of one script's
  ;; taxonomy, so "transient" is decided by a versioned regex set and an exit
  ;; code rather than by a tired agent that would rather retry than debug.
  {:from "qa-staging" :to "qa-staging"
   :check "bash .loop/skills/spark-run/classify.sh --expect transient"
   :backoff-s 30
   :on-fail "abort"}
  {:from "qa-staging" :to "debug"
   :check "bash .loop/skills/spark-run/classify.sh --expect real"}
  {:from "qa-staging" :to "validate-contract"
   :check "bash .loop/skills/spark-run/classify.sh --expect pass"
   :criteria "The output sample satisfies every QA case, not just the job's exit status."}

  {:from "debug" :to "qa-staging"
   :check "bash .loop/skills/spark-build/build.sh"
   :criteria "A concrete fix to the diagnosed failure was applied — not a retry, a widened assertion, or a disabled check."
   :on-fail "retry"}

  {:from "validate-contract" :to "implement"
   :criteria "The staging response does not match the committed OpenAPI schema."}
  {:from "validate-contract" :to "open-pr"
   :check "bash .loop/skills/contract-check/check.sh /accounts/42"}

  {:from "open-pr" :to "done"
   :criteria "A pull request exists for this branch with a populated description."}]

 ;; ── Loops ─────────────────────────────────────────────────────────────────
 ;; states[0] is the loop head — the state whose re-entry counts a cycle.
 :loops
 [{:name "qa" :states ["qa-staging" "debug"] :max-cycles 4 :on-exhausted "escalate"}
  {:name "qa-transient" :states ["qa-staging"] :max-cycles 3 :on-exhausted "escalate"}]}
