;; A machine written against `:transition-mode`, which selected the schema of
;; the injected `transition` tool's `to` parameter. There is no injected tool
;; any more — a worker writes its proposal to `$LOOP_HANDOFF` — so the key
;; selects between nothing and nothing. Accepting and ignoring it would leave
;; an author believing they had configured something.
{:ticket "PROJ-BAD"
 :task "inline task"
 :plan "inline plan"
 :entry "a"
 :terminals ["done"]
 :transition-mode "open"
 :states {:a {:playbook "a"}}
 :transitions [{:from "a" :to "done"}]}
