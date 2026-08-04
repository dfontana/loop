;; `:timeut-s` is a typo for `:timeout-s`. The `:check` table was the one wire
;; struct without `deny_unknown_fields`, so this used to load clean and
;; silently take the default timeout — on the one key whose whole purpose is
;; to stop a long check being killed early.
{:ticket "PROJ-CHECK-TYPO"
 :task "inline task"
 :plan "inline plan"
 :entry "a"
 :terminals ["done"]
 :states {:a {:playbook "implement"}}
 :transitions [{:from "a" :to "done"
                :check {:cmd "true" :timeut-s 300}}]}
