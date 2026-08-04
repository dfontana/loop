;; `:playbok` is a typo for `:playbook`, and `:max-cyles` for `:max-cycles`.
;; The hand-written walker read the keys it knew and ignored the rest, so this
;; file used to load — the state fell through to "needs either :playbook or
;; :prompt" (a confusing error about a key that IS present) and a misspelled
;; cycle bound would have silently kept its default. Both are now caught by
;; name, at their path in the file.
{:ticket "PROJ-TYPO"
 :task "inline task"
 :plan "inline plan"
 :entry "a"
 :terminals ["done"]
 :states {:a {:playbok "implement"}}
 :transitions [{:from "a" :to "done"}]}
