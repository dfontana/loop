;; A machine written against `:playbook`, which is now `:stage-prompt`.
;;
;; Unlike `:when` or `:transition-mode`, this key did not stop meaning
;; something — it means exactly what it always did, under a new name. That is
;; why it needs its own message rather than `deny_unknown_fields`' generic one:
;; the field list an author would be shown offers both `stage-prompt` and
;; `prompt`, and picking the wrong one loads a machine that runs the wrong
;; text as its stage's prompt.
{:ticket "PROJ-BAD"
 :task "inline task"
 :plan "inline plan"
 :entry "a"
 :terminals ["done"]
 :states {:a {:playbook "a"}}
 :transitions [{:from "a" :to "done"}]}
